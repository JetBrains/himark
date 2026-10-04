// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The panel over the model: the send road, the mounts, the pages, and
//! the furniture beside the conversation. The fold itself is pinned in
//! `model_tests`; what is tested here is that a MOUNT shows what the
//! model holds, and that the roads a user takes move the model.

use std::sync::Arc;

use ahp_types::actions::{
    ChatDeltaAction, ChatErrorAction, ChatPendingMessageRemovedAction, ChatPendingMessageSetAction,
    ChatResponsePartAction, ChatToolCallCompleteAction, ChatToolCallConfirmedAction,
    ChatToolCallReadyAction, ChatToolCallStartAction, ChatTurnCancelledAction,
    ChatTurnCompleteAction, ChatTurnStartedAction,
};
use ahp_types::common::StringOrMarkdown;
use ahp_types::state::{
    ChatState, ErrorInfo, ErrorResponsePart, MarkdownResponsePart, Message, MessageKind,
    MessageOrigin, PendingMessageKind, ResponsePart, ToolCallResult, Turn as WireTurn, TurnState,
};

use super::*;

// ----------------------------------------------------------------------
// The harness.

fn ui() -> &'static UiCtx {
    ::editor::test_document::test_ui()
}

fn panel(store: &mut Store, chat: &str) -> ChatPanel {
    let mut host = crate::higent::HostId::LOCAL;
    store.update::<crate::higent::Servers>(|servers| {
        host = servers.mint(crate::higent::client::inert());
    });
    // The collection the panel files into — minted here, the way the
    // session ceremony does it; the panel never asks who owns it.
    let chats = imba::store::Id::<crate::higent::Chats>::mint();
    store.put_entity(chats, crate::higent::Chats::wired(imba::store::Id::mint()));
    let mut panel = ChatPanel::new(store, ui(), host, "s", chats, chat);
    panel.state = Link::Ready;
    panel
}

fn mount(panel: &mut ChatPanel, store: &mut Store) -> ChatViewId {
    let id = ChatViewId::mint();
    let mut batch = imba::effect::Batch::new();
    panel.ensure_view(store, ui(), id, &mut batch.effects());
    id
}

fn feed(panel: &mut ChatPanel, store: &mut Store, actions: Vec<StateAction>) {
    let mut batch = imba::effect::Batch::new();
    panel.apply_actions(store, ui(), actions, &mut batch.effects());
}

fn send(panel: &mut ChatPanel, store: &mut Store, text: &str) -> bool {
    let mut batch = imba::effect::Batch::new();
    panel.send_text(
        store,
        ui(),
        text.to_owned(),
        None,
        None,
        &mut batch.effects(),
    )
}

fn submit(panel: &mut ChatPanel, store: &mut Store, view: ChatViewId, text: &str) {
    let mut batch = imba::effect::Batch::new();
    panel.submit(
        store,
        ui(),
        view,
        text.to_owned(),
        None,
        None,
        &mut batch.effects(),
    );
}

fn land(panel: &mut ChatPanel, store: &mut Store, state: ChatState) {
    let mut batch = imba::effect::Batch::new();
    panel.apply_snapshot(store, ui(), Ok(state), &mut batch.effects());
}

fn land_older(panel: &mut ChatPanel, store: &mut Store, page: crate::higent::TurnsPage) {
    let mut batch = imba::effect::Batch::new();
    panel.apply_older(store, ui(), Ok(page), &mut batch.effects());
}

// ----------------------------------------------------------------------
// Oracles: the MODEL, and what a MOUNT shows.

fn model_ids(panel: &ChatPanel) -> Vec<String> {
    panel
        .conversation
        .turns()
        .map(|turn| turn.id.as_str().to_owned())
        .collect()
}

fn rows(panel: &ChatPanel, view: ChatViewId) -> Vec<(String, Vec<(String, String)>)> {
    ChatPanel::rows_oracle(panel.views.get(&view).expect("the view record"))
}

/// The TURN rows a mount holds, in order — the loader is chrome, and
/// `loader_armed` is what asks about it.
fn keys(panel: &ChatPanel, view: ChatViewId) -> Vec<String> {
    rows(panel, view)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| key != "…")
        .collect()
}

