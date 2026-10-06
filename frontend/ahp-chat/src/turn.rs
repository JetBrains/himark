// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    list::{ListCommand, ListView},
    store::Store,
    ui::UiCtx,
    Thunk, View,
};
use skia_safe::Size;

use crate::cell::{Cell, CellCommand, CellKind, DiffHeader};
use crate::chat::model::PartId;
use crate::file_edit::FileEditRefs;
use crate::tool_group::{ToolCallSpec, ToolFace};
use ahp_types::common::Uri;

pub(crate) type TurnCommand = ListCommand<CellCommand>;

#[derive(Clone)]
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
    pub(crate) fn of(refs: &FileEditRefs) -> Self {
        Self {
            header: DiffHeader {
                title: refs.display_name(),
                added: refs.counts.added,
                removed: refs.counts.removed,
                // The WORKING COPY's uri — the file the edit landed in,
                // not the content snapshots. The header's OPEN rides it.
                uri: refs
                    .after
                    .as_ref()
                    .or(refs.before.as_ref())
                    .map(|side| side.uri.clone()),
            },
            before: refs.before.as_ref().map(|side| side.content.uri.clone()),
            after: refs.after.as_ref().map(|side| side.content.uri.clone()),
        }
    }
}

/// A cell's place in a turn. Keyed, never counted: a streamed delta
/// reaches its own cell by PART, so no arithmetic can land it in
/// someone else's text.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CellKey {
    /// The message that opened the turn.
    Prompt,
    Part(PartId),
    /// How the turn ended, when it did not simply complete.
    Life,
    /// What the turn spent.
    Usage,
}

#[derive(Clone)]
pub struct TurnView {
    id: String,
    cells: ListView<Cell, CellKey>,
}

impl TurnView {
    pub(crate) fn new(
        id: impl Into<String>,
        laid_width: f32,
        cells: imba::list::ListSlice<Cell, CellKey>,
    ) -> Self {
        Self {
            id: id.into(),
            cells: ListView::from_slice_at(laid_width, cells),
        }
    }

