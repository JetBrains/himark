mod common;
#[allow(unused_imports)]
use workbench_shell::capi::WindowEngine as _;
use common::{bind_backend, block_on};
// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_locations::find::NativeFindHandler;
use ahp_locations::FindEffect;
use ahp_wire::client as ahp;
use ahp_wire::fs::ClientDirectory;
use editor::location::ResourceLocation;
use editor::location::ResourceType;
use imba::effect::EffectHandler;
use std::sync::Arc;

fn wire_backend() -> (tempfile::TempDir, ahp_wire::client::Client) {
    let (dir, wire) = wire_backend_host();
    (dir, ahp_wire::client::Client::of(wire))
}

/// A real backend on a socket and a real `WireHost` dialed into it —
/// the production wire, nothing scripted.
pub(crate) fn wire_backend_host() -> (tempfile::TempDir, Arc<ahp_wire::wire::WireHost>) {
    let dir = tempfile::tempdir().expect("backend home");
    let url = bind_backend(dir.path(), "false".to_owned());
    let seat = Arc::new(ahp_wire::wire::WireHost::at(
        ahp_wire::wire::test_runtime(),
        himark_plugins::test_connector(),
        url,
    ));
    (dir, seat)
}

/// A real `agent_host::server::Host` on its own runtime and thread, bound on a
/// unix socket under `dir`; answers the url to dial.


fn located(authority: &str, path: &std::path::Path) -> ResourceLocation {
    let segments: Vec<String> = path
        .to_str()
        .expect("utf8 path")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    ResourceLocation::new(
        ResourceType::directory(),
        editor::location::Authority::new(authority),
        segments,
    )
}

fn find(
    directory: &Arc<ClientDirectory>,
    folders: Vec<ResourceLocation>,
    term: &str,
) -> Vec<ResourceLocation> {
    let handler = NativeFindHandler {
        directory: Arc::clone(directory),
    };
    let effect = FindEffect {
        folders,
        term: term.to_owned(),
    };
    block_on(Box::pin(async move { handler.handle(effect).await }))
}

#[test]
fn session_folders_ask_their_seat() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("files/src")).expect("mkdir");
    let files = dir.path().join("files").canonicalize().expect("canonical");
    std::fs::write(files.join("src/lib.rs"), "fn main() {}\n").expect("write");
    std::fs::write(files.join("README.md"), "conflation is delivery\n").expect("write");
    std::fs::write(dir.path().join("loose.md"), "conflation outside\n").expect("write");

    let directory = Arc::new(ClientDirectory::new(Arc::new(|_| {}), Arc::new(|_| {})));
    let encoded = ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &ahp_wire::client::SessionUri::new(host_discovery::LOCAL_FS_SESSION),
    );
    let (server, _) = ahp::parse(&encoded).expect("round-trips");
    directory.record(server, seat.clone());

    let base = located(&encoded, &files);
    let hits = find(&directory, vec![base.clone()], "readme");
    assert_eq!(hits.len(), 1, "only the in-folder hit: {hits:?}");
    assert_eq!(hits[0].authority(), base.authority(), "authority inherited");
    assert!(hits[0].kind().is_document());
    assert!(hits[0].path().join("/").ends_with("README.md"), "{hits:?}");
}

#[test]
fn local_folders_ask_the_designated_backend() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
    let files = dir.path().join("notes").canonicalize().expect("canonical");
    std::fs::write(files.join("a.md"), "conflation is delivery\n").expect("write");

    let directory = Arc::new(ClientDirectory::new(Arc::new(|_| {}), Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &ahp_wire::client::SessionUri::new(host_discovery::LOCAL_FS_SESSION),
    ))
    .expect("parses");
    directory.record(server, seat.clone());
    directory.set_local(server);

    let base = located("local", &files);
    let hits = find(&directory, vec![base.clone()], "a.md");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].authority().as_str(), "local", "local identity kept");
    assert!(hits[0].path().join("/").ends_with("notes/a.md"), "{hits:?}");

    let names = find(&directory, vec![base.clone()], "am");
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn symbol_locations_route_answers_and_resolves_honestly() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
    let files = dir.path().join("notes").canonicalize().expect("canonical");

    let directory = Arc::new(ClientDirectory::new(Arc::new(|_| {}), Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &ahp_wire::client::SessionUri::new(host_discovery::LOCAL_FS_SESSION),
    ))
    .expect("parses");
    directory.record(server, seat.clone());
    directory.set_local(server);

    let handler = ahp_locations::routes::RouteSymbolLocations {
        directory: Arc::clone(&directory),
    };
    let effect = ahp_locations::SymbolLocationsEffect {
        folders: vec![located("local", &files)],
        query: "Catalog".to_owned(),
        limit: 100,
    };
    let channel = block_on(Box::pin(async move { handler.handle(effect).await })).expect("channel");

    let mut state = {
        let seat = Arc::clone(&channel.client);
        let subscribed = channel.channel.clone();
        block_on(Box::pin(async move {
            seat.subscribe_locations(subscribed).await
        }))
        .expect("snapshot")
    };
    while !state.done {
        let seat = Arc::clone(&channel.client);
        let polled = channel.channel.clone();
        let batches = block_on(Box::pin(async move { seat.poll_locations(polled).await }));
        for batch in batches {
            state.concat(batch);
        }
    }
    // This harness host runs no FSP sidecar — the one engine with a
    // symbol vocabulary — so the lane resolves CUT OFF with nothing
    // (docs/ahp/ahp-locations.md §3.2): "no index", distinguishable
    // from "no such symbol".
    assert!(state.truncated, "{state:?}");
    assert!(state.locations.is_empty(), "{state:?}");
}