fn loader_shown(panel: &ChatPanel, view: ChatViewId) -> bool {
    rows(panel, view).first().is_some_and(|(key, _)| key == "…")
}

/// Every text a mount shows, in order.
fn shown(panel: &ChatPanel, view: ChatViewId) -> String {
    rows(panel, view)
        .into_iter()
        .flat_map(|(_, cells)| cells.into_iter().map(|(_, text)| text))
        .collect::<Vec<_>>()
        .join("|")
}

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

fn delta(turn: &str, part: &str, content: &str) -> StateAction {
    StateAction::ChatDelta(ChatDeltaAction {
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

fn tool_done(turn: &str, tool: &str, said: &str) -> StateAction {
    StateAction::ChatToolCallComplete(ChatToolCallCompleteAction {
        turn_id: turn.to_owned(),
        tool_call_id: tool.to_owned(),
        result: ToolCallResult {
            success: true,
            past_tense_message: StringOrMarkdown::Plain(said.to_owned()),
            content: None,
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

fn failed(turn: &str) -> StateAction {
    StateAction::ChatError(ChatErrorAction {
        turn_id: turn.to_owned(),
        duration: 1,
        part: ErrorResponsePart {
            error: ErrorInfo {
                error_type: "boom".to_owned(),
                message: "it broke".to_owned(),
                stack: None,
                meta: None,
            },
            resumable: None,
        },
        meta: None,
    })
}

fn queue_set(id: &str, text: &str) -> StateAction {
    StateAction::ChatPendingMessageSet(ChatPendingMessageSetAction {
        kind: PendingMessageKind::Queued,
        id: id.to_owned(),
        message: user(text),
    })
}

fn queue_removed(id: &str) -> StateAction {
    StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
        kind: PendingMessageKind::Queued,
        id: id.to_owned(),
    })
}

fn streamed(turn: &str, prompt: &str, reply: &str) -> Vec<StateAction> {
    vec![
        started(turn, prompt),
        markdown(turn, "p1", ""),
        delta(turn, "p1", reply),
    ]
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
    }
}

fn snapshot(chat: &str, turns: Vec<WireTurn>, cursor: Option<&str>) -> ChatState {
    ChatState {
        resource: chat.to_owned(),
        title: "a chat".to_owned(),
        status: 0,
        activity: None,
        modified_at: String::new(),
        origin: None,
        interactivity: None,
        working_directories: None,
        turns,
        turns_next_cursor: cursor.map(str::to_owned),
        active_turn: None,
        steering_message: None,
        queued_messages: None,
        draft: None,
        meta: None,
    }
}

// ======================================================================
// A. The send road: write-ahead, no placeholder.

#[test]
fn a_sent_message_is_on_screen_at_once() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a1");
    let view = mount(&mut panel, &mut store);

    assert!(send(&mut panel, &mut store, "my own message"));

    assert_eq!(model_ids(&panel).len(), 1, "the turn is ours already");
    assert!(
        shown(&panel, view).contains("my own message"),
        "and the mount shows it: {:?}",
        rows(&panel, view)
    );
}

#[test]
fn a_sent_message_is_the_turn_in_flight() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a2");
    send(&mut panel, &mut store, "mine");

    assert!(panel.conversation.is_running());
    assert!(panel.cancel_target().is_some(), "STOP has a turn to name");
}

#[test]
fn the_hosts_echo_of_our_send_shows_one_row() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a3");
    let view = mount(&mut panel, &mut store);
    send(&mut panel, &mut store, "mine");
    let turn = panel.conversation.live().expect("live").clone();

    // The host echoes the action we dispatched, verbatim.
    feed(&mut panel, &mut store, vec![started(turn.as_str(), "mine")]);

    assert_eq!(keys(&panel, view).len(), 1, "one row, not two");
    assert_eq!(shown(&panel, view), "mine");
}

#[test]
fn the_reply_to_a_sent_message_lands_under_it() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a4");
    let view = mount(&mut panel, &mut store);
    send(&mut panel, &mut store, "mine");
    let turn = panel.conversation.live().expect("live").as_str().to_owned();

    feed(
        &mut panel,
        &mut store,
        vec![markdown(&turn, "p1", "the "), delta(&turn, "p1", "answer")],
    );

    assert_eq!(shown(&panel, view), "mine|the answer");
}

