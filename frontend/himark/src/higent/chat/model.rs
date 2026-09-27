// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The conversation MODEL — AHP chat state as persistent values.
//!
//! The shape is the protocol's own (docs/ahp/agents.md, and the
//! reference reducer in the `ahp` crate), because the protocol's shape
//! is what makes the hard cases impossible rather than handled:
//!
//! * A turn IS its id. Landing a turn we already hold REPLACES it
//!   where it stands, so a re-delivered page or a doubled stream can
//!   never mint a second copy, and no page road has to dedupe.
//! * Parts, deltas, tool calls and usage land ONLY in the turn in
//!   flight (`live`). A settled turn is immutable, so no stream can
//!   write into the middle of the conversation.
//! * A turn start RESETS the live turn's parts. A replayed stream
//!   therefore rebuilds it to exactly one copy — no rewind marker, no
//!   latch, no cell arithmetic.
//! * Sending is write-ahead: the client mints the turn id, folds its
//!   own `chat/turnStarted`, and dispatches that same action. The
//!   host's echo re-applies it and changes nothing, so there is no
//!   placeholder to reconcile and nothing to leave the composer stuck.
//!
//! Everything here is a persistent value and every fold is a pure
//! function: `fold` takes a conversation and an action and returns the
//! next conversation plus what CHANGED, so a view updates exactly the
//! row or cell that moved. Nothing on this road walks the transcript.

use ahp_types::state::{
    ActiveTurn, ChatState, Message, MessageKind, ResponsePart, ToolCallResult, ToolCallState,
    ToolInput, Turn as WireTurn, TurnState, UsageInfo,
};
use rpds::{HashTrieMapSync, VectorSync};

use crate::higent::cell::CellKind;
use crate::higent::TurnId;

/// A response part's id: what `chat/delta` and `chat/reasoning`
/// address. A tool call is addressed by its own id, which lives in
/// the same namespace as far as the wire's part lookup is concerned.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PartId(String);

impl PartId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PartId {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl std::borrow::Borrow<str> for PartId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Where a tool call stands. The protocol's lifecycle, kept whole so
/// the view can dress it without asking the wire again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ToolStatus {
    /// `chat/toolCallStart`: the agent is composing the call.
    Streaming,
    /// `chat/toolCallReady` without `confirmed`: the user must answer.
    Waiting,
    Running,
    Done {
        ok: bool,
        said: String,
        /// What the tool printed, as the block shows it.
        output: String,
    },
    Denied,
}

/// One tool call inside a turn.
#[derive(Clone)]
pub struct ToolCall {
    pub id: PartId,
    pub display: String,
    pub invocation: String,
    pub input: Option<String>,
    pub status: ToolStatus,
}

/// One piece of a turn, in the order the agent produced it.
#[derive(Clone)]
pub enum Part {
    /// Markdown, reasoning, a system notice, an error: text with a voice.
    Said {
        voice: CellKind,
        text: String,
    },
    Tool(ToolCall),
    /// A file edit a tool call carried, shown as a diff.
    Edit(crate::higent::FileEditRefs),
}

/// How a turn ended — `TurnState` plus the error the wire carries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Life {
    Live,
    Complete,
    Cancelled,
    Failed(String),
}

/// One turn: the prompt that opened it and everything said since.
#[derive(Clone)]
pub struct Turn {
    pub id: TurnId,
    pub prompt: (CellKind, String),
    /// The order parts arrived in.
    order: VectorSync<PartId>,
    /// The part behind each id — a part IS its id, so a replayed part
    /// replaces its own text instead of appending a second copy.
    parts: HashTrieMapSync<PartId, Part>,
    pub life: Life,
    pub usage: Option<String>,
}

impl Turn {
    fn opened(id: TurnId, message: &Message) -> Self {
        Self {
            id,
            prompt: voiced(message),
            order: VectorSync::new_sync(),
            parts: HashTrieMapSync::new_sync(),
            life: Life::Live,
            usage: None,
        }
    }

