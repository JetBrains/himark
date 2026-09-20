// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    AppCommand, ForestList, ForestNode, ForestSearcher, ModalRequest, ModalView, ResourceLocation,
    ResourceType, SpeedSearchCommand, SpeedSearchView, TreeListCommand,
};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View, Widget,
};
use skia_safe::{Rect, Size};

use crate::hicomments::comments_markup;
use crate::hicomments::sync::{AnnotationId, CommentRecord, Comments};

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
        tint: crate::TreeTint::Label,
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
            tint: crate::TreeTint::Label,
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
                tint: crate::TreeTint::Label,
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
            tint: crate::TreeTint::Label,
            action: None,
            children: leaves,
        });
    }
    children
}

pub enum CommentsCommand {
    Rows(SpeedSearchCommand<TreeListCommand>),

    SendAll,

    Select(isize),

    Fold(bool),

    Pick,

    Refresh,

    Dismiss,
}

pub struct CommentsView {
    list: SpeedSearchView<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    workspace: crate::SessionId,
    window: crate::WindowId,

    seen: u64,
    request: Option<ModalRequest>,
}

impl Clone for CommentsView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            workspace: self.workspace.clone(),
            window: self.window,
            seen: self.seen,

            request: None,
        }
    }
}

impl CommentsView {
    pub fn open(
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
        workspace: crate::SessionId,
    ) -> Self {
        let mut panel = Self {
            list: SpeedSearchView::new(
                ForestList::new(store),
                ForestSearcher::default(),
                store, ui,
                crate::env::Fonts::of(store),
            ),
            items: rpds::HashTrieMapSync::new_sync(),
            workspace,
            window,
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
        self.seen = Comments::generation(store);
        let records = Comments::records(store);
        let mut items = rpds::HashTrieMapSync::new_sync();
        let mut nodes: Vec<ForestNode<ResourceLocation>> =
            crate::higent::session_folders(store, &self.workspace)
                .iter()
                .filter_map(|folder| folder_node(folder, &records, &mut items))
                .collect();
        if nodes.is_empty() {
            let note = ResourceLocation::new(
                ResourceType::new("note"),
                crate::Authority::new("comments"),
                vec!["empty".to_owned()],
            );
            items.insert_mut(note.clone(), RowItem::Note);
            nodes.push(ForestNode {
                key: note,
                label: "no comments".to_owned(),
                pick: false,
                dim: true,
                trail: Vec::new(),
                tint: crate::TreeTint::Label,
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

                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(NavigateToComment { annotation: id }),
                )));
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for CommentsView {
    type Command = CommentsCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CommentsCommand> {
        use imba::focus::FocusData;
        let searching = self.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(CommentsCommand::Dismiss),
                InputKey::Up if !searching => EventResult::Command(CommentsCommand::Select(-1)),
                InputKey::Down if !searching => EventResult::Command(CommentsCommand::Select(1)),
                InputKey::Left if !searching => EventResult::Command(CommentsCommand::Fold(false)),
                InputKey::Right if !searching => EventResult::Command(CommentsCommand::Fold(true)),
                InputKey::Enter if searching => EventResult::Commands(vec![
                    CommentsCommand::Pick,
                    CommentsCommand::Rows(SpeedSearchCommand::Clear),
                ]),
                InputKey::Enter => EventResult::Command(CommentsCommand::Pick),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.list.focus_data(store, ui).map(CommentsCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
        fx.scope(CommentsCommand::Rows, |fx| self.list.clear(store, ui, fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            CommentsCommand::Rows(command) => {
                if let SpeedSearchCommand::Inner(inner) = &command {
                    if let Some((index, _)) = crate::tree_interaction(inner) {
                        return self.activate(index, store, ui);
                    }
                }
                fx.scope(CommentsCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            CommentsCommand::Select(delta) => self.list.inner_mut().list_mut().cursor_step(delta),
            CommentsCommand::Fold(expand) => self.list.inner_mut().fold_cursor(expand, store, ui),
            CommentsCommand::Pick => {
                if let Some(key) = self.list.inner().list().cursor().cloned() {
                    self.activate_key(&key, store, ui);
                }
            }
            CommentsCommand::Refresh => self.refresh(store, ui),
            CommentsCommand::SendAll => {
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
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::hicomments::SendComments { ids }),
                )));
            }
            CommentsCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            let chrome = crate::env::Themes::of(store).ui().peeker.clone();
            let chip_height = chrome.hint_size * 2.0;
            let band = chrome.margin + chip_height + PANEL_PAD;
            let rows = imba::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(size.width, size.height - band)),
            )
            .map(CommentsCommand::Rows);
            overlay.place(0.0, band, rows);

            let chip_font = crate::fonts::ui_font(ui, chrome.hint_size);
            let advance = chip_font.measure_str("SEND ALL", None).0;
            let chip_width = advance + chrome.hint_size * 2.0;
            let chip_radius = chrome.well_radius;
            let rule = chrome.rule.0;
            let dim = chrome.dim_text.0;
            let chip = leaf::<CommentsCommand>(chip_width, chip_height)
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
                    Event::MouseDown { .. } => EventResult::Command(CommentsCommand::SendAll),
                    _ => EventResult::Ignored,
                });

            let searching = self.list.searching();
            let keymap = leaf::<CommentsCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(CommentsCommand::Dismiss),
                    Event::KeyDown {
                        key: InputKey::Up, ..
                    } if !searching => EventResult::Command(CommentsCommand::Select(-1)),
                    Event::KeyDown {
                        key: InputKey::Down,
                        ..
                    } if !searching => EventResult::Command(CommentsCommand::Select(1)),
                    Event::KeyDown {
                        key: InputKey::Left,
                        ..
                    } if !searching => EventResult::Command(CommentsCommand::Fold(false)),
                    Event::KeyDown {
                        key: InputKey::Right,
                        ..
                    } if !searching => EventResult::Command(CommentsCommand::Fold(true)),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } if searching => EventResult::Commands(vec![
                        CommentsCommand::Pick,
                        CommentsCommand::Rows(SpeedSearchCommand::Clear),
                    ]),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } => EventResult::Command(CommentsCommand::Pick),
                    _ => EventResult::Ignored,
                },
            );
            overlay.place(0.0, 0.0, keymap);
            overlay.place(
                (size.width - chrome.margin - chip_width).max(0.0),
                chrome.margin,
                chip,
            );

            let stale = Comments::generation(store) != self.seen;
            overlay.wrap(move |inner| ReconcileShell { inner, stale })
        })
    }
}

