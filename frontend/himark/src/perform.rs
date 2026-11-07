// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's command vocabulary and interpreter. `AppCommand`
//! is window-free: the addressed road (`At`), the windowless verb
//! lane, the file-event border, the editor-environment
//! registrations — and ONE erased arm for the shell's own
//! vocabulary, interpreted by whatever shell the boot installed
//! (`crate::shell` for the workbench). The engine routes; it never
//! learns what a window is (docs/himark-core.md, commands
//! section).

use imba::command::{Addressed, Verb};

use std::sync::Arc;

use imba::{effect::Effects, store::Store, ui::UiCtx};

#[derive(Clone)]
pub enum AppCommand {
    /// The app-level erased vocabulary (imba::command) — windowless:
    /// addressed entity commands, dynamic commands and one-shot
    /// landings that know collections by id and never a window.
    Verb(imba::command::Verb),

    /// THE one command road (docs/entities.md law 5): any collection,
    /// addressed by its id, stamped at launch, carried through the
    /// whole landing chain — nothing re-derives a scope that was never
    /// lost. Built by `AppCommand::at`; the erasure is what keeps the
    /// enum from growing an arm per collection.
    At(Addressed),

    /// A command addressed to a MOUNTED VIEW — the engine's own
    /// registry road (docs/himark-core.md): the bridge id names
    /// the retained view, the erased command is its own. The road
    /// QUIESCES the view's stages first, so the mutation never
    /// happens under a borrow a host still holds.
    ToView(crate::bridge::ViewBridgeId, imba::dyn_view::DynCommand),

    /// A REGISTERED command at its gesture's ORIGIN bridge — the
    /// bindings resolve at dispatch, through the seated
    /// [`OriginBindings`] resolver (the workbench answers for the
    /// origins that ride a window's raw id) or the engine's own
    /// fallback (the origin and its session). The engine never
    /// learns what the origin shows.
    AtOrigin(
        crate::bridge::ViewBridgeId,
        Arc<dyn crate::commands::HimarkCommand>,
    ),

    /// A registered command with the focus walk's facts captured at
    /// mint — the palette and keymap road on every shell.
    Bound(
        imba::focus::Bindings,
        Arc<dyn crate::commands::HimarkCommand>,
    ),

    /// Register a command into the engine's registry.
    Register(Arc<dyn crate::commands::HimarkCommand>),

    FileChanged(documents::watch::Subscription),

    /// A watch DIED with its connection (`WatchesDied` fans at the
    /// batch tail) — owners re-open what they still show.
    WatchDied(documents::watch::Subscription),

    RegisterLanguages(::editor::reparse::SyntaxLanguages),

    RegisterDiffPolicy(std::sync::Arc<dyn ::editor::diff::DiffPolicy>),

    RegisterEnrichers(::editor::enrich::Enrichers),
}

impl AppCommand {
    /// The one addressed-command constructor: every launch stamp and
    /// every landing re-wrap goes through here.
    pub fn at<T: imba::store::Entity>(id: imba::store::Id<T>, command: T::Command) -> AppCommand {
        AppCommand::At(Addressed::at(id, command))
    }
}

/// Registered MOUNT BINDERS: the typed facts a feature half adds
/// to a mounted origin's bindings (the AHP plugin binds the
/// mount's session and its state). Each answers a BAG to merge —
/// never a bare `ViewBridgeId` (that bind is the engine's signal).
#[derive(Clone, Default)]
pub struct MountBinders(
    #[allow(clippy::type_complexity)]
    pub  Vec<
        Arc<
            dyn Fn(&Store, crate::bridge::ViewBridgeId) -> imba::focus::Bindings + Send + Sync,
        >,
    >,
);

pub(crate) fn mount_bindings(
    store: &Store,
    origin: crate::bridge::ViewBridgeId,
) -> imba::focus::Bindings {
    let Some(binders) = store.get::<MountBinders>() else {
        return imba::focus::Bindings::new();
    };
    let mut bag = imba::focus::Bindings::new();
    for binder in binders.0.iter() {
        bag = bag.merge_under(binder(store, origin));
    }
    bag
}

