// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The conversation model against the protocol's own semantics (the
//! reference reducer in the `ahp` crate is the contract). Pure values
//! in, pure values out — no store, no window, no paint.

use ahp_types::actions::{
    ChatDeltaAction, ChatErrorAction, ChatReasoningAction, ChatResponsePartAction,
    ChatToolCallCompleteAction, ChatToolCallConfirmedAction, ChatToolCallReadyAction,
    ChatToolCallStartAction, ChatTruncatedAction, ChatTurnCancelledAction, ChatTurnCompleteAction,
    ChatTurnStartedAction, ChatTurnsLoadedAction, ChatUsageAction, StateAction,
};
use ahp_types::common::StringOrMarkdown;
use ahp_types::state::{
    ActiveTurn, ChatState, ErrorInfo, MarkdownResponsePart, Message, MessageKind, MessageOrigin,
    ReasoningResponsePart, ResponsePart, ToolCallResult, Turn as WireTurn, TurnState, UsageInfo,
};

use super::model::{Change, Conversation, Life, Part, PartId, ToolStatus};
use crate::higent::cell::CellKind;
use crate::higent::TurnId;

// ----------------------------------------------------------------------
// Wire builders.

fn user(text: &str) -> Message {
    Message {
        text: text.to_owned(),
        origin: MessageOrigin {
            kind: MessageKind::User,
        },
        attachments: None,
        model: None,
        agent: None,
        meta: None,
    }
}

fn agent(text: &str) -> Message {
    Message {
        text: text.to_owned(),
        origin: MessageOrigin {
            kind: MessageKind::Agent,
        },
        attachments: None,
        model: None,
        agent: None,
        meta: None,
    }
}

fn started(turn: &str, text: &str) -> StateAction {
    StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: turn.to_owned(),
        started_at: String::new(),
        message: user(text),
        queued_message_id: None,
        meta: None,
    })
}

fn markdown(turn: &str, part: &str, content: &str) -> StateAction {
    StateAction::ChatResponsePart(ChatResponsePartAction {
        turn_id: turn.to_owned(),
        part: ResponsePart::Markdown(MarkdownResponsePart {
            id: part.to_owned(),
            content: content.to_owned(),
        }),
        meta: None,
    })
}

fn reasoning(turn: &str, part: &str, content: &str) -> StateAction {
    StateAction::ChatResponsePart(ChatResponsePartAction {
        turn_id: turn.to_owned(),
        part: ResponsePart::Reasoning(ReasoningResponsePart {
            id: part.to_owned(),
            content: content.to_owned(),
        }),
        meta: None,
    })
}

fn delta(turn: &str, part: &str, content: &str) -> StateAction {
    StateAction::ChatDelta(ChatDeltaAction {
        turn_id: turn.to_owned(),
        part_id: part.to_owned(),
        content: content.to_owned(),
        meta: None,
    })
}

fn reasoning_delta(turn: &str, part: &str, content: &str) -> StateAction {
    StateAction::ChatReasoning(ChatReasoningAction {
        turn_id: turn.to_owned(),
        part_id: part.to_owned(),
        content: content.to_owned(),
        meta: None,
    })
}

fn tool_start(turn: &str, tool: &str, display: &str) -> StateAction {
    StateAction::ChatToolCallStart(ChatToolCallStartAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        tool_name: display.to_lowercase(),
        display_name: display.to_owned(),
        intention: None,
        contributor: None,
        meta: None,
    })
}

fn tool_ready(turn: &str, tool: &str, invocation: &str, auto: bool) -> StateAction {
    StateAction::ChatToolCallReady(ChatToolCallReadyAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        meta: None,
        contributor: None,
        intention: None,
        invocation_message: StringOrMarkdown::Plain(invocation.to_owned()),
        tool_input: None,
        confirmation_title: None,
        risk_assessment: None,
        edits: None,
        editable: None,
        confirmed: auto.then_some(ahp_types::state::ToolCallConfirmationReason::Setting),
        options: None,
    })
}

fn tool_confirmed(turn: &str, tool: &str, approved: bool) -> StateAction {
    StateAction::ChatToolCallConfirmed(ChatToolCallConfirmedAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        meta: None,
        approved,
        confirmed: None,
        reason: None,
        edited_tool_input: None,
        user_suggestion: None,
        reason_message: None,
        selected_option_id: None,
    })
}

fn tool_done(turn: &str, tool: &str, ok: bool, said: &str) -> StateAction {
    StateAction::ChatToolCallComplete(ChatToolCallCompleteAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        result: ToolCallResult {
            success: ok,
            past_tense_message: StringOrMarkdown::Plain(said.to_owned()),
            content: None,
            structured_content: None,
            error: None,
        },
        requires_result_confirmation: None,
        meta: None,
    })
}

