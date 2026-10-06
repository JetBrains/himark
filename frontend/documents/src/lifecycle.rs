// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::{
    document::Document, editor::EditorEffects, editor::EditorId, editor_view::EditorCommand,
};
use imba::store::Store;

use crate::{DocumentId, OpenDocuments};

pub fn mount_editor(
    store: &Store,
    ui: &imba::ui::UiCtx,
    document: &mut Document,
    width: f32,
    target: Option<std::ops::Range<crate::text_ext::LineCol>>,
    fx: &mut EditorEffects<'_>,
) -> EditorId {
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let editor = document.add_editor(
        width,
        None,
        ::editor::document::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &theme,
        fx,
    );
    if let Some(target) = target {
        let byte = crate::text_ext::offset_at(&mut document.text().view(), target.start) as u32;
        document.reveal_at_instant(editor, byte, store, ui, &fonts, &theme, fx);
    }

    if let Some(parsers) = editor::env::Parsers::of(store) {
        document.launch_reparse(parsers, fx);
    }
    editor
}

pub fn close_editor(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    document_id: DocumentId,
    editor: EditorId,
) {
    if let Some(mut document) = OpenDocuments::document(store, documents, document_id) {
        document.remove_editor(editor);
        OpenDocuments::put_document(store, documents, document_id, document);
    }
}

/// The editor delivery is a PLUGIN BOUNDARY: `land_reparse` and
/// `Document::perform` run enrichers and view destroys that open and
/// release SIBLINGS in this same collection — so the row must be IN
/// the table while they run (the router swaps it back first), and the
/// spans that touch the row stay narrow, as they always were.
pub fn deliver(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    document_id: DocumentId,
    command: EditorCommand,
    fx: &mut EditorEffects<'_>,
) {
    if let EditorCommand::ApplyReparse(outcome) = command {
        // No anchor gate: `land_reparse` never needs an editor, and a
        // parse captured while the document briefly had none (a canvas
        // row mid-succession) must still land — dropping it leaves the
        // edited text uncolored under the diff wash until the next edit.
        let location = OpenDocuments::location(store, documents, document_id);
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let Some(mut document) = OpenDocuments::document(store, documents, document_id) else {
            return;
        };
        document.land_reparse(outcome, location, store, ui, &fonts, &theme, fx);
        OpenDocuments::put_document(store, documents, document_id, document);
        return;
    }
    let editor = match &command {
        EditorCommand::ApplyRepair(repaired) => match repaired.first() {
            Some(item) => item.editor(),
            None => return,
        },
        EditorCommand::ApplyEnrichment(outcome) => match outcome.anchor() {
            Some(anchor) => anchor,
            None => return,
        },
        EditorCommand::ApplyScrollStripes(outcome) => outcome.editor(),
        _ => return,
    };
    let Some(mut document) = OpenDocuments::document(store, documents, document_id) else {
        return;
    };
    document.perform(store, ui, editor, command, fx);
    OpenDocuments::put_document(store, documents, document_id, document);
}
