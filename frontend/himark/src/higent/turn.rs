// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_types::state::{
    Message, MessageKind, ResponsePart, ToolCallState, Turn, TurnState, UsageInfo,
};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    list::{ListCommand, ListView},
    store::Store,
    Thunk, UiCtx, View,
};
use skia_safe::Size;

use crate::higent::cell::{Cell, CellCommand, CellKind, DiffHeader};
use crate::higent::tool_group::{ToolCallSpec, ToolFace};
use crate::higent::FileEditRefs;
use ahp_types::common::Uri;

pub type TurnCommand = ListCommand<CellCommand>;

pub(crate) enum CellSpec {
    Text(CellKind, String),
    Diff(DiffSpec),

    Tools(Vec<ToolCallSpec>),
}

#[derive(Clone)]
pub(crate) struct DiffSpec {
    pub header: DiffHeader,
    pub before: Option<Uri>,
    pub after: Option<Uri>,
}

impl DiffSpec {
    fn of(refs: &FileEditRefs) -> Self {
        Self {
            header: DiffHeader {
                title: refs.display_name(),
                added: refs.counts.added,
                removed: refs.counts.removed,
            },
            before: refs.before.as_ref().map(|side| side.content.uri.clone()),
            after: refs.after.as_ref().map(|side| side.content.uri.clone()),
        }
    }
}

#[derive(Clone)]
pub struct TurnView {
    id: String,
    cells: ListView<Cell, ()>,
}

impl TurnView {
    pub(crate) fn new(id: impl Into<String>, laid_width: f32, cells: Vec<(Cell, f32)>) -> Self {
        Self {
            id: id.into(),
            cells: ListView::from_rope_at(laid_width, imba::list::measured(cells)),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn content_width(width: f32) -> f32 {
        width.max(1.0)
    }

    pub(crate) fn cells_oracle(&self) -> Vec<(String, String)> {
        self.cells.rows().map(|cell| cell.oracle()).collect()
    }

    pub(crate) fn append(&mut self, cell: Cell, height: f32) {
        let len = self.cells.len();
        self.cells.splice(len..len, [(cell, height)]);
    }
}

impl View for TurnView {
    type Command = TurnCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        self.cells.destroy(store, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        self.cells.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width.max(1.0);
            let content_width = Self::content_width(width);
            let inner = imba::Layout::layout(
                self.cells.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(content_width, 0.0),
                    max: Size::new(content_width, f32::MAX),
                },
            );
            let height = inner.size().height;
            let mut row = container(arena, Size::new(width, height));
            row.place(((width - content_width) / 2.0).max(0.0), 0.0, inner);
            row
        })
    }
}

pub(crate) fn turn_cells(turn: &Turn) -> Vec<CellSpec> {
    let (kind, text) = message_cell(&turn.message);
    let mut cells = vec![CellSpec::Text(kind, text)];
    for part in &turn.response_parts {
        for spec in part_cells(part) {
            push_spec(&mut cells, spec);
        }
    }
    match turn.state {
        TurnState::Complete => {}
        TurnState::Cancelled => cells.push(CellSpec::Text(
            CellKind::Notice,
            "*Turn cancelled.*".to_owned(),
        )),
        TurnState::Error => {
            let detail = turn
                .error
                .as_ref()
                .map(|error| format!("**Turn failed** ({}): {}", error.error_type, error.message))
                .unwrap_or_else(|| "**Turn failed.**".to_owned());
            cells.push(CellSpec::Text(CellKind::Error, detail));
        }
    }
    if let Some(usage) = &turn.usage {
        if let Some((kind, text)) = usage_cell(usage) {
            cells.push(CellSpec::Text(kind, text));
        }
    }
    cells
}

pub(crate) fn push_spec(cells: &mut Vec<CellSpec>, spec: CellSpec) {
    if let (CellSpec::Tools(calls), Some(CellSpec::Tools(open))) = (&spec, cells.last_mut()) {
        open.extend(calls.iter().cloned());
        return;
    }
    cells.push(spec);
}

pub(crate) fn usage_cell(usage: &UsageInfo) -> Option<(CellKind, String)> {
    let mut bits = Vec::new();
    if let Some(model) = &usage.model {
        bits.push(model.clone());
    }
    if let Some(input) = usage.input_tokens {
        bits.push(format!("{input} in"));
    }
    if let Some(output) = usage.output_tokens {
        bits.push(format!("{output} out"));
    }
    (!bits.is_empty()).then(|| (CellKind::Notice, format!("*{}*", bits.join(" · "))))
}

