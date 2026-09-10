use super::*;
use operation::Operation;

#[derive(Clone, Debug, Default)]
struct EditLog {
    entries: Vec<LogEntry>,
}

#[derive(Clone, Debug, PartialEq)]
struct LogEntry {
    stable: u64,
    identity: u64,
    op: Operation,
}

fn identity() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MINT: AtomicU64 = AtomicU64::new(1);
    MINT.fetch_add(1, Ordering::Relaxed)
}

impl EditLog {
    fn push(&mut self, stable: u64, op: Operation) {
        self.entries.push(LogEntry {
            stable,
            identity: identity(),
            op,
        });
    }

    fn head(&self) -> u64 {
        self.entries.last().map(|entry| entry.stable).unwrap_or(0)
    }
}

fn common_base(before: &EditLog, after: &EditLog) -> Option<(usize, usize)> {
    let mut i = before.entries.len() as isize - 1;
    let mut j = after.entries.len() as isize - 1;
    while i >= 0 && j >= 0 {
        if before.entries[i as usize].identity == after.entries[j as usize].identity {
            return Some((i as usize, j as usize));
        }
        if i < j && j > 0 {
            j -= 1;
        } else {
            i -= 1;
        }
    }
    None
}

fn composed(entries: &[LogEntry]) -> Option<Operation> {
    let mut composed: Option<Operation> = None;
    for entry in entries {
        composed = Some(match composed {
            Some(before) => before.compose(&entry.op),
            None => entry.op.clone(),
        });
    }
    composed
}

fn bridge(from: &EditLog, to: &EditLog) -> Operation {
    let (before, after) = match common_base(from, to) {
        Some((i, j)) => (&from.entries[i + 1..], &to.entries[j + 1..]),
        None => (&from.entries[..], &to.entries[..]),
    };
    match (composed(before), composed(after)) {
        (None, None) => Operation::default(),
        (None, Some(after)) => after,
        (Some(before), None) => before.invert(),
        (Some(before), Some(after)) => before.invert().compose(&after),
    }
}

#[derive(Clone, Debug, Default)]
struct Doc {
    text: text::Text,
    log: EditLog,
}

impl Doc {
    fn new(source: &str) -> Self {
        Self {
            text: text::Text::from_string_exact(source),
            log: EditLog::default(),
        }
    }

    fn read(&self) -> String {
        let end = self.text.byte_count() as u32;
        self.text.view().substring(0..end)
    }
}

#[derive(Clone, Debug)]
struct Edit {
    id: u64,
    base: EditLog,
    op: Operation,
}

impl Action for Edit {
    type State = Doc;

    fn apply(self, state: &mut Doc) -> Option<Self> {
        let base = state.log.clone();
        let op = self.op.transform(&bridge(&self.base, &state.log));
        state.text = state.text.edit(&op);
        state.log.push(self.id, op.clone());
        Some(Edit {
            id: self.id,
            base,
            op,
        })
    }
}

fn edit(state: &Doc, id: u64, at: usize, delete: &str, insert: &str) -> Edit {
    let mut builder = operation::OperationBuilder::new();
    if at > 0 {
        builder.push_retain(at as u32);
    }
    if !delete.is_empty() {
        builder.push_delete(delete.to_owned());
    }
    if !insert.is_empty() {
        builder.push_insert(insert.to_owned());
    }
    let tail = state.text.byte_count() - at - delete.len();
    if tail > 0 {
        builder.push_retain(tail as u32);
    }
    Edit {
        id,
        base: state.log.clone(),
        op: builder.finish(),
    }
}

fn log(source: &str) -> RebaseLog<u64, Edit> {
    RebaseLog::new(Doc::new(source), 0)
}

#[test]
fn a_settled_edit_dispatches_at_once() {
    let mut log = log("hello");
    let op = edit(log.display(), 1, 5, "", " world");
    let sent = log.local(1, op).expect("a settled edit goes out at once");
    assert_eq!(sent.base, 0, "chained on the confirmed version");
    assert_eq!(log.display().read(), "hello world");
    assert_eq!(log.committed().read(), "hello", "nothing is confirmed yet");
}

