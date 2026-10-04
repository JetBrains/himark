// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The wire across RECONNECTS, against a real backend, with the races
//! driven on purpose. The bug that started this: the keepalive declared
//! a connection dead and the wire dialed a new one — but the old socket
//! was still open (a deaf host is not a closed one), a parked poll held
//! it alive, the host kept its rows, and every chat delta was appended
//! twice. Pulling on that thread found the rest: two asks on a dead
//! connection dialed two connections; a parked poll on a dead connection
//! stayed parked for good; a subscribe answered after the reconnect
//! lived on the dead side; a push straddling the reconnect came back in
//! the replay; live events could overtake the replay.
//!
//! Every test here runs the production wire (`WireHost`) against a real
//! `agent_host::Host` on a unix socket. The only instrumentation is the
//! `Probe` connector: it counts dials, can hold a dial open, delay a
//! connection's inbound, or cut its socket — the things a host, a
//! network or a scheduler do to a wire.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use himark::higent::ahp_types::actions::StateAction;
use himark::higent::client::ResourceUri;
use himark::higent::{
    ChannelUri, ChatClient as _, ChatUri, ClientFuture, ServerEvent, AnnotationsClient as _, ChangesClient as _, DocumentsClient as _, ResourceClient as _,
    SessionClient as _, TerminalClient as _, LocationsClient as _, LspClient as _,
    SessionOptions, SessionUri,
};

use crate::findroute_tests::{bind_backend, block_on};
use crate::hiahp::transport::{Connector, DeadLatched, Dialing};
use crate::hiahp::wire::WireHost;

// ─── the probe ───────────────────────────────────────────────────────────

/// What the test can do to the wire from outside: hold the next dial
/// open (a slow host), count dials (one connection per reconnect, no
/// more), and reach each connection's `Link`.
#[derive(Default)]
struct Probe {
    /// Dials that went through to the socket.
    dials: AtomicUsize,
    /// Dials attempted, refused ones included.
    attempts: AtomicUsize,
    hold_dials: AtomicBool,
    /// The host is down: every dial fails at once.
    refuse_dials: AtomicBool,
    /// Dials go to this url instead — another host behind the same
    /// address, as after a host restart.
    redirect: Mutex<Option<String>>,
    links: Mutex<Vec<Arc<Link>>>,
}

/// One dialed connection, in dial order.
#[derive(Default)]
struct Link {
    /// Inbound lines wait here before the client sees them.
    hold: AtomicBool,
    /// The socket breaks: reads and writes fail, the stream is dropped,
    /// the host sees EOF.
    cut: AtomicBool,
}

impl Probe {
    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    fn gate_dials(&self, held: bool) {
        self.hold_dials.store(held, Ordering::SeqCst);
    }

    fn refuse_dials(&self, refused: bool) {
        self.refuse_dials.store(refused, Ordering::SeqCst);
    }

    fn redirect(&self, url: String) {
        *self.redirect.lock().expect("probe redirect") = Some(url);
    }

    fn link(&self, index: usize) -> Arc<Link> {
        Arc::clone(&self.links.lock().expect("probe links")[index])
    }

