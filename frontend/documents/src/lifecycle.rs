use editor::{Document, EditorCommand, EditorEffects, EditorId};
use imba::store::Store;

use crate::{DocumentId, OpenDocuments};

pub fn mount_editor(
    store: &Store,
    document: &mut Document,
    width: f32,
    target: Option<std::ops::Range<crate::LineCol>>,
    fx: &mut EditorEffects<'_>,
) -> EditorId {
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let editor = document.add_editor(
        width,
        None,
        ::editor::EditorBuild::Bounded,
        &[],
        &fonts,
        &theme,
        fx,
    );
    if let Some(target) = target {
        let byte = crate::offset_at(&mut document.text().view(), target.start) as u32;
        document.reveal_at(editor, byte, &fonts, &theme, fx);
    }

    if let Some(parsers) = editor::env::Parsers::of(store) {
        document.launch_reparse(parsers, fx);
    }
    editor
}

pub fn close_editor(store: &mut Store, document_id: DocumentId, editor: EditorId) {
    if let Some(mut document) = OpenDocuments::document(store, document_id) {
        document.remove_editor(editor);
        OpenDocuments::put_document(store, document_id, document);
    }
}

pub fn deliver(
    store: &mut Store,
    ui: &imba::UiCtx,
    document_id: DocumentId,
    command: EditorCommand,
    fx: &mut EditorEffects<'_>,
) {
    if let EditorCommand::ApplyReparse(outcome) = command {
        if outcome.anchor().is_none() {
            return;
        }
        let location = OpenDocuments::location(store, document_id);
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let Some(mut document) = OpenDocuments::document(store, document_id) else {
            return;
        };
        document.land_reparse(outcome, location, store, &fonts, &theme, fx);
        OpenDocuments::put_document(store, document_id, document);
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
        _ => return,
    };
    let Some(mut document) = OpenDocuments::document(store, document_id) else {
        return;
    };
    document.perform(store, ui, editor, command, fx);
    OpenDocuments::put_document(store, document_id, document);
}