#[test]
fn dispatches_chain_on_their_predecessor() {
    let mut log = log("a");
    let first = edit(log.display(), 1, 1, "", "b");
    log.local(1, first).expect("sent");
    let second = edit(log.display(), 2, 2, "", "c");
    let sent = log.local(2, second).expect("sent");
    assert_eq!(sent.base, 1, "the second chains on the first");
    assert_eq!(log.display().read(), "abc");
    assert_eq!(log.pending(), 2);
}

#[test]
fn an_ack_advances_the_committed_state() {
    let mut log = log("a");
    let op = edit(log.display(), 7, 1, "", "b");
    log.local(7, op).expect("sent");
    assert!(log.ack(&7), "our head");
    assert_eq!(log.committed().read(), "ab");
    assert_eq!(*log.version(), 7, "the confirmed id IS the version");
    assert!(log.is_settled());
    assert!(!log.ack(&7), "a stale echo is not ours to ack");
}

#[test]
fn a_foreign_action_rebases_the_chain() {
    let mut log = log("hello");
    let ours = edit(log.display(), 1, 5, "", "!");
    log.local(1, ours).expect("sent");
    assert_eq!(log.display().read(), "hello!");

    let theirs = edit(log.committed(), 9, 0, "", ">> ");
    log.remote(9, theirs);
    assert!(log.is_rebasing());
    assert_eq!(log.committed().read(), ">> hello");

    let sent = log.step().expect("a re-applied entry always re-dispatches");
    assert_eq!(sent.id, 1, "same id, fresh base (§5)");
    assert_eq!(sent.base, 9, "chained on the new confirmed version");
    assert!(!log.is_rebasing());
    assert_eq!(
        log.display().read(),
        ">> hello!",
        "both sides survive, each where it meant to be"
    );
}

#[test]
fn entries_cross_every_foreign_action_that_landed() {
    let mut log = log("hello");
    log.local(1, edit(log.display(), 1, 5, "", "!")).expect("sent");
    let first = edit(log.committed(), 9, 0, "", "A");
    log.remote(9, first);
    let second = edit(log.committed(), 10, 0, "", "B");
    log.remote(10, second);
    assert_eq!(log.committed().read(), "BAhello");

    log.step().expect("re-dispatched");
    assert!(!log.is_rebasing());
    assert_eq!(log.display().read(), "BAhello!");
}

#[test]
fn an_edit_made_mid_rebase_crosses_last() {
    let mut log = log("hello");
    log.local(1, edit(log.display(), 1, 5, "", "!")).expect("sent");
    let shown = log.display().clone();
    let theirs = edit(log.committed(), 9, 0, "", ">> ");
    log.remote(9, theirs);

    let late = edit(&shown, 2, 6, "", "?");
    assert!(log.local(2, late).is_none(), "it waits its turn");

    let mut sent = Vec::new();
    while let Some(dispatch) = log.step() {
        sent.push(dispatch.id);
    }
    assert_eq!(sent, vec![1, 2], "in order, both re-dispatched");
    assert_eq!(log.display().read(), ">> hello!?");
}

#[test]
fn an_offer_slices_into_one_operation() {
    let mut log = log("hello");
    let origin = log.display().log.clone();
    log.local(1, edit(log.display(), 1, 5, "", "!")).expect("sent");
    let after_first = log.display().log.clone();
    log.local(2, edit(log.display(), 2, 0, "", ">> ")).expect("sent");

    let display = log.display();
    let whole = text::Text::from_string_exact("hello").edit(&bridge(&origin, &display.log));
    assert_eq!(display.text, whole, "from the origin, the slice IS the difference");

    let tail = text::Text::from_string_exact("hello!").edit(&bridge(&after_first, &display.log));
    assert_eq!(display.text, tail, "and from anywhere else in the log");
}

#[test]
fn a_rebased_entry_keeps_its_name_and_takes_a_fresh_identity() {
    let mut log = log("hello");
    log.local(1, edit(log.display(), 1, 5, "", "!")).expect("sent");
    let before = log.display().log.entries.last().expect("an entry").clone();
    assert_eq!(before.stable, 1);
    assert_eq!(log.display().log.head(), 1, "the log names its head");

    let theirs = edit(log.committed(), 9, 0, "", ">> ");
    log.remote(9, theirs);
    log.step().expect("re-dispatched");

    let after = log.display().log.entries.last().expect("an entry");
    assert_eq!(after.stable, 1, "the same operation, by name");
    assert_ne!(
        after.identity, before.identity,
        "a different instance — the scan must be able to tell"
    );
}