    fn wait_for_dials(&self, count: usize, within: Duration) {
        let deadline = Instant::now() + within;
        while self.dials() < count {
            assert!(Instant::now() < deadline, "only {} dials", self.dials());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct ProbeConnector(Arc<Probe>);

impl Connector for ProbeConnector {
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing {
        let probe = Arc::clone(&self.0);
        Box::pin(async move {
            probe.attempts.fetch_add(1, Ordering::SeqCst);
            if probe.refuse_dials.load(Ordering::SeqCst) {
                return Err("agent host unreachable: refused".to_owned());
            }
            while probe.hold_dials.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // A dial held open fails too when the host went down meanwhile.
            if probe.refuse_dials.load(Ordering::SeqCst) {
                return Err("agent host unreachable: refused".to_owned());
            }
            probe.dials.fetch_add(1, Ordering::SeqCst);
            let url = probe
                .redirect
                .lock()
                .expect("probe redirect")
                .clone()
                .unwrap_or(url);
            let path = url.strip_prefix("unix:").expect("a unix url");
            let inner = desktop::UnixTransport::connect(path, tag, Arc::clone(&dead)).await?;
            let link = Arc::new(Link::default());
            probe
                .links
                .lock()
                .expect("probe links")
                .push(Arc::clone(&link));
            Ok(ahp::transport::BoxedTransport::new(DeadLatched {
                inner: Probed { inner, link },
                dead,
            }))
        })
    }
}

struct Probed {
    inner: desktop::UnixTransport,
    link: Arc<Link>,
}

async fn cut(link: &Link) {
    while !link.cut.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

impl ahp::Transport for Probed {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        if self.link.cut.load(Ordering::SeqCst) {
            return Err(ahp::TransportError::Io("cut".to_owned()));
        }
        self.inner.send(msg).await
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        let received = tokio::select! {
            received = self.inner.recv() => received,
            () = cut(&self.link) => return Err(ahp::TransportError::Io("cut".to_owned())),
        };
        while self.link.hold.load(Ordering::SeqCst) {
            if self.link.cut.load(Ordering::SeqCst) {
                return Err(ahp::TransportError::Io("cut".to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        received
    }
}

// ─── the bench ───────────────────────────────────────────────────────────

/// A real backend with the fake CLI, the probed client `a` on it, and
/// as many plain clients as a test wants (`other`): the host serves
/// them all, and what the others dispatch is what `a` must see.
struct Bench {
    _dir: tempfile::TempDir,
    url: String,
    probe: Arc<Probe>,
    a: Arc<WireHost>,
}

fn bench() -> Bench {
    let dir = tempfile::tempdir().expect("backend home");
    let url = bind_backend(
        dir.path(),
        agent_host::testing::fake_cli_command(dir.path()),
    );
    let probe = Arc::new(Probe::default());
    let a = Arc::new(WireHost::at(
        crate::hiahp::wire::test_runtime(),
        Arc::new(ProbeConnector(Arc::clone(&probe))),
        url.clone(),
    ));
    Bench {
        _dir: dir,
        url,
        probe,
        a,
    }
}

impl Bench {
    fn other(&self) -> Arc<WireHost> {
        Arc::new(WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            self.url.clone(),
        ))
    }

    fn folder(&self) -> String {
        format!("file://{}", self._dir.path().display())
    }
}

fn local() -> SessionUri {
    SessionUri::new(host_discovery::LOCAL_FS_SESSION)
}

fn annotations_of(session: &SessionUri) -> ChannelUri {
    ChannelUri::new(format!("{}/annotations", session.as_str()))
}

fn annotation_set(id: &str) -> StateAction {
    serde_json::from_value(serde_json::json!({
        "type": "annotations/set",
        "annotation": {
            "id": id,
            "origin": {"session": host_discovery::LOCAL_FS_SESSION},
            "resource": "file:///tmp/notes.md",
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 4}},
            "resolved": false,
            "entries": [{"id": format!("{id}-e1"), "text": {"markdown": "why?"},
                         "_meta": {"himark": {"author": "user"}}}],
        },
    }))
    .expect("a wire action")
}

/// Dispatch on the annotations channel of `session` and wait for the
/// line to be on the wire.
fn dispatch(seat: &WireHost, session: &SessionUri, id: &str) {
    block_on(seat.dispatch_action(annotations_of(session), annotation_set(id)))
        .unwrap_or_else(|error| panic!("dispatch {id}: {error}"));
}

fn ids(actions: &[StateAction]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|action| match action {
            StateAction::AnnotationsSet(set) => Some(set.annotation.id.clone()),
            _ => None,
        })
        .collect()
}

/// Drive a future on this thread with no deadline — for the poller
/// threads, which park for as long as the test runs.
fn drive<T>(mut future: ClientFuture<T>) -> T {
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
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(100)),
        }
    }
}

/// The shape every live channel has in the app: a poll parked on the
/// feed, re-armed the moment it answers — on its own thread here, so
/// the test can meanwhile do things to the wire. Every batch it gets,
/// empty ones included, lands on `rx` in order.
struct Poller {
    rx: std::sync::mpsc::Receiver<Vec<StateAction>>,
}

impl Poller {
    fn on(poll: impl Fn() -> ClientFuture<Vec<StateAction>> + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || loop {
            let batch = drive(poll());
            if tx.send(batch).is_err() {
                break;
            }
        });
        Self { rx }
    }

    fn annotations(seat: &Arc<WireHost>, session: &SessionUri) -> Self {
        let seat = Arc::clone(seat);
        let session = session.clone();
        Self::on(move || seat.poll_annotations(session.clone()))
    }

    /// Every action until `count` of them arrived, or the deadline.
    fn take(&self, count: usize, within: Duration) -> Vec<StateAction> {
        let deadline = Instant::now() + within;
        let mut taken = Vec::new();
        while taken.len() < count {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(batch) => taken.extend(batch),
                Err(_) => break,
            }
        }
        taken
    }

    fn take_ids(&self, count: usize, within: Duration) -> Vec<String> {
        ids(&self.take(count, within))
    }

    /// Every action until one satisfies `last`, or the deadline.
    fn take_until(
        &self,
        last: impl Fn(&StateAction) -> bool,
        within: Duration,
    ) -> Vec<StateAction> {
        let deadline = Instant::now() + within;
        let mut taken = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(batch) => {
                    let done = batch.iter().any(&last);
                    taken.extend(batch);
                    if done {
                        return taken;
                    }
                }
                Err(_) => return taken,
            }
        }
    }

    /// Nothing arrives for `quiet` — no second copy of anything.
    fn stays_quiet(&self, quiet: Duration) -> Result<(), Vec<String>> {
        let deadline = Instant::now() + quiet;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(batch) if batch.is_empty() => continue,
                Ok(batch) => return Err(ids(&batch)),
                Err(_) => return Ok(()),
            }
        }
    }
}

