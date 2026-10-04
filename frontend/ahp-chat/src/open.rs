// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use documents::FetchDocumentEffect;
use editor::location::ResourceLocation;
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

pub fn document_for(
    languages: &editor::reparse::SyntaxLanguages,
    name: &str,
    source: &str,
    store: &imba::store::Store,
    ui: &imba::ui::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &editor::theme::Theme,
) -> editor::document::Document {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    // Markdown is just another registered language; anything the
    // registry does not know reads as markdown, like it always has.
    let language = match extension.as_str() {
        "md" | "markdown" => "markdown",
        known if languages.knows(known) => known,
        _ => "markdown",
    };
    let mut document = editor::document::Document::from_language(
        text::text::Text::from_string_exact(source),
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

/// The chat's diff cell, built off the UI thread: fetch both sides over
/// the client, then run the seeded pair recipe (two parses, the diff, the
/// marks) in the workshop. The cell only lays the editors.
pub struct BuildFileEditHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<editor::env::Workshop>,
    pub languages: Arc<editor::reparse::SyntaxLanguages>,
    pub diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
}

impl EffectHandler<crate::file_edit::BuildFileEditEffect> for BuildFileEditHandler {
    async fn handle(
        &self,
        effect: crate::file_edit::BuildFileEditEffect,
    ) -> Result<crate::file_edit::BuiltFileEdit, String> {
        let contents = self
            .caller
            .call(ahp_wire::effects::FetchFileEditEffect {
                client: effect.client,
                before: effect.before,
                after: effect.after,
            })
            .await
            .ok_or_else(|| "the build was cancelled".to_owned())??;
        let fonts = self.workshop.fonts();
        let theme = self.workshop.theme();
        Ok(self.workshop.with_ctx(|store, ui| {
            crate::file_edit::build_file_edit(
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

/// The ONE off-thread step both diff roads share (docs/editor/diff-canvas.md
/// §4): ensure each side is a registered document. An OPEN side passes
/// through by id; a CLOSED side is fetched and built here (the landing
/// registers it). No diff runs here — the diff view's normalize lane
/// computes it from the registered documents (docs/no-diff-on-ui-thread).
#[derive(Clone)]
pub struct DiffOpenShop {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<editor::env::Workshop>,
    pub languages: Arc<editor::reparse::SyntaxLanguages>,
}

impl DiffOpenShop {
    /// Resolve a side to the thing the landing installs, and whether it
    /// was reachable. An open side passes through; a closed side is
    /// fetched and built (registered at the landing, at its location).
    async fn resolve(
        &self,
        input: documents::diff_views::DiffSideInput,
    ) -> (documents::diff_views::DiffSide, bool) {
        match input {
            documents::diff_views::DiffSideInput::Open(document) => {
                (documents::diff_views::DiffSide::Open(document), true)
            }
            documents::diff_views::DiffSideInput::Fetch(location) => {
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
                    documents::diff_views::DiffSide::Built {
                        location,
                        document: documents::BuiltDocument { document },
                    },
                    present,
                )
            }
        }
    }

    pub async fn open_pair(
        &self,
        old: documents::diff_views::DiffSideInput,
        new: documents::diff_views::DiffSideInput,
        width: f32,
    ) -> documents::diff_views::OpenedDiffPair {
        let (old_side, old_present) = self.resolve(old).await;
        let (new_side, new_present) = self.resolve(new).await;
        documents::diff_views::OpenedDiffPair {
            old: old_side,
            new: new_side,
            width,
            failed: !old_present && !new_present,
        }
    }
}

pub struct OpenDiffPairHandler(pub DiffOpenShop);

impl EffectHandler<documents::diff_views::OpenDiffPairEffect> for OpenDiffPairHandler {
    async fn handle(
        &self,
        effect: documents::diff_views::OpenDiffPairEffect,
    ) -> documents::diff_views::OpenedDiffPair {
        self.0.open_pair(effect.old, effect.new, effect.width).await
    }
}

/// The diff canvas's per-item build (docs/editor/diff-canvas.md §4): both
/// fetches, both documents, the Myers pass and the mark prep all run
/// here, off the UI thread; the landing only mounts editors.
pub struct BuildDocumentHandler(pub Arc<editor::env::Workshop>, pub Arc<editor::reparse::SyntaxLanguages>);

impl EffectHandler<documents::BuildDocumentEffect> for BuildDocumentHandler {
    async fn handle(&self, effect: documents::BuildDocumentEffect) -> documents::BuiltDocument {
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

        documents::BuiltDocument { document }
    }
}