struct Server {
    doc: Doc,
    version: u64,
    broadcast: Vec<Edit>,
}

impl Server {
    fn offer(&mut self, dispatch: &Dispatch<u64, Edit>) -> bool {
        if dispatch.base != self.version {
            return false;
        }
        let applied = dispatch
            .action
            .clone()
            .apply(&mut self.doc)
            .expect("the server applies what it accepts");
        self.version = dispatch.id;
        self.broadcast.push(applied);
        true
    }
}

struct Client {
    id: u64,
    log: RebaseLog<u64, Edit>,
    outbox: Vec<Dispatch<u64, Edit>>,
    heard: usize,
    minted: u64,
}

impl Client {
    fn new(id: u64, source: &str) -> Self {
        Self {
            id,
            log: RebaseLog::new(Doc::new(source), 0),
            outbox: Vec::new(),
            heard: 0,
            minted: 0,
        }
    }

    fn edit(&mut self, at: usize, delete: &str, insert: &str) {
        self.minted += 1;
        let id = self.id * 1000 + self.minted;
        let action = edit(self.log.display(), id, at, delete, insert);
        if let Some(dispatch) = self.log.local(id, action) {
            self.outbox.push(dispatch);
        }
    }

    fn hear(&mut self, server: &Server) {
        while self.heard < server.broadcast.len() {
            let action = server.broadcast[self.heard].clone();
            self.heard += 1;
            if self.log.ack(&action.id) {
                continue;
            }
            self.outbox.clear();
            self.log.remote(action.id, action);
            while let Some(dispatch) = self.log.step() {
                self.outbox.push(dispatch);
            }
        }
    }
}

fn crank(server: &mut Server, clients: [&mut Client; 2]) {
    let [alice, bob] = clients;
    for _ in 0..8 {
        for client in [&mut *alice, &mut *bob] {
            for dispatch in std::mem::take(&mut client.outbox) {
                if !server.offer(&dispatch) {
                    break;
                }
            }
        }
        alice.hear(server);
        bob.hear(server);
    }
}

#[test]
fn two_clients_converge() {
    let mut server = Server {
        doc: Doc::new("hello"),
        version: 0,
        broadcast: Vec::new(),
    };
    let mut alice = Client::new(1, "hello");
    let mut bob = Client::new(2, "hello");

    alice.edit(5, "", " alice");
    bob.edit(0, "", "BOB ");
    crank(&mut server, [&mut alice, &mut bob]);

    assert!(alice.log.is_settled() && bob.log.is_settled());
    assert_eq!(alice.log.display().read(), bob.log.display().read());
    assert_eq!(alice.log.display().read(), server.doc.read());
    let settled = server.doc.read();
    assert!(
        settled.contains("alice") && settled.contains("BOB"),
        "both edits survived: {settled}"
    );
}

#[test]
fn a_client_that_keeps_typing_through_a_rebase_converges() {
    let mut server = Server {
        doc: Doc::new("hello"),
        version: 0,
        broadcast: Vec::new(),
    };
    let mut alice = Client::new(1, "hello");
    let mut bob = Client::new(2, "hello");

    alice.edit(5, "", "!");
    bob.edit(0, "", ">> ");
    for dispatch in std::mem::take(&mut bob.outbox) {
        assert!(server.offer(&dispatch));
    }
    alice.edit(6, "", "?");
    crank(&mut server, [&mut alice, &mut bob]);

    assert_eq!(alice.log.display().read(), bob.log.display().read());
    assert_eq!(alice.log.display().read(), server.doc.read());
    assert_eq!(server.doc.read(), ">> hello!?");
}

use tokio::sync::mpsc;

struct Ui {
    shown: text::Text,

    at: EditLog,

    sent: u64,
    minted: u64,
    id: u64,
}