#[test]
fn an_empty_message_is_not_sent() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a5");
    assert!(!send(&mut panel, &mut store, ""));
    assert!(panel.conversation.is_empty());
}

#[test]
fn a_message_before_the_link_is_ready_is_not_sent() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a6");
    panel.state = Link::Idle;
    assert!(!send(&mut panel, &mut store, "mine"));
    assert!(panel.conversation.is_empty());
}

#[test]
fn a_send_the_host_refused_says_so_in_place() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a7");
    let view = mount(&mut panel, &mut store);
    send(&mut panel, &mut store, "mine");
    let turn = panel.conversation.live().expect("live").clone();

    let mut batch = imba::effect::Batch::new();
    panel.apply_send_failed(
        &mut store,
        ui(),
        turn,
        "the host refused".to_owned(),
        &mut batch.effects(),
    );

    let shown = shown(&panel, view);
    assert!(shown.contains("mine"), "the words are kept: {shown}");
    assert!(
        shown.contains("the host refused"),
        "and the reason: {shown}"
    );
    assert!(!panel.conversation.is_running(), "the turn is over");
}

#[test]
fn a_refused_send_frees_the_composer() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a8");
    send(&mut panel, &mut store, "mine");
    let turn = panel.conversation.live().expect("live").clone();
    let mut batch = imba::effect::Batch::new();
    panel.apply_send_failed(
        &mut store,
        ui(),
        turn,
        "nope".to_owned(),
        &mut batch.effects(),
    );

    assert!(send(&mut panel, &mut store, "again"));
    assert!(panel.steering.is_none(), "a real send, not a steer");
    assert_eq!(model_ids(&panel).len(), 2);
}

#[test]
fn every_send_mints_its_own_turn() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:a9");
    send(&mut panel, &mut store, "one");
    let first = panel.conversation.live().expect("live").clone();
    feed(&mut panel, &mut store, vec![complete(first.as_str())]);
    send(&mut panel, &mut store, "two");
    let second = panel.conversation.live().expect("live").clone();

    assert_ne!(first, second, "two sends are two turns");
    assert_eq!(model_ids(&panel).len(), 2);
}

// ======================================================================
// B. The composer road: a view-scoped command that moves the model.

#[test]
fn a_submitted_message_is_on_screen_at_once() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:b1");
    let view = mount(&mut panel, &mut store);

    submit(&mut panel, &mut store, view, "typed");

    assert!(
        shown(&panel, view).contains("typed"),
        "the view that sent it shows it: {:?}",
        rows(&panel, view)
    );
}

#[test]
fn a_submitted_message_reaches_every_mount() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:b2");
    let sender = mount(&mut panel, &mut store);
    let watcher = mount(&mut panel, &mut store);

    submit(&mut panel, &mut store, sender, "typed");

    assert_eq!(shown(&panel, watcher), shown(&panel, sender));
    assert!(shown(&panel, watcher).contains("typed"));
}

#[test]
fn a_submitted_message_survives_the_reply() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:b3");
    let view = mount(&mut panel, &mut store);
    submit(&mut panel, &mut store, view, "typed");
    let turn = panel.conversation.live().expect("live").as_str().to_owned();

    feed(
        &mut panel,
        &mut store,
        vec![markdown(&turn, "p1", "answered")],
    );

    assert_eq!(shown(&panel, view), "typed|answered");
    assert_eq!(keys(&panel, view).len(), 1);
}

#[test]
fn the_stop_button_reaches_the_model() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:b4");
    let view = mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));
    panel.steering = Some("a standing steer".to_owned());

    let mut batch = imba::effect::Batch::new();
    panel.perform_model(
        &mut store,
        ui(),
        ChatPanelCommand::InView(
            view,
            Box::new(ChatPanelCommand::Composer(ComposerCommand::Stop)),
        ),
        &mut batch.effects(),
    );

    assert!(
        panel.steering.is_none(),
        "the click reached stop(), not the composer's empty arm"
    );
}

// ======================================================================
// C. Steering: a prompt typed at a running agent.

#[test]
fn a_prompt_at_a_running_turn_steers() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:c1");
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));

    assert!(send(&mut panel, &mut store, "instead, this"));
    assert_eq!(panel.steering.as_deref(), Some("instead, this"));
    assert_eq!(model_ids(&panel), vec!["t1"], "no second turn yet");
}

