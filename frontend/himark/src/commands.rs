// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;


/// A REGISTERED command: a palette entry, a keymap target or a
/// deferred landing that speaks the app's command stream. What it
/// acts ON arrives through the focus walk's bindings — the UI binds
/// its facts (the window, the origin bridge, the session) on the
/// way down, and the command reads the keys it needs: a session
/// command works in every shell; a dock toggle asks for the window
/// and drops loudly where none is bound. The windowless kind is
/// `imba::command::DynamicCommand` — collections and views ride
/// that one.
pub trait HimarkCommand: Send + Sync {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    );
}


/// The gesture's origin bridge as a BOUND FACT — distinct from a
/// bare bound `ViewBridgeId`, which commands read as the SIGNAL
/// "windowless origin" (the window walk must not raise it). Every
/// origin resolver binds this one.
#[derive(Clone, Copy)]
pub struct BoundOrigin(pub crate::bridge::ViewBridgeId);

/// The navigation prelude: the gesture's origin as a bridge id —
/// the mounted origin itself, or the window riding the same raw id
/// (the shell seat forks on `is_mounted`). The currency
/// `shell::navigate_origin` takes.
pub fn bound_origin(
    command: &str,
    bound: &imba::focus::Bindings,
) -> Option<crate::bridge::ViewBridgeId> {
    let origin = bound
        .bound::<crate::bridge::ViewBridgeId>()
        .copied()
        .or_else(|| bound.bound::<BoundOrigin>().map(|origin| origin.0));
    if origin.is_none() {
        eprintln!("[shell] a navigating command landed with no origin bound — dropped: {command}");
    }
    origin
}


#[derive(Clone, Default)]
pub struct Commands(pub(crate) Vec<Arc<dyn HimarkCommand>>);

impl Commands {
    pub fn of(store: &Store) -> Commands {
        crate::registry::Registry::of(store)
            .map(|registry| registry.commands.clone())
            .unwrap_or_default()
    }

    pub fn register(store: &mut Store, command: Arc<dyn HimarkCommand>) {
        crate::registry::Registry::update(store, |registry| registry.commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn HimarkCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn HimarkCommand>> {
        self.0.iter()
    }
}

#[derive(Clone, Default)]
pub struct AppRequests(Vec<Arc<dyn HimarkCommand>>);

impl AppRequests {
    pub fn push(store: &mut Store, request: Arc<dyn HimarkCommand>) {
        store.update::<AppRequests>(|requests| requests.0.push(request));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn drain(store: &mut Store) -> Vec<Arc<dyn HimarkCommand>> {
        let Some(requests) = store.get::<AppRequests>() else {
            return Vec::new();
        };
        let drained = requests.0.clone();
        if !drained.is_empty() {
            store.put(AppRequests(Vec::new()));
        }
        drained
    }
}

