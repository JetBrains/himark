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
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> himark::Document {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    if extension != "md" && extension != "markdown" && languages.knows(&extension) {
        return himark::Document::from_language(
            himark::Text::from_string_exact(source),
            &extension,
            languages,
            store,
            ui,
            fonts,
            theme,
        );
    }
    himarkdown::document_from_markdown(source, store, ui, fonts, theme)
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
    let _ = &diff_policy;
    let shop = DiffOpenShop {
        caller,
        workshop,
        languages,
    };
    app.register_handler::<himark::OpenDiffByLocationsEffect>(OpenDiffByLocationsHandler(
        shop.clone(),
    ));
    app.register_handler::<himark::OpenDiffPairEffect>(OpenDiffPairHandler(shop));
    app.register_navigator(DiffNavigator);
}

/// The ONE off-thread step both diff roads share (docs/editor/diff-canvas.md
/// §4): ensure each side is a registered document. An OPEN side passes
/// through by id; a CLOSED side is fetched and built here (the landing
/// registers it). No diff runs here — the diff view's normalize lane
/// computes it from the registered documents (docs/no-diff-on-ui-thread).
#[derive(Clone)]
struct DiffOpenShop {
    caller: imba::effect::EffectCaller,
    workshop: Arc<::himark::Workshop>,
    languages: Arc<himark::SyntaxLanguages>,
}

impl DiffOpenShop {
    /// Resolve a side to the thing the landing installs, and whether it
    /// was reachable. An open side passes through; a closed side is
    /// fetched and built (registered at the landing, at its location).
    async fn resolve(&self, input: himark::DiffSideInput) -> (himark::DiffSide, bool) {
        match input {
            himark::DiffSideInput::Open(document) => (himark::DiffSide::Open(document), true),
            himark::DiffSideInput::Fetch(location) => {
                let text = self
                    .caller
                    .call(FetchDocumentEffect {
                        location: location.clone(),
                    })
                    .await
                    .flatten();
                let present = text.is_some();
                let document = self.workshop.with_ctx(|store, ui| {
                    document_for(
                        &self.languages,
                        location.name(),
                        text.as_deref().unwrap_or(""),
                        store,
                        ui,
                        &self.workshop.fonts(),
                        &self.workshop.theme(),
                    )
                });
                (
                    himark::DiffSide::Built {
                        location,
                        document: himark::BuiltDocument { document },
                    },
                    present,
                )
            }
        }
    }

    async fn open_pair(
        &self,
        old: himark::DiffSideInput,
        new: himark::DiffSideInput,
        width: f32,
    ) -> himark::OpenedDiffPair {
        let (old_side, old_present) = self.resolve(old).await;
        let (new_side, new_present) = self.resolve(new).await;
        himark::OpenedDiffPair {
            old: old_side,
            new: new_side,
            width,
            failed: !old_present && !new_present,
        }
    }
}

struct OpenDiffByLocationsHandler(DiffOpenShop);

impl EffectHandler<himark::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: himark::OpenDiffByLocationsEffect) -> AppCommand {
        // The pane's editor width is resolved by `install_opened_pair`
        // (non-embedded → OPEN_HALF_WIDTH); the carried width is unused.
        let pair = self.0.open_pair(effect.old, effect.new, 0.0).await;
        if pair.failed {
            // Both sides absent → both were fetched, so a built side
            // carries its location for the notice.
            let location = [&pair.old, &pair.new]
                .into_iter()
                .find_map(|side| match side {
                    himark::DiffSide::Built { location, .. } => Some(location.clone()),
                    himark::DiffSide::Open(_) => None,
                })
                .expect("a failed pair has a built side");
            return AppCommand::Dynamic(effect.window, Arc::new(FetchFailed { location }));
        }
        AppCommand::Dynamic(
            effect.window,
            Arc::new(OpenDiffPair {
                window: effect.window,
                pair,
            }),
        )
    }
}

struct OpenDiffPairHandler(DiffOpenShop);

impl EffectHandler<himark::OpenDiffPairEffect> for OpenDiffPairHandler {
    async fn handle(&self, effect: himark::OpenDiffPairEffect) -> himark::OpenedDiffPair {
        self.0.open_pair(effect.old, effect.new, effect.width).await
    }
}

pub struct DiffNavigator;

impl himark::Navigator for DiffNavigator {
    type Place = hidiff::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::UiCtx,
        window: himark::WindowId,
        place: &hidiff::DiffPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<himark::Panel> {
        // Resolve both sides on the UI thread — an open side hands over
        // its live snapshot; the prep runs off-thread and the landing
        // opens the dressed pane. Diffing never runs here.
        let old = himark::DiffSideInput::resolve(store, place.old.clone());
        let new = himark::DiffSideInput::resolve(store, place.new.clone());
        fx.push(AnyEffect::new(himark::OpenDiffByLocationsEffect {
            window,
            old,
            new,
        }));
        None
    }
}

pub struct OpenDiffPair {
    window: himark::WindowId,
    pair: himark::OpenedDiffPair,
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
        app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let _ = hidiff::open_opened_diff_pane(store, ui, self.window, self.pair.clone(), fx);
        himark::sync_document_watches(store, fx);
        himark::sync_stripe_bases(store, fx);
    }
}

/// The diff canvas's per-item build (docs/editor/diff-canvas.md §4): both
/// fetches, both documents, the Myers pass and the mark prep all run
/// here, off the UI thread; the landing only mounts editors.
pub struct BuildDocumentHandler(
    pub Arc<::himark::Workshop>,
    pub Arc<himark::SyntaxLanguages>,
);

impl EffectHandler<himark::BuildDocumentEffect> for BuildDocumentHandler {
    async fn handle(&self, effect: himark::BuildDocumentEffect) -> himark::BuiltDocument {
        let fonts = self.0.fonts();
        let theme = self.0.theme();
        let document = self.0.with_ctx(|store, ui| {
            document_for(
                &self.1,
                effect.location.name(),
                &effect.text,
                store,
                ui,
                &fonts,
                &theme,
            )
        });

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
                let document = self.workshop.with_ctx(|store, ui| {
                    document_for(
                        &self.languages,
                        effect.location.name(),
                        &text,
                        store,
                        ui,
                        &self.workshop.fonts(),
                        &self.workshop.theme(),
                    )
                });
                AppCommand::Opened(
                    effect.window,
                    himark::OpenedDocument {
                        name: effect.location.name().to_owned(),
                        document,
                        location: Some(effect.location),
                        primary: effect.primary,
                        target: effect.target,
                        focus: effect.focus,
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