#[test]
fn a_steer_fires_when_the_turn_is_cancelled() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:c2");
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));
    send(&mut panel, &mut store, "instead, this");

    feed(&mut panel, &mut store, vec![cancelled("t1")]);

    assert!(panel.steering.is_none());
    assert_eq!(model_ids(&panel).len(), 2, "the steer became a turn");
    assert!(panel.conversation.is_running());
}

#[test]
fn a_steer_fires_when_the_turn_completes() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:c3");
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));
    send(&mut panel, &mut store, "next please");

    feed(&mut panel, &mut store, vec![complete("t1")]);

    assert!(panel.steering.is_none());
    assert_eq!(model_ids(&panel).len(), 2);
}

#[test]
fn a_steer_fires_when_the_turn_fails() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:c4");
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));
    send(&mut panel, &mut store, "try again");

    feed(&mut panel, &mut store, vec![failed("t1")]);

    assert_eq!(model_ids(&panel).len(), 2);
}

#[test]
fn a_steer_drops_the_standing_queue() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:c5");
    feed(&mut panel, &mut store, streamed("t1", "one", "reply"));
    feed(&mut panel, &mut store, vec![queue_set("q1", "queued")]);
    assert_eq!(panel.queue_oracle().len(), 1);

    send(&mut panel, &mut store, "steer");

    assert!(panel.queue_oracle().is_empty());
}

// ======================================================================
// D. Mounts: one conversation, many views.

#[test]
fn a_fresh_mount_rebuilds_the_conversation() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d1");
    feed(&mut panel, &mut store, streamed("t1", "hi", "the reply"));

    let view = mount(&mut panel, &mut store);

    assert_eq!(keys(&panel, view), vec!["t1"]);
    assert_eq!(shown(&panel, view), "hi|the reply");
}

#[test]
fn a_reply_streamed_with_no_mount_open_is_not_lost() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d2");
    feed(&mut panel, &mut store, streamed("t1", "hi", "the reply"));
    feed(&mut panel, &mut store, vec![complete("t1")]);

    let view = mount(&mut panel, &mut store);
    assert_eq!(shown(&panel, view), "hi|the reply");
}

#[test]
fn a_mutation_reaches_every_mount_identically() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d3");
    let a = mount(&mut panel, &mut store);
    let b = mount(&mut panel, &mut store);

    feed(&mut panel, &mut store, streamed("t1", "hi", "the reply"));

    assert_eq!(rows(&panel, a), rows(&panel, b));
    assert_eq!(shown(&panel, a), "hi|the reply");
}

#[test]
fn a_mount_minted_mid_stream_catches_up() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d4");
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), markdown("t1", "p1", "half")],
    );

    let late = mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, vec![delta("t1", "p1", " and half")]);

    assert_eq!(shown(&panel, late), "hi|half and half");
}

#[test]
fn a_dropped_mount_leaves_the_conversation_whole() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d5");
    let view = mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, streamed("t1", "hi", "the reply"));
    panel.views.remove_mut(&view);

    let reopened = mount(&mut panel, &mut store);
    assert_eq!(shown(&panel, reopened), "hi|the reply");
}

#[test]
fn every_mount_holds_the_same_transcript() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d6");
    mount(&mut panel, &mut store);
    mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, streamed("t1", "hi", "reply"));
    feed(&mut panel, &mut store, vec![tool_start("t1", "c1", "Bash")]);

    let all = panel.view_transcripts();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], all[1]);
}

#[test]
fn a_delta_grows_the_cell_it_belongs_to() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d7");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![
            started("t1", "hi"),
            markdown("t1", "p1", "one"),
            markdown("t1", "p2", "two"),
        ],
    );

    feed(&mut panel, &mut store, vec![delta("t1", "p1", "!")]);

    assert_eq!(
        shown(&panel, view),
        "hi|one!|two",
        "the delta grew ITS cell, not its neighbour"
    );
}

#[test]
fn a_tool_face_settles_in_the_mount() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d8");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    assert!(shown(&panel, view).contains("preparing"));

    feed(
        &mut panel,
        &mut store,
        vec![tool_done("t1", "c1", "ran it")],
    );

    assert!(
        shown(&panel, view).contains("ran it"),
        "the settled face reached the view: {:?}",
        rows(&panel, view)
    );
}

