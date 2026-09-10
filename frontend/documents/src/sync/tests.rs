use super::*;
use operation::{Op, OperationBuilder};
use rebase::{Action, RebaseLog};

fn state(source: &str) -> SyncState {
    SyncState::new(Text::from_string_exact(source), EditLog::new())
}

fn read(state: &SyncState) -> String {
    let end = state.text.byte_count() as u32;
    state.text.view().substring(0..end)
}

fn edit(state: &SyncState, at: usize, delete: &str, insert: &str) -> SyncEdit {
    let mut builder = OperationBuilder::new();
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
    SyncEdit::captured(state.log.clone(), builder.finish(), EditIdentity::mint())
}

#[test]
fn an_edit_that_lands_where_it_was_made_keeps_its_identity() {
    let mut replica = state("hello");
    let edit = edit(&replica, 5, "", "!");
    let identity = edit.identity().expect("ours");
    let applied = edit.apply(&mut replica).expect("it landed");
    assert_eq!(read(&replica), "hello!");
    assert_eq!(applied.identity(), Some(identity), "one instance, two logs");
    assert_eq!(replica.log.head(), Some(identity));
}

#[test]
fn a_replayed_edit_crosses_the_bridge_and_is_a_new_instance() {
    let mine = state("hello");
    let ours = edit(&mine, 5, "", "!");
    let identity = ours.identity().expect("ours");

    let mut theirs = state("hello");
    edit(&theirs, 0, "", ">> ").apply(&mut theirs).expect("it landed");

    let applied = ours.apply(&mut theirs).expect("it landed");
    assert_eq!(read(&theirs), ">> hello!", "it landed where it meant to");
    assert_ne!(applied.identity(), Some(identity), "a replay is a new instance");
}

#[test]
fn a_slice_carries_a_taker_to_the_offered_state() {
    let mut replica = state("hello");
    let standing = replica.log.clone();
    edit(&replica, 5, "", "!").apply(&mut replica).expect("it landed");
    edit(&replica, 0, "", ">> ").apply(&mut replica).expect("it landed");

    let slice = replica.slice_from(&standing).expect("one history");
    let caught_up = Text::from_string_exact("hello").edit(&slice);
    assert_eq!(caught_up, replica.text, "the slice IS the difference");
    assert!(
        slice.iter().any(|op| matches!(op, Op::Insert(_))),
        "and it is an ordinary operation"
    );
}

struct Server {
    state: SyncState,
    version: u64,
    broadcast: Vec<(u64, SyncEdit)>,
}

impl Server {
    fn offer(&mut self, dispatch: &rebase::Dispatch<u64, SyncEdit>) -> bool {
        if dispatch.base != self.version {
            return false;
        }
        let applied = dispatch.action.clone().apply(&mut self.state).expect("it landed");
        self.version = dispatch.id;
        self.broadcast.push((dispatch.id, applied));
        true
    }
}

struct Client {
    id: u64,
    log: RebaseLog<u64, SyncEdit>,
    outbox: Vec<rebase::Dispatch<u64, SyncEdit>>,
    heard: usize,
    minted: u64,
}

impl Client {
    fn new(id: u64, source: &str) -> Self {
        Self {
            id,
            log: RebaseLog::new(state(source), 0),
            outbox: Vec::new(),
            heard: 0,
            minted: 0,
        }
    }

    fn edit(&mut self, at: usize, delete: &str, insert: &str) {
        self.minted += 1;
        let id = self.id * 1000 + self.minted;
        let action = edit(self.log.display(), at, delete, insert);
        if let Some(dispatch) = self.log.local(id, action) {
            self.outbox.push(dispatch);
        }
    }

    fn hear(&mut self, server: &Server) {
        while self.heard < server.broadcast.len() {
            let (id, action) = server.broadcast[self.heard].clone();
            self.heard += 1;
            if self.log.ack(&id) {
                continue;
            }
            self.outbox.clear();
            self.log.remote(id, action);
            while let Some(dispatch) = self.log.step() {
                self.outbox.push(dispatch);
            }
        }
    }
}

fn crank(server: &mut Server, alice: &mut Client, bob: &mut Client) {
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
        state: state("hello"),
        version: 0,
        broadcast: Vec::new(),
    };
    let mut alice = Client::new(1, "hello");
    let mut bob = Client::new(2, "hello");

    alice.edit(5, "", " alice");
    bob.edit(0, "", "BOB ");
    crank(&mut server, &mut alice, &mut bob);

    assert!(alice.log.is_settled() && bob.log.is_settled());
    assert_eq!(read(alice.log.display()), read(bob.log.display()));
    assert_eq!(read(alice.log.display()), read(&server.state));
    let settled = read(&server.state);
    assert!(settled.contains("alice") && settled.contains("BOB"), "{settled}");
}

#[test]
fn a_client_that_keeps_typing_through_a_rebase_converges() {
    let mut server = Server {
        state: state("hello"),
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
    crank(&mut server, &mut alice, &mut bob);

    assert_eq!(read(alice.log.display()), read(bob.log.display()));
    assert_eq!(read(alice.log.display()), read(&server.state));
    assert_eq!(read(&server.state), ">> hello!?");
}
