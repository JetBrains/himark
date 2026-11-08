// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The FEATURES against the wire's TWO failure modes — which are
//! very different animals:
//!
//! - **Connection loss, host alive**: the wire owes CONTINUITY —
//!   feeds are carried, the replay fills the gap, features notice
//!   nothing. `reconnect_tests` drives that mode exhaustively. The
//!   one exception is watches: the HOST scopes watch rows to the
//!   connection, so a cut kills them and `WatchEvent::Died` must
//!   say so (the file-tree law) — pinned here.
//!
//! - **Host restart**: the reborn host keeps the DISK and loses
//!   every in-memory thing — document mirrors, language servers,
//!   PTYs, minted channels. Continuity is impossible by
//!   construction; the contract is HONESTY: the resume's `missing`
//!   list is the restart signal, and a feature must HEAR that its
//!   channel died and re-create from the top (with restart
//!   semantics — a re-opened document mirror is the disk's text,
//!   not the lost one's), never freeze on a feed nobody will fill.
//!
//! Each test drives the production wire (`WireHost`) through the
//! shared Probe connector against a real `agent_host::server::Host`;
//! a restart is a second host over the SAME state dir behind a
//! redirect, the old socket cut.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ahp_wire::client::DocumentsClient as _;
use ahp_wire::client::ResourceClient as _;
use ahp_wire::client::SessionClient as _;
use ahp_wire::client::TerminalClient as _;
use ahp_wire::client::{ChannelUri, ResourceUri, SessionUri, TerminalEvent, WatchEvent};
use ahp_wire::wire::WireHost;

mod common;
use common::probe::{Probe, ProbeConnector};
use common::{bind_backend_at, block_on};

const TEN: Duration = Duration::from_secs(10);

/// One probed client on a restartable host. The state dir is shared
/// by every incarnation — the disk survives a restart, the memory
/// does not, exactly like the real thing.
struct Bench {
    dir: tempfile::TempDir,
    probe: Arc<Probe>,
    a: Arc<WireHost>,
    incarnation: usize,
    url: String,
}

fn bench() -> Bench {
    let dir = tempfile::tempdir().expect("backend home");
    let url = bind_backend_at(
        dir.path().join("backend-0.sock"),
        dir.path(),
        agent_host::testing::fake_cli_command(dir.path()),
        Vec::new(),
    );
    let probe = Arc::new(Probe::default());
    let a = Arc::new(WireHost::at(
        ahp_wire::wire::test_runtime(),
        Arc::new(ProbeConnector(Arc::clone(&probe))),
        url.clone(),
    ));
    Bench {
        dir,
        probe,
        a,
        incarnation: 0,
        url,
    }
}

impl Bench {
    /// The host restarts: a new process over the same disk, every
    /// in-memory channel gone. The client's standing connection is
    /// cut; its next reconnect lands on the reborn host.
    fn restart(&mut self) {
        self.incarnation += 1;
        let url = bind_backend_at(
            self.dir
                .path()
                .join(format!("backend-{}.sock", self.incarnation)),
            self.dir.path(),
            agent_host::testing::fake_cli_command(self.dir.path()),
            Vec::new(),
        );
        self.probe.redirect(url.clone());
        self.url = url;
        self.probe.last_link().cut.store(true, Ordering::SeqCst);
        // The dead latch: give the cut a moment to be seen, so the
        // next ask reconnects instead of riding the corpse.
        std::thread::sleep(Duration::from_millis(150));
    }

    /// The CURRENT incarnation's address — where a plain second
    /// client dials.
    fn url(&self) -> String {
        self.url.clone()
    }

    fn local(&self) -> SessionUri {
        SessionUri::new(host_discovery::LOCAL_FS_SESSION)
    }

    fn file_uri(&self, name: &str) -> ResourceUri {
        let file = self.dir.path().join(name);
        ResourceUri::new(format!(
            "file://{}",
            file.canonicalize().expect("canonical").display()
        ))
    }
}

/// One re-armed poll loop on its own thread — the shape every lane
/// gives its channel. What lands on `rx` is what the lane would see.
fn poll_loop<T: Send + 'static>(
    poll: impl Fn() -> ahp_wire::client::ClientFuture<T> + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let batch = drive(poll());
        if tx.send(batch).is_err() {
            break;
        }
    });
    rx
}