impl Ui {
    fn edit(&mut self, at: usize, delete: &str, insert: &str) -> Edit {
        self.minted += 1;
        let id = self.id * 1000 + self.minted;
        let mut builder = operation::OperationBuilder::new();
        if at > 0 {
            builder.push_retain(at as u32);
        }
        if !delete.is_empty() {
            builder.push_delete(delete.to_owned());
        }
        if !insert.is_empty() {
            builder.push_insert(insert.to_owned());
        }
        let tail = self.shown.byte_count() - at - delete.len();
        if tail > 0 {
            builder.push_retain(tail as u32);
        }
        let op = builder.finish();
        self.shown = self.shown.edit(&op);
        self.sent += 1;
        let mut base = self.at.clone();
        base.push(id, op.clone());
        let captured = Edit {
            id,
            base: self.at.clone(),
            op,
        };

        self.at = base;
        captured
    }

    fn offered(&mut self, offer: Offer<Doc>) -> Option<Local<Edit>> {
        if offer.seen_local != self.sent {
            return None;
        }
        self.shown = self.shown.edit(&bridge(&self.at, &offer.state.log));

        self.at = offer.state.log;
        Some(Local::Took {
            seen_local: offer.seen_local,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn two_looped_clients_converge_through_channels() {
    let source = "hello";
    let (a_local, a_local_rx) = mpsc::unbounded_channel();
    let (a_remote, a_remote_rx) = mpsc::channel(64);
    let (a_wire, mut a_wire_rx) = mpsc::channel(64);
    let (a_offers, mut a_offers_rx) = mpsc::channel(64);
    let (b_local, b_local_rx) = mpsc::unbounded_channel();
    let (b_remote, b_remote_rx) = mpsc::channel(64);
    let (b_wire, mut b_wire_rx) = mpsc::channel(64);
    let (b_offers, mut b_offers_rx) = mpsc::channel(64);

    tokio::spawn(run(
        RebaseLog::new(Doc::new(source), 0),
        a_local_rx,
        a_remote_rx,
        a_wire,
        a_offers,
        {
            let mut seen = 0u64;
            move || {
                seen += 1;
                1000 + seen
            }
        },
    ));
    tokio::spawn(run(
        RebaseLog::new(Doc::new(source), 0),
        b_local_rx,
        b_remote_rx,
        b_wire,
        b_offers,
        {
            let mut seen = 0u64;
            move || {
                seen += 1;
                2000 + seen
            }
        },
    ));

    let mut server = Server {
        doc: Doc::new(source),
        version: 0,
        broadcast: Vec::new(),
    };
    let mut alice = Ui {
        shown: text::Text::from_string_exact(source),
        at: EditLog::default(),
        sent: 0,
        minted: 0,
        id: 1,
    };
    let mut bob = Ui {
        shown: text::Text::from_string_exact(source),
        at: EditLog::default(),
        sent: 0,
        minted: 0,
        id: 2,
    };

    a_local
        .send(Local::Edit(alice.edit(5, "", " alice")))
        .expect("sent");
    b_local.send(Local::Edit(bob.edit(0, "", "BOB "))).expect("sent");

    for _ in 0..200 {
        tokio::task::yield_now().await;
        while let Ok(dispatch) = a_wire_rx.try_recv() {
            server.offer(&dispatch);
        }
        while let Ok(dispatch) = b_wire_rx.try_recv() {
            server.offer(&dispatch);
        }
        for action in server.broadcast.drain(..) {
            let id = action.id;
            a_remote
                .send(Applied {
                    id,
                    action: action.clone(),
                })
                .await
                .expect("heard");
            b_remote.send(Applied { id, action }).await.expect("heard");
        }
        while let Ok(offer) = a_offers_rx.try_recv() {
            if let Some(took) = alice.offered(offer) {
                a_local.send(took).expect("heard");
            }
        }
        while let Ok(offer) = b_offers_rx.try_recv() {
            if let Some(took) = bob.offered(offer) {
                b_local.send(took).expect("heard");
            }
        }
    }

    let read = |value: &text::Text| {
        let end = value.byte_count() as u32;
        value.view().substring(0..end)
    };
    assert_eq!(read(&alice.shown), read(&bob.shown), "the UIs show one text");
    assert_eq!(read(&alice.shown), server.doc.read(), "and it is the server's");
    let settled = server.doc.read();
    assert!(
        settled.contains("alice") && settled.contains("BOB"),
        "both edits survived: {settled}"
    );
}