    /// The parts in the order they were said.
    pub fn parts(&self) -> impl Iterator<Item = (&PartId, &Part)> + '_ {
        self.order
            .iter()
            .filter_map(move |id| self.parts.get(id).map(|part| (id, part)))
    }

    pub fn part_count(&self) -> usize {
        self.order.len()
    }

    pub fn part(&self, id: &PartId) -> Option<&Part> {
        self.parts.get(id)
    }

    /// Put a part at its place: a new one joins the end, a known one is
    /// replaced where it already stands.
    fn with_part(&self, id: PartId, part: Part) -> Self {
        let mut next = self.clone();
        if !next.parts.contains_key(&id) {
            next.order.push_back_mut(id.clone());
        }
        next.parts.insert_mut(id, part);
        next
    }

    /// Append streamed text to a part that already stands.
    fn grown(&self, id: &PartId, text: &str) -> Option<Self> {
        let Some(Part::Said { voice, text: held }) = self.parts.get(id) else {
            return None;
        };
        let grown = Part::Said {
            voice: *voice,
            text: format!("{held}{text}"),
        };
        let mut next = self.clone();
        next.parts.insert_mut(id.clone(), grown);
        Some(next)
    }

    fn with_tool(&self, id: &PartId, dress: impl FnOnce(ToolCall) -> ToolCall) -> Option<Self> {
        let Some(Part::Tool(call)) = self.parts.get(id) else {
            return None;
        };
        let mut next = self.clone();
        next.parts
            .insert_mut(id.clone(), Part::Tool(dress(call.clone())));
        Some(next)
    }

    pub fn tool(&self, id: &PartId) -> Option<&ToolCall> {
        match self.parts.get(id) {
            Some(Part::Tool(call)) => Some(call),
            _ => None,
        }
    }

    /// The turn as the wire serves it — a settled turn from a page, or
    /// the live turn from a snapshot.
    fn of_wire(
        id: TurnId,
        message: &Message,
        parts: &[ResponsePart],
        life: Life,
        usage: Option<&UsageInfo>,
    ) -> Self {
        let mut turn = Self {
            id,
            prompt: voiced(message),
            order: VectorSync::new_sync(),
            parts: HashTrieMapSync::new_sync(),
            life,
            usage: usage.and_then(spent),
        };
        for part in parts {
            for (id, laid) in read_part(part).iter().cloned() {
                if !turn.parts.contains_key(&id) {
                    turn.order.push_back_mut(id.clone());
                }
                turn.parts.insert_mut(id, laid);
            }
        }
        turn
    }

    pub fn of_settled(wire: &WireTurn) -> Self {
        let life = match wire.state {
            TurnState::Complete => Life::Complete,
            TurnState::Cancelled => Life::Cancelled,
            TurnState::Error => Life::Failed(
                wire.error
                    .as_ref()
                    .map(|error| format!("{}: {}", error.error_type, error.message))
                    .unwrap_or_else(|| "the turn failed".to_owned()),
            ),
        };
        Self::of_wire(
            TurnId::new(wire.id.clone()),
            &wire.message,
            &wire.response_parts,
            life,
            wire.usage.as_ref(),
        )
    }

    fn of_live(wire: &ActiveTurn) -> Self {
        Self::of_wire(
            TurnId::new(wire.id.clone()),
            &wire.message,
            &wire.response_parts,
            Life::Live,
            wire.usage.as_ref(),
        )
    }
}

/// What a fold moved, so a view can update exactly that much.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// The action meant nothing here (the protocol's own `NoOp`).
    Nothing,
    /// A turn joined the tail, or replaced itself where it stood.
    Said(TurnId),
    /// One part landed or was re-dressed: that cell, nothing else.
    Part { turn: TurnId, part: PartId },
    /// Several parts at once — a tool completion re-dresses its call
    /// and lands the edits it carried. Every part named, in order.
    Parts {
        turn: TurnId,
        parts: VectorSync<PartId>,
    },
    /// Streamed text joined a part that already stands.
    Grew {
        turn: TurnId,
        part: PartId,
        text: String,
    },
    /// A turn settled: its face changes, its content does not.
    Retired(TurnId),
    /// A page landed — the whole list re-lays.
    Page,
}

/// One conversation: the turns already said, and the turn in flight.
#[derive(Clone, Default)]
pub struct Conversation {
    said: VectorSync<TurnId>,
    turns: HashTrieMapSync<TurnId, Turn>,
    live: Option<TurnId>,
    older: Option<String>,
}

impl Conversation {
    pub fn len(&self) -> usize {
        self.said.len()
    }

