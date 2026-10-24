// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    layout::LayoutExt as _,
    list::{ListCommand, ListSlice, ListView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    Thunk, View,
};
use skia_safe::Size;

use crate::cell::{Cell, CellCommand, CellKind};
use editor::env;
use hikit::{tree_item::TreeItemCommand, tree_item::TreeItemView};

#[derive(Clone)]
pub struct ToolFace {
    /// The call in one line — what the oracle reads.
    pub line: String,

    /// What the row shows after the tool's name: the invocation (a
    /// command, a path), or where the call stands.
    pub detail: String,

    /// A FAILED call's reason, one line — its own mono line under
    /// the row.
    pub error: Option<String>,

    pub markdown: text::text::Text,
    pub failed: bool,

    pub live: bool,
}

#[derive(Clone)]
pub struct ToolCallSpec {
    pub id: String,
    pub display_name: String,
    pub face: ToolFace,
}

#[derive(Clone)]
pub enum ToolUpdate {
    Add(ToolCallSpec),

    Face { id: String, face: ToolFace },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ToolRowKey {
    Group,
    Face(String),
    Body(String),
}

#[derive(Clone)]
pub enum ToolRowCommand {
    Cell(Box<CellCommand>),
}

impl std::fmt::Display for ToolRowCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolRowCommand::Cell(command) => command.fmt(out),
        }
    }
}

pub(crate) type ToolRowsCommand = ListCommand<TreeItemCommand<ToolRowCommand>>;

/// One row's words: a call's name and its invocation, or a run's
/// names with their counts ("Bash ×3").
#[derive(Clone)]
pub(crate) struct FaceLine {
    names: Vec<(String, usize)>,
    detail: String,
    error: Option<String>,
    open: bool,
    failed: bool,
    live: bool,
}

#[derive(Clone)]
pub(crate) enum ToolRowView {
    Face(FaceLine),
    Body(Cell),
}

impl View for ToolRowView {
    type Command = ToolRowCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        if let Self::Body(cell) = self {
            fx.scope(
                |command| ToolRowCommand::Cell(Box::new(command)),
                |fx| cell.destroy(store, fx),
            );
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        let (Self::Body(cell), ToolRowCommand::Cell(command)) = (self, command) else {
            return;
        };
        fx.scope(
            |command| ToolRowCommand::Cell(Box::new(command)),
            |fx| cell.perform(store, ui, *command, fx),
        );
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width.max(1.0);
            match self {
                Self::Body(cell) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        cell.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(|command| ToolRowCommand::Cell(Box::new(command))),
                ),
                Self::Face(face) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        face_row(arena, store, ui, face, width),
                        arena,
                        constraints,
                    ),
                ),
            }
        })
    }
}

#[derive(Clone)]
struct CallEntry {
    id: String,
    display_name: String,
    face: ToolFace,

    expanded: bool,
}

#[derive(Clone)]
pub(crate) struct ToolGroup {
    calls: rpds::VectorSync<CallEntry>,

    expanded: bool,

    touched: bool,
    rows: ListView<TreeItemView<ToolRowView>, ToolRowKey>,
    width: f32,
}

impl ToolGroup {
    pub(crate) fn new(
        store: &Store,
        ui: &UiCtx,
        specs: Vec<ToolCallSpec>,
        width: f32,
    ) -> (Self, f32) {
        let calls: rpds::VectorSync<CallEntry> = specs
            .into_iter()
            .map(|spec| CallEntry {
                id: spec.id,
                display_name: spec.display_name,
                face: spec.face,
                expanded: false,
            })
            .collect();
        let expanded = calls.iter().any(|call| call.face.live);
        let mut group = Self {
            calls,
            expanded,
            touched: false,
            rows: ListView::empty_at(width),
            width,
        };
        group.rebuild(store, ui);
        let height = group.rows.total_height();
        (group, height)
    }

    /// The rows' standing height — the cell's height accounting reads
    /// it after any command that may have grown or folded a row.
    pub(crate) fn rows_height(&self) -> f32 {
        self.rows.total_height()
    }

