// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The catalog's gather/scatter semantics — exercised with the
//! SHELL's AppState batches, so the tests live here while the
//! catalog lives in the protocol crate.

#![allow(unused_imports)]

use imba::store::Store;

use crate::higent::*;
use crate::higent::{ChatUri, Chats};

fn session(uri: &str) -> crate::SessionId {
    crate::SessionId {
        host: HostId::LOCAL,
        session: SessionUri::new(uri),
    }
}

/// Mint a chat into ITS session's collection — the mint door is the
/// one catalog consult; the record carries the collection id after.
fn put(store: &mut Store, session: &crate::SessionId, chat: &str) -> ChatUri {
    let uri = ChatUri::new(chat);
    let chats = Hosts::ensure_state(store, session).chats();
    let panel = crate::higent::chat::ChatPanel::new(
        store,
        ::editor::test_document::test_ui(),
        session.host,
        session.session.clone(),
        chats,
        uri.clone(),
    );
    Chats::put(store, chats, uri.clone(), panel);
    uri
}

/// The cold read a test takes: session → collection → record.
fn chat_in(store: &Store, session: &crate::SessionId, uri: &ChatUri) -> bool {
    Hosts::state(store, session)
        .and_then(|state| Chats::chat_ref(store, state.chats(), uri))
        .is_some()
}

fn list_in(store: &Store, session: &crate::SessionId) -> Vec<ChatUri> {
    Hosts::state(store, session)
        .map(|state| Chats::list(store, state.chats()))
        .unwrap_or_default()
}

/// A chat is reached by its OWN session, so a batch gathered for
/// ANOTHER session — the pane road, whenever the window is not on
/// the chat's session — still finds the one record.
#[test]
fn a_chat_is_reached_by_its_own_session() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let home = session("s-a");

    let mut store = state.gather(None, Some(&home), &clients);
    let uri = put(&mut store, &home, "chat:1");
    state.scatter(store, Some(&home));

    // The next batch is gathered for a DIFFERENT session.
    let store = state.gather(None, Some(&session("s-b")), &clients);
    assert!(
        chat_in(&store, &home, &uri),
        "the record is found by its own address"
    );
}

#[test]
fn a_chat_survives_a_scopeless_batch() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let home = session("s-a");

    let mut store = state.gather(None, Some(&home), &clients);
    let uri = put(&mut store, &home, "chat:2");
    state.scatter(store, Some(&home));

    // A batch with NO session scope: it writes other things, and
    // the chats must not be dragged out of their session with them.
    let mut store = state.gather(None, None, &clients);
    state.scatter(std::mem::replace(&mut store, Store::new()), None);

    let store = state.gather(None, Some(&home), &clients);
    assert!(chat_in(&store, &home, &uri));
}

#[test]
fn a_write_in_a_foreign_gather_lands_in_the_right_family() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let home = session("s-a");
    let elsewhere = session("s-b");

    // The batch is gathered for s-b; the chat belongs to s-a.
    let mut store = state.gather(None, Some(&elsewhere), &clients);
    let uri = put(&mut store, &home, "chat:3");
    state.scatter(store, Some(&home));

    let store = state.gather(None, Some(&home), &clients);
    assert!(
        chat_in(&store, &home, &uri),
        "filed by the panel's session, not the gather's"
    );
    assert!(!chat_in(&store, &elsewhere, &uri), "and nowhere else");
}

#[test]
fn a_sessions_chats_are_its_own() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let a = session("s-a");
    let b = session("s-b");

    let mut store = state.gather(None, Some(&a), &clients);
    let mine = put(&mut store, &a, "chat:a");
    let theirs = put(&mut store, &b, "chat:b");
    state.scatter(store, Some(&a));

    let store = state.gather(None, Some(&a), &clients);
    assert_eq!(list_in(&store, &a), vec![mine]);
    assert_eq!(list_in(&store, &b), vec![theirs]);
}

/// The session is the LIFETIME of its chats.
#[test]
fn letting_a_session_go_takes_its_chats() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let home = session("s-a");

    let mut store = state.gather(None, Some(&home), &clients);
    let uri = put(&mut store, &home, "chat:1");
    Hosts::dispose_state(&mut store, &home);
    state.scatter(store, Some(&home));

    let store = state.gather(None, Some(&home), &clients);
    assert!(!chat_in(&store, &home, &uri));
    assert!(list_in(&store, &home).is_empty());
}

/// Disposal is the whole ceremony: the session row leaves `Hosts`
/// and EVERY entity row its ids named retracts from the table —
/// nothing session-scoped can outlive its session
/// (docs/entities.md step 2).
#[test]
fn disposal_retracts_every_family_entity() {
    let mut state = crate::AppState::default();
    let clients = crate::higent::Servers::default();
    let home = session("s-a");

    let mut store = state.gather(None, Some(&home), &clients);
    put(&mut store, &home, "chat:1");
    let row = Hosts::ensure_state(&mut store, &home);
    store.update_entity(
        row.recents,
        |_recents: &mut crate::higent::RecentLocations| {},
    );
    store.update_entity(row.trees, |_trees| {});
    store.update_entity(row.terminals, |_terminals| {});
    state.scatter(store, Some(&home));

    let mut store = state.gather(None, Some(&home), &clients);
    assert!(Hosts::state(&store, &home).is_some(), "the row is live");
    Hosts::dispose_state(&mut store, &home);

    assert!(Hosts::state(&store, &home).is_none(), "the row is gone");
    assert!(store.entity(row.chats).is_none());
    assert!(store.entity(row.trees).is_none());
    assert!(store.entity(row.recents).is_none());
    assert!(store.entity(row.changes).is_none());
    assert!(store.entity(row.history).is_none());
    assert!(store.entity(row.comments).is_none());
    assert!(store.entity(row.terminals).is_none());
    assert!(store.entity(row.documents).is_none());
    assert!(store.entity(row.scratch_names).is_none());
    state.scatter(store, Some(&home));

    // And scatter resurrects nothing from the scaffolding.
    let store = state.gather(None, Some(&home), &clients);
    assert!(Hosts::state(&store, &home).is_none());
}