const TEN: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(700);

// ─── the tests ───────────────────────────────────────────────────────────

/// Eight asks land on a connection that was just declared dead, while
/// the host is slow to answer the dial. ONE connection is dialed; every
/// ask rides it; every action arrives once; the feeds survive. (Two
/// dials were two connections, the second without the feeds, and
/// whichever stored last orphaned the other's subscriptions.)
#[test]
fn concurrent_asks_on_a_dead_connection_dial_once() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    dispatch(a, &session, "c-0");
    assert_eq!(poller.take_ids(1, TEN), vec!["c-0"]);
    assert_eq!(probe.dials(), 1);

    probe.gate_dials(true);
    a.mark_dead();
    let hands: Vec<_> = (1..=8)
        .map(|index| {
            let a = Arc::clone(a);
            let session = session.clone();
            std::thread::spawn(move || dispatch(&a, &session, &format!("c-{index}")))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(probe.dials(), 1, "a dial went through the closed gate");
    probe.gate_dials(false);
    for hand in hands {
        hand.join().expect("the ask completes");
    }

    let mut got = poller.take_ids(8, TEN);
    got.sort();
    let expected: Vec<String> = (1..=8).map(|index| format!("c-{index}")).collect();
    assert_eq!(got, expected);
    assert_eq!(probe.dials(), 2, "every ask dialed a connection of its own");
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// A poll is parked on the feed when the keepalive gives up on the
/// connection, and nothing else asks the wire for anything — the app
/// idle on a chat that is streaming. The poll must come back by itself
/// so that its re-arm reconnects: the pumps feeding it have stopped and
/// a poll that stays parked freezes the channel until the user happens
/// to do something.
#[test]
fn a_parked_poll_reconnects_on_its_own_when_its_connection_dies() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    dispatch(&b, &session, "p-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["p-1"]);

    a.mark_dead();
    std::thread::sleep(Duration::from_millis(100));
    dispatch(&b, &session, "p-2");
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["p-2"],
        "the parked poll never came back: the channel is frozen"
    );
    assert_eq!(probe.dials(), 2);
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// A subscribe is in flight when the connection is declared dead and
/// replaced; its answer arrives on the OLD connection after the new
/// one carried over what was subscribed. Either the subscribe is
/// refused — the caller subscribes again — or, if it reports success,
/// the channel is live on the connection that serves the wire now.
/// Reported success with a feed nobody pumps is the bug.
#[test]
fn a_subscribe_answered_by_a_dead_connection_is_refused_or_live() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let second = block_on(a.create_session(Vec::new(), SessionOptions::default()))
        .expect("a second session");

    let first_link = probe.link(0);
    first_link.hold.store(true, Ordering::SeqCst);
    let subscribing = {
        let a = Arc::clone(a);
        let second = second.clone();
        std::thread::spawn(move || block_on(a.subscribe_annotations(second)))
    };
    std::thread::sleep(Duration::from_millis(100));
    a.mark_dead();
    dispatch(a, &session, "x-1");
    probe.wait_for_dials(2, TEN);
    first_link.hold.store(false, Ordering::SeqCst);

    match subscribing.join().expect("the subscribe answers") {
        Err(_) => {}
        Ok(_) => {
            let poller = Poller::annotations(a, &second);
            dispatch(&b, &second, "y-1");
            assert_eq!(
                poller.take_ids(1, Duration::from_secs(5)),
                vec!["y-1"],
                "the subscribe reported success on a connection that is gone"
            );
        }
    }
}

/// The socket breaks for real (the host dropped the connection): the
/// host forgets the rows, other clients keep dispatching, and the
/// client reconnects only once the gap has grown. The replay fills the
/// gap exactly once and in order, and live delivery resumes after it.
#[test]
fn a_cut_socket_reconnects_and_the_replay_fills_the_gap_once() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    dispatch(&b, &session, "g-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["g-1"]);

    probe.gate_dials(true);
    probe.link(0).cut.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    for id in ["g-2", "g-3", "g-4"] {
        dispatch(&b, &session, id);
    }
    std::thread::sleep(Duration::from_millis(100));
    probe.gate_dials(false);

    assert_eq!(poller.take_ids(3, TEN), vec!["g-2", "g-3", "g-4"]);
    dispatch(&b, &session, "g-5");
    assert_eq!(poller.take_ids(1, TEN), vec!["g-5"]);
    assert_eq!(probe.dials(), 2);
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// The gap outgrows the host's replay depth: the host answers
/// snapshots and the gap is lost whole — by design, and the wire must
/// say nothing of it twice or by halves: no partial replay, and live
/// delivery exactly once afterwards.
#[test]
fn a_gap_past_the_replay_depth_is_lost_whole_and_the_wire_lives() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    block_on(b.subscribe_annotations(session.clone())).expect("b's annotations feed");
    let poller = Poller::annotations(a, &session);
    // b hears everything the host applies: the witness that the whole
    // gap is on the host before a is let back in. (A dispatch resolves
    // when its line is written; the host applies it when it gets there.)
    let witness = Poller::annotations(&b, &session);
    dispatch(&b, &session, "d-0");
    assert_eq!(poller.take_ids(1, TEN), vec!["d-0"]);
    assert_eq!(witness.take_ids(1, TEN), vec!["d-0"]);

    probe.gate_dials(true);
    probe.link(0).cut.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    for index in 1..=600 {
        dispatch(&b, &session, &format!("d-{index}"));
    }
    let applied = witness.take_until(
        |action| matches!(action, StateAction::AnnotationsSet(set) if set.annotation.id == "d-600"),
        Duration::from_secs(30),
    );
    assert_eq!(ids(&applied).len(), 600, "the host applied the whole gap");
    probe.gate_dials(false);
    // Dispatched by `a` itself: the ask waits out a's reconnect, so
    // this lands after the host re-subscribed a — not in the lost gap.
    dispatch(a, &session, "d-after");
    assert_eq!(probe.dials(), 2);
    let got = poller.take_until(
        |action| matches!(action, StateAction::AnnotationsSet(set) if set.annotation.id == "d-after"),
        TEN,
    );
    assert_eq!(
        ids(&got),
        vec!["d-after"],
        "a partial replay of the lost gap"
    );
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// Every live channel re-arms its poll by cancelling the standing one
/// and arming another. A poll dropped unanswered must leave the feed
/// alone: the batch that lands next belongs to the poll that replaced
/// it. (A poll run as a task of the runtime's kept draining after its
/// caller let go, and the relaunched poll lost that batch.)
#[test]
fn a_dropped_poll_leaves_the_next_batch_to_its_replacement() {
    let bench = bench();
    let (a, session) = (&bench.a, local());
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");

    let mut dropped: Vec<ClientFuture<Vec<StateAction>>> = Vec::new();
    for _ in 0..4 {
        let mut poll = a.poll_annotations(session.clone());
        // Armed — polled once, parked on the feed — then let go.
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(&waker);
        assert!(poll.as_mut().poll(&mut context).is_pending());
        dropped.push(poll);
    }
    drop(dropped);

    // The batch lands while nothing live is parked on the feed: it
    // must still be there for the poll armed next.
    dispatch(a, &session, "k-1");
    std::thread::sleep(Duration::from_millis(300));
    let poller = Poller::annotations(a, &session);
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["k-1"],
        "a dropped poll took the batch"
    );
    dispatch(a, &session, "k-2");
    assert_eq!(poller.take_ids(1, TEN), vec!["k-2"]);
    assert_eq!(poller.stays_quiet(QUIET), Ok(()));
}

/// A stream of three hundred actions from another client, the wire
/// declared dead thirty times under it. Every action arrives exactly
/// once and in order — the replay before the live tail, never a live
/// event overtaking the replay, never a straddling push doubled — on
/// one connection per reconnect.
#[test]
fn a_reconnect_storm_under_a_stream_delivers_every_action_once_in_order() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);

    let chaos = {
        let a = Arc::clone(a);
        std::thread::spawn(move || {
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(15));
                a.mark_dead();
            }
        })
    };
    let streamed = {
        let b = Arc::clone(&b);
        let session = session.clone();
        std::thread::spawn(move || {
            for index in 0..300 {
                dispatch(&b, &session, &format!("s-{index:03}"));
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    chaos.join().expect("the chaos ran");
    streamed.join().expect("the stream ran");
    dispatch(&b, &session, "s-end");

    let got = ids(&poller.take_until(
        |action| matches!(action, StateAction::AnnotationsSet(set) if set.annotation.id == "s-end"),
        Duration::from_secs(30),
    ));
    let mut expected: Vec<String> = (0..300).map(|index| format!("s-{index:03}")).collect();
    expected.push("s-end".to_owned());
    assert_eq!(
        got.len(),
        expected.len(),
        "{} dials; got {got:?}",
        probe.dials()
    );
    assert_eq!(got, expected, "{} dials", probe.dials());
    assert!(probe.dials() <= 31, "{} dials for 30 deaths", probe.dials());
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// The symptom itself: a chat turn streaming character by character
/// while the wire is declared dead twenty times under it. The text the
/// chat feed assembles is the text the agent sent — not a character
/// doubled, not one lost — and the turn completes once.
#[test]
fn a_chat_stream_across_reconnects_is_appended_exactly_once() {
    let bench = bench();
    let (a, probe) = (&bench.a, &bench.probe);
    let session = block_on(a.create_session(
        vec![bench.folder()],
        SessionOptions {
            provider: Some("claude".to_owned()),
            ..SessionOptions::default()
        },
    ))
    .expect("a session");
    let state = block_on(a.subscribe_session(session.clone())).expect("the session feed");
    let chat = ChatUri::new(state.default_chat.expect("the default chat"));
    block_on(a.subscribe_chat(chat.clone())).expect("the chat feed");
    let poller = {
        let a = Arc::clone(a);
        let chat = chat.clone();
        Poller::on(move || a.poll_chat(chat.clone()))
    };

    let text: String = (0..1500)
        .map(|index| char::from(b'a' + (index % 26) as u8))
        .collect();
    block_on(a.start_turn(chat, format!("trickle:{text}:end"), None, None)).expect("the turn");
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        a.mark_dead();
    }

    let got = poller.take_until(
        |action| matches!(action, StateAction::ChatTurnComplete(_)),
        Duration::from_secs(60),
    );
    let deltas: Vec<&str> = got
        .iter()
        .filter_map(|action| match action {
            StateAction::ChatDelta(delta) => Some(delta.content.as_str()),
            _ => None,
        })
        .collect();
    let completed = got
        .iter()
        .filter(|action| matches!(action, StateAction::ChatTurnComplete(_)))
        .count();
    let assembled: String = deltas.concat();
    assert!(
        probe.dials() > 5,
        "{} dials: the stream ended before the storm",
        probe.dials()
    );
    assert_eq!(
        assembled,
        text,
        "{} deltas over {} dials",
        deltas.len(),
        probe.dials()
    );
    assert_eq!(deltas.len(), text.len(), "a delta doubled or merged");
    assert_eq!(completed, 1, "the turn completed {completed} times");
}

/// A session of `a`'s own, with the bench folder.
fn own_session(bench: &Bench) -> SessionUri {
    block_on(bench.a.create_session(
        vec![bench.folder()],
        SessionOptions {
            provider: Some("claude".to_owned()),
            ..SessionOptions::default()
        },
    ))
    .expect("a session")
}

/// The host is down when the connection dies: the dial is refused.
/// The poll re-armed on that failure must not park for good — when
/// some ask reconnects later, the channel comes back with it.
#[test]
fn a_poll_that_found_no_connection_follows_the_next_reconnect() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    dispatch(&b, &session, "n-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["n-1"]);

    probe.refuse_dials(true);
    a.mark_dead();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        probe.attempts.load(Ordering::SeqCst) >= 2,
        "the re-armed poll never dialed"
    );
    probe.refuse_dials(false);
    // The host is back; a user's ask reconnects.
    dispatch(a, &session, "n-2");
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["n-2"],
        "the poll parked for good on the refused dial"
    );
    assert_eq!(poller.stays_quiet(QUIET), Ok(()));
}

/// Nothing asks while the host is down: the re-armed poll alone must
/// bring the channel back once the host returns, by its own backoff.
#[test]
fn a_poll_alone_brings_the_channel_back_when_the_host_returns() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    dispatch(&b, &session, "o-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["o-1"]);

    probe.refuse_dials(true);
    a.mark_dead();
    std::thread::sleep(Duration::from_secs(4));
    probe.refuse_dials(false);
    std::thread::sleep(Duration::from_secs(1));
    dispatch(&b, &session, "o-2");
    assert_eq!(
        poller.take_ids(1, Duration::from_secs(20)),
        vec!["o-2"],
        "the channel never came back: {} attempts, {} dials",
        probe.attempts.load(Ordering::SeqCst),
        probe.dials()
    );
    assert_eq!(poller.stays_quiet(QUIET), Ok(()));
}

/// Eight asks wait out one failed reconnect. They share its failure:
/// one dial, eight errors — not eight dials (and eight connect
/// timeouts) in a row. An ask that comes later dials afresh.
#[test]
fn asks_that_waited_out_a_failed_reconnect_share_its_failure() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let attempted = probe.attempts.load(Ordering::SeqCst);

    probe.gate_dials(true);
    a.mark_dead();
    let hands: Vec<_> = (0..8)
        .map(|index| {
            let a = Arc::clone(a);
            let session = session.clone();
            std::thread::spawn(move || {
                block_on(a.dispatch_action(
                    annotations_of(&session),
                    annotation_set(&format!("f-{index}")),
                ))
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    // The dial in flight fails; the gate opens onto a host that is down.
    probe.refuse_dials(true);
    probe.gate_dials(false);
    let outcomes: Vec<_> = hands
        .into_iter()
        .map(|hand| hand.join().expect("answered"))
        .collect();
    assert!(outcomes.iter().all(Result::is_err), "{outcomes:?}");
    assert_eq!(
        probe.attempts.load(Ordering::SeqCst) - attempted,
        1,
        "every waiter dialed on its own"
    );

    probe.refuse_dials(false);
    let poller = Poller::annotations(a, &session);
    dispatch(a, &session, "f-after");
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["f-after"],
        "no fresh dial after the failure"
    );
}

/// A second tap on a channel the client already holds — another
/// reason, another lifetime. The host keeps a row per tap; the client
/// must still deliver every action ONCE to the one feed both taps
/// share, across a reconnect as well.
#[test]
fn a_second_tap_on_a_channel_does_not_double_its_feed() {
    let bench = bench();
    let (a, session) = (&bench.a, local());
    block_on(a.subscribe_annotations(session.clone())).expect("the first tap");
    block_on(a.subscribe_annotations(session.clone())).expect("the second tap");
    let poller = Poller::annotations(a, &session);
    dispatch(a, &session, "t-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["t-1"]);
    assert_eq!(
        poller.stays_quiet(QUIET),
        Ok(()),
        "the second tap doubled the feed"
    );

    a.mark_dead();
    dispatch(a, &session, "t-2");
    assert_eq!(poller.take_ids(1, TEN), vec!["t-2"]);
    assert_eq!(
        poller.stays_quiet(QUIET),
        Ok(()),
        "the second tap doubled the feed after a reconnect"
    );
}

/// Four subscribes of one channel land together. They share ONE feed:
/// a poll armed on any of them sees the action, once.
#[test]
fn concurrent_subscribes_of_one_channel_share_its_feed() {
    let bench = bench();
    let (a, session) = (&bench.a, local());
    let hands: Vec<_> = (0..4)
        .map(|_| {
            let a = Arc::clone(a);
            let session = session.clone();
            std::thread::spawn(move || block_on(a.subscribe_annotations(session)))
        })
        .collect();
    for hand in hands {
        hand.join()
            .expect("subscribed")
            .expect("the annotations feed");
    }
    let poller = Poller::annotations(a, &session);
    dispatch(a, &session, "u-1");
    assert_eq!(poller.take_ids(1, TEN), vec!["u-1"]);
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a feed per subscribe");
}

fn edit(
    base: himark_ahp_ext_types::Uid,
    id: u128,
    text: &str,
) -> himark_ahp_ext_types::DocumentApplied {
    use himark_ahp_ext_types::{Replacement, TextOperation, TextPosition, TextRange};
    himark_ahp_ext_types::DocumentApplied {
        base,
        operation: TextOperation {
            replacements: vec![Replacement {
                range: TextRange {
                    start: TextPosition {
                        line: 0,
                        character: 0,
                    },
                    end: TextPosition {
                        line: 0,
                        character: 0,
                    },
                },
                text: text.to_owned(),
            }],
        },
        id: himark_ahp_ext_types::Uid(id),
        origin: None,
    }
}

/// The document channel rides the same pump: a cut socket, edits from
/// another client while down, the replay fills the gap once and in
/// order, live edits follow. (Edits chain on the host's version — one
/// doubled or missing and the rest are refused.)
#[test]
fn a_document_channel_across_a_cut_replays_every_edit_once() {
    let bench = bench();
    let (a, probe) = (&bench.a, &bench.probe);
    let b = bench.other();
    let session = own_session(&bench);
    let opened = block_on(a.open_document(session.clone(), None, Some(String::new())))
        .expect("an untitled document");
    let channel = ChannelUri::new(opened.document);
    block_on(a.subscribe_document(channel.clone())).expect("a's document feed");
    let state = block_on(b.subscribe_document(channel.clone())).expect("b's document feed");
    let poller = {
        let a = Arc::clone(a);
        let channel = channel.clone();
        Poller::on(move || {
            let poll = a.poll_document(channel.clone());
            Box::pin(async move {
                poll.await
                    .into_iter()
                    .map(|applied| {
                        StateAction::Unknown(serde_json::to_value(applied).expect("serializes"))
                    })
                    .collect()
            })
        })
    };
    let witness = {
        let b = Arc::clone(&b);
        let channel = channel.clone();
        Poller::on(move || {
            let poll = b.poll_document(channel.clone());
            Box::pin(async move {
                poll.await
                    .into_iter()
                    .map(|applied| {
                        StateAction::Unknown(serde_json::to_value(applied).expect("serializes"))
                    })
                    .collect()
            })
        })
    };
    let edit_ids = |actions: &[StateAction]| -> Vec<u128> {
        actions
            .iter()
            .filter_map(|action| match action {
                StateAction::Unknown(value) => {
                    serde_json::from_value::<himark_ahp_ext_types::DocumentApplied>(value.clone())
                        .ok()
                        .map(|applied| applied.id.0)
                }
                _ => None,
            })
            .collect()
    };
    let mut version = state.version;
    let mut send = |index: u128| {
        b.dispatch_document(&channel, edit(version, 1000 + index, "x"));
        version = himark_ahp_ext_types::Uid(1000 + index);
        std::thread::sleep(Duration::from_millis(10));
    };

    send(1);
    assert_eq!(edit_ids(&poller.take(1, TEN)), vec![1001]);
    assert_eq!(edit_ids(&witness.take(1, TEN)), vec![1001]);

    probe.gate_dials(true);
    probe.link(0).cut.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    for index in 2..=20 {
        send(index);
    }
    assert_eq!(
        edit_ids(&witness.take(19, TEN)).len(),
        19,
        "the host applied every edit"
    );
    probe.gate_dials(false);
    assert_eq!(
        edit_ids(&poller.take(19, TEN)),
        (2..=20).map(|index| 1000 + index).collect::<Vec<u128>>()
    );
    send(21);
    assert_eq!(edit_ids(&poller.take(1, TEN)), vec![1021]);
    assert_eq!(probe.dials(), 2);
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a second copy arrived");
}

/// The session channel under a reconnect storm: forty working
/// directories set by another client, every one of them seen once,
/// in order.
#[test]
fn a_session_channel_under_a_reconnect_storm_delivers_once_in_order() {
    let bench = bench();
    let (a, probe) = (&bench.a, &bench.probe);
    let b = bench.other();
    let session = own_session(&bench);
    block_on(a.subscribe_session(session.clone())).expect("the session feed");
    let poller = {
        let a = Arc::clone(a);
        let session = session.clone();
        Poller::on(move || a.poll_session(session.clone()))
    };
    let folders: Vec<String> = (0..40)
        .map(|index| {
            let folder = bench._dir.path().join(format!("wd-{index:02}"));
            std::fs::create_dir_all(&folder).expect("a folder");
            format!("file://{}", folder.display())
        })
        .collect();

    let chaos = {
        let a = Arc::clone(a);
        std::thread::spawn(move || {
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(20));
                a.mark_dead();
            }
        })
    };
    for folder in &folders {
        block_on(b.dispatch_action(
            ChannelUri::new(session.as_str()),
            StateAction::SessionWorkingDirectorySet(
                himark::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                    directory: folder.clone(),
                },
            ),
        ))
        .expect("dispatched");
        std::thread::sleep(Duration::from_millis(5));
    }
    chaos.join().expect("the chaos ran");

    let last = folders.last().expect("forty folders").clone();
    let got = poller.take_until(
        |action| matches!(action, StateAction::SessionWorkingDirectorySet(set) if set.directory == last),
        Duration::from_secs(30),
    );
    let set: Vec<String> = got
        .iter()
        .filter_map(|action| match action {
            StateAction::SessionWorkingDirectorySet(set) => Some(set.directory.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(set, folders, "{} dials", probe.dials());
}

/// The root channel: sessions another client creates after each
/// reconnect reach the root feed, once each. (Root notifications are
/// not replayed; a gap is lost by design — the live road must be
/// whole.)
#[test]
fn the_root_feed_follows_every_reconnect() {
    let bench = bench();
    let (a, session) = (&bench.a, local());
    let b = bench.other();
    block_on(a.connect()).expect("the root");
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = {
        let a = Arc::clone(a);
        Poller::on(move || {
            let poll = a.poll_root();
            Box::pin(async move {
                poll.await
                    .into_iter()
                    .filter_map(|event| match event {
                        ServerEvent::SessionAdded(summary) => {
                            Some(annotation_set(&summary.resource))
                        }
                        _ => None,
                    })
                    .collect()
            })
        })
    };
    for round in 0..3 {
        a.mark_dead();
        dispatch(a, &session, &format!("root-{round}"));
        let created = block_on(b.create_session(Vec::new(), SessionOptions::default()))
            .expect("a session")
            .into_string();
        // Waited for before the next death: a notification in flight
        // on a connection declared dead is dropped with it, and root
        // notifications have no replay to come back on.
        let got = ids(&poller.take_until(
            |action| {
                matches!(action, StateAction::AnnotationsSet(set) if set.annotation.id == created)
            },
            TEN,
        ));
        assert_eq!(got, vec![created], "round {round}");
    }
    assert_eq!(poller.stays_quiet(QUIET), Ok(()), "a session added twice");
}

/// The host restarts: a new process behind the same address, counting
/// its seqs from zero. The client's cursor is from the old count and
/// is PAST everything the new host has, so a reconnect that kept it
/// would be told every gap is covered — and the next gap would be lost.
/// The cursor must follow the host it is on.
#[test]
fn a_restarted_host_does_not_lose_the_next_gap() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let poller = Poller::annotations(a, &session);
    // The old host counts well past where the new one will start.
    for index in 0..30 {
        dispatch(&b, &session, &format!("old-{index}"));
    }
    assert_eq!(poller.take_ids(30, TEN).len(), 30);

    // The restart: another host behind the address, with its own count.
    let reborn = tempfile::tempdir().expect("the reborn host's home");
    let url = bind_backend(
        reborn.path(),
        agent_host::testing::fake_cli_command(reborn.path()),
    );
    let c = Arc::new(WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        url.clone(),
    ));
    block_on(c.subscribe_annotations(session.clone())).expect("c's feed on the reborn host");
    let witness = Poller::annotations(&c, &session);
    probe.redirect(url);
    probe.link(0).cut.store(true, Ordering::SeqCst);
    // Latched dead before anyone asks: a dispatch is fire-and-forget,
    // and one queued on a socket that breaks the next millisecond is
    // gone with it — by protocol, not by this wire.
    std::thread::sleep(Duration::from_millis(100));
    dispatch(a, &session, "new-1");
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["new-1"],
        "a is not on the reborn host"
    );
    assert_eq!(witness.take_ids(1, TEN), vec!["new-1"]);

    // A gap on the reborn host, at seqs the old cursor is past.
    probe.gate_dials(true);
    probe.link(1).cut.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    for index in 2..=5 {
        dispatch(&c, &session, &format!("new-{index}"));
    }
    assert_eq!(
        witness.take_ids(4, TEN).len(),
        4,
        "the reborn host applied the gap"
    );
    probe.gate_dials(false);
    assert_eq!(
        poller.take_ids(4, TEN),
        vec!["new-2", "new-3", "new-4", "new-5"],
        "the gap on the reborn host was lost to the old cursor"
    );
    assert_eq!(poller.stays_quiet(QUIET), Ok(()));
}

/// Unsubscribing from a channel while the connection is dead needs no
/// connection: the dead one takes its rows with it. No dial is made
/// for it, and the reconnect that follows does not carry the channel.
#[test]
fn an_unsubscribe_on_a_dead_connection_does_not_dial() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let second = own_session(&bench);
    block_on(a.subscribe_annotations(second.clone())).expect("the second feed");
    dispatch(&b, &second, "v-1");
    // One poll, answered and not re-armed: nothing of a's is waiting
    // when the connection dies, so nothing but the unsubscribe could
    // dial.
    assert_eq!(
        ids(&block_on(a.poll_annotations(second.clone()))),
        vec!["v-1"]
    );

    a.mark_dead();
    a.unsubscribe_annotations(&second);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(probe.dials(), 1, "a dial just to unsubscribe");

    // Some ask reconnects; the first channel comes back, the second
    // does not.
    let first = Poller::annotations(a, &session);
    dispatch(a, &session, "v-2");
    assert_eq!(first.take_ids(1, TEN), vec!["v-2"]);
    assert_eq!(probe.dials(), 2);
    let poller = Poller::annotations(a, &second);
    dispatch(&b, &second, "v-3");
    assert_eq!(
        poller.stays_quiet(QUIET),
        Ok(()),
        "the unsubscribed channel was carried over"
    );
}