/// Registered FILE ROUTES: where a watch subscription's change
/// lands (the AHP plugin routes it to the owning session's
/// documents collection and refetches).
#[derive(Clone, Default)]
pub struct FileRoutes(
    #[allow(clippy::type_complexity)]
    pub  Vec<
        Arc<
            dyn Fn(&mut Store, documents::watch::Subscription, &mut AppFx<'_>) + Send + Sync,
        >,
    >,
);

/// The seated ORIGIN-BINDINGS resolver: whoever owns richer origins
/// (the workbench: a window rides the same raw id, and its walk
/// binds the window and the focused document) answers the walk's
/// bindings for one. `None` falls to the engine's own resolution —
/// the origin id and its session.
#[derive(Clone)]
pub struct OriginBindings(
    #[allow(clippy::type_complexity)]
    pub  Arc<
        dyn Fn(&Store, &UiCtx, crate::bridge::ViewBridgeId) -> Option<imba::focus::Bindings>
            + Send
            + Sync,
    >,
);

/// The origin's bindings, resolved at dispatch — the seated
/// resolver first, the engine's fallback otherwise. The fallback
/// binds the origin id itself; a resolver's answer must NOT gain
/// one unasked: a bound `ViewBridgeId` is the SIGNAL commands read
/// as "windowless origin" (hicode's ApplyNavigation forks on it),
/// and the resolver's walk binds what its views chose to bind.
pub(crate) fn origin_bindings(
    store: &Store,
    ui: &UiCtx,
    origin: crate::bridge::ViewBridgeId,
) -> imba::focus::Bindings {
    if let Some(resolver) = store.get::<OriginBindings>().cloned() {
        if let Some(bound) = (resolver.0)(store, ui, origin) {
            // The origin rides along as the NON-SIGNAL fact
            // (`BoundOrigin`): `bound_origin` reads it; a bare bound
            // `ViewBridgeId` would flip commands into their
            // windowless fork.
            return bound.bind(crate::commands::BoundOrigin(origin));
        }
    }
    imba::focus::Bindings::new()
        .bind(origin)
        .bind(crate::commands::BoundOrigin(origin))
        .merge_under(mount_bindings(store, origin))
}

/// Perform a registered command at its origin: resolve the
/// bindings, run it, and re-wrap what it deferred — requests keep
/// the SAME origin, so a landing chain never re-derives a scope
/// that was never lost.
pub fn perform_at_origin(
    store: &mut Store,
    ui: &UiCtx,
    origin: crate::bridge::ViewBridgeId,
    command: Arc<dyn crate::commands::HimarkCommand>,
    fx: &mut AppFx<'_>,
) {
    let bound = origin_bindings(store, ui, origin);
    command.perform(store, ui, &bound, fx);
    for request in crate::commands::AppRequests::drain(store) {
        fx.follow_up(AppCommand::AtOrigin(origin, request));
    }
    for request in imba::command::Requests::drain(store) {
        fx.follow_up(AppCommand::Verb(Verb::Dynamic(request)));
    }
}

/// Perform a registered command with bindings captured at mint.
pub fn perform_bound(
    store: &mut Store,
    ui: &UiCtx,
    bound: imba::focus::Bindings,
    command: Arc<dyn crate::commands::HimarkCommand>,
    fx: &mut AppFx<'_>,
) {
    command.perform(store, ui, &bound, fx);
    for request in crate::commands::AppRequests::drain(store) {
        fx.follow_up(AppCommand::Bound(bound.clone(), request));
    }
    for request in imba::command::Requests::drain(store) {
        fx.follow_up(AppCommand::Verb(Verb::Dynamic(request)));
    }
}

impl std::fmt::Display for AppCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&command_label(self))
    }
}

pub type AppEffects = imba::effect::Batch<AppCommand>;
pub type AppFx<'a> = Effects<'a, AppCommand>;

/// Wrap a shell command for the verb lane — the opaque escape a kit
/// request rides when it must name the application (a window-coupled
/// gesture). Interpreted by the drains below, never by `Verb::run`.
pub fn shell_verb(command: AppCommand) -> Verb {
    Verb::shell(Box::new(command))
}

pub fn command_label(command: &AppCommand) -> std::borrow::Cow<'static, str> {
    // The addressed and shell commands print themselves (`Display`
    // through the erasure) — the type is gone by the time the trace
    // reads one.
    match command {
        AppCommand::At(addressed) => addressed.to_string().into(),
        AppCommand::ToView(_, command) => command.to_string().into(),
        AppCommand::Verb(..) => std::borrow::Cow::Borrowed("verb"),
        AppCommand::AtOrigin(_, command) => format!("at origin: {}", command.id()).into(),
        AppCommand::Bound(_, command) => format!("bound: {}", command.id()).into(),
        AppCommand::Register(..) => std::borrow::Cow::Borrowed("register"),
        AppCommand::FileChanged(..) => std::borrow::Cow::Borrowed("file changed"),
        AppCommand::WatchDied(..) => std::borrow::Cow::Borrowed("watch died"),
        AppCommand::RegisterLanguages(_) => std::borrow::Cow::Borrowed("register languages"),
        AppCommand::RegisterDiffPolicy(_) => std::borrow::Cow::Borrowed("register diff policy"),
        AppCommand::RegisterEnrichers(_) => std::borrow::Cow::Borrowed("register enrichers"),
    }
}

/// The perform CLOSURE: what the command interpreter touches besides
/// the store — borrowed from the application for one batch, so a
/// perform never holds the application itself.
pub(crate) struct Performer<'app> {
    pub(crate) pending_file_events: &'app mut Vec<documents::watch::Subscription>,
    pub(crate) pending_watch_deaths: &'app mut Vec<documents::watch::Subscription>,
    pub(crate) workshop: &'app Arc<::editor::env::Workshop>,
    pub(crate) bridges:
        &'app mut std::collections::HashMap<crate::bridge::ViewBridgeId, crate::himark::Mounted>,
    pub(crate) stages:
        &'app mut crate::bridge::Handles<(crate::bridge::ViewBridgeId, crate::bridge::Stage)>,
}

