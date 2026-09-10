use std::ops::Range;

use imba::effect::{Effect, Effects};
use imba::store::Store;
use operation::{Op, Operation};
use std::sync::Arc;

use text::{LineNumber, Text, TextView};

use ::editor::Document;
use editor::ResourceLocation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

pub fn line_col_at(view: &mut TextView, offset: usize) -> LineCol {
    let line = view.line_at(offset);
    let start = view.line_start_offset(line);
    LineCol {
        line: line.0 as u32,
        col: (offset - start) as u32,
    }
}

pub fn offset_at(view: &mut TextView, position: LineCol) -> usize {
    let last = view.line_count().0 - 1;
    let line = LineNumber((position.line as usize).min(last));
    let start = view.line_start_offset(line);
    let end = view.line_end_offset(line);
    let content_end = match line.0 < last {
        true => end - 1,
        false => end,
    };
    (start + position.col as usize).min(content_end)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Range<LineCol>,
    pub text: String,
}

pub struct DocumentChangeEffect {
    pub folders: Vec<ResourceLocation>,

    pub location: ResourceLocation,

    pub base_revision: u64,

    pub revision: u64,

    pub changes: Vec<TextChange>,

    pub text: Text,
}

impl Effect for DocumentChangeEffect {
    type Result = ();
}

pub type FolderSource = Arc<dyn Fn(&Store) -> Vec<editor::ResourceLocation> + Send + Sync>;

#[derive(Clone, Default)]
pub struct ChangeObserver;

impl ChangeObserver {
    pub fn install(store: &mut Store, folders: FolderSource) {
        if store.get::<ChangeObserver>().is_none() {
            store.put(ChangeObserver);
        }

        ::editor::InstalledChangeSink::install(
            store,
            std::sync::Arc::new(ObserverSink { folders }),
        );
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<ChangeObserver>().is_some()
    }
}

pub fn notify_change<R: 'static>(
    store: &Store,
    document_id: crate::DocumentId,
    document: &Document,
    base_revision: u64,
    text_before: &Text,
    folders: Vec<editor::ResourceLocation>,
    fx: &mut Effects<'_, R>,
) {
    if !ChangeObserver::installed(store) {
        return;
    }
    let Some(entity) = crate::OpenDocuments::entity(store, document_id) else {
        return;
    };
    let Some(location) = entity.location else {
        return;
    };
    notify_change_at(location, document, base_revision, text_before, folders, fx)
}

struct ObserverSink {
    folders: FolderSource,
}

impl ::editor::ChangeSink for ObserverSink {
    fn changed(
        &self,
        store: &Store,
        document: &Document,
        location: &editor::ResourceLocation,
        base_revision: u64,
        text_before: &Text,
        fx: &mut Effects<'_, ::editor::EditorCommand>,
    ) {
        notify_change_at(
            location.clone(),
            document,
            base_revision,
            text_before,
            (self.folders)(store),
            fx,
        )
    }
}

fn notify_change_at<R: 'static>(
    location: editor::ResourceLocation,
    document: &Document,
    base_revision: u64,
    text_before: &Text,
    folders: Vec<editor::ResourceLocation>,
    fx: &mut Effects<'_, R>,
) {
    if document.revision() == base_revision {
        return;
    }
    let Some(composed) = document.log().compose_since(base_revision) else {
        return;
    };
    fx.notify(DocumentChangeEffect {
        folders,
        location,
        base_revision,
        revision: document.revision(),
        changes: text_changes(text_before, &composed),
        text: document.text().clone(),
    });
}

fn text_changes(before: &Text, operation: &Operation) -> Vec<TextChange> {
    struct Group {
        start: u32,
        deleted: u32,
        inserted: String,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut offset = 0u32;
    let mut open = false;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                open = false;
                offset += len;
            }
            Op::Delete(text) => {
                if !open {
                    groups.push(Group {
                        start: offset,
                        deleted: 0,
                        inserted: String::new(),
                    });
                    open = true;
                }
                let len = text.len() as u32;
                groups.last_mut().expect("just opened").deleted += len;
                offset += len;
            }
            Op::Insert(text) => {
                if !open {
                    groups.push(Group {
                        start: offset,
                        deleted: 0,
                        inserted: String::new(),
                    });
                    open = true;
                }
                groups
                    .last_mut()
                    .expect("just opened")
                    .inserted
                    .push_str(&text);
            }
        }
    }

    let mut view = before.view();
    let mut changes: Vec<TextChange> = groups
        .into_iter()
        .map(|group| TextChange {
            range: line_col_at(&mut view, group.start as usize)
                ..line_col_at(&mut view, (group.start + group.deleted) as usize),
            text: group.inserted,
        })
        .collect();
    changes.reverse();
    changes
}

#[cfg(test)]
mod tests;
