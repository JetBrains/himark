// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The app-level command vocabulary: work addressed to collections
//! by id (`At`), or erased behind the two dynamic traits. Windowless
//! by design — which window to touch is the shell's business, and a
//! command that needs one is a shell command, not a verb. Feature
//! and collection crates speak THIS language; the shell folds it
//! into its own command stream.

use std::fmt;
use std::sync::Arc;

use crate::effect::Effects;
use crate::store::{Entity, Id, Store};
use crate::ui::UiCtx;

pub type Fx<'a> = Effects<'a, Verb>;

/// A reusable, named command — the registry/palette face and the
/// deferred-request currency.
pub trait DynamicCommand: Send + Sync {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn perform(&self, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>);
}

/// A one-shot command — the landing shape: `perform` consumes it, so
/// an effect's payload moves into the store without a clone.
pub trait DynamicOnceCommand: Send + Sync {
    fn perform(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>);
}

/// A single-use payload with DATA citizenship: commands are data,
/// so every command clones — but a landing's value still moves
/// exactly once. Cloning shares the in-flight slot; whoever runs
/// first takes the value, and a spent twin drops loudly.
pub struct OneShot<T>(Arc<std::sync::Mutex<Option<T>>>);

impl<T> OneShot<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(value))))
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().expect("the one-shot payload").take()
    }
}

impl<T> Clone for OneShot<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

#[derive(Clone)]
pub enum Verb {
    /// The one addressed road (docs/entities.md law 5): lease the
    /// row, perform under its own address, put it back.
    At(Addressed),

    Dynamic(Arc<dyn DynamicCommand>),

    Once(OneShot<Box<dyn DynamicOnceCommand>>),

    /// The OPAQUE escape: a shell-level payload (an application
    /// command, a window-coupled ask) carried through the verb lane.
    /// Built and interpreted only by the shell — the generic runner
    /// never sees one; a shell folds it back into its own stream
    /// before `run`.
    Shell(OneShot<Box<dyn std::any::Any + Send + Sync>>),
}

impl Verb {
    /// The addressed-command constructor: every launch stamp and
    /// every landing re-wrap goes through here.
    pub fn at<T: Entity>(id: Id<T>, command: T::Command) -> Verb {
        Verb::At(Addressed::at(id, command))
    }

    /// The one-shot landing constructor.
    pub fn once(command: impl DynamicOnceCommand + 'static) -> Verb {
        Verb::Once(OneShot::new(Box::new(command)))
    }

    /// The opaque shell-payload constructor.
    pub fn shell(payload: Box<dyn std::any::Any + Send + Sync>) -> Verb {
        Verb::Shell(OneShot::new(payload))
    }

    pub fn run(self, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>) {
        match self {
            Verb::At(addressed) => addressed.run(store, ui, fx),
            Verb::Dynamic(command) => command.perform(store, ui, fx),
            Verb::Once(command) => match command.take() {
                Some(command) => command.perform(store, ui, fx),
                None => eprintln!("[imba] a spent once-landing ran again — dropped"),
            },
            Verb::Shell(_) => {
                debug_assert!(false, "a shell verb reached the generic runner");
                eprintln!("[imba] a shell verb reached the generic runner — dropped");
            }
        }
    }
}

impl fmt::Display for Verb {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verb::At(addressed) => addressed.fmt(out),
            Verb::Dynamic(command) => out.write_str(command.id()),
            Verb::Once(_) => out.write_str("once"),
            Verb::Shell(_) => out.write_str("shell"),
        }
    }
}

/// An addressed command with its entity type erased — one box around
/// the typed pair below. Erasure is per COMMAND, not per collection —
/// the vocabulary stays one arm no matter how many collections become
/// addressable. An `At` answers NO session scope: the store is single
/// and global, so the address owes nothing beyond the id it already
/// is.
pub struct Addressed(Box<dyn AddressedCommand>);

impl Addressed {
    pub fn at<T: Entity>(id: Id<T>, command: T::Command) -> Self {
        Self(Box::new(AtVerb { id, command }))
    }

    pub fn run(self, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>) {
        self.0.run(store, ui, fx)
    }

    /// Commands are data — an address opens back up: the typed
    /// `(id, command)` pair if this `At` targets `T`, else itself.
    /// The road an entity uses to recognize a command addressed to
    /// the row it is currently performing AS, instead of routing
    /// into its own lease.
    pub fn downcast<T: Entity>(self) -> Result<(Id<T>, T::Command), Addressed> {
        if self.0.as_any().is::<AtVerb<T>>() {
            let at = self
                .0
                .into_any()
                .downcast::<AtVerb<T>>()
                .expect("the probe just matched");
            Ok((at.id, at.command))
        } else {
            Err(self)
        }
    }
}

impl Clone for Addressed {
    fn clone(&self) -> Self {
        Self(self.0.clone_box())
    }
}

impl fmt::Display for Addressed {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(out)
    }
}

/// The erased face of `AtVerb<T>` — implemented exactly once; a
/// collection joins the road through `Entity`, never through this.
/// `Display` is the reconcile trace's name for the command.
trait AddressedCommand: Send + Sync + fmt::Display {
    fn run(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>);

    /// Commands are data: the typed command underneath is `Clone`
    /// by bound, so the erasure clones too.
    fn clone_box(&self) -> Box<dyn AddressedCommand>;

    fn as_any(&self) -> &dyn std::any::Any;

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

struct AtVerb<T: Entity> {
    id: Id<T>,
    command: T::Command,
}

impl<T: Entity> fmt::Display for AtVerb<T> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.command.fmt(out)
    }
}

impl<T: Entity> AddressedCommand for AtVerb<T> {
    fn run(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut Fx<'_>) {
        let AtVerb { id, command } = *self;
        store.route(id, command, ui, move |command| Verb::at(id, command), fx);
    }

    fn clone_box(&self) -> Box<dyn AddressedCommand> {
        Box::new(AtVerb {
            id: self.id,
            command: self.command.clone(),
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

/// Deferred, windowless asks — the outbox a model door or a document
/// hook fills when it cannot push effects itself (it runs behind a
/// lease, or inside another entity's perform). The shell's batch
/// loop drains it into the verb lane the same batch.
#[derive(Clone, Default)]
pub struct Requests(Vec<Arc<dyn DynamicCommand>>);

impl Requests {
    pub fn push(store: &mut Store, request: Arc<dyn DynamicCommand>) {
        store.update::<Requests>(|requests| requests.0.push(request));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn drain(store: &mut Store) -> Vec<Arc<dyn DynamicCommand>> {
        let Some(requests) = store.get::<Requests>() else {
            return Vec::new();
        };
        let drained = requests.0.clone();
        if !drained.is_empty() {
            store.put(Requests(Vec::new()));
        }
        drained
    }
}
