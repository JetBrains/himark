// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::{
    AppCommand, AppFx, DynamicCommand, FetchDocumentEffect, OpenByLocationEffect, ResourceLocation,
};
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

pub fn document_for(
    languages: &crate::SyntaxLanguages,
    name: &str,
    source: &str,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::Theme,
) -> crate::Document {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    // Markdown is just another registered language; anything the
    // registry does not know reads as markdown, like it always has.
    let language = match extension.as_str() {
        "md" | "markdown" => "markdown",
        known if languages.knows(known) => known,
        _ => "markdown",
    };
    let mut document = crate::Document::from_language(
        crate::Text::from_string_exact(source),
        language,
        languages,
        store,
        ui,
        fonts,
        theme,
    );
    if let Some(enrichers) = ::editor::env::Enrichers::of(store) {
        document.enrich_now(&enrichers, store, ui, fonts, theme);
    }
    document
}

/// Building a document from text in hand (Text, layout, syntax) is not a
/// filesystem capability — the chat's cells build off-thread with no host
/// in sight. It installs at boot, on its own.
pub fn install_build_handler(
    app: &mut crate::Application,
    languages: Arc<crate::SyntaxLanguages>,
    diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
) {
    let workshop = Arc::clone(app.workshop());
    let caller = app.effect_caller();
    app.register_handler::<crate::BuildDocumentEffect>(BuildDocumentHandler(
        Arc::clone(&workshop),
        Arc::clone(&languages),
    ));
    app.register_handler::<crate::higent::BuildFileEditEffect>(BuildFileEditHandler {
        caller,
        workshop,
        languages,
        diff_policy,
    });
}

/// The chat's diff cell, built off the UI thread: fetch both sides over
/// the seat, then run the seeded pair recipe (two parses, the diff, the
/// marks) in the workshop. The cell only lays the editors.
struct BuildFileEditHandler {
    caller: imba::effect::EffectCaller,
    workshop: Arc<crate::Workshop>,
    languages: Arc<crate::SyntaxLanguages>,
    diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
}

impl EffectHandler<crate::higent::BuildFileEditEffect> for BuildFileEditHandler {
    async fn handle(
        &self,
        effect: crate::higent::BuildFileEditEffect,
    ) -> Result<crate::higent::BuiltFileEdit, String> {
        let contents = self
            .caller
            .call(crate::higent::FetchFileEditEffect {
                seat: effect.seat,
                before: effect.before,
                after: effect.after,
            })
            .await
            .ok_or_else(|| "the build was cancelled".to_owned())??;
        let fonts = self.workshop.fonts();
        let theme = self.workshop.theme();
        Ok(self.workshop.with_ctx(|store, ui| {
            crate::higent::build_file_edit(
                &effect.name,
                &contents,
                &self.languages,
                &self.diff_policy,
                store,
                ui,
                &fonts,
                &theme,
            )
        }))
    }
}

pub fn install_open_handlers(
    app: &mut crate::Application,
    languages: Arc<crate::SyntaxLanguages>,
    diff_policy: Arc<dyn crate::diff::DiffPolicy>,
) {
    let caller = app.effect_caller();
    let workshop = Arc::clone(app.workshop());
    app.register_handler::<OpenByLocationEffect>(OpenByLocationHandler {
        caller: caller.clone(),
        workshop: Arc::clone(&workshop),
        languages: Arc::clone(&languages),
    });
    let _ = &diff_policy;
    let shop = DiffOpenShop {
        caller,
        workshop,
        languages,
    };
    app.register_handler::<crate::OpenDiffByLocationsEffect>(OpenDiffByLocationsHandler(
        shop.clone(),
    ));
    app.register_handler::<crate::OpenDiffPairEffect>(OpenDiffPairHandler(shop));
    app.register_windowed_navigator(DiffNavigator);
}

/// The ONE off-thread step both diff roads share (docs/editor/diff-canvas.md
/// §4): ensure each side is a registered document. An OPEN side passes
/// through by id; a CLOSED side is fetched and built here (the landing
/// registers it). No diff runs here — the diff view's normalize lane
/// computes it from the registered documents (docs/no-diff-on-ui-thread).
#[derive(Clone)]
struct DiffOpenShop {
    caller: imba::effect::EffectCaller,
    workshop: Arc<crate::Workshop>,
    languages: Arc<crate::SyntaxLanguages>,
}