/// A completion carrying file edits: (path, content uri) per side pair —
/// the same uri before and after keeps the helper short.
fn tool_done_with_edits(turn: &str, tool: &str, edits: Vec<(&str, &str)>) -> StateAction {
    let content = edits
        .into_iter()
        .map(|(path, uri)| {
            ahp_types::state::ToolResultContent::FileEdit(
                crate::higent::FileEditRefs {
                    before: Some(crate::higent::snapshot(path, uri)),
                    after: Some(crate::higent::snapshot(path, uri)),
                    counts: crate::higent::DiffCounts::default(),
                }
                .to_content(),
            )
        })
        .collect();
    StateAction::ChatToolCallComplete(ChatToolCallCompleteAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        result: ToolCallResult {
            success: true,
            past_tense_message: StringOrMarkdown::Plain("edited".to_owned()),
            content: Some(content),
            structured_content: None,
            error: None,
        },
        requires_result_confirmation: None,
        meta: None,
    })
}

fn complete(turn: &str) -> StateAction {
    StateAction::ChatTurnComplete(ChatTurnCompleteAction {
        turn_id: turn.to_owned(),
        duration: 1,
        meta: None,
    })
}

fn cancelled(turn: &str) -> StateAction {
    StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
        turn_id: turn.to_owned(),
        duration: 1,
        meta: None,
    })
}

fn failed(turn: &str, kind: &str, message: &str) -> StateAction {
    StateAction::ChatError(ChatErrorAction {
        turn_id: turn.to_owned(),
        duration: 1,
        error: ErrorInfo {
            error_type: kind.to_owned(),
            message: message.to_owned(),
            stack: None,
            meta: None,
        },
        meta: None,
    })
}

fn usage(turn: &str, input: i64, output: i64) -> StateAction {
    StateAction::ChatUsage(ChatUsageAction {
        turn_id: turn.to_owned(),
        usage: UsageInfo {
            input_tokens: Some(input),
            output_tokens: Some(output),
            model: None,
            cache_read_tokens: None,
            meta: None,
        },
        meta: None,
    })
}

fn truncated(turn: Option<&str>) -> StateAction {
    StateAction::ChatTruncated(ChatTruncatedAction {
        turn_id: turn.map(str::to_owned),
    })
}

fn said_turn(id: &str, prompt: &str, reply: &str) -> WireTurn {
    WireTurn {
        id: id.to_owned(),
        started_at: None,
        duration: None,
        message: user(prompt),
        response_parts: vec![ResponsePart::Markdown(MarkdownResponsePart {
            id: format!("{id}-p1"),
            content: reply.to_owned(),
        })],
        usage: None,
        state: TurnState::Complete,
        error: None,
    }
}

fn bare_turn(id: &str, prompt: &str, state: TurnState) -> WireTurn {
    WireTurn {
        id: id.to_owned(),
        started_at: None,
        duration: None,
        message: user(prompt),
        response_parts: Vec::new(),
        usage: None,
        state,
        error: None,
    }
}

fn live_turn(id: &str, prompt: &str, parts: Vec<ResponsePart>) -> ActiveTurn {
    ActiveTurn {
        id: id.to_owned(),
        started_at: String::new(),
        message: user(prompt),
        response_parts: parts,
        usage: None,
    }
}

fn snapshot(turns: Vec<WireTurn>, live: Option<ActiveTurn>, cursor: Option<&str>) -> ChatState {
    ChatState {
        resource: "ahp-chat:/c".to_owned(),
        title: "a chat".to_owned(),
        status: 0,
        activity: None,
        modified_at: String::new(),
        origin: None,
        interactivity: None,
        working_directories: None,
        turns,
        turns_next_cursor: cursor.map(str::to_owned),
        active_turn: live,
        steering_message: None,
        queued_messages: None,
        draft: None,
        meta: None,
    }
}

// ----------------------------------------------------------------------
// Oracles.

fn turn(chat: &Conversation, id: &str) -> super::model::Turn {
    chat.turn(&TurnId::new(id))
        .unwrap_or_else(|| panic!("no turn {id}; held: {:?}", ids(chat)))
        .clone()
}

fn ids(chat: &Conversation) -> Vec<String> {
    chat.turns()
        .map(|turn| turn.id.as_str().to_owned())
        .collect()
}

/// Every text a turn carries, prompt first, in order.
fn texts(chat: &Conversation, id: &str) -> Vec<String> {
    let held = turn(chat, id);
    let mut said = vec![held.prompt.1.clone()];
    for (_, part) in held.parts() {
        if let Part::Said { text, .. } = part {
            said.push(text.clone());
        }
    }
    said
}

fn voices(chat: &Conversation, id: &str) -> Vec<CellKind> {
    let held = turn(chat, id);
    let mut voices = vec![held.prompt.0];
    for (_, part) in held.parts() {
        if let Part::Said { voice, .. } = part {
            voices.push(*voice);
        }
    }
    voices
}