    pub(crate) fn oracle(&self) -> Vec<String> {
        let mut out = Vec::new();
        for index in 0..self.rows.len() {
            let key = self.rows.key_at(index).cloned();
            let depth = self
                .rows
                .rows_from(index)
                .next()
                .map(|row| usize::from(row.depth()))
                .unwrap_or(0);
            let indent = "  ".repeat(depth);
            match key {
                Some(ToolRowKey::Group) => out.push(format!(
                    "{indent}{} {}",
                    chevron(self.expanded),
                    self.summary()
                )),
                Some(ToolRowKey::Face(id)) => {
                    let call = self.call(&id);
                    let open = call.map(|call| call.expanded).unwrap_or(false);
                    let line = call.map(|call| call.face.line.clone()).unwrap_or_default();
                    out.push(format!("{indent}{} {line}", chevron(open)));
                }
                Some(ToolRowKey::Body(id)) => {
                    let markdown = self
                        .call(&id)
                        .map(|call| materialize(&call.face.markdown))
                        .unwrap_or_default();
                    out.push(format!("{indent}[body] {markdown}"));
                }
                None => out.push(format!("{indent}?")),
            }
        }
        out
    }

    pub(crate) fn summary(&self) -> String {
        self.counts()
            .into_iter()
            .map(|(name, count)| format!("{count} × {name}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The run's tools in first-call order, each with how many times
    /// it ran.
    fn counts(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for call in self.calls.iter() {
            match counts.iter_mut().find(|(name, _)| *name == call.display_name) {
                Some((_, count)) => *count += 1,
                None => counts.push((call.display_name.clone(), 1)),
            }
        }
        counts
    }

    fn call(&self, id: &str) -> Option<&CallEntry> {
        self.calls.iter().find(|call| call.id == id)
    }

    fn call_index(&self, id: &str) -> Option<usize> {
        self.calls.iter().position(|call| call.id == id)
    }

    fn has_group_row(&self) -> bool {
        self.calls.len() > 1
    }

    fn depth(&self) -> u16 {
        u16::from(self.has_group_row())
    }

    pub(crate) fn update(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        update: ToolUpdate,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        match update {
            ToolUpdate::Add(spec) => {
                if self.call_index(&spec.id).is_some() {
                    return self.update(
                        store,
                        ui,
                        ToolUpdate::Face {
                            id: spec.id,
                            face: spec.face,
                        },
                        fx,
                    );
                }
                self.calls.push_back_mut(CallEntry {
                    id: spec.id,
                    display_name: spec.display_name,
                    face: spec.face,
                    expanded: false,
                });
                self.follow_the_work();

                self.resync(store, ui, fx);
            }
            ToolUpdate::Face { id, face } => {
                let Some(index) = self.call_index(&id) else {
                    return;
                };
                // A run re-dress hands EVERY call back; a face that did
                // not move must not splice rows or rewrite bodies.
                let held = &self.calls[index].face;
                if held.line == face.line
                    && held.detail == face.detail
                    && held.error == face.error
                    && held.failed == face.failed
                    && held.live == face.live
                    && held.markdown == face.markdown
                {
                    return;
                }
                let mut call = self.calls[index].clone();
                let was_open = call.expanded;
                call.face = face;
                self.calls.set_mut(index, call);
                self.follow_the_work();

                if was_open && self.expanded {
                    let markdown = self.calls[index].face.markdown.clone();
                    self.route(
                        store,
                        ui,
                        ToolRowKey::Body(id.clone()),
                        TreeItemCommand::Inner(ToolRowCommand::Cell(Box::new(
                            CellCommand::Rewrite(markdown),
                        ))),
                        fx,
                    );
                }
                self.resync(store, ui, fx);
            }
        }
    }

    fn follow_the_work(&mut self) {
        if self.touched {
            return;
        }
        self.expanded = self.calls.iter().any(|call| call.face.live);
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ToolRowsCommand,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        if let ListCommand::Focus(index, then) = command {
            fx.scope(CellCommand::ToolRows, |fx| {
                self.rows
                    .perform(store, ui, ListCommand::Focus(index, None), fx)
            });
            let Some(then) = then else {
                return;
            };
            return self.perform(store, ui, *then, fx);
        }
        if let ListCommand::Child(index, TreeItemCommand::Toggle) = &command {
            let key = self.rows.key_at(*index).cloned();
            if let Some(key) = key {
                self.toggle(store, ui, key, fx);
            }
            return;
        }
        let ListCommand::Child(index, inner) = command else {
            return fx.scope(CellCommand::ToolRows, |fx| {
                self.rows.perform(store, ui, command, fx)
            });
        };
        let Some(key) = self.rows.key_at(index).cloned() else {
            return;
        };
        self.route(store, ui, key, inner, fx);
    }

    pub(crate) fn perform_keyed(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ToolRowKey,
        command: TreeItemCommand<ToolRowCommand>,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        if matches!(command, TreeItemCommand::Toggle) {
            self.toggle(store, ui, key, fx);
            return;
        }
        self.route(store, ui, key, command, fx);
    }

    fn route(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ToolRowKey,
        command: TreeItemCommand<ToolRowCommand>,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let Some(range) = self.rows.row_range(&key) else {
            return;
        };
        let index = range.start;
        let lifted = key.clone();
        fx.scope(
            move |command: ToolRowsCommand| lift(&lifted, command),
            |fx| {
                self.rows
                    .perform(store, ui, ListCommand::Child(index, command), fx)
            },
        );
    }

    fn toggle(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ToolRowKey,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        match key {
            ToolRowKey::Group => {
                self.expanded = !self.expanded;
                self.touched = true;
            }
            ToolRowKey::Face(id) | ToolRowKey::Body(id) => {
                let Some(index) = self.call_index(&id) else {
                    return;
                };
                let mut call = self.calls[index].clone();
                call.expanded = !call.expanded;
                self.calls.set_mut(index, call);
            }
        }
        self.resync(store, ui, fx);
    }

    fn resync(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut Effects<'_, CellCommand>) {
        let wanted = self.wanted_keys();
        let present: Vec<ToolRowKey> = (0..self.rows.len())
            .filter_map(|index| self.rows.key_at(index).cloned())
            .collect();
        if wanted == present {
            self.refresh_faces(store, ui);
            return;
        }

        for key in &present {
            if let ToolRowKey::Body(id) = key {
                if !wanted.contains(key) {
                    self.destroy_row(store, ui, ToolRowKey::Body(id.clone()), fx);
                }
            }
        }

        let mut head = 0;
        while head < wanted.len() && head < present.len() && wanted[head] == present[head] {
            head += 1;
        }
        let mut tail = 0;
        while tail < wanted.len() - head
            && tail < present.len() - head
            && wanted[wanted.len() - 1 - tail] == present[present.len() - 1 - tail]
        {
            tail += 1;
        }

        let head = head.saturating_sub(1);
        let mut slice = ListSlice::new();
        for key in &wanted[head..wanted.len() - tail] {
            let key = key.clone();
            match &key {
                // Group and face rows measure themselves; a BODY is a
                // cell whose height depends on the laid width, so it
                // keeps its width-true measurement.
                ToolRowKey::Group => slice.push_keyed(key, self.group_row(store, ui), store, ui),
                ToolRowKey::Face(id) => {
                    let view = self.face_row(store, ui, id);
                    slice.push_keyed(key, view, store, ui)
                }
                ToolRowKey::Body(id) => {
                    let (view, height) = match self.rows.row_range(&key) {
                        Some(range) => {
                            let view = self.rows.rows_from(range.start).next();
                            match view {
                                Some(view) => {
                                    let height = self.measure(store, ui, &view);
                                    (view, height)
                                }
                                None => self.body_row(store, ui, id, fx),
                            }
                        }
                        None => self.body_row(store, ui, id, fx),
                    };
                    slice.push_keyed_sized(key, view, height);
                }
            }
        }
        self.rows
            .splice_slice_animated(head..present.len() - tail, slice);
    }

    fn wanted_keys(&self) -> Vec<ToolRowKey> {
        let mut keys = Vec::new();
        if self.has_group_row() {
            keys.push(ToolRowKey::Group);
        }
        if self.expanded || !self.has_group_row() {
            for call in self.calls.iter() {
                keys.push(ToolRowKey::Face(call.id.clone()));
                if call.expanded {
                    keys.push(ToolRowKey::Body(call.id.clone()));
                }
            }
        }
        keys
    }

    fn rebuild(&mut self, store: &Store, ui: &UiCtx) {
        let mut slice = ListSlice::new();
        for key in self.wanted_keys() {
            let view = match &key {
                ToolRowKey::Group => self.group_row(store, ui),
                ToolRowKey::Face(id) => self.face_row(store, ui, id),

                ToolRowKey::Body(_) => continue,
            };
            slice.push_keyed(key, view, store, ui);
        }
        let len = self.rows.len();
        self.rows.splice_slice(0..len, slice);
    }

    fn refresh_faces(&mut self, store: &Store, ui: &UiCtx) {
        let keys: Vec<(usize, String)> = (0..self.rows.len())
            .filter_map(|index| match self.rows.key_at(index) {
                Some(ToolRowKey::Face(id)) => Some((index, id.clone())),
                _ => None,
            })
            .collect();
        for (index, id) in keys {
            let view = self.face_row(store, ui, &id);
            let mut slice = ListSlice::new();
            slice.push_keyed(ToolRowKey::Face(id), view, store, ui);
            self.rows.splice_slice(index..index + 1, slice);
        }
        if self.has_group_row() {
            let view = self.group_row(store, ui);
            let mut slice = ListSlice::new();
            slice.push_keyed(ToolRowKey::Group, view, store, ui);
            self.rows.splice_slice(0..1, slice);
        }
    }

    fn group_row(&self, store: &Store, ui: &UiCtx) -> TreeItemView<ToolRowView> {
        let failed = self.calls.iter().any(|call| call.face.failed);
        let live = self.calls.iter().any(|call| call.face.live);
        let view = TreeItemView::branch(
            ToolRowView::Face(FaceLine {
                names: self.counts(),
                detail: String::new(),
                error: None,
                open: self.expanded,
                failed,
                live,
            }),
            0,
            self.expanded,
        )
        .toggling_on_body();
        let _ = (store, ui);
        view
    }

    fn face_row(&self, store: &Store, ui: &UiCtx, id: &str) -> TreeItemView<ToolRowView> {
        let call = self.call(id);
        let name = call.map(|call| call.display_name.clone()).unwrap_or_default();
        let detail = call.map(|call| call.face.detail.clone()).unwrap_or_default();
        let error = call.and_then(|call| call.face.error.clone());
        let failed = call.map(|call| call.face.failed).unwrap_or(false);
        let live = call.map(|call| call.face.live).unwrap_or(false);
        let open = call.map(|call| call.expanded).unwrap_or(false);
        let view = TreeItemView::branch(
            ToolRowView::Face(FaceLine {
                names: vec![(name, 1)],
                detail,
                error,
                open,
                failed,
                live,
            }),
            self.depth(),
            open,
        )
        .toggling_on_body();
        let _ = (store, ui);
        view
    }

    fn body_row(
        &self,
        store: &Store,
        ui: &UiCtx,
        id: &str,
        fx: &mut Effects<'_, CellCommand>,
    ) -> (TreeItemView<ToolRowView>, f32) {
        let call = self.call(id);
        let markdown = call
            .map(|call| call.face.markdown.clone())
            .unwrap_or_else(text::text::Text::default);
        let kind = match call.map(|call| call.face.failed).unwrap_or(false) {
            true => CellKind::Error,
            false => CellKind::Notice,
        };
        let key = ToolRowKey::Body(id.to_owned());
        let width = self.body_width(store);
        let (cell, height) = fx.scope(
            move |command: ::editor::editor_view::EditorCommand| CellCommand::ToolRow {
                key: key.clone(),
                command: TreeItemCommand::Inner(ToolRowCommand::Cell(Box::new(
                    CellCommand::Editor(command),
                ))),
            },
            |fx| Cell::build_bare(store, ui, kind, markdown, width, fx),
        );
        (
            TreeItemView::leaf(ToolRowView::Body(cell), self.depth() + 1),
            height,
        )
    }

    fn destroy_row(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ToolRowKey,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let Some(range) = self.rows.row_range(&key) else {
            return;
        };
        let Some(mut view) = self.rows.rows_from(range.start).next() else {
            return;
        };
        let _ = ui;
        let lifted = key.clone();
        fx.scope(
            move |command: TreeItemCommand<ToolRowCommand>| CellCommand::ToolRow {
                key: lifted.clone(),
                command,
            },
            |fx| view.destroy(store, fx),
        );
    }

    fn body_width(&self, store: &Store) -> f32 {
        let tree = env::Themes::of(store).part(::hikit::chrome::TreeChrome::KEY);
        let offset = f32::from(self.depth() + 1) * tree.indent + tree.text_x;
        (self.width - offset).max(120.0)
    }

    fn measure(&self, store: &Store, ui: &UiCtx, view: &TreeItemView<ToolRowView>) -> f32 {
        imba::layout::Layout::layout(
            view.display(&Arena::default(), store, ui),
            &Arena::default(),
            Constraints {
                min: Size::default(),
                max: Size::new(self.width.max(1.0), f32::MAX),
            },
        )
        .size()
        .height
    }

    pub(crate) fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, CellCommand>) {
        fx.scope(CellCommand::ToolRows, |fx| self.rows.destroy(store, fx));
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, ToolRowsCommand> + 'a {
        imba::layout::Layout::layout(self.rows.display(arena, store, ui), arena, constraints)
    }
}

/// A tool call's ROW (docs/Design.md: a tool call is its own row in
/// the text column, never a label on the prose): the disclosure
/// chevron hanging into the gutter, the tool's name in bold, then the
/// run's count or the call's invocation in mono, ellipsized. A failed
/// call reads in the removed ink and carries its reason on a mono
/// line of its own.
fn face_row<'a>(
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    face: &FaceLine,
    width: f32,
) -> impl imba::layout::Layout<'a, ToolRowCommand> + imba::layout::LayoutValue + 'a {
    let theme = env::Themes::of(store);
    let chat = theme.part(::hikit::chrome::ChatChrome::KEY);
    let ink = theme.part(::editor::theme::InkChrome::KEY);
    let shaper = imba::layout::TextShaper::of(ui);
    let style = |font: skia_safe::Font, color: skia_safe::Color| hikit::ui::TextStyle {
        font,
        color,
        tracking: 0.0,
        shaper: shaper.clone(),
    };
    let (name_ink, word_ink, detail_ink) = match (face.failed, face.live) {
        (true, _) => (ink.removed.0, ink.removed.0, ink.ink_2.0),
        (false, true) => (ink.ink.0, ink.ink_3.0, ink.ink_2.0),
        (false, false) => (ink.ink_2.0, ink.ink_3.0, ink.ink_3.0),
    };
    let name = style(hikit::fonts::role_font(ui, &chat.tool_name), name_ink);
    let word = style(hikit::fonts::ui_text_font(ui, chat.tool_name.size), word_ink);
    let detail = style(hikit::fonts::mono_font(ui, chat.tool_detail_size), detail_ink);
    let chevron = Disclosure {
        open: face.open,
        color: match face.failed {
            true => ink.removed.0,
            false => chat.chevron_color.0,
        },
        size: chat.chevron_size,
        stroke: chat.chevron_stroke,
    };

    let mut words = imba::layout::Row::new(arena).gap(chat.tool_gap);
    for (index, (tool, count)) in face.names.iter().enumerate() {
        if index > 0 {
            words = words.child_by_baseline(hikit::ui::text(&word, "·"));
        }
        words = words.child_by_baseline(hikit::ui::text(&name, tool.clone()));
        if *count > 1 {
            words = words.child_by_baseline(hikit::ui::text(&word, format!("×{count}")));
        }
    }
    if face.failed {
        words = words.child_by_baseline(hikit::ui::text(&word, "failed"));
    }
    if !face.detail.is_empty() {
        // Last and flexible: the invocation takes what the row has
        // left and ellipsizes inside it.
        words = words.child_by_baseline(hikit::ui::text(&detail, face.detail.clone()));
    }
    let row = imba::layout::Row::new(arena)
        .align_items(imba::layout::CrossAlign::Center)
        .child(chevron.layout(chat.chevron_hang, chat.tool_row_height))
        .child(words)
        .sized(width, chat.tool_row_height);
    let mut column = imba::layout::Column::new(arena).child(row);
    if let Some(error) = &face.error {
        let error_style = style(
            hikit::fonts::mono_font(ui, chat.tool_error_size),
            ink.ink_3.0,
        );
        column = column.child(
            imba::layout::ZBox::new(arena)
                .child(imba::layout::spacer(0.0, chat.tool_error_line))
                .child_aligned(
                    imba::layout::Alignment::CenterStart,
                    hikit::ui::text(&error_style, error.clone()),
                )
                .pad_insets(imba::layout::Insets {
                    left: chat.chevron_hang,
                    top: chat.tool_error_gap,
                    right: 0.0,
                    bottom: 0.0,
                }),
        );
    }
    column
}

/// The DISCLOSURE chevron, a named glyph: ▸ closed, ▾ open, stroked
/// in a `size` box at the left of its slot, centered on the row.
#[derive(Clone, Copy)]
pub(crate) struct Disclosure {
    pub open: bool,
    pub color: skia_safe::Color,
    pub size: f32,
    pub stroke: f32,
}

impl Disclosure {
    /// The glyph in a slot `width` × `height` — the slot is the
    /// chevron's hang into the gutter.
    pub(crate) fn layout<'a, Command: 'a>(
        self,
        width: f32,
        height: f32,
    ) -> impl imba::layout::Layout<'a, Command> + imba::layout::LayoutValue + 'a {
        imba::layout::Fill::<Command>::new()
            .sized(width, height)
            .backdrop(move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                // The mock's 16-unit glyph: M6 3 l5 5 -5 5 closed,
                // M3 6 l5 5 5 -5 open.
                let unit = self.size / 16.0;
                let left = rect.left;
                let top = rect.top + (rect.height() - self.size) * 0.5;
                let at = |x: f32, y: f32| (left + x * unit, top + y * unit);
                let mut path = skia_safe::PathBuilder::new();
                match self.open {
                    false => {
                        path.move_to(at(6.0, 3.0));
                        path.line_to(at(11.0, 8.0));
                        path.line_to(at(6.0, 13.0));
                    }
                    true => {
                        path.move_to(at(3.0, 6.0));
                        path.line_to(at(8.0, 11.0));
                        path.line_to(at(13.0, 6.0));
                    }
                }
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                paint.set_stroke(true);
                paint.set_stroke_width(self.stroke);
                paint.set_stroke_cap(skia_safe::PaintCap::Round);
                paint.set_stroke_join(skia_safe::PaintJoin::Round);
                paint.set_color(self.color);
                canvas.draw_path(&path.detach(), &paint);
            })
    }
}

fn materialize(text: &text::text::Text) -> String {
    let end = text.byte_count().min(u32::MAX as usize) as u32;
    text.view().substring(0..end)
}

fn chevron(expanded: bool) -> &'static str {
    match expanded {
        true => "v",
        false => ">",
    }
}

fn lift(key: &ToolRowKey, command: ToolRowsCommand) -> CellCommand {
    match command {
        ListCommand::Child(_, command) => CellCommand::ToolRow {
            key: key.clone(),
            command,
        },
        other => CellCommand::ToolRows(other),
    }
}

#[cfg(test)]
mod tests;