#[test]
fn opening_a_long_chat_builds_only_the_tail() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:d9");
    let page: Vec<WireTurn> = (0..40)
        .map(|at| said_turn(&format!("t{at}"), &format!("prompt {at}"), "reply"))
        .collect();
    land(&mut panel, &mut store, snapshot("chat:d9", page, None));

    let view = mount(&mut panel, &mut store);
    let laid = panel.views.get(&view).expect("the view");
    let sleeping = laid
        .rows
        .content()
        .rows()
        .filter(|row| matches!(row, ChatRow::Sleeping(_)))
        .count();
    assert_eq!(sleeping, 40 - EAGER_TAIL, "only the tail built its editors");
}

// ======================================================================
// E. Pages through the panel.

#[test]
fn a_snapshot_lands_its_page_in_the_mount() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e1");
    let view = mount(&mut panel, &mut store);

    land(
        &mut panel,
        &mut store,
        snapshot(
            "chat:e1",
            vec![
                said_turn("t1", "one", "reply one"),
                said_turn("t2", "two", "reply two"),
            ],
            None,
        ),
    );

    assert_eq!(keys(&panel, view), vec!["t1", "t2"]);
    assert_eq!(shown(&panel, view), "one|reply one|two|reply two");
}

#[test]
fn a_re_snapshot_keeps_the_history_it_does_not_name() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e2");
    let view = mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, streamed("t1", "one", "reply one"));
    feed(&mut panel, &mut store, vec![complete("t1")]);

    land(
        &mut panel,
        &mut store,
        snapshot(
            "chat:e2",
            vec![said_turn("t2", "two", "reply two")],
            Some("c1"),
        ),
    );

    assert_eq!(keys(&panel, view), vec!["t1", "t2"]);
}

/// A standing cursor means older turns exist: the mount arms the
/// loader at the top, and that is what `fetchTurns` hangs off.
#[test]
fn a_standing_cursor_arms_the_loader() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e8");
    let view = mount(&mut panel, &mut store);
    land(
        &mut panel,
        &mut store,
        snapshot("chat:e8", vec![said_turn("t2", "two", "reply")], Some("c1")),
    );
    assert!(loader_shown(&panel, view), "the loader stands at the top");
}

#[test]
fn a_page_with_no_cursor_shows_no_loader() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e9");
    let view = mount(&mut panel, &mut store);
    land(
        &mut panel,
        &mut store,
        snapshot("chat:e9", vec![said_turn("t1", "one", "reply")], None),
    );
    assert!(!loader_shown(&panel, view), "nothing older to fetch");
}

#[test]
fn a_snapshot_turns_the_link_ready() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e3");
    panel.state = Link::Idle;
    land(
        &mut panel,
        &mut store,
        snapshot("chat:e3", Vec::new(), None),
    );
    assert!(panel.ready());
}

#[test]
fn a_snapshot_takes_the_title() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e4");
    land(
        &mut panel,
        &mut store,
        snapshot("chat:e4", Vec::new(), None),
    );
    assert_eq!(panel.title, "a chat");
}

#[test]
fn an_older_page_lands_above_what_the_mount_holds() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e5");
    let view = mount(&mut panel, &mut store);
    land(
        &mut panel,
        &mut store,
        snapshot(
            "chat:e5",
            vec![said_turn("t2", "two", "reply two")],
            Some("c1"),
        ),
    );

    land_older(
        &mut panel,
        &mut store,
        crate::higent::TurnsPage {
            turns: vec![said_turn("t1", "one", "reply one")],
            next_cursor: None,
        },
    );

    assert_eq!(keys(&panel, view), vec!["t1", "t2"]);
}

