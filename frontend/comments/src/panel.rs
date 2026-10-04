// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use editor::{location::ResourceLocation, location::ResourceType};
use imba::list::ActivateTrigger;
use hikit::{forest::ForestList, forest::ForestNode, forest::ForestSearcher, list_keyboard::ListKeyCommand, list_keyboard::ListKeyboardController, modal::ModalRequest, modal::ModalView, tree_item::TreeListCommand};
use imba::list::ListOps;
use imba::{arena::Arena, constraints::Constraints, container::container, effect::Effects, event::{Event, EventResult, Key as InputKey}, leaf::leaf, store::Store, thunk_ext::ThunkExt, ui::UiCtx, View, Widget};
use skia_safe::{Rect, Size};

use crate::view::comments_markup;
use crate::{AnnotationId, CommentRecord, Comments};

const PANEL_PAD: f32 = 6.0;

const COMMENT_KIND: &str = "comment";

#[derive(Clone)]
enum RowItem {
    Branch,

    Comment(AnnotationId),

    Note,
}

#[derive(Default)]
struct DirTrie {
    dirs: BTreeMap<String, DirTrie>,
    files: BTreeMap<String, FileNode>,
}

#[derive(Default)]
struct FileNode {
    location: Option<ResourceLocation>,
    comments: Vec<(AnnotationId, CommentRecord)>,
}

impl DirTrie {
    fn insert(&mut self, rel: &[String], id: AnnotationId, record: CommentRecord) {
        let mut node = self;
        for segment in &rel[..rel.len().saturating_sub(1)] {
            node = node.dirs.entry(segment.clone()).or_default();
        }
        let name = rel.last().cloned().unwrap_or_default();
        let file = node.files.entry(name).or_default();
        file.location = Some(record.location.clone());
        file.comments.push((id, record));
    }
}

fn comment_label(record: &CommentRecord) -> String {
    const LABEL_CAP: usize = 160;
    let head = record
        .entries
        .first()
        .map(|entry| {
            let page = entry.text.page_at(0, entry.text.byte_count(), LABEL_CAP);
            String::from_utf8_lossy(&page).into_owned()
        })
        .unwrap_or_default();
    let mut label = head
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('\u{fffd}')
        .to_owned();
    if label.is_empty() {
        label = "(empty comment)".to_owned();
    }
    let count = record.entries.len();
    if count > 1 {
        label.push_str(&format!("  ·{count}"));
    }
    label
}

fn folder_node(
    folder: &ResourceLocation,
    records: &[(AnnotationId, CommentRecord)],
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
) -> Option<ForestNode<ResourceLocation>> {
    let mut trie = DirTrie::default();
    let mut any = false;
    for (id, record) in records {
        let location = &record.location;
        if location.authority() != folder.authority() {
            continue;
        }
        let path = location.path();
        if path.len() <= folder.path().len() || !path.starts_with(folder.path()) {
            continue;
        }
        trie.insert(&path[folder.path().len()..], id.clone(), record.clone());
        any = true;
    }
    if !any {
        return None;
    }
    items.insert_mut(folder.clone(), RowItem::Branch);
    Some(ForestNode {
        key: folder.clone(),
        label: folder.name().to_owned(),
        pick: false,
        dim: false,
        trail: Vec::new(),
        tint: hikit::tree_item::TreeTint::Label,
        action: None,
        children: dir_children(folder, trie, items),
    })
}

fn dir_children(
    at: &ResourceLocation,
    trie: DirTrie,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
) -> Vec<ForestNode<ResourceLocation>> {
    let mut children = Vec::new();
    for (name, sub) in trie.dirs {
        let mut label = name.clone();
        let mut location = at.child(ResourceType::directory(), &name);
        let mut sub = sub;
        while sub.files.is_empty() && sub.dirs.len() == 1 {
            let (name, inner) = sub.dirs.into_iter().next().expect("the single child");
            label.push('/');
            label.push_str(&name);
            location = location.child(ResourceType::directory(), &name);
            sub = inner;
        }
        items.insert_mut(location.clone(), RowItem::Branch);
        let nested = dir_children(&location, sub, items);
        children.push(ForestNode {
            key: location,
            label,
            pick: false,
            dim: true,
            trail: Vec::new(),
            tint: hikit::tree_item::TreeTint::Label,
            action: None,
            children: nested,
        });
    }
    for (name, file) in trie.files {
        let Some(location) = file.location else {
            continue;
        };
        items.insert_mut(location.clone(), RowItem::Branch);
        let mut leaves = Vec::new();
        let mut comments = file.comments;
        comments.sort_by(|a, b| {
            let line = |record: &CommentRecord| {
                record
                    .range
                    .as_ref()
                    .map(|range| range.start.line)
                    .unwrap_or(0)
            };
            line(&a.1).cmp(&line(&b.1))
        });
        for (id, record) in comments {
            let key = location.child(ResourceType::new(COMMENT_KIND), &id);
            items.insert_mut(key.clone(), RowItem::Comment(id));
            leaves.push(ForestNode {
                key,
                label: comment_label(&record),
                pick: true,
                dim: record.resolved,
                trail: Vec::new(),
                tint: hikit::tree_item::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
        }
        children.push(ForestNode {
            key: location,
            label: name,
            pick: false,
            dim: false,
            trail: Vec::new(),
            tint: hikit::tree_item::TreeTint::Label,
            action: None,
            children: leaves,
        });
    }
    children
}

type Rows = ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>;

#[derive(Clone)]
pub enum CommentsViewCommand {
    Rows(ListKeyCommand<TreeListCommand>),

    SendAll,

    Refresh,

    Dismiss,
}

impl std::fmt::Display for CommentsViewCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentsViewCommand::Rows(command) => command.fmt(out),
            CommentsViewCommand::SendAll => out.write_str("comments send all"),
            CommentsViewCommand::Refresh => out.write_str("comments refresh"),
            CommentsViewCommand::Dismiss => out.write_str("comments dismiss"),
        }
    }
}

