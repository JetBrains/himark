// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::{Effect, EffectHandler};

use crate::{
    document::Document,
    document_layout::DocumentLayout,
    editor::{Editor, EditorId},
    editor_view::EditorCommand,
};

pub struct RepairedLayout {
    pub(crate) editor: EditorId,
    pub(crate) revision: u64,
    pub(crate) markup_generation: u64,
    pub(crate) width: f32,
    pub(crate) layout: DocumentLayout,
}

impl RepairedLayout {
    pub fn editor(&self) -> EditorId {
        self.editor
    }
}

struct PendingEditor {
    editor: EditorId,
    width: f32,

    anchor: u32,
    layout: DocumentLayout,

    markups: Vec<crate::markup::MarkupId>,
}

pub struct RepairEffect {
    document: Document,
    editors: Vec<PendingEditor>,
}

impl RepairEffect {
    pub fn editor_count(&self) -> usize {
        self.editors.len()
    }

    pub fn collect<'a>(
        document: &Document,
        editors: impl Iterator<Item = (&'a EditorId, &'a Editor)>,
    ) -> Option<Self> {
        let editors: Vec<_> = editors
            .filter(|(_, editor)| editor.layout.repair_pending().is_some())
            .map(|(id, editor)| PendingEditor {
                editor: *id,
                width: editor.layout.layout_width(),

                anchor: editor
                    .viewport
                    .as_ref()
                    .map(|viewport| editor.layout.byte_at_y(viewport.start))
                    .unwrap_or(0),
                layout: editor.layout.clone(),

                markups: editor.markups.clone(),
            })
            .collect();
        if editors.is_empty() {
            return None;
        }
        Some(Self {
            document: document.substance(),
            editors,
        })
    }
}

const WORKER_REPAIR_BUDGET: f32 = 60_000.0;

pub struct RepairHandler(pub std::sync::Arc<crate::env::Workshop>);

impl EffectHandler<RepairEffect> for RepairHandler {
    async fn handle(&self, effect: RepairEffect) -> EditorCommand {
        EditorCommand::ApplyRepair(self.repairs(effect))
    }
}

impl RepairHandler {
    pub(crate) fn repairs(&self, effect: RepairEffect) -> Vec<RepairedLayout> {
        let fonts = self.0.fonts();
        let theme = self.0.theme();
        let revision = effect.document.revision();
        let markup_generation = effect.document.markup_generation();
        let document = effect.document;
        effect
            .editors
            .into_iter()
            .map(|mut editor| {
                let pending = editor
                    .layout
                    .repair_pending_at_or_after(editor.anchor)
                    .or_else(|| editor.layout.repair_pending());
                if let Some(pending) = pending {
                    let mut extras = document.markup_refs(&editor.markups);
                    extras.extend(
                        document
                            .document_scoped_markups()
                            .filter(|(id, _)| !editor.markups.contains(id)),
                    );
                    self.0.measure(editor.width, |measure| {
                        editor.layout.repair_layout_bounded(
                            document.text(),
                            crate::markup::OverlaidMarkup::new(document.markup(), &extras),
                            measure,
                            &fonts,
                            &theme,
                            pending,
                            WORKER_REPAIR_BUDGET,
                        )
                    });
                }
                RepairedLayout {
                    editor: editor.editor,
                    revision,
                    markup_generation,
                    width: editor.width,
                    layout: editor.layout,
                }
            })
            .collect()
    }
}

impl Effect for RepairEffect {
    type Result = EditorCommand;
}