#[test]
fn search_locations_route_streams_and_cancels_over_the_wire() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
    let files = dir.path().join("notes").canonicalize().expect("canonical");
    std::fs::write(files.join("a.md"), "plain\nconflation is delivery\n").expect("write");
    std::fs::write(files.join("b.md"), "conflation twice, conflation\n").expect("write");

    let directory = Arc::new(ClientDirectory::new(Arc::new(|_| {}), Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &ahp_wire::client::SessionUri::new(host_discovery::LOCAL_FS_SESSION),
    ))
    .expect("parses");
    directory.record(server, seat.clone());
    directory.set_local(server);

    let handler = ahp_locations::routes::RouteSearchLocations {
        directory: Arc::clone(&directory),
    };
    let effect = ahp_locations::SearchLocationsEffect {
        folders: vec![located("local", &files)],
        query: "conflation".to_owned(),
        regex: false,
        case_sensitive: false,
        limit: 100,
    };
    let channel = block_on(Box::pin(async move { handler.handle(effect).await })).expect("channel");

    let mut state = {
        let seat = Arc::clone(&channel.client);
        let subscribed = channel.channel.clone();
        block_on(Box::pin(async move {
            seat.subscribe_locations(subscribed).await
        }))
        .expect("snapshot")
    };
    while !state.done {
        let seat = Arc::clone(&channel.client);
        let polled = channel.channel.clone();
        let batches = block_on(Box::pin(async move { seat.poll_locations(polled).await }));
        for batch in batches {
            state.concat(batch);
        }
    }
    assert!(!state.truncated, "{state:?}");
    assert_eq!(state.locations.len(), 3, "{state:?}");
    let resolved = (channel.resolve)(&state.locations[0].uri).expect("resolves");
    assert_eq!(resolved.authority().as_str(), "local", "identity kept");
    assert!(resolved.kind().is_document());
    let contexts: Vec<&str> = state
        .locations
        .iter()
        .map(|location| location.context.as_str())
        .collect();
    assert!(contexts.contains(&"conflation is delivery"), "{contexts:?}");

    // The cancel: dropping the one subscription disposes the channel —
    // a fresh subscribe finds nothing behind the URI.
    channel.client.unsubscribe_locations(&channel.channel);
    let refused = {
        let seat = Arc::clone(&channel.client);
        let gone = channel.channel.clone();
        block_on(Box::pin(async move {
            let mut waited = 0;
            loop {
                match seat.subscribe_locations(gone.clone()).await {
                    Err(error) => return error,
                    Ok(_) => {
                        // the unsubscribe notification races this
                        // subscribe; drop the row and retry
                        seat.unsubscribe_locations(&gone);
                        waited += 1;
                        assert!(waited < 100, "the channel never disposed");
                        // no reactor on this thread — the harness
                        // polls by parking, so a thread sleep is the
                        // honest wait here
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
        }))
    };
    assert!(refused.contains("subscribe"), "{refused}");
}

#[test]
fn undesignated_and_foreign_folders_answer_nothing() {
    let directory = Arc::new(ClientDirectory::new(Arc::new(|_| {}), Arc::new(|_| {})));
    let local = ResourceLocation::new(
        ResourceType::directory(),
        editor::location::Authority::new("local"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![local], "x").is_empty());
    let foreign = ResourceLocation::new(
        ResourceType::directory(),
        editor::location::Authority::new("remote:box"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![foreign], "x").is_empty());
}