pub struct CommentsView {
    list: Rows,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    /// The collection whose records this dock lists.
    comments: imba::store::Id<Comments>,

    seen: u64,
    request: Option<ModalRequest>,
}

impl Clone for CommentsView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            comments: self.comments,
            seen: self.seen,

            request: None,
        }
    }
}

impl CommentsView {
    pub fn open(store: &Store, ui: &UiCtx, comments: imba::store::Id<Comments>) -> Self {
        let mut panel = Self {
            list: ListKeyboardController::searchable(
                ForestList::new(store),
                ForestSearcher::default(),
                store,
                ui,
                editor::env::Fonts::of(store),
            )
            .with_folds(),
            items: rpds::HashTrieMapSync::new_sync(),
            comments,
            seen: 0,
            request: None,
        };
        panel.refresh(store, ui);
        panel
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.list.inner().forest.rows()
    }

    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        self.seen = Comments::generation(store, self.comments);
        let records = Comments::records(store, self.comments);
        let mut items = rpds::HashTrieMapSync::new_sync();
        let mut nodes: Vec<ForestNode<ResourceLocation>> = Comments::folders(store, self.comments)
            .iter()
            .filter_map(|folder| folder_node(folder, &records, &mut items))
            .collect();
        if nodes.is_empty() {
            let note = ResourceLocation::new(
                ResourceType::new("note"),
                editor::location::Authority::new("comments"),
                vec!["empty".to_owned()],
            );
            items.insert_mut(note.clone(), RowItem::Note);
            nodes.push(ForestNode {
                key: note,
                label: "no comments".to_owned(),
                pick: false,
                dim: true,
                trail: Vec::new(),
                tint: hikit::tree_item::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
        }
        self.items = items;
        self.list.inner_mut().set(&nodes, store, ui);
    }

    pub fn activate(&mut self, index: usize, store: &Store, ui: &UiCtx) {
        let Some(key) = self.list.inner().list().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&key, store, ui);
    }