/// Drive a future on this thread with no deadline — the poller
/// threads park for as long as the test runs.
fn drive<T>(mut future: ahp_wire::client::ClientFuture<T>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(50)),
        }
    }
}

// ─── the documents (docsync) ─────────────────────────────────────────────

/// The reborn host never saw the document mirror: its channel comes
/// back in the reconnect's `missing` list. The standing poll — the
/// docsync lane's one ear — must ANSWER (so the lane can re-open),
/// not park forever on a feed nobody will fill. The frozen poll is
/// the file-tree watch bug one layer up.
#[test]
#[ignore = "RED BY DESIGN: the resume's missing list drops the feed silently and the poll waits forever — unignore with the frozen-channel fix"]
fn a_document_poll_hears_the_restart_instead_of_freezing() {
    let mut bench = bench();
    block_on(bench.a.connect()).expect("connected");
    std::fs::write(bench.dir.path().join("shared.md"), "shared\n").expect("the file");
    let uri = bench.file_uri("shared.md");
    let opened = block_on(bench.a.open_document(bench.local(), Some(uri), None)).expect("opened");
    let channel = ChannelUri::new(opened.document);
    block_on(bench.a.subscribe_document(channel.clone())).expect("subscribed");

    let polls = {
        let a = Arc::clone(&bench.a);
        let channel = channel.clone();
        poll_loop(move || a.poll_document(channel.clone()))
    };
    // The poll is parked on the live feed before the host dies.
    std::thread::sleep(Duration::from_millis(200));

    bench.restart();
    // Something asks, the wire reconnects, the reborn host answers
    // the resume.
    block_on(bench.a.connect()).expect("reconnected");

    // THE LAW, half one: the poll must resolve — any answer at all —
    // when its connection dies. A lane that never hears never re-opens.
    assert!(
        polls.recv_timeout(TEN).is_ok(),
        "the document poll is FROZEN across the restart"
    );

    // THE LAW, half two: the re-armed poll is LIVE, not a black
    // hole. The disk survived, the resume re-subscribed the channel
    // on the reborn host — so an edit dispatched by a second client
    // must reach the standing poll loop.
    let b = Arc::new(WireHost::at(
        ahp_wire::wire::test_runtime(),
        himark_plugins::test_connector(),
        bench.url(),
    ));
    block_on(b.connect()).expect("b connected");
    let reopened =
        block_on(b.open_document(bench.local(), Some(bench.file_uri("shared.md")), None))
            .expect("b reopened");
    assert_eq!(
        ChannelUri::new(reopened.document.clone()),
        channel,
        "one document, one channel — the mint is stable across the restart"
    );
    let snapshot = block_on(b.subscribe_document(channel.clone())).expect("b subscribed");
    let before = text::text::Text::from_string_exact(&snapshot.text);
    let mut builder = operation::builder::OperationBuilder::new();
    builder.push_insert("fresh ".to_owned());
    builder.push_retain(before.byte_count() as u32);
    b.dispatch_document(
        &channel,
        himark_ahp_ext_types::documents::DocumentApplied {
            base: snapshot.version,
            operation: ahp_docsync::codec::wire_operation(&before, &builder.finish()),
            id: himark_ahp_ext_types::documents::Uid(42),
            origin: None,
        },
    );
    // The edit APPLIED: b's own poll hears its echo — so whatever a
    // misses below, it is a's standing feed that is dead, not the
    // edit, not the host.
    let echoed = ahp_wire::wire::test_runtime().block_on({
        let b = Arc::clone(&b);
        let channel = channel.clone();
        async move {
            tokio::time::timeout(TEN, async move {
                loop {
                    if !b.poll_document(channel.clone()).await.is_empty() {
                        return;
                    }
                }
            })
            .await
        }
    });
    assert!(echoed.is_ok(), "b's edit never applied on the reborn host");

    let deadline = Instant::now() + TEN;
    loop {
        match polls.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(batch) if !batch.is_empty() => break,
            Ok(_) => continue,
            Err(_) => panic!(
                "the re-armed document poll never saw b's edit — the \
                 channel on the reborn host is a black hole for the \
                 client that held it across the restart"
            ),
        }
    }
}