impl Performer<'_> {
    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: AppCommand,
        fx: &mut AppFx<'_>,
    ) {
        match command {
            AppCommand::At(addressed) => {
                // The one command road (docs/entities.md law 5): lease
                // the row, perform under its own address, put it back.
                fx.scope(AppCommand::Verb, |fx| addressed.run(store, ui, fx));
            }
            AppCommand::Verb(Verb::Shell(payload)) => {
                // A shell escape that landed WINDOWLESS (an effect
                // mapped through `shell_verb`): the payload is an
                // AppCommand — it rides the FOLLOW-UP lane so the loop
                // re-dispatches it with its own scope (a window-scoped
                // command performed inside this verb's scopeless gather
                // would scatter its window writes away). A deferred
                // window-taking ask has no window here and drops loudly.
                let Some(payload) = payload.take() else {
                    eprintln!("[app] a spent shell verb landed again — dropped");
                    return;
                };
                match payload.downcast::<AppCommand>() {
                    Ok(command) => fx.follow_up(*command),
                    Err(_) => {
                        eprintln!("[app] a window-taking shell verb landed windowless — dropped")
                    }
                }
            }
            AppCommand::Verb(verb) => {
                fx.scope(AppCommand::Verb, |fx| verb.run(store, ui, fx));
            }
            AppCommand::AtOrigin(origin, command) => {
                perform_at_origin(store, ui, origin, command, fx);
            }
            AppCommand::Bound(bound, command) => {
                perform_bound(store, ui, bound, command, fx);
            }
            AppCommand::Register(command) => {
                crate::commands::Commands::register(store, command);
            }
            AppCommand::ToView(id, command) => {
                // A POPOVER's view pins this parent to lay against:
                // quiesce each child's frames (they borrow the
                // parent) and drop the pin, or the parent can never
                // be taken mutably while a popover stands.
                let children: Vec<_> = self
                    .bridges
                    .iter()
                    .filter(|(_, slot)| slot.overlay_parent == Some(id))
                    .map(|(child, _)| *child)
                    .collect();
                for child in children {
                    if let Some(slot) = self.bridges.get_mut(&child) {
                        crate::bridge::quiesce(child, slot, self.stages);
                        if let Some(mount) = slot
                            .bridge
                            .view_mut()
                            .and_then(|view| view.as_any_mut())
                            .and_then(|any| any.downcast_mut::<crate::mounts::OverlayMount>())
                        {
                            mount.release_parent();
                        }
                    }
                }
                let Some(slot) = self.bridges.get_mut(&id) else {
                    eprintln!(
                        "[app] a view command addressed an unmounted bridge — dropped: {command}"
                    );
                    return;
                };
                crate::bridge::quiesce(id, slot, self.stages);
                let Some(view) = slot.bridge.view_mut() else {
                    eprintln!(
                        "[app] a view is still borrowed by foreign stages — dropped: {command} (bridge {}, refs {})",
                        id.raw(),
                        slot.bridge.view_refs()
                    );
                    return;
                };
                view.perform(command, id, store, ui, fx);
                // The MOUNTED twin of the Content arm's drain: a
                // deferred request the view's perform pushed rides
                // the windowed lane with this bridge's id — the
                // Windowed arm forks it windowless when no window
                // entity answers. Without this, a mounted road's
                // push waits for a window command that never comes.
                for request in crate::commands::AppRequests::drain(store) {
                    if std::env::var_os("HIMARK_TRACE_NAV").is_some() {
                        eprintln!(
                            "[nav] deferred '{}' drained at ToView, riding bridge {}",
                            request.name(),
                            id.raw()
                        );
                    }
                    fx.follow_up(AppCommand::AtOrigin(id, request));
                }
            }
            AppCommand::WatchDied(subscription) => {
                self.pending_watch_deaths.push(subscription);
            }
            AppCommand::FileChanged(subscription) => {
                // The border road: the event names only a
                // subscription; the registered routes decide where
                // it lands (the AHP plugin: the owning session's
                // documents collection, refetched).
                let routes = store.get::<FileRoutes>().cloned().unwrap_or_default();
                for route in routes.0.iter() {
                    route(store, subscription, fx);
                }

                self.pending_file_events.push(subscription);
            }
            AppCommand::RegisterLanguages(languages) => {
                store.put(::editor::env::Parsers(std::sync::Arc::new(languages)));
            }
            AppCommand::RegisterDiffPolicy(policy) => {
                store.put(::editor::env::Differ(policy));
            }
            AppCommand::RegisterEnrichers(enrichers) => {
                let enrichers = std::sync::Arc::new(enrichers);
                self.workshop.install_enrichers(enrichers.clone());
                store.put(::editor::env::Enrichers(enrichers));
            }
        }
    }
}