    pub fn is_empty(&self) -> bool {
        self.said.is_empty()
    }

    pub fn turn(&self, id: &TurnId) -> Option<&Turn> {
        self.turns.get(id)
    }

    /// The turns in the order they were said, the live one last.
    pub fn turns(&self) -> impl Iterator<Item = &Turn> + '_ {
        self.said.iter().filter_map(|id| self.turns.get(id))
    }

    pub fn tail(&self) -> Option<&Turn> {
        self.said.last().and_then(|id| self.turns.get(id))
    }

    /// The turn in flight — what STOP cancels, and the only turn a
    /// stream may write into.
    pub fn live(&self) -> Option<&TurnId> {
        self.live.as_ref()
    }

    pub fn is_running(&self) -> bool {
        self.live.is_some()
    }

    /// A standing cursor means older turns exist behind the window.
    pub fn older(&self) -> Option<&String> {
        self.older.as_ref()
    }

    fn with_turn(&self, turn: Turn) -> Self {
        let mut next = self.clone();
        if !next.turns.contains_key(&turn.id) {
            next.said.push_back_mut(turn.id.clone());
        }
        next.turns.insert_mut(turn.id.clone(), turn);
        next
    }

    fn update(&self, id: &TurnId, mutate: impl FnOnce(&Turn) -> Option<Turn>) -> Option<Self> {
        let held = self.turns.get(id)?;
        let next = mutate(held)?;
        let mut conversation = self.clone();
        conversation.turns.insert_mut(id.clone(), next);
        Some(conversation)
    }

    /// The live turn, if the action names it — the protocol's guard:
    /// everything streamed is out of scope for any other turn.
    fn streaming(&self, turn: &str) -> Option<TurnId> {
        self.live.clone().filter(|live| live.as_str() == turn)
    }

    // ------------------------------------------------------------------
    // The write-ahead send.

    /// Mint the client's own `chat/turnStarted`: the id is ours, the
    /// row appears at once, and the host's echo of this very action
    /// folds to nothing.
    pub fn opening(
        turn: &TurnId,
        text: &str,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ahp_types::state::ModelSelection>,
    ) -> ahp_types::actions::ChatTurnStartedAction {
        ahp_types::actions::ChatTurnStartedAction {
            turn_id: turn.as_str().to_owned(),
            started_at: humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string(),
            message: Message {
                text: text.to_owned(),
                origin: ahp_types::state::MessageOrigin {
                    kind: MessageKind::User,
                },
                attachments,
                model: Some(model.unwrap_or_else(|| ahp_types::state::ModelSelection {
                    id: "@provider=anthropic:default".to_owned(),
                    config: None,
                })),
                agent: None,
                meta: None,
            },
            queued_message_id: None,
            meta: None,
        }
    }

    // ------------------------------------------------------------------
    // Pages: a subscribe snapshot, and the window behind the cursor.

    /// A subscribe snapshot. An empty conversation takes the page
    /// whole; a live one MERGES — the wire window is the host's tail,
    /// and history we hold beyond it is ours to keep.
    pub fn landed(&self, state: &ChatState) -> (Self, Change) {
        let mut next = self.clone();
        next.older = match self.is_empty() {
            true => state.turns_next_cursor.clone(),
            false => self.older.clone().or(state.turns_next_cursor.clone()),
        };
        for wire in &state.turns {
            let settled = Turn::of_settled(wire);
            // Never regress the turn we are folding live, and never let
            // a lagging page overwrite a richer local fold.
            let ours = next.turns.get(&settled.id);
            let keep = ours.is_some_and(|held| {
                held.life == Life::Live || held.part_count() > settled.part_count()
            });
            if !keep {
                next = next.with_turn(settled);
            }
        }
        match &state.active_turn {
            Some(wire) => {
                let live = Turn::of_live(wire);
                let id = live.id.clone();
                let ours = next.turns.get(&id);
                // A snapshot that lags the stream we already fold must
                // not regress it.
                if !ours.is_some_and(|held| held.part_count() > live.part_count()) {
                    next = next.with_turn(live);
                }
                next.live = Some(id);
            }
            None => next.live = None,
        }
        (next, Change::Page)
    }

    /// The page behind the cursor: older turns, oldest first. A turn we
    /// already hold keeps its place — the wire may serve its cursor
    /// inclusively, and a turn is its id.
    pub fn prepended(&self, page: &[WireTurn], cursor: Option<String>) -> (Self, Change) {
        let mut said = VectorSync::new_sync();
        let mut turns = self.turns.clone();
        for wire in page {
            let turn = Turn::of_settled(wire);
            if !self.turns.contains_key(&turn.id) {
                said.push_back_mut(turn.id.clone());
            }
            turns.insert_mut(turn.id.clone(), turn);
        }
        for id in self.said.iter() {
            said.push_back_mut(id.clone());
        }
        (
            Self {
                said,
                turns,
                live: self.live.clone(),
                older: cursor,
            },
            Change::Page,
        )
    }

    // ------------------------------------------------------------------
    // The fold: one wire action in, the next conversation out.

    pub fn fold(&self, action: &ahp_types::actions::StateAction) -> (Self, Change) {
        use ahp_types::actions::StateAction as A;
        match action {
            A::ChatTurnStarted(a) => {
                let id = TurnId::new(a.turn_id.clone());
                // A start RESETS the turn's parts: a replayed stream
                // rebuilds it to one copy, and a start for a turn we
                // already hold replaces it where it stands.
                let mut next = self.with_turn(Turn::opened(id.clone(), &a.message));
                next.live = Some(id.clone());
                (next, Change::Said(id))
            }

            A::ChatResponsePart(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let mut landed = Change::Nothing;
                let mut next = self.clone();
                for (part, laid) in read_part(&a.part).iter().cloned() {
                    let Some(grown) =
                        next.update(&turn, |held| Some(held.with_part(part.clone(), laid)))
                    else {
                        return self.nothing();
                    };
                    next = grown;
                    landed = Change::Part {
                        turn: turn.clone(),
                        part,
                    };
                }
                (next, landed)
            }

            A::ChatDelta(a) => self.appended(&a.turn_id, &a.part_id, &a.content),
            A::ChatReasoning(a) => self.appended(&a.turn_id, &a.part_id, &a.content),

            A::ChatToolCallStart(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let part = PartId::new(a.tool_call_id.clone());
                let call = ToolCall {
                    id: part.clone(),
                    display: a.display_name.clone(),
                    invocation: String::new(),
                    input: None,
                    status: ToolStatus::Streaming,
                };
                match self.update(&turn, |held| {
                    Some(held.with_part(part.clone(), Part::Tool(call)))
                }) {
                    Some(next) => (next, Change::Part { turn, part }),
                    None => self.nothing(),
                }
            }

            A::ChatToolCallReady(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let part = PartId::new(a.tool_call_id.clone());
                let invocation = a.invocation_message.as_text().to_owned();
                let input = match &a.tool_input {
                    Some(ToolInput::Inline(text)) => Some(text.clone()),
                    _ => None,
                };
                let confirmed = a.confirmed.is_some();
                match self.update(&turn, |held| {
                    held.with_tool(&part, |mut call| {
                        call.invocation = invocation;
                        call.input = input;
                        call.status = match confirmed {
                            true => ToolStatus::Running,
                            false => ToolStatus::Waiting,
                        };
                        call
                    })
                }) {
                    Some(next) => (next, Change::Part { turn, part }),
                    None => self.nothing(),
                }
            }

            A::ChatToolCallConfirmed(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let part = PartId::new(a.tool_call_id.clone());
                let approved = a.approved;
                match self.update(&turn, |held| {
                    held.with_tool(&part, |mut call| {
                        call.status = match approved {
                            true => ToolStatus::Running,
                            false => ToolStatus::Denied,
                        };
                        call
                    })
                }) {
                    Some(next) => (next, Change::Part { turn, part }),
                    None => self.nothing(),
                }
            }

            A::ChatToolCallComplete(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let part = PartId::new(a.tool_call_id.clone());
                let done = ToolStatus::Done {
                    ok: a.result.success,
                    said: a.result.past_tense_message.as_text().to_owned(),
                    output: said_output(a.result.content.as_deref()),
                };
                // The completion is idempotent: the status is REPLACED
                // and the edits it carried are keyed by their own part
                // ids, so a replayed completion changes nothing.
                let edits = edits_of(&a.result);
                match self.update(&turn, |held| {
                    let dressed = held.with_tool(&part, |mut call| {
                        call.status = done;
                        call
                    })?;
                    Some(
                        edits
                            .iter()
                            .cloned()
                            .fold(dressed, |turn, (id, edit)| turn.with_part(id, edit)),
                    )
                }) {
                    Some(next) => {
                        let mut parts = VectorSync::new_sync();
                        parts.push_back_mut(part);
                        for (id, _) in edits.iter() {
                            parts.push_back_mut(id.clone());
                        }
                        (next, Change::Parts { turn, parts })
                    }
                    None => self.nothing(),
                }
            }

            A::ChatUsage(a) => {
                let Some(turn) = self.streaming(&a.turn_id) else {
                    return self.nothing();
                };
                let spent = spent(&a.usage);
                match self.update(&turn, |held| {
                    let mut next = held.clone();
                    next.usage = spent;
                    Some(next)
                }) {
                    Some(next) => (next, Change::Retired(turn)),
                    None => self.nothing(),
                }
            }

            A::ChatTurnComplete(a) => self.retired(&a.turn_id, Life::Complete),
            A::ChatTurnCancelled(a) => self.retired(&a.turn_id, Life::Cancelled),
            A::ChatError(a) => self.retired(
                &a.turn_id,
                Life::Failed(format!("{}: {}", a.error.error_type, a.error.message)),
            ),

            A::ChatTurnsLoaded(a) => self.prepended(&a.turns, a.turns_next_cursor.clone()),

            // `chat/truncated`: the host dropped history. Everything
            // after the named turn goes, and a bare truncate empties
            // the conversation.
            A::ChatTruncated(a) => {
                let mut said = VectorSync::new_sync();
                let mut turns = HashTrieMapSync::new_sync();
                if let Some(mark) = a.turn_id.as_deref() {
                    for id in self.said.iter() {
                        let Some(turn) = self.turns.get(id) else {
                            continue;
                        };
                        said.push_back_mut(id.clone());
                        turns.insert_mut(id.clone(), turn.clone());
                        if id.as_str() == mark {
                            break;
                        }
                    }
                }
                (
                    Self {
                        said,
                        turns,
                        live: None,
                        older: None,
                    },
                    Change::Page,
                )
            }

            _ => self.nothing(),
        }
    }

    fn nothing(&self) -> (Self, Change) {
        (self.clone(), Change::Nothing)
    }

    fn appended(&self, turn: &str, part: &str, text: &str) -> (Self, Change) {
        let Some(turn) = self.streaming(turn) else {
            return self.nothing();
        };
        let part = PartId::new(part);
        match self.update(&turn, |held| held.grown(&part, text)) {
            Some(next) => (
                next,
                Change::Grew {
                    turn,
                    part,
                    text: text.to_owned(),
                },
            ),
            None => self.nothing(),
        }
    }

    fn retired(&self, turn: &str, life: Life) -> (Self, Change) {
        let Some(turn) = self.streaming(turn) else {
            return self.nothing();
        };
        let Some(mut next) = self.update(&turn, |held| {
            let mut settled = held.clone();
            settled.life = life;
            Some(settled)
        }) else {
            return self.nothing();
        };
        next.live = None;
        (next, Change::Retired(turn))
    }
}