/// The recovery recipe a lane rides once it HEARS the death: open
/// the document again on the reborn host — the disk survived, so the
/// mirror re-mints — and the fresh channel is live.
#[test]
fn a_fresh_open_after_the_restart_mints_a_live_channel() {
    let mut bench = bench();
    block_on(bench.a.connect()).expect("connected");
    std::fs::write(bench.dir.path().join("shared.md"), "shared\n").expect("the file");
    let uri = bench.file_uri("shared.md");
    let opened = block_on(
        bench
            .a
            .open_document(bench.local(), Some(uri.clone()), None),
    )
    .expect("opened");
    let stale = ChannelUri::new(opened.document);
    block_on(bench.a.subscribe_document(stale.clone())).expect("subscribed");

    bench.restart();

    let reopened =
        block_on(bench.a.open_document(bench.local(), Some(uri), None)).expect("reopened");
    let fresh = ChannelUri::new(reopened.document);
    let snapshot = block_on(bench.a.subscribe_document(fresh)).expect("resubscribed");
    assert_eq!(
        snapshot.text, "shared\n",
        "the reborn host serves the document from disk"
    );
}

// ─── the watches (the law that started this) ─────────────────────────────

/// The watch law at the wire: a watch lives exactly as long as its
/// connection — the HOST scopes watch rows to the connection, so a
/// MERE CONNECTION CUT kills it even though the host survives — and
/// its death is SAID (one `WatchEvent::Died`), never implied. The
/// file-tree's whole recovery stands on this one event. (This is
/// the one subscription whose failure mode is connection loss, not
/// host restart; the channels above survive a cut by replay.)
#[test]
fn a_watch_says_died_when_its_connection_dies() {
    let bench = bench();
    block_on(bench.a.connect()).expect("connected");
    std::fs::create_dir_all(bench.dir.path().join("watched")).expect("the folder");
    let uri = bench.file_uri("watched");

    let deaths = Arc::new(Mutex::new(0usize));
    let events = {
        let deaths = Arc::clone(&deaths);
        Arc::new(move |event: WatchEvent| {
            if matches!(event, WatchEvent::Died) {
                *deaths.lock().expect("deaths") += 1;
            }
        })
    };
    let handle = block_on(bench.a.resource_watch(bench.local(), uri, events));
    assert!(handle.is_some(), "the watch stood");

    // The connection dies; the HOST STAYS. No restart, no redirect —
    // the pure failure mode this law is about.
    bench.probe.last_link().cut.store(true, Ordering::SeqCst);

    let deadline = Instant::now() + TEN;
    while *deaths.lock().expect("deaths") == 0 {
        assert!(
            Instant::now() < deadline,
            "the watch died with its connection and nobody said so"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Exactly once: a second Died would re-open the folder twice.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(*deaths.lock().expect("deaths"), 1, "Died fired twice");
}

// ─── the terminals ───────────────────────────────────────────────────────

/// The reborn host has no PTYs. The wire's resume re-attaches every
/// seat; one the host no longer knows must EXIT its pane honestly —
/// a dead prompt, not a frozen one.
#[test]
fn a_terminal_on_the_reborn_host_exits_instead_of_freezing() {
    let mut bench = bench();
    block_on(bench.a.connect()).expect("connected");

    let saw = Arc::new(Mutex::new((false, false))); // (data, exited)
    let events = {
        let saw = Arc::clone(&saw);
        Arc::new(move |event: TerminalEvent| {
            let mut saw = saw.lock().expect("terminal events");
            match event {
                TerminalEvent::Data(_) => saw.0 = true,
                TerminalEvent::Exited(_) => saw.1 = true,
                TerminalEvent::Reset => {}
            }
        })
    };
    let channel = ChannelUri::new("ahp-terminal:/feature-reconnect/1".to_owned());
    let handle =
        block_on(
            bench
                .a
                .terminal_open(bench.local(), channel.clone(), None, 80, 24, events),
        );
    assert!(handle.is_some(), "the terminal opened");
    bench.a.terminal_input(&channel, "echo alive\n".to_owned());
    let deadline = Instant::now() + TEN;
    while !saw.lock().expect("terminal events").0 {
        assert!(Instant::now() < deadline, "no output from the live PTY");
        std::thread::sleep(Duration::from_millis(10));
    }

    bench.restart();
    block_on(bench.a.connect()).expect("reconnected");

    let deadline = Instant::now() + TEN;
    while !saw.lock().expect("terminal events").1 {
        assert!(
            Instant::now() < deadline,
            "the PTY died with the host and the pane never heard — \
             a frozen prompt instead of an honest exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