/// THE MIDDLE-OF-THE-CHAT BUG at the mount: an overlapping page must
/// not put a second row under the same turn id.
#[test]
fn an_overlapping_older_page_shows_each_turn_once() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e6");
    let view = mount(&mut panel, &mut store);
    land(
        &mut panel,
        &mut store,
        snapshot(
            "chat:e6",
            vec![
                said_turn("t1", "one", "reply one"),
                said_turn("t2", "two", "reply two"),
            ],
            Some("c1"),
        ),
    );

    land_older(
        &mut panel,
        &mut store,
        crate::higent::TurnsPage {
            turns: vec![
                said_turn("t0", "zero", "reply zero"),
                said_turn("t1", "one", "reply one"),
            ],
            next_cursor: None,
        },
    );

    assert_eq!(keys(&panel, view), vec!["t0", "t1", "t2"]);
    assert_eq!(
        shown(&panel, view),
        "zero|reply zero|one|reply one|two|reply two"
    );
}

#[test]
fn a_delta_after_an_older_page_still_finds_its_turn() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:e7");
    let view = mount(&mut panel, &mut store);
    feed(&mut panel, &mut store, streamed("t2", "two", "half"));

    land_older(
        &mut panel,
        &mut store,
        crate::higent::TurnsPage {
            turns: vec![said_turn("t0", "zero", "reply zero")],
            next_cursor: None,
        },
    );
    feed(&mut panel, &mut store, vec![delta("t2", "p1", " and half")]);

    assert_eq!(shown(&panel, view), "zero|reply zero|two|half and half");
}

// ======================================================================
// F. The furniture beside the conversation.

#[test]
fn a_ready_tool_raises_the_ask() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f1");
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_ready("t1", "c1", "rm -rf /", false)],
    );

    assert_eq!(
        panel.stack.ask_turn().map(|turn| turn.as_str().to_owned()),
        Some("t1".to_owned())
    );
}

#[test]
fn an_auto_confirmed_tool_raises_no_ask() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f2");
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_ready("t1", "c1", "ls", true)],
    );

    assert!(panel.stack.ask_turn().is_none());
}

#[test]
fn an_answered_ask_clears() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f3");
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_ready("t1", "c1", "ls", false)],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_confirmed("t1", "c1", true)],
    );

    assert!(panel.stack.ask_turn().is_none());
}

#[test]
fn the_ask_clears_when_the_turn_ends() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f4");
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_ready("t1", "c1", "ls", false)],
    );
    assert!(panel.stack.ask_turn().is_some());

    feed(&mut panel, &mut store, vec![cancelled("t1")]);

    assert!(panel.stack.ask_turn().is_none());
    assert!(panel.cancel_target().is_none(), "nothing left to stop");
}

#[test]
fn a_queued_message_joins_and_leaves_the_queue() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f5");
    feed(&mut panel, &mut store, vec![queue_set("q1", "later")]);
    assert_eq!(panel.queue_oracle().len(), 1);

    feed(&mut panel, &mut store, vec![queue_removed("q1")]);
    assert!(panel.queue_oracle().is_empty());
}

#[test]
fn a_queued_message_is_no_turn() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f6");
    feed(&mut panel, &mut store, vec![queue_set("q1", "later")]);
    assert!(panel.conversation.is_empty());
}

#[test]
fn a_replayed_batch_leaves_one_row_and_one_copy() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:f7");
    let view = mount(&mut panel, &mut store);
    let batch = streamed("t1", "hi", "the reply");
    feed(&mut panel, &mut store, batch.clone());
    feed(&mut panel, &mut store, batch);

    assert_eq!(keys(&panel, view), vec!["t1"]);
    assert_eq!(shown(&panel, view), "hi|the reply");
}

// ======================================================================
// G. Tool runs: consecutive calls collapse into ONE cell
// (docs/ahp/agents.md, "Tool runs collapse").

/// A completion carrying a file edit — the same uri before and after
/// keeps the helper short.
fn tool_done_with_edit(turn: &str, tool: &str, path: &str, uri: &str) -> StateAction {
    let content = vec![ahp_types::state::ToolResultContent::FileEdit(
        crate::higent::FileEditRefs {
            before: Some(crate::higent::snapshot(path, uri)),
            after: Some(crate::higent::snapshot(path, uri)),
            counts: crate::higent::DiffCounts::default(),
        }
        .to_content(),
    )];
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

#[test]
fn consecutive_tool_calls_collapse_into_one_run() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:g1");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            tool_start("t1", "c2", "Bash"),
        ],
    );

    let (_, cells) = rows(&panel, view).into_iter().next().expect("the turn");
    assert_eq!(cells.len(), 2, "a prompt and ONE run: {cells:?}");
    assert!(
        cells[1].1.contains("2 × Bash"),
        "the run counts its calls: {}",
        cells[1].1
    );
}

