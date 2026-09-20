// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use himark::{
    AppCommand, AppFx, DynamicCommand, FetchDocumentEffect, OpenByLocationEffect, ResourceLocation,
};
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

pub fn document_for(
    languages: &himark::SyntaxLanguages,
    name: &str,
    source: &str,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> himark::Document {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    if extension != "md" && extension != "markdown" && languages.knows(&extension) {
        return himark::Document::from_language(
            himark::Text::from_string_exact(source),
            &extension,
            languages,
            fonts,
            theme,
        );
    }
    himarkdown::document_from_markdown(source, fonts, theme)
}

pub fn install_open_handlers(
    app: &mut himark::Application,
    languages: Arc<himark::SyntaxLanguages>,
    diff_policy: Arc<dyn himark::diff::DiffPolicy>,
) {
    let caller = app.effect_caller();
    let workshop = Arc::clone(app.workshop());
    app.register_handler::<OpenByLocationEffect>(OpenByLocationHandler {
        caller: caller.clone(),
        workshop: Arc::clone(&workshop),
        languages: Arc::clone(&languages),
    });
    app.register_handler::<himark::BuildDocumentEffect>(BuildDocumentHandler(
        Arc::clone(&workshop),
        Arc::clone(&languages),
    ));
    app.register_handler::<himark::OpenDiffByLocationsEffect>(OpenDiffByLocationsHandler {
        caller: caller.clone(),
        workshop: Arc::clone(&workshop),
        languages: Arc::clone(&languages),
        diff_policy: Arc::clone(&diff_policy),
    });
    app.register_handler::<himark::BuildFileDiffEffect>(BuildFileDiffHandler {
        caller,
        workshop,
        languages,
        diff_policy,
    });
    app.register_navigator(DiffNavigator);
}

/// The diff canvas's per-item build (docs/editor/diff-canvas.md §4): both
/// fetches, both documents, the Myers pass and the mark prep all run
/// here, off the UI thread; the landing only mounts editors.
pub struct BuildFileDiffHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<::himark::Workshop>,
    pub languages: Arc<himark::SyntaxLanguages>,
    pub diff_policy: Arc<dyn himark::diff::DiffPolicy>,
}

impl EffectHandler<himark::BuildFileDiffEffect> for BuildFileDiffHandler {
    async fn handle(&self, effect: himark::BuildFileDiffEffect) -> himark::BuiltFileDiff {
        let fetch = |location: ResourceLocation| self.caller.call(FetchDocumentEffect { location });
        let old_text = fetch(effect.old.clone()).await.flatten();
        let new_text = fetch(effect.new.clone()).await.flatten();
        let failed = (old_text.is_none() && new_text.is_none())
            .then(|| format!("contents unavailable: {}", effect.new.name()));
        let build = |location: &ResourceLocation, text: &str| {
            document_for(
                &self.languages,
                location.name(),
                text,
                &self.workshop.fonts(),
                &self.workshop.theme(),
            )
        };
        let old = build(&effect.old, old_text.as_deref().unwrap_or(""));
        let new = build(&effect.new, new_text.as_deref().unwrap_or(""));
        let operation =
            self.diff_policy
                .diff(old.text(), new.text(), diff_syntax(&old, &new).as_ref());
        let marks = himark::prepare_marks(&operation, old.text());
        himark::BuiltFileDiff {
            old,
            new,
            operation,
            marks,
            width: effect.width,
            failed,
        }
    }
}

pub struct OpenDiffByLocationsHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<::himark::Workshop>,
    pub languages: Arc<himark::SyntaxLanguages>,
    pub diff_policy: Arc<dyn himark::diff::DiffPolicy>,
}

impl OpenDiffByLocationsHandler {
    async fn open(
        &self,
        window: himark::WindowId,
        old_location: ResourceLocation,
        new_location: ResourceLocation,
    ) -> AppCommand {
        let fetch = |location: ResourceLocation| self.caller.call(FetchDocumentEffect { location });
        let old_text = fetch(old_location.clone()).await.flatten();
        let new_text = fetch(new_location.clone()).await.flatten();
        if old_text.is_none() && new_text.is_none() {
            return AppCommand::Dynamic(
                window,
                Arc::new(FetchFailed {
                    location: new_location,
                }),
            );
        }
        let build = |location: &ResourceLocation, text: &str| {
            document_for(
                &self.languages,
                location.name(),
                text,
                &self.workshop.fonts(),
                &self.workshop.theme(),
            )
        };
        let old = build(&old_location, old_text.as_deref().unwrap_or(""));
        let new = build(&new_location, new_text.as_deref().unwrap_or(""));

        let operation =
            self.diff_policy
                .diff(old.text(), new.text(), diff_syntax(&old, &new).as_ref());
        let marks = himark::prepare_marks(&operation, old.text());
        AppCommand::Dynamic(
            window,
            Arc::new(OpenDiffPair {
                old_location,
                old,
                new,
                new_location,
                prep: hidiff::DiffPrep { operation, marks },
            }),
        )
    }
}