impl DiffOpenShop {
    /// Resolve a side to the thing the landing installs, and whether it
    /// was reachable. An open side passes through; a closed side is
    /// fetched and built (registered at the landing, at its location).
    async fn resolve(&self, input: crate::DiffSideInput) -> (crate::DiffSide, bool) {
        match input {
            crate::DiffSideInput::Open(document) => (crate::DiffSide::Open(document), true),
            crate::DiffSideInput::Fetch(location) => {
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
                    crate::DiffSide::Built {
                        location,
                        document: crate::BuiltDocument { document },
                    },
                    present,
                )
            }
        }
    }

    async fn open_pair(
        &self,
        old: crate::DiffSideInput,
        new: crate::DiffSideInput,
        width: f32,
    ) -> crate::OpenedDiffPair {
        let (old_side, old_present) = self.resolve(old).await;
        let (new_side, new_present) = self.resolve(new).await;
        crate::OpenedDiffPair {
            old: old_side,
            new: new_side,
            width,
            failed: !old_present && !new_present,
        }
    }
}

struct OpenDiffByLocationsHandler(DiffOpenShop);

impl EffectHandler<crate::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: crate::OpenDiffByLocationsEffect) -> AppCommand {
        // The pane's editor width is resolved by `install_opened_pair`
        // (non-embedded → OPEN_HALF_WIDTH); the carried width is unused.
        let pair = self.0.open_pair(effect.old, effect.new, 0.0).await;
        if pair.failed {
            // Both sides absent → both were fetched, so a built side
            // carries its location for the notice.
            let location = [&pair.old, &pair.new]
                .into_iter()
                .find_map(|side| match side {
                    crate::DiffSide::Built { location, .. } => Some(location.clone()),
                    crate::DiffSide::Open(_) => None,
                })
                .expect("a failed pair has a built side");
            return AppCommand::Dynamic(effect.window, Arc::new(FetchFailed { location }));
        }
        AppCommand::Dynamic(
            effect.window,
            Arc::new(OpenDiffPair {
                window: effect.window,
                documents: effect.documents,
                pair,
            }),
        )
    }
}

struct OpenDiffPairHandler(DiffOpenShop);

impl EffectHandler<crate::OpenDiffPairEffect> for OpenDiffPairHandler {
    async fn handle(&self, effect: crate::OpenDiffPairEffect) -> crate::OpenedDiffPair {
        self.0.open_pair(effect.old, effect.new, effect.width).await
    }
}

pub struct DiffNavigator;

impl crate::navigation::WindowedNavigator for DiffNavigator {
    type Place = crate::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::UiCtx,
        window: crate::WindowId,
        place: &crate::DiffPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<crate::Panel> {
        // Resolve both sides on the UI thread — an open side hands over
        // its live snapshot; the prep runs off-thread and the landing
        // opens the dressed pane. Diffing never runs here.
        let documents = crate::Windows::session_family(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old = crate::DiffSideInput::resolve(store, documents, place.old.clone());
        let new = crate::DiffSideInput::resolve(store, documents, place.new.clone());
        fx.push(AnyEffect::new(crate::OpenDiffByLocationsEffect {
            window,
            documents,
            old,
            new,
        }));
        None
    }
}

pub struct OpenDiffPair {
    window: crate::WindowId,
    documents: imba::store::Id<crate::OpenDocuments>,
    pair: crate::OpenedDiffPair,
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
        app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let _ = crate::open_opened_diff_pane(
            store,
            ui,
            self.window,
            self.documents,
            self.pair.clone(),
            fx,
        );
        crate::sync_document_watches(store, self.documents, fx);
        crate::sync_stripe_bases(store, self.documents, ui, fx);
    }
}

/// The diff canvas's per-item build (docs/editor/diff-canvas.md §4): both
/// fetches, both documents, the Myers pass and the mark prep all run
/// here, off the UI thread; the landing only mounts editors.
pub struct BuildDocumentHandler(pub Arc<crate::Workshop>, pub Arc<crate::SyntaxLanguages>);

impl EffectHandler<crate::BuildDocumentEffect> for BuildDocumentHandler {
    async fn handle(&self, effect: crate::BuildDocumentEffect) -> crate::BuiltDocument {
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

        crate::BuiltDocument { document }
    }
}

pub struct OpenByLocationHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<crate::Workshop>,
    pub languages: Arc<crate::SyntaxLanguages>,
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
                    crate::OpenedDocument {
                        documents: effect.documents,
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
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut AppFx<'_>,
    ) {
        eprintln!("[himark] fetch failed: {:?}", self.location);
    }
}