/// Everything the conversation says, in order — the "said once" oracle.
fn all(chat: &Conversation) -> String {
    chat.turns()
        .flat_map(|turn| {
            std::iter::once(turn.prompt.1.clone()).chain(turn.parts().filter_map(|(_, part)| {
                match part {
                    Part::Said { text, .. } => Some(text.clone()),
                    _ => None,
                }
            }))
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn status(chat: &Conversation, id: &str, tool: &str) -> ToolStatus {
    turn(chat, id)
        .tool(&PartId::new(tool))
        .unwrap_or_else(|| panic!("no tool {tool} in {id}"))
        .status
        .clone()
}

/// Fold a batch, keeping the last change.
fn fold(chat: &Conversation, actions: Vec<StateAction>) -> (Conversation, Change) {
    actions
        .into_iter()
        .fold((chat.clone(), Change::Nothing), |(chat, _), action| {
            chat.fold(&action)
        })
}

fn only(action: StateAction) -> (Conversation, Change) {
    Conversation::default().fold(&action)
}

fn streamed(turn: &str, prompt: &str, reply: &str) -> Vec<StateAction> {
    vec![
        started(turn, prompt),
        markdown(turn, "p1", ""),
        delta(turn, "p1", reply),
    ]
}

// ======================================================================
// A. A turn starts.

#[test]
fn a_start_opens_a_turn() {
    let (chat, change) = only(started("t1", "hello"));
    assert_eq!(ids(&chat), vec!["t1"]);
    assert_eq!(texts(&chat, "t1"), vec!["hello"]);
    assert_eq!(change, Change::Said(TurnId::new("t1")));
}

#[test]
fn a_start_makes_the_turn_live() {
    let (chat, _) = only(started("t1", "hello"));
    assert_eq!(chat.live().map(TurnId::as_str), Some("t1"));
    assert!(chat.is_running());
}

#[test]
fn a_users_prompt_speaks_as_the_user() {
    let (chat, _) = only(started("t1", "hello"));
    assert_eq!(voices(&chat, "t1"), vec![CellKind::User]);
}

#[test]
fn an_agents_prompt_speaks_as_the_agent() {
    let (chat, _) =
        Conversation::default().fold(&StateAction::ChatTurnStarted(ChatTurnStartedAction {
            turn_id: "t1".to_owned(),
            started_at: String::new(),
            message: agent("from the agent"),
            queued_message_id: None,
            meta: None,
        }));
    assert_eq!(voices(&chat, "t1"), vec![CellKind::Agent]);
}

#[test]
fn two_starts_keep_both_turns_in_order() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "one"), complete("t1"), started("t2", "two")],
    );
    assert_eq!(ids(&chat), vec!["t1", "t2"]);
}

#[test]
fn a_start_for_a_turn_we_hold_mints_no_second_copy() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hello"), started("t1", "hello")],
    );
    assert_eq!(ids(&chat), vec!["t1"]);
    assert_eq!(all(&chat), "hello");
}

#[test]
fn a_start_resets_the_turns_parts() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "hello", "a reply"));
    let (chat, _) = chat.fold(&started("t1", "hello"));
    assert_eq!(
        texts(&chat, "t1"),
        vec!["hello"],
        "the replayed stream starts from the prompt again"
    );
}

#[test]
fn a_start_for_another_turn_closes_the_first() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "one", "reply"));
    let (chat, _) = chat.fold(&started("t2", "two"));
    assert_eq!(chat.live().map(TurnId::as_str), Some("t2"));
}

// ======================================================================
// B. The write-ahead send: the client mints the turn.

#[test]
fn a_sent_message_is_a_turn_at_once() {
    let opening = Conversation::opening(&TurnId::new("mine-1"), "my own message", None, None);
    let (chat, change) = Conversation::default().fold(&StateAction::ChatTurnStarted(opening));

    assert_eq!(ids(&chat), vec!["mine-1"]);
    assert_eq!(texts(&chat, "mine-1"), vec!["my own message"]);
    assert_eq!(change, Change::Said(TurnId::new("mine-1")));
}

#[test]
fn a_sent_message_speaks_as_the_user() {
    let opening = Conversation::opening(&TurnId::new("mine-1"), "mine", None, None);
    let (chat, _) = Conversation::default().fold(&StateAction::ChatTurnStarted(opening));
    assert_eq!(voices(&chat, "mine-1"), vec![CellKind::User]);
}

/// The host echoes the very action the client dispatched. It must fold
/// to NOTHING visible — this is the road that used to leave a
/// placeholder standing and the composer stuck.
#[test]
fn the_hosts_echo_of_our_own_send_changes_nothing() {
    let opening = Conversation::opening(&TurnId::new("mine-1"), "my own message", None, None);
    let (chat, _) = Conversation::default().fold(&StateAction::ChatTurnStarted(opening.clone()));
    let before = all(&chat);

    let (chat, _) = chat.fold(&StateAction::ChatTurnStarted(opening));

    assert_eq!(all(&chat), before, "said exactly once");
    assert_eq!(ids(&chat), vec!["mine-1"], "one turn, not two");
}

#[test]
fn a_sent_message_is_the_turn_stop_cancels() {
    let opening = Conversation::opening(&TurnId::new("mine-1"), "mine", None, None);
    let (chat, _) = Conversation::default().fold(&StateAction::ChatTurnStarted(opening));
    assert_eq!(
        chat.live().map(TurnId::as_str),
        Some("mine-1"),
        "there is always a turn to cancel once a send is folded"
    );
}