    fn activate_key(&mut self, key: &ResourceLocation, store: &Store, ui: &UiCtx) {
        match self.items.get(key).cloned() {
            Some(RowItem::Branch) => self.list.inner_mut().toggle(key, store, ui),
            Some(RowItem::Comment(id)) => {
                self.list.inner_mut().list_mut().select_only(key.clone());
                let Some(record) = Comments::record(store, self.comments, &id) else {
                    return;
                };
                // The caret target: the card's LIVE range when its
                // document is open, else the stored one — resolved
                // here; the shell only supplies the window.
                let target = live_range(store, self.comments, &id)
                    .or(record.range.clone())
                    .unwrap_or(
                        documents::text_ext::LineCol { line: 0, col: 0 }..documents::text_ext::LineCol {
                            line: 0,
                            col: 0,
                        },
                    );
                self.request = Some(ModalRequest::OpenAt {
                    location: record.location,
                    target: Some(target),
                    focus: false,
                });
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for CommentsView {
    type Command = CommentsViewCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CommentsViewCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's (docs/ui/list-keyboard.md
        // §3); the surface keeps only its own dismissal.
        let searching = self.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => {
                    EventResult::Command(CommentsViewCommand::Dismiss)
                }
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            self.list
                .focus_data(store, ui)
                .map(CommentsViewCommand::Rows),
        )
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::ui::UiCtx::dont_use_too_slow();
        fx.scope(CommentsViewCommand::Rows, |fx| {
            self.list.clear(store, ui, fx)
        });
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            CommentsViewCommand::Rows(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.list.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = hikit::tree_item::tree_toggle(inner) {
                            return self.activate(index, store, ui);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Rows::activated(&command) {
                    let searching = self.list.searching();
                    self.activate(index, store, ui);
                    // The deliberate pick ends the search in the same
                    // stroke; a browsing click leaves it standing.
                    match trigger {
                        ActivateTrigger::Enter if searching => {
                            return self.perform(
                                store,
                                ui,
                                CommentsViewCommand::Rows(ListKeyCommand::Clear),
                                fx,
                            );
                        }
                        ActivateTrigger::Enter | ActivateTrigger::Click => {}
                    }
                }
                fx.scope(CommentsViewCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            CommentsViewCommand::Refresh => self.refresh(store, ui),
            CommentsViewCommand::SendAll => {
                let ids: Vec<AnnotationId> = self
                    .items
                    .iter()
                    .filter_map(|(_, item)| match item {
                        RowItem::Comment(id) => Some(id.clone()),
                        _ => None,
                    })
                    .collect();
                if ids.is_empty() {
                    return;
                }
                self.request = Some(ModalRequest::Perform(imba::command::Verb::Dynamic(
                    Arc::new(crate::view::SendComments {
                        comments: self.comments,
                        ids,
                    }),
                )));
            }
            CommentsViewCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            let chrome = editor::env::Themes::of(store).ui().peeker.clone();
            let chip_height = chrome.hint_size * 2.0;
            let band = chrome.margin + chip_height + PANEL_PAD;
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(size.width, size.height - band)),
            )
            .map(CommentsViewCommand::Rows);
            overlay.place(0.0, band, rows);

            let chip_font = hikit::fonts::ui_font(ui, chrome.hint_size);
            let advance = chip_font.measure_str("SEND ALL", None).0;
            let chip_width = advance + chrome.hint_size * 2.0;
            let chip_radius = chrome.well_radius;
            let rule = chrome.rule.0;
            let dim = chrome.dim_text.0;
            let chip = leaf::<CommentsViewCommand>(chip_width, chip_height)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut paint = skia_safe::Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.0);
                    paint.set_color(rule);
                    canvas.draw_round_rect(
                        rect.with_inset((0.5, 0.5)),
                        chip_radius,
                        chip_radius,
                        &paint,
                    );
                    paint.set_stroke(false);
                    paint.set_color(dim);
                    canvas.draw_str(
                        "SEND ALL",
                        (
                            rect.left + (rect.width() - advance) * 0.5,
                            rect.top + rect.height() * 0.5 + 6.0,
                        ),
                        &chip_font,
                        &paint,
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(CommentsViewCommand::SendAll),
                    _ => EventResult::Ignored,
                });

            // The key table lives in the controller's own overlay;
            // the surface keeps only its dismissal.
            let searching = self.list.searching();
            let keymap = leaf::<CommentsViewCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(CommentsViewCommand::Dismiss),
                    _ => EventResult::Ignored,
                },
            );
            overlay.place(0.0, 0.0, keymap);
            overlay.place(
                (size.width - chrome.margin - chip_width).max(0.0),
                chrome.margin,
                chip,
            );

            let stale = Comments::generation(store, self.comments) != self.seen;
            overlay.wrap(move |inner| ReconcileShell { inner, stale })
        })
    }
}

struct ReconcileShell<Inner> {
    inner: Inner,
    stale: bool,
}

impl<'a, Inner: Widget<'a, CommentsViewCommand>> Widget<'a, CommentsViewCommand>
    for ReconcileShell<Inner>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CommentsViewCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CommentsViewCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.stale {
            return result.merge(EventResult::Command(CommentsViewCommand::Refresh));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, CommentsViewCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

impl ModalView for CommentsView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn live_range(
    store: &Store,
    comments: imba::store::Id<Comments>,
    annotation: &AnnotationId,
) -> Option<std::ops::Range<documents::text_ext::LineCol>> {
    let (document, key) = Comments::card(store, comments, annotation)?;
    let documents = Comments::documents_of(store, comments)?;
    let doc = documents::OpenDocuments::document_ref(store, documents, document)?;
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let markup = doc.feature_markup(comments_markup())?;
    let extras = [(comments_markup(), markup)];
    let interval = editor::markup::OverlaidMarkup::new(doc.markup(), &extras)
        .all_inlays_in(0..byte_count)
        .into_iter()
        .find(|interval| interval.key == key)?;
    let mut view = doc.text().view();
    Some(
        documents::text_ext::line_col_at(&mut view, interval.range.start as usize)
            ..documents::text_ext::line_col_at(&mut view, interval.range.end as usize),
    )
}