impl EffectHandler<himark::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: himark::OpenDiffByLocationsEffect) -> AppCommand {
        self.open(effect.window, effect.old, effect.new).await
    }
}

pub struct DiffNavigator;

impl himark::Navigator for DiffNavigator {
    type Place = hidiff::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        window: himark::WindowId,
        place: &hidiff::DiffPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<himark::Panel> {
        let old = himark::OpenDocuments::by_location(store, &place.old);
        let new = himark::OpenDocuments::by_location(store, &place.new);
        let (Some(old), Some(new)) = (old, new) else {
            fx.push(AnyEffect::new(himark::OpenDiffByLocationsEffect {
                window,
                old: place.old.clone(),
                new: place.new.clone(),
            }));
            return None;
        };

        let panel = hidiff::diff_panel(store, old, new, None)?;
        Some(himark::Panel::Plugin(Box::new(panel)))
    }
}

pub struct OpenDiffPair {
    old_location: ResourceLocation,
    old: himark::Document,
    new: himark::Document,
    new_location: ResourceLocation,

    prep: hidiff::DiffPrep,
}

impl DynamicCommand for OpenDiffPair {
    fn id(&self) -> &'static str {
        "vcs.open-diff-pair"
    }
    fn name(&self) -> String {
        "Open Diff Pane".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let mut side = |location: &ResourceLocation, document: &himark::Document| {
            match himark::OpenDocuments::by_location(store, location) {
                Some(open_id) => (open_id, false),
                None => (
                    himark::OpenDocuments::register(
                        store,
                        document.clone(),
                        Some(location.clone()),
                        location.name().to_owned(),
                        document.revision(),
                    ),
                    true,
                ),
            }
        };
        let (old_id, old_fresh) = side(&self.old_location, &self.old);
        let (new_id, new_fresh) = side(&self.new_location, &self.new);

        let prep = (old_fresh && new_fresh).then(|| self.prep.clone());
        let _ = hidiff::open_diff_documents(store, window, old_id, new_id, prep, fx);

        himark::sync_document_watches(store, fx);
        himark::sync_stripe_bases(store, fx);
    }
}

pub struct BuildDocumentHandler(
    pub Arc<::himark::Workshop>,
    pub Arc<himark::SyntaxLanguages>,
);

impl EffectHandler<himark::BuildDocumentEffect> for BuildDocumentHandler {
    async fn handle(&self, effect: himark::BuildDocumentEffect) -> himark::BuiltDocument {
        let fonts = self.0.fonts();
        let theme = self.0.theme();
        let document = document_for(
            &self.1,
            effect.location.name(),
            &effect.text,
            &fonts,
            &theme,
        );

        himark::BuiltDocument { document }
    }
}

pub struct OpenByLocationHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<::himark::Workshop>,
    pub languages: Arc<himark::SyntaxLanguages>,
}

impl EffectHandler<OpenByLocationEffect> for OpenByLocationHandler {
    async fn handle(&self, effect: OpenByLocationEffect) -> AppCommand {
        let fetched = self
            .caller
            .call(FetchDocumentEffect {
                location: effect.location.clone(),
            })
            .await;
        match fetched.flatten() {
            Some(text) => {
                let document = document_for(
                    &self.languages,
                    effect.location.name(),
                    &text,
                    &self.workshop.fonts(),
                    &self.workshop.theme(),
                );
                AppCommand::Opened(
                    effect.window,
                    himark::OpenedDocument {
                        name: effect.location.name().to_owned(),
                        document,
                        location: Some(effect.location),
                        primary: effect.primary,
                        target: effect.target,
                    },
                )
            }

            None => AppCommand::Dynamic(
                effect.window,
                Arc::new(FetchFailed {
                    location: effect.location,
                }),
            ),
        }
    }
}

pub struct FetchFailed {
    location: ResourceLocation,
}

impl DynamicCommand for FetchFailed {
    fn id(&self) -> &'static str {
        "file.fetch-failed"
    }
    fn name(&self) -> String {
        "Fetch Failed".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        _store: &mut Store,
        _window: himark::WindowId,
        _fx: &mut AppFx<'_>,
    ) {
        eprintln!("[himark] fetch failed: {:?}", self.location);
    }
}

/// Syntax context for the built pair: the fresh documents were parsed
/// right here, so their trees match their texts exactly.
fn diff_syntax<'a>(
    old: &'a himark::Document,
    new: &'a himark::Document,
) -> Option<himark::diff::DiffSyntax<'a>> {
    let target = new.syntax()?;
    let base_tree = old
        .syntax()
        .filter(|base| base.language == target.language)
        .and_then(|base| base.tree.as_deref());
    Some(himark::diff::DiffSyntax {
        language: &target.language,
        base_tree,
        target_tree: target.tree.as_deref(),
    })
}