#[test]
fn the_reply_to_a_sent_message_lands_in_it() {
    let turn = TurnId::new("mine-1");
    let (chat, _) = Conversation::default().fold(&StateAction::ChatTurnStarted(
        Conversation::opening(&turn, "mine", None, None),
    ));
    let (chat, _) = fold(
        &chat,
        vec![
            markdown("mine-1", "p1", "the "),
            delta("mine-1", "p1", "answer"),
        ],
    );
    assert_eq!(texts(&chat, "mine-1"), vec!["mine", "the answer"]);
}

// ======================================================================
// C. Parts and deltas.

#[test]
fn a_markdown_part_joins_the_live_turn() {
    let (chat, change) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), markdown("t1", "p1", "the reply")],
    );
    assert_eq!(texts(&chat, "t1"), vec!["hi", "the reply"]);
    assert_eq!(
        change,
        Change::Part {
            turn: TurnId::new("t1"),
            part: PartId::new("p1")
        }
    );
}

/// A REPLAYED completed tool call arrives as one response part that
/// lands several model parts — the call and its edits. The change
/// names them all; a view that placed only the last would lose the
/// call's cell and every edit but one.
#[test]
fn a_completed_tool_part_names_its_call_and_its_edits() {
    use ahp_types::state::{
        ToolCallCompletedState, ToolCallConfirmationReason, ToolCallResponsePart, ToolCallState,
        ToolResultContent,
    };
    let edit = |path: &str, uri: &str| {
        ToolResultContent::FileEdit(
            crate::higent::FileEditRefs {
                before: Some(crate::higent::snapshot(path, uri)),
                after: Some(crate::higent::snapshot(path, uri)),
                counts: crate::higent::DiffCounts::default(),
            }
            .to_content(),
        )
    };
    let replayed = StateAction::ChatResponsePart(ChatResponsePartAction {
        turn_id: "t1".to_owned(),
        part: ResponsePart::ToolCall(Box::new(ToolCallResponsePart {
            tool_call: ToolCallState::Completed(ToolCallCompletedState {
                tool_call_id: "tool-1".to_owned(),
                tool_name: "edit".to_owned(),
                display_name: "Edit".to_owned(),
                intention: None,
                contributor: None,
                meta: None,
                invocation_message: StringOrMarkdown::Plain("editing".to_owned()),
                tool_input: None,
                success: true,
                past_tense_message: StringOrMarkdown::Plain("edited".to_owned()),
                content: Some(vec![
                    edit("src/a.rs", "ahp-content:/a1"),
                    edit("src/b.rs", "ahp-content:/b1"),
                ]),
                structured_content: None,
                error: None,
                confirmed: ToolCallConfirmationReason::NotNeeded,
                selected_option: None,
            }),
        })),
        meta: None,
    });
    let (chat, change) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), replayed],
    );
    let Change::Parts { turn, parts } = change else {
        panic!("a completed tool part names its parts: {change:?}");
    };
    assert_eq!(turn, TurnId::new("t1"));
    assert_eq!(
        parts
            .iter()
            .map(|part| part.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["tool-1", "edit:ahp-content:/a1", "edit:ahp-content:/b1"]
    );
    assert_eq!(chat.turn(&turn).expect("the turn").part_count(), 3);
}

/// A completion moves its call AND every edit it carried: the change
/// names them all, so a view lays each edit's cell without walking the
/// turn.
#[test]
fn a_completion_names_its_call_and_its_edits() {
    let (_, change) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "tool-1", "Edit"),
            tool_done_with_edits(
                "t1",
                "tool-1",
                vec![
                    ("src/a.rs", "ahp-content:/a1"),
                    ("src/b.rs", "ahp-content:/b1"),
                ],
            ),
        ],
    );
    let Change::Parts { turn, parts } = change else {
        panic!("a completion names its parts: {change:?}");
    };
    assert_eq!(turn, TurnId::new("t1"));
    assert_eq!(
        parts
            .iter()
            .map(|part| part.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["tool-1", "edit:ahp-content:/a1", "edit:ahp-content:/b1"]
    );
}

#[test]
fn a_markdown_part_speaks_as_the_agent() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), markdown("t1", "p1", "reply")],
    );
    assert_eq!(voices(&chat, "t1"), vec![CellKind::User, CellKind::Agent]);
}

#[test]
fn a_delta_grows_its_part() {
    let (chat, change) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            markdown("t1", "p1", "a"),
            delta("t1", "p1", "b"),
        ],
    );
    assert_eq!(texts(&chat, "t1")[1], "ab");
    assert_eq!(
        change,
        Change::Grew {
            turn: TurnId::new("t1"),
            part: PartId::new("p1"),
            text: "b".to_owned()
        }
    );
}

#[test]
fn forty_deltas_assemble_in_order() {
    let mut chat = Conversation::default();
    for action in vec![started("t1", "hi"), markdown("t1", "p1", "")] {
        (chat, _) = chat.fold(&action);
    }
    for chunk in 0..40 {
        (chat, _) = chat.fold(&delta("t1", "p1", &format!("{chunk} ")));
    }
    let expected: String = (0..40).map(|chunk| format!("{chunk} ")).collect();
    assert_eq!(texts(&chat, "t1")[1], expected);
}

