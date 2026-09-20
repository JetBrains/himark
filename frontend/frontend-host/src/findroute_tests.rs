// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::hiahp::find::NativeFindHandler;
use crate::hiahp::fs::SeatDirectory;
use himark::higent::seat as ahp;
use himark::ResourceType;
use himark::{FindEffect, ResourceLocation};
use imba::effect::EffectHandler;
use std::sync::Arc;

fn wire_backend() -> (tempfile::TempDir, Arc<dyn himark::higent::AhpServer>) {
    let dir = tempfile::tempdir().expect("backend home");
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }
    let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    (dir, seat)
}

fn block_on<T>(mut future: std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "future never answered"
                );
                std::thread::park_timeout(std::time::Duration::from_millis(100));
            }
        }
    }
}

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
        himark::Authority::new(authority),
        segments,
    )
}

fn find(
    directory: &Arc<SeatDirectory>,
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

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let encoded = ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    );
    let (server, _) = ahp::parse(&encoded).expect("round-trips");
    directory.record(server, Arc::clone(&seat));

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

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    ))
    .expect("parses");
    directory.record(server, Arc::clone(&seat));
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
fn search_locations_route_streams_and_cancels_over_the_wire() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
    let files = dir.path().join("notes").canonicalize().expect("canonical");
    std::fs::write(files.join("a.md"), "plain\nconflation is delivery\n").expect("write");
    std::fs::write(files.join("b.md"), "conflation twice, conflation\n").expect("write");

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    ))
    .expect("parses");
    directory.record(server, Arc::clone(&seat));
    directory.set_local(server);

    let handler = crate::hiahp::locations::RouteSearchLocations {
        directory: Arc::clone(&directory),
    };
    let effect = himark::SearchLocationsEffect {
        folders: vec![located("local", &files)],
        query: "conflation".to_owned(),
        regex: false,
        case_sensitive: false,
        limit: 100,
    };
    let channel = block_on(Box::pin(async move { handler.handle(effect).await })).expect("channel");

    let mut state = {
        let seat = Arc::clone(&channel.seat);
        let subscribed = channel.channel.clone();
        block_on(Box::pin(
            async move { seat.subscribe_locations(subscribed).await },
        ))
        .expect("snapshot")
    };
    while !state.done {
        let seat = Arc::clone(&channel.seat);
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
    assert!(
        contexts.contains(&"conflation is delivery"),
        "{contexts:?}"
    );

    // The cancel: dropping the one subscription disposes the channel —
    // a fresh subscribe finds nothing behind the URI.
    channel.seat.unsubscribe_locations(&channel.channel);
    let refused = {
        let seat = Arc::clone(&channel.seat);
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
    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let local = ResourceLocation::new(
        ResourceType::directory(),
        himark::Authority::new("local"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![local], "x").is_empty());
    let foreign = ResourceLocation::new(
        ResourceType::directory(),
        himark::Authority::new("remote:box"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![foreign], "x").is_empty());
}
