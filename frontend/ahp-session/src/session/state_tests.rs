// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The catalog's lifetime semantics over THE one store: records are
//! reached by their own session, disposal retracts everything a
//! session's row names, and the batch-tail sweep collects all-empty
//! sessions — unless a live window holds them.

use imba::store::Store;

use ahp_chat::chats::Chats;
use super::state::{Hosts, WindowGrip};
use ahp_wire::client::{ChatUri, HostId, SessionUri};

fn session(uri: &str) -> ahp_wire::SessionId {
    ahp_wire::SessionId {
        host: HostId::LOCAL,
        session: SessionUri::new(uri),
    }
}

fn store() -> Store {
    let mut store = Store::new();
    store.put(Hosts::default());
    store
}

/// Mint a chat into ITS session's collection — the mint door is the
/// one catalog consult; the record carries the collection id after.
fn put(store: &mut Store, session: &ahp_wire::SessionId, chat: &str) -> ChatUri {
    let uri = ChatUri::new(chat);
    let chats = Hosts::ensure_state(store, session).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        store,
        ::editor::test_document::test_ui(),
        session.host,
        session.session.clone(),
        chats,
        uri.clone(),
        Chats::catalog(store, chats).unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    Chats::put(store, chats, uri.clone(), panel);
    uri
}

/// The cold read a test takes: session → collection → record.
fn chat_in(store: &Store, session: &ahp_wire::SessionId, uri: &ChatUri) -> bool {
    Hosts::state(store, session)
        .and_then(|state| Chats::chat_ref(store, state.chats(), uri))
        .is_some()
}

fn list_in(store: &Store, session: &ahp_wire::SessionId) -> Vec<ChatUri> {
    Hosts::state(store, session)
        .map(|state| Chats::list(store, state.chats()))
        .unwrap_or_default()
}

/// A chat is reached by its OWN session, whatever else a batch
/// touches — the pane road, whenever the window is not on the chat's
/// session, still finds the one record.
#[test]
fn a_chat_is_reached_by_its_own_session() {
    let mut store = store();
    let home = session("s-a");

    let uri = put(&mut store, &home, "chat:1");
    put(&mut store, &session("s-b"), "chat:other");

    assert!(
        chat_in(&store, &home, &uri),
        "the record is found by its own address"
    );
}

#[test]
fn a_sessions_chats_are_its_own() {
    let mut store = store();
    let a = session("s-a");
    let b = session("s-b");

    let mine = put(&mut store, &a, "chat:a");
    let theirs = put(&mut store, &b, "chat:b");

    assert_eq!(list_in(&store, &a), vec![mine]);
    assert_eq!(list_in(&store, &b), vec![theirs]);
}

/// The batch-tail sweep collects a session whose every collection
/// EMPTIED — and spares one a live window holds.
#[test]
fn the_sweep_collects_all_empty_sessions_unless_a_window_holds_them() {
    let mut store = store();
    let empty = session("s-empty");
    let held = session("s-held");

    // Both held content, so both are subject to the sweep: a row is
    // garbage when what it held goes away.
    put(&mut store, &empty, "chat:gone");
    put(&mut store, &held, "chat:gone");
    Hosts::sweep_empty(&mut store);
    for scope in [&empty, &held] {
        let chats = Hosts::ensure_state(&mut store, scope).chats();
        store.retract(chats);
    }

    let kept = held.clone();
    store.put(WindowGrip(std::sync::Arc::new(move |_, scope| {
        *scope == kept
    })));

    Hosts::sweep_empty(&mut store);

    assert!(
        Hosts::state(&store, &empty).is_none(),
        "the all-empty session left the catalog"
    );
    assert!(
        Hosts::state(&store, &held).is_some(),
        "a held session is not garbage, however empty"
    );
}

/// A NEWBORN row is empty because nothing landed in it yet, not
/// because its content went away — and the sweep runs between the
/// mint and the fill (every guard drop, every window transaction,
/// every batch tail). Collecting it there would pull the ids out from
/// under the caller mid-setup: the mint door hands back a row whose
/// collections are already retracted, and the fill lands in a
/// collection nothing can reach.
#[test]
fn the_sweep_spares_a_row_that_has_not_been_filled_yet() {
    let mut store = store();
    let fresh = session("s-fresh");

    let chats = Hosts::ensure_state(&mut store, &fresh).chats();
    Hosts::sweep_empty(&mut store);

    assert!(
        Hosts::state(&store, &fresh).is_some(),
        "the newborn row survives the sweep that lands before its content"
    );

    let uri = put(&mut store, &fresh, "chat:1");
    assert_eq!(
        Hosts::ensure_state(&mut store, &fresh).chats(),
        chats,
        "and it is the SAME row — the ids the caller took still name it"
    );
    Hosts::sweep_empty(&mut store);
    assert!(chat_in(&store, &fresh, &uri), "the fill stuck");
}

/// The session is the LIFETIME of its chats.
#[test]
fn letting_a_session_go_takes_its_chats() {
    let mut store = store();
    let home = session("s-a");

    let uri = put(&mut store, &home, "chat:1");
    Hosts::dispose_state(&mut store, &home);

    assert!(!chat_in(&store, &home, &uri));
    assert!(list_in(&store, &home).is_empty());
}

/// Disposal is the whole ceremony: the session row leaves `Hosts`
/// and EVERY entity row its ids named retracts from the table —
/// nothing session-scoped can outlive its session
/// (docs/entities.md step 2).
#[test]
fn disposal_retracts_every_session_entity() {
    let mut store = store();
    let home = session("s-a");

    put(&mut store, &home, "chat:1");
    let row = Hosts::ensure_state(&mut store, &home);
    store.update_entity(
        row.recents,
        |_recents: &mut ahp_chat::recents::RecentLocations| {},
    );
    store.update_entity(row.trees, |_trees| {});
    store.update_entity(row.terminals, |_terminals| {});

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

    // And the sweep resurrects nothing.
    Hosts::sweep_empty(&mut store);
    assert!(Hosts::state(&store, &home).is_none());
}