#[test]
fn a_reply_between_calls_closes_the_run() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:g2");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Bash"),
            markdown("t1", "p1", "between"),
            tool_start("t1", "c2", "Bash"),
        ],
    );

    let (_, cells) = rows(&panel, view).into_iter().next().expect("the turn");
    let kinds: Vec<&str> = cells.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["User", "Tool", "Agent", "Tool"],
        "two runs, the reply between: {cells:?}"
    );
}

#[test]
fn a_late_call_joins_the_standing_run_cell() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:g3");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![started("t1", "hi"), tool_start("t1", "c1", "Bash")],
    );
    feed(
        &mut panel,
        &mut store,
        vec![tool_done("t1", "c1", "ran it")],
    );
    feed(&mut panel, &mut store, vec![tool_start("t1", "c2", "Grep")]);

    let (_, cells) = rows(&panel, view).into_iter().next().expect("the turn");
    assert_eq!(cells.len(), 2, "the second call joined the run: {cells:?}");
    assert!(
        cells[1].1.contains("1 × Bash, 1 × Grep"),
        "the group row counts both: {}",
        cells[1].1
    );

    // A mount built AFTER the stream dresses from the model alone —
    // it must agree with the mount the ops grew.
    mount(&mut panel, &mut store);
    let all = panel.view_transcripts();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], all[1], "the grown mount and the fresh one agree");
}

#[test]
fn an_edit_between_calls_closes_the_run() {
    let mut store = Store::new();
    let mut panel = panel(&mut store, "chat:g4");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Edit"),
            tool_done_with_edit("t1", "c1", "src/a.rs", "ahp-content:/a1"),
            tool_start("t1", "c2", "Bash"),
        ],
    );

    let (_, cells) = rows(&panel, view).into_iter().next().expect("the turn");
    let kinds: Vec<&str> = cells.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["User", "Tool", "Tool", "Tool"],
        "run, diff, run: {cells:?}"
    );
    assert!(
        cells[2].1.contains("diff a.rs"),
        "the edit stands between the runs: {cells:?}"
    );

    // The diff's header carries the WORKING COPY's uri — what OPEN
    // navigates to.
    let turn = panel
        .conversation
        .turn(&crate::higent::TurnId::new("t1"))
        .expect("the turn");
    let header_uri = crate::higent::turn::dress(turn)
        .iter()
        .find_map(|(_, spec)| match spec {
            CellSpec::Diff(spec) => Some(spec.header.uri.clone()),
            _ => None,
        })
        .expect("a diff cell");
    assert_eq!(header_uri.as_deref(), Some("src/a.rs"));
}

#[test]
fn the_diff_headers_open_reaches_the_app() {
    let mut store = Store::new();
    let opened: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
    {
        let noted = opened.clone();
        crate::higent::OpenEditedRoad::install(
            &mut store,
            crate::higent::OpenEditedRoad(std::sync::Arc::new(
                move |_store, _server, _session, uri| {
                    *noted.lock().unwrap() = Some(uri);
                },
            )),
        );
    }
    let mut panel = panel(&mut store, "chat:g5");
    let view = mount(&mut panel, &mut store);
    feed(
        &mut panel,
        &mut store,
        vec![
            started("t1", "hi"),
            tool_start("t1", "c1", "Edit"),
            tool_done_with_edit("t1", "c1", "src/a.rs", "ahp-content:/a1"),
        ],
    );

    // The header's OPEN click, as the rows road delivers it: the diff
    // is the turn's third cell (prompt, run, diff).
    let mut batch = imba::effect::Batch::new();
    panel.perform_in_view(
        &mut store,
        ui(),
        view,
        ChatPanelCommand::Rows(ScrollCommand::Content(ListCommand::Child(
            0,
            RowCommand::Turn(ListCommand::Child(
                2,
                CellCommand::OpenFile("src/a.rs".to_owned()),
            )),
        ))),
        &mut batch.effects(),
    );

    assert_eq!(
        opened.lock().unwrap().as_deref(),
        Some("src/a.rs"),
        "the open ask reached the installed shell road"
    );
}