pub(crate) fn message_cell(message: &Message) -> (CellKind, String) {
    match message.origin.kind {
        MessageKind::User => (CellKind::User, message.text.clone()),
        MessageKind::Agent => (CellKind::Agent, message.text.clone()),
        MessageKind::Tool | MessageKind::SystemNotification => {
            (CellKind::Notice, format!("*{}*", message.text))
        }
    }
}

pub(crate) fn part_cells(part: &ResponsePart) -> Vec<CellSpec> {
    if let ResponsePart::ToolCall(part) = part {
        let mut cells = vec![CellSpec::Tools(vec![tool_call_spec(&part.tool_call)])];
        if let ToolCallState::Completed(state) = &part.tool_call {
            for block in state.content.as_deref().unwrap_or_default() {
                use ahp_types::state::ToolResultContent;
                if let ToolResultContent::FileEdit(edit) = block {
                    if let Some(refs) = FileEditRefs::parse(edit) {
                        cells.push(CellSpec::Diff(DiffSpec::of(&refs)));
                    }
                }
            }
        }
        return cells;
    }
    part_cell(part)
        .map(|(kind, text)| CellSpec::Text(kind, text))
        .into_iter()
        .collect()
}

fn part_cell(part: &ResponsePart) -> Option<(CellKind, String)> {
    match part {
        ResponsePart::ToolCall(_) => None,
        ResponsePart::Markdown(part) => Some((CellKind::Agent, part.content.clone())),

        ResponsePart::Reasoning(part) if part.content.trim().is_empty() => None,
        ResponsePart::Reasoning(part) => Some((CellKind::Reasoning, part.content.clone())),
        ResponsePart::SystemNotification(part) => {
            Some((CellKind::Notice, format!("*{}*", part.content.as_text())))
        }
        ResponsePart::InputRequest(part) => {
            let question = part
                .request
                .message
                .clone()
                .unwrap_or_else(|| "The agent asked for input.".to_owned());
            let status = if part.response.is_some() {
                "answered"
            } else {
                "unanswered"
            };
            Some((
                CellKind::Notice,
                format!("*Input request ({status}): {question}*"),
            ))
        }
        ResponsePart::ContentRef(part) => {
            let kind = part.content_type.as_deref().unwrap_or("content");
            let size = part
                .size_hint
                .map(|bytes| format!(", {}", human_size(bytes)))
                .unwrap_or_default();
            Some((
                CellKind::Notice,
                format!("*\\[{kind}{size}\\] — stored by reference: `{}`*", part.uri),
            ))
        }

        ResponsePart::Unknown(_) => Some((
            CellKind::Notice,
            "*This turn carries a response part this build does not understand yet.*".to_owned(),
        )),
    }
}

pub(crate) fn tool_call_spec(state: &ToolCallState) -> ToolCallSpec {
    let (id, display_name) = tool_identity(state);
    let face = tool_face(state, &display_name);
    ToolCallSpec {
        id,
        display_name,
        face,
    }
}

fn tool_identity(state: &ToolCallState) -> (String, String) {
    match state {
        ToolCallState::Streaming(state) => (state.tool_call_id.clone(), state.display_name.clone()),
        ToolCallState::PendingConfirmation(state) => {
            (state.tool_call_id.clone(), state.display_name.clone())
        }
        ToolCallState::Running(state) => (state.tool_call_id.clone(), state.display_name.clone()),
        ToolCallState::AuthRequired(state) => {
            (state.tool_call_id.clone(), state.display_name.clone())
        }
        ToolCallState::PendingResultConfirmation(state) => {
            (state.tool_call_id.clone(), state.display_name.clone())
        }
        ToolCallState::Completed(state) => (state.tool_call_id.clone(), state.display_name.clone()),
        ToolCallState::Cancelled(state) => (state.tool_call_id.clone(), state.display_name.clone()),
        ToolCallState::Unknown(_) => (String::new(), "Tool call".to_owned()),
    }
}

