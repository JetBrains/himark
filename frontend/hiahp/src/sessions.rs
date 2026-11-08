// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Which SESSION each mounted bridge belongs to — the windowless
//! twin of "the window's session" — plus the files-tree follow
//! targets. Store residents the arms write and the binder reads.

use imba::dyn_view::DynCommand;
use imba::store::Store;

use himark::app::AppCommand;

use himark::bridge::ViewBridgeId;

/// Which SESSION each mounted bridge belongs to — the windowless
/// twin of "the window's session": the navigate resolver reads it
/// to bind a mounted origin's opens to the right collections.
#[derive(Clone, Default)]
pub struct MountSessions(pub rpds::HashTrieMapSync<u64, ahp_wire::SessionId>);

pub fn remember_session(store: &mut Store, id: ViewBridgeId, session: ahp_wire::SessionId) {
    let mut map = store.get::<MountSessions>().cloned().unwrap_or_default();
    map.0.insert_mut(id.raw(), session);
    store.put(map);
}

pub fn session_of(store: &Store, id: ViewBridgeId) -> Option<ahp_wire::SessionId> {
    store.get::<MountSessions>()?.0.get(&id.raw()).cloned()
}

/// A mounted origin SHOWING the session — the folder picker's
/// navigation anchor when no window holds it.
pub fn origin_of(store: &Store, session: &ahp_wire::SessionId) -> Option<ViewBridgeId> {
    store
        .get::<MountSessions>()?
        .0
        .iter()
        .find_map(|(raw, held)| (held == session).then(|| ViewBridgeId::from_raw(*raw)))
}

/// The session's mounted FILES TREES — the explicit follow targets
/// `himark_tree_follow` commands when the host lands a navigation —
/// and the STANDING follow: the active editor's location, kept so a
/// tree that mounts LATER (the rail was closed, another tab showed)
/// stands on it at birth. Fire-and-forget lost every follow that
/// arrived while no tree was live.
#[derive(Clone, Default)]
pub struct FollowTargets {
    trees: rpds::HashTrieMapSync<ahp_wire::SessionId, ViewBridgeId>,
    standing: rpds::HashTrieMapSync<ahp_wire::SessionId, editor::location::ResourceLocation>,
    generation: u64,
}

/// Register the mounted tree and answer the standing follow it
/// should replay (with a fresh generation), if the session has one.
pub fn remember_files_tree(
    store: &mut Store,
    session: ahp_wire::SessionId,
    id: ViewBridgeId,
) -> Option<(editor::location::ResourceLocation, u64)> {
    let mut map = store.get::<FollowTargets>().cloned().unwrap_or_default();
    map.trees.insert_mut(session.clone(), id);
    let standing = map.standing.get(&session).cloned();
    if standing.is_some() {
        map.generation += 1;
    }
    let generation = map.generation;
    store.put(map);
    standing.map(|location| (location, generation))
}

/// The EXPLICIT tree-follow: the host landed a navigation and
/// commands the session's files tree to stand on the location —
/// the same `TreeCommand::Follow` the workbench pushes from its
/// focus walk. One API, both shells (docs/mounted-parity.md).
pub fn follow_session_tree(
    app: &mut himark::app::Himark,
    session: &ahp_wire::SessionId,
    location: editor::location::ResourceLocation,
) -> bool {
    // The follow STANDS regardless of who is mounted: a tree that
    // arrives later replays it from `remember_files_tree`.
    let tree = app
        .store()
        .get::<FollowTargets>()
        .and_then(|map| map.trees.get(session).copied());
    let generation = {
        let session = session.clone();
        let location_kept = location.clone();
        let mut generation = 0;
        app.with_store(|store| {
            let mut map = store.get::<FollowTargets>().cloned().unwrap_or_default();
            map.standing.insert_mut(session, location_kept);
            map.generation += 1;
            generation = map.generation;
            store.put(map);
        });
        generation
    };
    let Some(tree) = tree else {
        // Parked as standing — applied at the next tree mount.
        return true;
    };
    app.perform_batch(vec![AppCommand::ToView(
        tree,
        DynCommand::new(filetree::TreeCommand::Follow {
            location,
            generation,
        }),
    )]);
    true
}

pub fn forget_session(store: &mut Store, id: ViewBridgeId) {
    let Some(mut map) = store.get::<MountSessions>().cloned() else {
        return;
    };
    map.0.remove_mut(&id.raw());
    store.put(map);
}