/// A subscribe is in flight when the connection dies and is replaced.
/// It is asked once more on the new connection: the caller gets its
/// snapshot and a LIVE channel, not an error to retry by hand.
#[test]
fn a_subscribe_that_died_under_a_reconnect_is_asked_again_and_live() {
    let bench = bench();
    let (a, probe, session) = (&bench.a, &bench.probe, local());
    let b = bench.other();
    block_on(a.subscribe_annotations(session.clone())).expect("the annotations feed");
    let second = own_session(&bench);

    let first_link = probe.link(0);
    first_link.hold.store(true, Ordering::SeqCst);
    let subscribing = {
        let a = Arc::clone(a);
        let second = second.clone();
        std::thread::spawn(move || block_on(a.subscribe_annotations(second)))
    };
    std::thread::sleep(Duration::from_millis(100));
    a.mark_dead();
    dispatch(a, &session, "w-1");
    probe.wait_for_dials(2, TEN);
    first_link.hold.store(false, Ordering::SeqCst);

    subscribing
        .join()
        .expect("the subscribe answers")
        .expect("the subscribe was asked again on the new connection");
    let poller = Poller::annotations(a, &second);
    dispatch(&b, &second, "w-2");
    assert_eq!(
        poller.take_ids(1, TEN),
        vec!["w-2"],
        "subscribed, but not live"
    );
    assert_eq!(poller.stays_quiet(QUIET), Ok(()));
}