#[test]
fn two_parts_keep_their_own_text() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            markdown("t1", "p1", "one"),
            markdown("t1", "p2", "two"),
            delta("t1", "p1", "!"),
            delta("t1", "p2", "?"),
        ],
    );
    assert_eq!(texts(&chat, "t1"), vec!["hi", "one!", "two?"]);
}

#[test]
fn parts_keep_the_order_they_arrived_in() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            markdown("t1", "p1", "first"),
            markdown("t1", "p2", "second"),
            markdown("t1", "p3", "third"),
        ],
    );
    assert_eq!(texts(&chat, "t1"), vec!["hi", "first", "second", "third"]);
}

#[test]
fn a_replayed_part_replaces_its_own_text() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            markdown("t1", "p1", "base"),
            delta("t1", "p1", " grown"),
            markdown("t1", "p1", "base"),
        ],
    );
    assert_eq!(
        texts(&chat, "t1")[1],
        "base",
        "the part rewound to its base"
    );
    assert_eq!(
        turn(&chat, "t1").part_count(),
        1,
        "and minted no second cell"
    );
}

#[test]
fn a_replayed_stream_folds_to_one_copy() {
    let stream = streamed("t1", "hi", "the reply");
    let (chat, _) = fold(&Conversation::default(), stream.clone());
    let (chat, _) = fold(&chat, stream);
    assert_eq!(texts(&chat, "t1"), vec!["hi", "the reply"]);
}

#[test]
fn a_long_replayed_stream_folds_to_one_copy() {
    let mut stream = vec![started("t1", "hi"), markdown("t1", "p1", "chapter ")];
    for chunk in 0..40 {
        stream.push(delta("t1", "p1", &format!("{chunk} ")));
    }
    stream.push(markdown("t1", "p2", "the closing word"));
    let (chat, _) = fold(&Conversation::default(), stream.clone());
    let once = texts(&chat, "t1");
    let (chat, _) = fold(&chat, stream);
    assert_eq!(texts(&chat, "t1"), once, "the replay folded to one copy");
}

#[test]
fn a_delta_for_an_unknown_part_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), markdown("t1", "p1", "one")],
    );
    let (next, change) = chat.fold(&delta("t1", "ghost", "junk"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(texts(&next, "t1"), vec!["hi", "one"]);
}

#[test]
fn a_delta_for_an_unknown_turn_is_nothing() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "hi", "one"));
    let (next, change) = chat.fold(&delta("t9", "p1", "junk"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(all(&next), all(&chat));
}

/// THE MIDDLE-OF-THE-CHAT BUG, structurally: a settled turn is
/// immutable, so a late delta cannot write into it.
#[test]
fn a_delta_for_a_settled_turn_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "one"),
            markdown("t1", "p1", "reply one"),
            complete("t1"),
            started("t2", "two"),
        ],
    );
    let (next, change) = chat.fold(&delta("t1", "p1", " MORE"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(
        texts(&next, "t1"),
        vec!["one", "reply one"],
        "the turn in the middle is untouched"
    );
}

#[test]
fn a_part_for_a_settled_turn_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), complete("t1")],
    );
    let (next, change) = chat.fold(&markdown("t1", "late", "too late"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(texts(&next, "t1"), vec!["hi"]);
}

#[test]
fn a_part_with_no_turn_in_flight_is_nothing() {
    let (_, change) = only(markdown("t1", "p1", "orphan"));
    assert_eq!(change, Change::Nothing);
}

#[test]
fn a_reasoning_part_speaks_as_reasoning() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), reasoning("t1", "r1", "thinking")],
    );
    assert_eq!(
        voices(&chat, "t1"),
        vec![CellKind::User, CellKind::Reasoning]
    );
}

#[test]
fn a_reasoning_delta_grows_its_part() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            reasoning("t1", "r1", "one"),
            reasoning_delta("t1", "r1", ", two"),
        ],
    );
    assert_eq!(texts(&chat, "t1")[1], "one, two");
}

#[test]
fn an_empty_reasoning_part_says_nothing_yet() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), reasoning("t1", "r1", "   ")],
    );
    assert_eq!(turn(&chat, "t1").part_count(), 0);
}

#[test]
fn the_same_part_id_in_a_new_turn_is_its_own_part() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "one"),
            markdown("t1", "p1", "first"),
            complete("t1"),
            started("t2", "two"),
            markdown("t2", "p1", "second"),
        ],
    );
    assert_eq!(texts(&chat, "t1"), vec!["one", "first"]);
    assert_eq!(texts(&chat, "t2"), vec!["two", "second"]);
}

// ======================================================================
// D. Tool calls.

#[test]
fn a_tool_start_joins_the_turn() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    assert_eq!(status(&chat, "t1", "c1"), ToolStatus::Streaming);
    assert_eq!(turn(&chat, "t1").part_count(), 1);
}

#[test]
fn a_tool_start_carries_its_display_name() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    assert_eq!(
        turn(&chat, "t1").tool(&PartId::new("c1")).unwrap().display,
        "Bash"
    );
}

#[test]
fn a_replayed_tool_start_is_one_call() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_start("t1", "c1", "Bash"),
        ],
    );
    assert_eq!(turn(&chat, "t1").part_count(), 1);
}