/// The voice a message speaks in.
fn voiced(message: &Message) -> (CellKind, String) {
    match message.origin.kind {
        MessageKind::User => (CellKind::User, message.text.clone()),
        MessageKind::Agent => (CellKind::Agent, message.text.clone()),
        MessageKind::Tool | MessageKind::SystemNotification => {
            (CellKind::Notice, format!("*{}*", message.text))
        }
    }
}

/// What a turn spent, as the notice the view shows.
fn spent(usage: &UsageInfo) -> Option<String> {
    let mut said = Vec::new();
    if let Some(model) = &usage.model {
        said.push(model.clone());
    }
    if let Some(input) = usage.input_tokens {
        said.push(format!("{input} in"));
    }
    if let Some(output) = usage.output_tokens {
        said.push(format!("{output} out"));
    }
    (!said.is_empty()).then(|| format!("*{}*", said.join(" · ")))
}

/// One wire part, read into the parts it lays — a tool call brings its
/// own edits, each keyed by the file it touched, so a replay lands the
/// same ids and replaces rather than doubles.
type Laid = VectorSync<(PartId, Part)>;

fn one(id: PartId, part: Part) -> Laid {
    let mut laid = VectorSync::new_sync();
    laid.push_back_mut((id, part));
    laid
}

fn read_part(part: &ResponsePart) -> Laid {
    match part {
        ResponsePart::Markdown(said) => one(
            PartId::new(said.id.clone()),
            Part::Said {
                voice: CellKind::Agent,
                text: said.content.clone(),
            },
        ),
        ResponsePart::Reasoning(said) if said.content.trim().is_empty() => VectorSync::new_sync(),
        ResponsePart::Reasoning(said) => one(
            PartId::new(said.id.clone()),
            Part::Said {
                voice: CellKind::Reasoning,
                text: said.content.clone(),
            },
        ),
        ResponsePart::SystemNotification(said) => one(
            PartId::new(format!("notice:{}", said.content.as_text())),
            Part::Said {
                voice: CellKind::Notice,
                text: format!("*{}*", said.content.as_text()),
            },
        ),
        ResponsePart::ToolCall(call) => read_tool(&call.tool_call),
        _ => VectorSync::new_sync(),
    }
}

