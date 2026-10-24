// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_types::state::{ConfirmationOption, ConfirmationOptionKind, Message, PendingMessage};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::{container, Container},
    layout::Layout as _,
    layout::LayoutExt as _,
    ui::UiCtx,
};
use skia_safe::Size;

type ChatChrome = editor::theme::ChatChrome;

#[derive(Clone)]
pub enum StackCommand {
    Answer(usize),

    ToggleQueue,

    RemoveQueued(String),
}

impl std::fmt::Display for StackCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackCommand::Answer(_) => out.write_str("stack answer"),
            StackCommand::ToggleQueue => out.write_str("toggle queue"),
            StackCommand::RemoveQueued(_) => out.write_str("remove queued"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PermissionAsk {
    turn: ahp_wire::client::TurnId,
    tool: String,
    title: String,
    invocation: String,

    input: Option<String>,
    options: rpds::VectorSync<ConfirmationOption>,
}

impl PermissionAsk {
    pub(crate) fn new(
        turn: ahp_wire::client::TurnId,
        tool: String,
        title: String,
        invocation: String,
        input: Option<String>,
        options: impl IntoIterator<Item = ConfirmationOption>,
    ) -> Self {
        Self {
            turn,
            tool,
            title,
            invocation,
            input,
            options: options.into_iter().collect(),
        }
    }

    fn deny_index(&self) -> Option<usize> {
        self.options
            .iter()
            .position(|option| matches!(option.kind, ConfirmationOptionKind::Deny))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AskKeys {
    pub count: usize,
    pub deny: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct WidgetStack {
    ask: Option<PermissionAsk>,

    queue: rpds::VectorSync<PendingMessage>,

    collapsed: bool,
}

impl WidgetStack {
    pub(crate) fn new() -> Self {
        Self {
            ask: None,
            queue: rpds::VectorSync::new_sync(),
            collapsed: false,
        }
    }

    pub(crate) fn set_ask(&mut self, ask: PermissionAsk) {
        self.ask = Some(ask);
    }

    pub(crate) fn clear_ask(&mut self) {
        self.ask = None;
    }

    pub(crate) fn clear_ask_for_tool(&mut self, tool_call_id: &str) {
        if self
            .ask
            .as_ref()
            .is_some_and(|ask| ask.tool == tool_call_id)
        {
            self.ask = None;
        }
    }

    pub(crate) fn ask_turn(&self) -> Option<ahp_wire::client::TurnId> {
        self.ask.as_ref().map(|ask| ask.turn.clone())
    }

    pub(crate) fn ask_keys(&self) -> Option<AskKeys> {
        self.ask.as_ref().map(|ask| AskKeys {
            count: ask.options.len(),
            deny: ask.deny_index(),
        })
    }

    pub(crate) fn answer_payload(
        &self,
        index: usize,
    ) -> Option<(String, String, ConfirmationOption)> {
        let ask = self.ask.as_ref()?;
        let option = ask.options.iter().nth(index).cloned()?;
        Some((ask.turn.as_str().to_owned(), ask.tool.clone(), option))
    }

    pub(crate) fn permission_oracle(
        &self,
    ) -> Option<(String, String, Option<String>, Vec<String>)> {
        self.ask.as_ref().map(|ask| {
            (
                ask.title.clone(),
                ask.invocation.clone(),
                ask.input.clone(),
                ask.options
                    .iter()
                    .map(|option| option.label.clone())
                    .collect(),
            )
        })
    }

    pub(crate) fn seed_queue(&mut self, queue: impl IntoIterator<Item = PendingMessage>) {
        self.queue = queue.into_iter().collect();
    }

    pub(crate) fn insert_queued(&mut self, id: String, message: Message) {
        if !self.queue.iter().any(|held| held.id == id) {
            self.queue.push_back_mut(PendingMessage { id, message });
        }
    }

    pub(crate) fn remove_queued(&mut self, id: &str) {
        self.queue = self
            .queue
            .iter()
            .filter(|held| held.id != id)
            .cloned()
            .collect();
    }

    pub(crate) fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub(crate) fn queue_oracle(&self) -> Vec<(String, String)> {
        self.queue
            .iter()
            .map(|held| (held.id.clone(), held.message.text.clone()))
            .collect()
    }

    /// One line of the stack: the transcript's row height.
    fn line(chrome: &ChatChrome) -> f32 {
        chrome.tool_row_height
    }

    fn ask_height(&self, chrome: &ChatChrome) -> f32 {
        let line = Self::line(chrome);
        self.ask
            .as_ref()
            .map(|ask| {
                // The caps line, the invocation, the input when there
                // is one, an option a row — and the row gap after.
                let lines = 2 + usize::from(ask.input.is_some()) + ask.options.len();
                chrome.row_gap + lines as f32 * line
            })
            .unwrap_or(0.0)
    }

    fn queue_height(&self, chrome: &ChatChrome) -> f32 {
        if self.queue.is_empty() {
            return 0.0;
        }
        let line = Self::line(chrome);
        let rows = if self.collapsed { 0 } else { self.queue.len() };
        chrome.row_gap + (1 + rows) as f32 * line
    }

    pub(crate) fn height(&self, _store: &imba::store::Store, chrome: &ChatChrome) -> f32 {
        self.ask_height(chrome) + self.queue_height(chrome)
    }

    /// The stack in the transcript's voice (docs/Design.md): no
    /// cards. The ask is a caps line, the invocation in the reading
    /// register, the input in mono, then its options as rows with the
    /// number in the gutter's margin. The queue is a caps line (it
    /// folds) over its messages, each with a quiet "remove".
    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a imba::store::Store,
        ui: &'a UiCtx,
        chrome: &ChatChrome,
        width: f32,
    ) -> Container<'a, StackCommand> {
        let grid = crate::cell::Grid::of(store, width, false);
        let line = Self::line(chrome);
        let gutter = (grid.edge - chrome.gutter_pad).max(0.0);
        let mut stack = container(arena, Size::new(width, self.height(store, chrome)));
        // A label vertically centered in one line of the column.
        let lined = |label: imba::layout::Text, width: f32| {
            imba::layout::ZBox::new(arena)
                .child(imba::layout::spacer(width, line))
                .child_aligned(imba::layout::Alignment::CenterStart, label)
                .layout(arena, Constraints::tight(Size::new(width, line)).loosen())
        };
        // The same, flush right against the text column — the margin.
        let margin = |label: imba::layout::Text| {
            imba::layout::ZBox::new(arena)
                .child(imba::layout::spacer(gutter, line))
                .child_aligned(imba::layout::Alignment::CenterEnd, label)
                .layout(arena, Constraints::tight(Size::new(gutter, line)).loosen())
        };
        let caps = hikit::ui::caps(store, ui);
        let reading = hikit::ui::reading(store, ui);
        let row = hikit::ui::row(store, ui);
        let mono = hikit::ui::margin(store, ui);
        let brief = hikit::ui::brief(store, ui);

        let mut y = chrome.row_gap;
        if let Some(ask) = &self.ask {
            stack.place_boxed(
                grid.edge,
                y,
                lined(hikit::ui::text(&caps, ask.title.to_uppercase()), grid.column),
            );
            y += line;
            let invocation = ask.invocation.lines().next().unwrap_or("").to_owned();
            stack.place_boxed(
                grid.edge,
                y,
                lined(hikit::ui::text(&reading, invocation), grid.column),
            );
            y += line;
            if let Some(preview) = &ask.input {
                let preview = preview.lines().next().unwrap_or("").to_owned();
                stack.place_boxed(
                    grid.edge,
                    y,
                    lined(hikit::ui::text(&mono, preview), grid.column),
                );
                y += line;
            }
            for (index, option) in ask.options.iter().enumerate() {
                stack.place_boxed(
                    0.0,
                    y,
                    margin(hikit::ui::text(&mono, format!("{}", index + 1))),
                );
                let pick = imba::layout::ZBox::new(arena)
                    .child(imba::layout::spacer(grid.column, line))
                    .child_aligned(
                        imba::layout::Alignment::CenterStart,
                        hikit::ui::text(&row, option.label.clone()),
                    )
                    .on_click(move || StackCommand::Answer(index))
                    .layout(arena, Constraints::tight(Size::new(grid.column, line)).loosen());
                stack.place_boxed(grid.edge, y, pick);
                y += line;
            }
            y += chrome.row_gap;
        }

        if !self.queue.is_empty() {
            let head = format!("QUEUED \u{00B7} {}", self.queue.len());
            let toggle = imba::layout::ZBox::new(arena)
                .child(imba::layout::spacer(grid.column, line))
                .child_aligned(
                    imba::layout::Alignment::CenterStart,
                    hikit::ui::text(&caps, head),
                )
                .on_click(|| StackCommand::ToggleQueue)
                .layout(arena, Constraints::tight(Size::new(grid.column, line)).loosen());
            stack.place_boxed(grid.edge, y, toggle);
            y += line;
            if !self.collapsed {
                let remove_w = imba::layout::text_advance(ui, &brief.font, "remove");
                for held in self.queue.iter() {
                    let id = held.id.clone();
                    let text: String = held.message.text.lines().next().unwrap_or("").to_owned();
                    let text_w = (grid.column - remove_w - chrome.options_gap).max(1.0);
                    stack.place_boxed(grid.edge, y, lined(hikit::ui::text(&row, text), text_w));
                    let remove = imba::layout::ZBox::new(arena)
                        .child(imba::layout::spacer(remove_w, line))
                        .child_aligned(
                            imba::layout::Alignment::CenterStart,
                            hikit::ui::text(&brief, "remove"),
                        )
                        .on_click(move || StackCommand::RemoveQueued(id.clone()))
                        .layout(arena, Constraints::tight(Size::new(remove_w, line)).loosen());
                    stack.place_boxed(grid.edge + grid.column - remove_w, y, remove);
                    y += line;
                }
            }
        }

        stack
    }
}