#[test]
fn two_tool_calls_are_two_parts() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_start("t1", "c2", "Read"),
        ],
    );
    assert_eq!(turn(&chat, "t1").part_count(), 2);
}

#[test]
fn a_ready_call_awaiting_approval_waits() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_ready("t1", "c1", "rm -rf /", false),
        ],
    );
    assert_eq!(status(&chat, "t1", "c1"), ToolStatus::Waiting);
    assert_eq!(
        turn(&chat, "t1")
            .tool(&PartId::new("c1"))
            .unwrap()
            .invocation,
        "rm -rf /"
    );
}

#[test]
fn an_auto_confirmed_call_runs() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_ready("t1", "c1", "ls", true),
        ],
    );
    assert_eq!(status(&chat, "t1", "c1"), ToolStatus::Running);
}

#[test]
fn an_approval_runs_the_call() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_ready("t1", "c1", "ls", false),
            tool_confirmed("t1", "c1", true),
        ],
    );
    assert_eq!(status(&chat, "t1", "c1"), ToolStatus::Running);
}

#[test]
fn a_denial_denies_the_call() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_ready("t1", "c1", "ls", false),
            tool_confirmed("t1", "c1", false),
        ],
    );
    assert_eq!(status(&chat, "t1", "c1"), ToolStatus::Denied);
}

#[test]
fn a_completion_settles_the_call() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_done("t1", "c1", true, "ran it"),
        ],
    );
    assert_eq!(
        status(&chat, "t1", "c1"),
        ToolStatus::Done {
            ok: true,
            said: "ran it".to_owned(),
            output: String::new()
        }
    );
}

#[test]
fn a_failed_call_says_it_failed() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_done("t1", "c1", false, "it broke"),
        ],
    );
    assert!(matches!(
        status(&chat, "t1", "c1"),
        ToolStatus::Done { ok: false, .. }
    ));
}

#[test]
fn a_replayed_completion_changes_nothing_twice() {
    let done = vec![
        started("t1", "hi"),
        tool_start("t1", "c1", "Bash"),
        tool_done("t1", "c1", true, "ran it"),
    ];
    let (chat, _) = fold(&Conversation::default(), done.clone());
    let once = turn(&chat, "t1").part_count();
    let (chat, _) = fold(&chat, vec![tool_done("t1", "c1", true, "ran it")]);
    assert_eq!(turn(&chat, "t1").part_count(), once);
}

#[test]
fn a_completion_for_an_unknown_call_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    let (_, change) = chat.fold(&tool_done("t1", "ghost", true, "nope"));
    assert_eq!(change, Change::Nothing);
}

#[test]
fn a_tool_start_for_a_settled_turn_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), complete("t1")],
    );
    let (next, change) = chat.fold(&tool_start("t1", "c1", "Bash"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(turn(&next, "t1").part_count(), 0);
}

#[test]
fn a_ready_for_an_unknown_call_is_nothing() {
    let (chat, _) = fold(&Conversation::default(), vec![started("t1", "hi")]);
    let (_, change) = chat.fold(&tool_ready("t1", "ghost", "ls", false));
    assert_eq!(change, Change::Nothing);
}

// ======================================================================
// E. How a turn ends.

#[test]
fn a_completion_settles_the_turn() {
    let (chat, change) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), complete("t1")],
    );
    assert_eq!(turn(&chat, "t1").life, Life::Complete);
    assert_eq!(change, Change::Retired(TurnId::new("t1")));
}

#[test]
fn a_completion_closes_the_stream() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), complete("t1")],
    );
    assert!(chat.live().is_none());
    assert!(!chat.is_running());
}

#[test]
fn a_completion_for_another_turn_is_nothing() {
    let (chat, _) = fold(&Conversation::default(), vec![started("t1", "hi")]);
    let (next, change) = chat.fold(&complete("t9"));
    assert_eq!(change, Change::Nothing);
    assert_eq!(next.live().map(TurnId::as_str), Some("t1"));
}

#[test]
fn a_completion_keeps_what_was_said() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "hi", "the reply"));
    let (chat, _) = chat.fold(&complete("t1"));
    assert_eq!(texts(&chat, "t1"), vec!["hi", "the reply"]);
}

#[test]
fn a_cancel_settles_the_turn_cancelled() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), cancelled("t1")],
    );
    assert_eq!(turn(&chat, "t1").life, Life::Cancelled);
    assert!(chat.live().is_none());
}

#[test]
fn an_error_settles_the_turn_failed() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), failed("t1", "overloaded", "try later")],
    );
    match turn(&chat, "t1").life {
        Life::Failed(said) => {
            assert!(said.contains("overloaded") && said.contains("try later"))
        }
        life => panic!("expected a failure, got {life:?}"),
    }
}

#[test]
fn an_error_closes_the_stream() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), failed("t1", "boom", "bang")],
    );
    assert!(chat.live().is_none());
}

#[test]
fn usage_lands_on_the_live_turn() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), usage("t1", 10, 20)],
    );
    let spent = turn(&chat, "t1").usage.expect("the usage notice");
    assert!(spent.contains("10 in") && spent.contains("20 out"));
}