fn tool_face(state: &ToolCallState, display_name: &str) -> ToolFace {
    match state {
        ToolCallState::Streaming(_) => streaming_tool_face(display_name),
        ToolCallState::PendingConfirmation(state) => {
            pending_tool_face(display_name, state.invocation_message.as_text())
        }
        ToolCallState::Running(state) => {
            running_tool_face(display_name, state.invocation_message.as_text())
        }
        ToolCallState::AuthRequired(_) => ToolFace {
            line: format!("{display_name} — waiting for authentication"),
            markdown: text(format!("**{display_name}** — waiting for authentication")),
            failed: false,
            live: true,
        },
        ToolCallState::PendingResultConfirmation(_) => ToolFace {
            line: format!("{display_name} — result awaiting review"),
            markdown: text(format!("**{display_name}** — result awaiting review")),
            failed: false,
            live: true,
        },
        ToolCallState::Completed(state) => {
            let call = match &state.tool_input {
                Some(ahp_types::state::ToolInput::Inline(text)) => Some(text.as_str()),
                _ => None,
            };
            completed_tool_face_with(
                display_name,
                call,
                state.past_tense_message.as_text(),
                state
                    .content
                    .as_deref()
                    .map(tool_output)
                    .unwrap_or_default(),
                state.success,
            )
        }
        ToolCallState::Cancelled(state) => {
            let reason = state
                .reason_message
                .as_ref()
                .map(|reason| format!(" — {}", reason.as_text()))
                .unwrap_or_default();
            ToolFace {
                line: one_line(&format!("{display_name} — cancelled{reason}")),
                markdown: text(format!("**{display_name}** — cancelled{reason}")),
                failed: false,
                live: false,
            }
        }
        ToolCallState::Unknown(_) => ToolFace {
            line: "Tool call — in a state this build does not understand yet".to_owned(),
            markdown: text("**Tool call** — in a state this build does not understand yet"),
            failed: false,
            live: false,
        },
    }
}

pub(crate) fn streaming_tool_face(display_name: &str) -> ToolFace {
    ToolFace {
        line: format!("{display_name} — preparing…"),
        markdown: text(format!("**{display_name}** — preparing…")),
        failed: false,
        live: true,
    }
}

pub(crate) fn pending_tool_face(display_name: &str, invocation: &str) -> ToolFace {
    ToolFace {
        line: one_line(&format!(
            "{display_name} — waiting for approval: {invocation}"
        )),
        markdown: text(format!(
            "**{display_name}** — waiting for approval\n\n{invocation}"
        )),
        failed: false,
        live: true,
    }
}

pub(crate) fn running_tool_face(display_name: &str, invocation: &str) -> ToolFace {
    ToolFace {
        line: one_line(&format!("{display_name} — running… {invocation}")),
        markdown: text(format!("**{display_name}** — running…\n\n{invocation}")),
        failed: false,
        live: true,
    }
}

pub(crate) fn denied_tool_face(display_name: &str) -> ToolFace {
    ToolFace {
        line: format!("{display_name} — denied"),
        markdown: text(format!("**{display_name}** — denied")),
        failed: false,
        live: false,
    }
}

pub(crate) fn completed_tool_face(
    display_name: &str,
    call: Option<&str>,
    result: &ahp_types::state::ToolCallResult,
) -> ToolFace {
    completed_tool_face_with(
        display_name,
        call,
        result.past_tense_message.as_text(),
        result
            .content
            .as_deref()
            .map(tool_output)
            .unwrap_or_default(),
        result.success,
    )
}

fn completed_tool_face_with(
    display_name: &str,
    call: Option<&str>,
    past_tense: &str,
    output: String,
    success: bool,
) -> ToolFace {
    let mut markdown = format!("**{display_name}** — {past_tense}");
    let call = call.map(str::trim).filter(|call| !call.is_empty());
    if let Some(call) = call {
        if *call != *display_name {
            markdown.push_str("\n\n```\n");
            markdown.push_str(call);
            markdown.push_str("\n```");
        }
    }
    if !output.is_empty() {
        markdown.push_str("\n\n```\n");
        markdown.push_str(&output);
        markdown.push_str("\n```");
    }
    ToolFace {
        line: one_line(&format!("{display_name} — {past_tense}")),
        markdown: text(markdown),
        failed: !success,
        live: false,
    }
}

fn text(markdown: impl AsRef<str>) -> crate::Text {
    crate::Text::from_string_exact(markdown)
}

fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned();
    const LIMIT: usize = 110;
    if line.chars().count() <= LIMIT {
        return line;
    }
    let cut: String = line.chars().take(LIMIT).collect();
    format!("{}…", cut.trim_end())
}

pub(crate) fn result_diff_specs(result: &ahp_types::state::ToolCallResult) -> Vec<CellSpec> {
    use ahp_types::state::ToolResultContent;
    result
        .content
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|block| match block {
            ToolResultContent::FileEdit(edit) => {
                FileEditRefs::parse(edit).map(|refs| CellSpec::Diff(DiffSpec::of(&refs)))
            }
            _ => None,
        })
        .collect()
}

fn tool_output(content: &[ahp_types::state::ToolResultContent]) -> String {
    use ahp_types::state::ToolResultContent;
    content
        .iter()
        .filter_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn human_size(bytes: i64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}