fn read_tool(state: &ToolCallState) -> Laid {
    let (id, display, invocation, input, status, result) = match state {
        ToolCallState::Streaming(call) => (
            &call.tool_call_id,
            &call.display_name,
            call.invocation_message.clone().unwrap_or_default(),
            None,
            ToolStatus::Streaming,
            None,
        ),
        ToolCallState::PendingConfirmation(call) => (
            &call.tool_call_id,
            &call.display_name,
            call.invocation_message.clone(),
            inline(&call.tool_input),
            ToolStatus::Waiting,
            None,
        ),
        ToolCallState::Running(call) => (
            &call.tool_call_id,
            &call.display_name,
            call.invocation_message.clone(),
            inline(&call.tool_input),
            ToolStatus::Running,
            None,
        ),
        ToolCallState::Completed(call) => (
            &call.tool_call_id,
            &call.display_name,
            call.invocation_message.clone(),
            inline(&call.tool_input),
            ToolStatus::Done {
                ok: call.success,
                said: call.past_tense_message.as_text().to_owned(),
                output: said_output(call.content.as_deref()),
            },
            call.content.as_deref(),
        ),
        ToolCallState::Cancelled(call) => (
            &call.tool_call_id,
            &call.display_name,
            call.invocation_message.clone(),
            inline(&call.tool_input),
            ToolStatus::Denied,
            None,
        ),
        _ => return VectorSync::new_sync(),
    };
    let part = PartId::new(id.clone());
    let mut laid = one(
        part.clone(),
        Part::Tool(ToolCall {
            id: part,
            display: display.clone(),
            invocation: invocation.as_text().to_owned(),
            input,
            status,
        }),
    );
    if let Some(content) = result {
        for edit in edits_in(content).iter() {
            laid.push_back_mut(edit.clone());
        }
    }
    laid
}

fn said_output(content: Option<&[ahp_types::state::ToolResultContent]>) -> String {
    content
        .map(crate::higent::turn::tool_output)
        .unwrap_or_default()
}

fn inline(input: &Option<ToolInput>) -> Option<String> {
    match input {
        Some(ToolInput::Inline(text)) => Some(text.clone()),
        _ => None,
    }
}

/// The file edits a tool result carried. Each is keyed by the uri it
/// touched, so a replayed completion replaces its own diff.
fn edits_of(result: &ToolCallResult) -> Laid {
    edits_in(result.content.as_deref().unwrap_or_default())
}

fn edits_in(content: &[ahp_types::state::ToolResultContent]) -> Laid {
    use ahp_types::state::ToolResultContent;
    content
        .iter()
        .filter_map(|block| match block {
            ToolResultContent::FileEdit(edit) => {
                let refs = crate::higent::FileEditRefs::parse(edit)?;
                let key = refs
                    .after
                    .as_ref()
                    .or(refs.before.as_ref())
                    .map(|side| side.content.uri.clone())
                    .unwrap_or_default();
                Some((PartId::new(format!("edit:{key}")), Part::Edit(refs)))
            }
            _ => None,
        })
        .collect()
}