#[test]
fn usage_for_a_settled_turn_is_nothing() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "hi"), complete("t1")],
    );
    let (_, change) = chat.fold(&usage("t1", 1, 2));
    assert_eq!(change, Change::Nothing);
}

// ======================================================================
// F. Pages: the subscribe snapshot and the window behind the cursor.

#[test]
fn a_first_snapshot_lands_its_page() {
    let (chat, change) = Conversation::default().landed(&snapshot(
        vec![
            said_turn("t1", "one", "reply one"),
            said_turn("t2", "two", "reply two"),
        ],
        None,
        None,
    ));
    assert_eq!(ids(&chat), vec!["t1", "t2"]);
    assert_eq!(all(&chat), "one|reply one|two|reply two");
    assert_eq!(change, Change::Page);
}

#[test]
fn a_snapshot_keeps_its_cursor() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t2", "two", "reply")],
        None,
        Some("older-1"),
    ));
    assert_eq!(chat.older().map(String::as_str), Some("older-1"));
}

#[test]
fn a_re_snapshot_keeps_history_outside_the_window() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "one"),
            markdown("t1", "p1", "reply one"),
            complete("t1"),
            started("t2", "two"),
            markdown("t2", "p1", "reply two"),
            complete("t2"),
        ],
    );
    let (chat, _) = chat.landed(&snapshot(
        vec![said_turn("t2", "two", "reply two")],
        None,
        Some("1"),
    ));
    assert_eq!(ids(&chat), vec!["t1", "t2"], "t1 outlived the tail window");
}

#[test]
fn a_poorer_wire_row_never_blanks_a_richer_fold() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![
            started("t1", "one"),
            markdown("t1", "p1", "the streamed reply"),
            complete("t1"),
        ],
    );
    let (chat, _) = chat.landed(&snapshot(
        vec![bare_turn("t1", "one", TurnState::Complete)],
        None,
        None,
    ));
    assert!(
        all(&chat).contains("the streamed reply"),
        "a prompt-only backfill did not blank the turn: {}",
        all(&chat)
    );
}

#[test]
fn a_richer_wire_row_refreshes_a_thin_turn() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "one"), complete("t1")],
    );
    let (chat, _) = chat.landed(&snapshot(
        vec![said_turn("t1", "one", "the full reply")],
        None,
        None,
    ));
    assert!(all(&chat).contains("the full reply"));
}

#[test]
fn a_snapshot_never_regresses_the_turn_in_flight() {
    let (chat, _) = fold(
        &Conversation::default(),
        streamed("t1", "one", "half a reply"),
    );
    let (chat, _) = chat.landed(&snapshot(
        vec![bare_turn("t1", "one", TurnState::Complete)],
        None,
        None,
    ));
    assert!(
        all(&chat).contains("half a reply"),
        "the page lags the live stream: {}",
        all(&chat)
    );
}

#[test]
fn a_snapshot_adopts_the_turn_in_flight() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        Vec::new(),
        Some(live_turn(
            "t1",
            "hello",
            vec![ResponsePart::Markdown(MarkdownResponsePart {
                id: "p1".to_owned(),
                content: "so far".to_owned(),
            })],
        )),
        None,
    ));
    assert_eq!(chat.live().map(TurnId::as_str), Some("t1"));
    assert_eq!(texts(&chat, "t1"), vec!["hello", "so far"]);
}

#[test]
fn the_adopted_stream_keeps_folding() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        Vec::new(),
        Some(live_turn(
            "t1",
            "hello",
            vec![ResponsePart::Markdown(MarkdownResponsePart {
                id: "p1".to_owned(),
                content: "so far".to_owned(),
            })],
        )),
        None,
    ));
    let (chat, _) = chat.fold(&delta("t1", "p1", " and more"));
    assert_eq!(texts(&chat, "t1")[1], "so far and more");
}

#[test]
fn a_snapshot_with_no_active_turn_closes_the_stream() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "hi", "reply"));
    let (chat, _) = chat.landed(&snapshot(vec![said_turn("t1", "hi", "reply")], None, None));
    assert!(chat.live().is_none());
}

#[test]
fn a_cancelled_wire_turn_reads_cancelled() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![bare_turn("t1", "one", TurnState::Cancelled)],
        None,
        None,
    ));
    assert_eq!(turn(&chat, "t1").life, Life::Cancelled);
}

#[test]
fn a_failed_wire_turn_carries_its_error() {
    let mut wire = bare_turn("t1", "one", TurnState::Error);
    wire.error = Some(ErrorInfo {
        error_type: "boom".to_owned(),
        message: "it broke".to_owned(),
        stack: None,
        meta: None,
    });
    let (chat, _) = Conversation::default().landed(&snapshot(vec![wire], None, None));
    assert!(matches!(turn(&chat, "t1").life, Life::Failed(said) if said.contains("it broke")));
}

#[test]
fn an_older_page_goes_before_what_we_hold() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t2", "two", "reply two")],
        None,
        Some("c1"),
    ));
    let (chat, change) = chat.prepended(&[said_turn("t0", "zero", "reply zero")], None);
    assert_eq!(ids(&chat), vec!["t0", "t2"]);
    assert_eq!(change, Change::Page);
}