    /// Where a cell sits right now — the O(1) hop a delta takes from
    /// its part id to the cell it grows.
    pub(crate) fn cell_at(&self, key: &CellKey) -> Option<usize> {
        self.cells.row_range(key).map(|range| range.start)
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

    /// Put a cell where its key belongs: a part goes after the parts
    /// (before the tail, if the turn already ended), the tail goes last.
    /// Two keyed lookups — never a walk over the cells.
    pub(crate) fn place(&mut self, key: CellKey, cell: Cell, height: f32) {
        let at = match key {
            CellKey::Prompt | CellKey::Part(_) => self
                .cell_at(&CellKey::Life)
                .or_else(|| self.cell_at(&CellKey::Usage))
                .unwrap_or(self.cells.len()),
            CellKey::Life => self.cell_at(&CellKey::Usage).unwrap_or(self.cells.len()),
            CellKey::Usage => self.cells.len(),
        };
        let mut slice = imba::list::ListSlice::new();
        slice.push_keyed_sized(key, cell, height);
        self.cells.splice_slice(at..at, slice);
    }
}

impl View for TurnView {
    type Command = TurnCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, TurnCommand> {
        self.cells.focus_data(store, ui)
    }

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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width.max(1.0);
            let content_width = Self::content_width(width);
            let inner = imba::layout::Layout::layout(
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

/// The cells a turn SHOWS, keyed by what they came from: the prompt,
/// one cell per part in the order it arrived — except consecutive tool
/// calls, which collapse into ONE cell, a RUN, keyed by the run's
/// first call (docs/ahp/agents.md, "Tool runs collapse": anything
/// else between two calls closes the run) — then how the turn ended
/// and what it spent. Pure dressing — the model holds none of it.
pub(crate) fn dress(turn: &crate::chat::model::Turn) -> DressedCells {
    use crate::chat::model::Part;
    let mut cells = DressedCells::new_sync();
    let (voice, text) = turn.prompt.clone();
    cells.push_back_mut((CellKey::Prompt, CellSpec::Text(voice, text)));
    let mut run: Option<(PartId, Vec<ToolCallSpec>)> = None;
    let close = |run: &mut Option<(PartId, Vec<ToolCallSpec>)>, cells: &mut DressedCells| {
        if let Some((first, specs)) = run.take() {
            cells.push_back_mut((CellKey::Part(first), CellSpec::Tools(specs)));
        }
    };
    for (id, part) in turn.parts() {
        match part {
            Part::Tool(call) => match &mut run {
                Some((_, specs)) => specs.push(tool_spec_of(call)),
                None => run = Some((id.clone(), vec![tool_spec_of(call)])),
            },
            other => {
                close(&mut run, &mut cells);
                cells.push_back_mut((CellKey::Part(id.clone()), dress_part(other)));
            }
        }
    }
    close(&mut run, &mut cells);
    for cell in dress_tail(turn).iter().cloned() {
        cells.push_back_mut(cell);
    }
    cells
}

/// The RUN a tool part belongs to, dressed whole: the cell's key is
/// the run's FIRST call, so every call of the run lands on the same
/// cell however late it joins. The spec carries the whole run — a view
/// that lacks the cell builds all of it; one that has it takes the
/// specs as keyed updates. None when the part is not a tool call.
pub(crate) fn dress_tool_run(
    turn: &crate::chat::model::Turn,
    part: &PartId,
) -> Option<(CellKey, CellSpec)> {
    use crate::chat::model::Part;
    let mut run: Option<(PartId, Vec<ToolCallSpec>)> = None;
    let mut hit = false;
    for (id, held) in turn.parts() {
        match held {
            Part::Tool(call) => {
                match &mut run {
                    Some((_, specs)) => specs.push(tool_spec_of(call)),
                    None => run = Some((id.clone(), vec![tool_spec_of(call)])),
                }
                if id == part {
                    hit = true;
                }
            }
            // Anything else closes the run: past the hit it is done,
            // before it the walk starts over.
            _ if hit => break,
            _ => run = None,
        }
    }
    if !hit {
        return None;
    }
    let (first, specs) = run?;
    Some((CellKey::Part(first), CellSpec::Tools(specs)))
}

/// ONE part's cell — what a part landing re-dresses. Never the turn.
pub(crate) fn dress_part(part: &crate::chat::model::Part) -> CellSpec {
    use crate::chat::model::Part;
    match part {
        Part::Said { voice, text } => CellSpec::Text(*voice, text.clone()),
        Part::Tool(call) => CellSpec::Tools(vec![tool_spec_of(call)]),
        Part::Edit(refs) => CellSpec::Diff(DiffSpec::of(refs)),
    }
}

/// The cells after the parts: how the turn ended (when it did not
/// simply complete) and what it spent. At most two.
pub(crate) fn dress_tail(turn: &crate::chat::model::Turn) -> DressedCells {
    use crate::chat::model::Life;
    let mut cells = DressedCells::new_sync();
    match &turn.life {
        Life::Live | Life::Complete => {}
        Life::Cancelled => cells.push_back_mut((
            CellKey::Life,
            CellSpec::Text(CellKind::Notice, "*Turn cancelled.*".to_owned()),
        )),
        Life::Failed(said) => cells.push_back_mut((
            CellKey::Life,
            CellSpec::Text(CellKind::Error, format!("**Turn failed** {said}")),
        )),
    }
    if let Some(spent) = &turn.usage {
        cells.push_back_mut((
            CellKey::Usage,
            CellSpec::Text(CellKind::Notice, spent.clone()),
        ));
    }
    cells
}

pub(crate) type DressedCells = rpds::VectorSync<(CellKey, CellSpec)>;

fn tool_spec_of(call: &crate::chat::model::ToolCall) -> ToolCallSpec {
    use crate::chat::model::ToolStatus;
    let face = match &call.status {
        ToolStatus::Streaming => streaming_tool_face(&call.display),
        ToolStatus::Waiting => pending_tool_face(&call.display, &call.invocation),
        ToolStatus::Running => running_tool_face(&call.display, &call.invocation),
        ToolStatus::Denied => denied_tool_face(&call.display),
        ToolStatus::Done { ok, said, output } => completed_tool_face_with(
            &call.display,
            call.input.as_deref(),
            said,
            output.clone(),
            *ok,
        ),
    };
    ToolCallSpec {
        id: call.id.as_str().to_owned(),
        display_name: call.display.clone(),
        face,
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

pub(crate) fn completed_tool_face_with(
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

fn text(markdown: impl AsRef<str>) -> text::text::Text {
    text::text::Text::from_string_exact(markdown)
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

pub(crate) fn tool_output(content: &[ahp_types::state::ToolResultContent]) -> String {
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