struct ReconcileShell<Inner> {
    inner: Inner,
    stale: bool,
}

impl<'a, Inner: Widget<'a, CommentsCommand>> Widget<'a, CommentsCommand> for ReconcileShell<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CommentsCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CommentsCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.stale {
            return result.merge(EventResult::Command(CommentsCommand::Refresh));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, CommentsCommand>
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

struct NavigateToComment {
    annotation: AnnotationId,
}

impl crate::DynamicCommand for NavigateToComment {
    fn id(&self) -> &'static str {
        "comments.navigate"
    }
    fn name(&self) -> String {
        "Go to Comment".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(record) = Comments::record(store, &self.annotation) else {
            return;
        };
        let target = live_range(store, &self.annotation)
            .or(record.range.clone())
            .unwrap_or(crate::LineCol { line: 0, col: 0 }..crate::LineCol { line: 0, col: 0 });
        match crate::OpenDocuments::by_location(store, &record.location) {
            Some(document) => {
                let Some(mut entity) = crate::Windows::window(store, window) else {
                    return;
                };
                entity.show_document(store, ui, window, document, Some(target), fx);
                crate::Windows::put(store, window, entity);
            }
            None => {
                fx.push(crate::open_by_location_effect(
                    window,
                    record.location.clone(),
                    true,
                    Some(target),
                ));
            }
        }
    }
}

fn live_range(store: &Store, annotation: &AnnotationId) -> Option<std::ops::Range<crate::LineCol>> {
    let (document, key) = Comments::card(store, annotation)?;
    let doc = crate::OpenDocuments::document_ref(store, document)?;
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let markup = doc.feature_markup(comments_markup())?;
    let extras = [(comments_markup(), markup)];
    let interval = crate::OverlaidMarkup::new(doc.markup(), &extras)
        .all_inlays_in(0..byte_count)
        .into_iter()
        .find(|interval| interval.key == key)?;
    let mut view = doc.text().view();
    Some(
        crate::line_col_at(&mut view, interval.range.start as usize)
            ..crate::line_col_at(&mut view, interval.range.end as usize),
    )
}

pub struct ToggleCommentsView;

impl crate::DynamicCommand for ToggleCommentsView {
    fn id(&self) -> &'static str {
        "comments.view"
    }
    fn name(&self) -> String {
        "Comments".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }
        let workspace = entity.current_session();
        for folder in crate::higent::session_folders(store, &workspace) {
            Comments::ensure(store, window, &folder, fx);
        }

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = CommentsView::open(store, &_app.ui_ctx(), window, workspace);
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "comments.view",
        order: 1.5,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let body = Rect::from_xywh(l + w * 0.18, t + h * 0.2, w * 0.64, h * 0.44);
            let radius = h * 0.12;
            canvas.draw_round_rect(body, radius, radius, &paint);
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l + w * 0.34, t + h * 0.64));
            path.line_to((l + w * 0.30, t + h * 0.8));
            path.line_to((l + w * 0.46, t + h * 0.64));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}