/// The wire may serve its cursor INCLUSIVELY. A turn is its id, so an
/// overlapping page cannot land a second copy — which is what used to
/// let a reply appear in the middle of the conversation.
#[test]
fn an_overlapping_older_page_lands_no_second_copy() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![
            said_turn("t1", "one", "reply one"),
            said_turn("t2", "two", "reply two"),
        ],
        None,
        Some("c1"),
    ));
    let (chat, _) = chat.prepended(
        &[
            said_turn("t0", "zero", "reply zero"),
            said_turn("t1", "one", "reply one"),
        ],
        None,
    );
    assert_eq!(ids(&chat), vec!["t0", "t1", "t2"]);
    assert_eq!(all(&chat), "zero|reply zero|one|reply one|two|reply two");
}

#[test]
fn an_older_page_keeps_the_live_turn_streaming() {
    let (chat, _) = fold(&Conversation::default(), streamed("t2", "two", "half"));
    let (chat, _) = chat.prepended(&[said_turn("t0", "zero", "reply")], None);
    let (chat, _) = chat.fold(&delta("t2", "p1", " and half"));
    assert_eq!(texts(&chat, "t2")[1], "half and half");
    assert_eq!(ids(&chat), vec!["t0", "t2"]);
}

#[test]
fn an_older_page_takes_the_next_cursor() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t2", "two", "reply")],
        None,
        Some("c1"),
    ));
    let (chat, _) = chat.prepended(&[said_turn("t1", "one", "reply")], Some("c2".to_owned()));
    assert_eq!(chat.older().map(String::as_str), Some("c2"));
}

#[test]
fn a_last_older_page_clears_the_cursor() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t2", "two", "reply")],
        None,
        Some("c1"),
    ));
    let (chat, _) = chat.prepended(&[said_turn("t1", "one", "reply")], None);
    assert!(chat.older().is_none(), "no more history behind it");
}

#[test]
fn a_turns_loaded_action_prepends_like_a_page() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t2", "two", "reply two")],
        None,
        Some("c1"),
    ));
    let (chat, change) = chat.fold(&StateAction::ChatTurnsLoaded(ChatTurnsLoadedAction {
        turns: vec![said_turn("t1", "one", "reply one")],
        turns_next_cursor: None,
    }));
    assert_eq!(ids(&chat), vec!["t1", "t2"]);
    assert_eq!(change, Change::Page);
}

// ======================================================================
// G. Truncation — the host dropped history.

#[test]
fn a_truncate_at_a_turn_keeps_it_and_drops_the_rest() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![
            said_turn("t1", "one", "reply one"),
            said_turn("t2", "two", "reply two"),
            said_turn("t3", "three", "reply three"),
        ],
        None,
        None,
    ));
    let (chat, _) = chat.fold(&truncated(Some("t2")));
    assert_eq!(ids(&chat), vec!["t1", "t2"]);
}

#[test]
fn a_bare_truncate_empties_the_conversation() {
    let (chat, _) = Conversation::default().landed(&snapshot(
        vec![said_turn("t1", "one", "reply")],
        None,
        None,
    ));
    let (chat, _) = chat.fold(&truncated(None));
    assert!(chat.is_empty());
    assert!(chat.live().is_none());
}

#[test]
fn a_truncate_closes_the_stream() {
    let (chat, _) = fold(&Conversation::default(), streamed("t1", "hi", "reply"));
    let (chat, _) = chat.fold(&truncated(None));
    assert!(chat.live().is_none());
}

// ======================================================================
// H. The shape itself.

#[test]
fn an_empty_conversation_says_nothing() {
    let chat = Conversation::default();
    assert!(chat.is_empty());
    assert_eq!(chat.len(), 0);
    assert!(chat.tail().is_none());
    assert!(chat.live().is_none());
}

#[test]
fn the_tail_is_the_last_turn_said() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "one"), complete("t1"), started("t2", "two")],
    );
    assert_eq!(chat.tail().map(|turn| turn.id.as_str()), Some("t2"));
}

#[test]
fn the_length_counts_turns_once() {
    let (chat, _) = fold(
        &Conversation::default(),
        vec![started("t1", "one"), started("t1", "one")],
    );
    assert_eq!(chat.len(), 1);
}

#[test]
fn an_unknown_action_is_nothing() {
    let (chat, _) = fold(&Conversation::default(), vec![started("t1", "hi")]);
    let (_, change) = chat.fold(&StateAction::ChatActivityChanged(
        ahp_types::actions::ChatActivityChangedAction { activity: None },
    ));
    assert_eq!(change, Change::Nothing);
}

#[test]
fn folding_never_mutates_what_it_folded() {
    let (before, _) = fold(&Conversation::default(), streamed("t1", "hi", "reply"));
    let snapshot_of_before = all(&before);
    let (_after, _) = before.fold(&delta("t1", "p1", " more"));
    assert_eq!(
        all(&before),
        snapshot_of_before,
        "the conversation folded FROM is untouched"
    );
}
