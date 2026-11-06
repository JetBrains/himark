// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench shell's command vocabulary and its interpreter.
//! `ShellCommand` is the window-shaped half of what used to be
//! `AppCommand`: a REAL `imba::Command` (clone + display), erased
//! into the engine's `AppCommand::Shell(DynCommand)` arm and
//! interpreted by `perform_shell`, which the shell boot installs
//! into the engine (docs/himark-core.md, commands section). The
//! engine routes; this file is the only party that knows what a
//! window is. The road ahead: these arms become an entity command
//! addressed over `At`, and the enum here is already shaped for it.

use std::sync::Arc;

use gfx::Size;
use imba::command::Verb;
use imba::store::{Entity, Id};
use imba::{store::Store, ui::UiCtx, View};

use ::workbench::window::WindowId;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;

use crate::app::{open_effect, DocumentBuild};
use himark::perform::{AppCommand, AppFx};

pub use imba::command::OneShot;

pub enum ShellCommand {
    /// An ENGINE command the ceremony raised while performing as
    /// the shell: the entity's scope wraps everything it raises
    /// back to itself, and this arm carries the engine vocabulary
    /// home — `run_app` unwraps and dispatches.
    App(AppCommand),

    Content(WindowId, ::workbench::window::WindowCommand),

    OpenAsync {
        window: WindowId,
        name: String,
        primary: bool,

        location: Option<editor::location::ResourceLocation>,
        build: OneShot<DocumentBuild>,
    },

    OpenPanel(WindowId, OneShot<Box<dyn hikit::panel::DynPanelView>>),

    OpenModal(WindowId, OneShot<Box<dyn ModalView>>),

    CloseModal(WindowId),

    ViewportResized(WindowId, Size),
}

impl Clone for ShellCommand {
    fn clone(&self) -> Self {
        match self {
            Self::App(command) => Self::App(command.clone()),
            Self::Content(window, command) => Self::Content(*window, command.clone()),
            Self::OpenAsync {
                window,
                name,
                primary,
                location,
                build,
            } => Self::OpenAsync {
                window: *window,
                name: name.clone(),
                primary: *primary,
                location: location.clone(),
                build: build.clone(),
            },
            Self::OpenPanel(window, panel) => Self::OpenPanel(*window, panel.clone()),
            Self::OpenModal(window, modal) => Self::OpenModal(*window, modal.clone()),
            Self::CloseModal(window) => Self::CloseModal(*window),
            Self::ViewportResized(window, size) => Self::ViewportResized(*window, *size),
        }
    }
}

impl std::fmt::Display for ShellCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::App(command) = self {
            return command.fmt(out);
        }
        out.write_str(match self {
            Self::App(_) => unreachable!(),
            Self::Content(_, ::workbench::window::WindowCommand::Base(_)) => "pane",
            Self::Content(_, ::workbench::window::WindowCommand::Toolbar(_)) => "toolbar",
            Self::Content(_, ::workbench::window::WindowCommand::Side(command)) => {
                match command.downcast_ref::<::workbench::drawer::DrawerCommand>() {
                    Some(::workbench::drawer::DrawerCommand::Tick(_)) => "side slide-tick",
                    Some(::workbench::drawer::DrawerCommand::Content(_)) => "side content",
                    None => "side",
                }
            }
            Self::Content(_, ::workbench::window::WindowCommand::SideFocusLost) => "side",
            Self::Content(_, ::workbench::window::WindowCommand::Dock(command)) => {
                match command.downcast_ref::<::workbench::dock::DockCommand>() {
                    Some(::workbench::dock::DockCommand::Tick(_)) => "dock slide-tick",
                    Some(::workbench::dock::DockCommand::Content(_)) => "dock content",
                    _ => "dock",
                }
            }
            Self::Content(_, ::workbench::window::WindowCommand::Modal(_)) => "modal",
            Self::Content(_, ::workbench::window::WindowCommand::Focus(_)) => "focus",
            Self::OpenAsync { .. } => "open async",
            Self::OpenPanel(..) => "open panel",
            Self::OpenModal(..) => "open modal",
            Self::CloseModal(_) => "close modal",
            Self::ViewportResized(..) => "viewport",
        })
    }
}

/// The workbench shell as the ADDRESSED entity: shell commands
/// ride the one command road (`At`), like every collection — the
/// engine routes by id and knows nothing of what a window is.
#[derive(Clone)]
pub struct WorkbenchShell;

impl Entity for WorkbenchShell {
    type Command = ShellCommand;

    fn perform(
        &mut self,
        _id: Id<Self>,
        command: ShellCommand,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, ShellCommand>,
    ) {
        // The ceremony speaks the ENGINE vocabulary (its commands,
        // helpers and effects are all AppCommand-shaped); the scope
        // wraps what it raises back to this entity's own `App` arm.
        fx.scope(ShellCommand::App, |fx| run(store, ui, command, fx));
    }

    fn destroy(&mut self, _store: &mut Store) {}
}

/// The one well-known shell id, minted once per process; the boot
/// puts the entity at it (`Himark::new`).
pub fn shell_id() -> Id<WorkbenchShell> {
    static SHELL: std::sync::OnceLock<Id<WorkbenchShell>> = std::sync::OnceLock::new();
    *SHELL.get_or_init(Id::mint)
}

/// The address door: a shell command onto the one command road.
pub fn shell(command: ShellCommand) -> AppCommand {
    AppCommand::at(shell_id(), command)
}

/// An engine command raised while performing AS the shell. The
/// self-addressed ones unwrap and run inline — routing them would
/// re-enter the shell's own lease; everything else rides the roads
/// it would ride at the top of the loop.
fn run_app(store: &mut Store, ui: &UiCtx, command: AppCommand, fx: &mut AppFx<'_>) {
    match command {
        AppCommand::At(addressed) => match addressed.downcast::<WorkbenchShell>() {
            Ok((_id, command)) => run(store, ui, command, fx),
            Err(addressed) => fx.scope(AppCommand::Verb, |fx| addressed.run(store, ui, fx)),
        },
        AppCommand::Verb(Verb::Shell(payload)) => {
            let Some(payload) = payload.take() else {
                eprintln!("[shell] a spent shell verb surfaced again — dropped");
                return;
            };
            match payload.downcast::<AppCommand>() {
                Ok(command) => run_app(store, ui, *command, fx),
                Err(_) => {
                    eprintln!("[shell] a window-taking shell verb surfaced windowless — dropped")
                }
            }
        }
        AppCommand::Verb(verb) => {
            fx.scope(AppCommand::Verb, |fx| verb.run(store, ui, fx));
        }
        // The engine's registered-command arms run inline here the
        // way the old shell arms did: a modal's Perform must land
        // within this ceremony, not wait for the next batch.
        AppCommand::AtOrigin(origin, command) => {
            himark::perform::perform_at_origin(store, ui, origin, command, fx);
        }
        AppCommand::Bound(bound, command) => {
            himark::perform::perform_bound(store, ui, bound, command, fx);
        }
        AppCommand::Register(command) => {
            himark::commands::Commands::register(store, command);
        }
        other => {
            // The engine-only arms (file events, environment
            // registrations) are never raised by a gesture; a stray
            // one is a routing bug, not work to swallow.
            eprintln!("[shell] an engine command surfaced in the shell — dropped: {other}");
        }
    }
}

pub fn content(window: WindowId, command: ::workbench::window::WindowCommand) -> AppCommand {
    shell(ShellCommand::Content(window, command))
}

/// A registered command at a WINDOW origin — the window rides its
/// raw id as the origin bridge; the seated [`OriginBindings`]
/// resolver answers the walk at dispatch. (The engine arm
/// `AppCommand::AtOrigin` is the road; this mint is the
/// window-shaped door the workbench code speaks.)
pub fn windowed(window: WindowId, command: Arc<dyn himark::commands::HimarkCommand>) -> AppCommand {
    AppCommand::AtOrigin(
        himark::bridge::ViewBridgeId::from_raw(window.raw()),
        command,
    )
}

pub fn bound(
    bound: imba::focus::Bindings,
    command: Arc<dyn himark::commands::HimarkCommand>,
) -> AppCommand {
    AppCommand::Bound(bound, command)
}

/// The window-origin bindings, resolved from the entity: what the
/// focus walk would have bound — the window and its session.
pub fn window_bindings(store: &imba::store::Store, window: WindowId) -> imba::focus::Bindings {
    let mut bound = imba::focus::Bindings::new()
        .bind(window)
        .bind(himark::commands::BoundOrigin(
            himark::bridge::ViewBridgeId::from_raw(window.raw()),
        ));
    if let Some(entity) = ::workbench::window::Windows::window_ref(store, window) {
        bound = bound.bind(crate::workspace::entity_session(entity));
    }
    if let Some(state) = crate::workspace::session_state(store, window) {
        bound = bound.bind(state);
    }
    bound
}

pub fn register(command: Arc<dyn himark::commands::HimarkCommand>) -> AppCommand {
    AppCommand::Register(command)
}

pub fn opened(window: WindowId, document: himark::opening::OpenedDocument) -> AppCommand {
    himark::opening::opened(
        himark::bridge::ViewBridgeId::from_raw(window.raw()),
        document,
    )
}

pub fn open_async(
    window: WindowId,
    name: String,
    primary: bool,
    location: Option<editor::location::ResourceLocation>,
    build: DocumentBuild,
) -> AppCommand {
    shell(ShellCommand::OpenAsync {
        window,
        name,
        primary,
        location,
        build: OneShot::new(build),
    })
}

pub fn open_panel(window: WindowId, panel: Box<dyn hikit::panel::DynPanelView>) -> AppCommand {
    shell(ShellCommand::OpenPanel(window, OneShot::new(panel)))
}

pub fn open_modal(window: WindowId, modal: Box<dyn ModalView>) -> AppCommand {
    shell(ShellCommand::OpenModal(window, OneShot::new(modal)))
}

pub fn close_modal(window: WindowId) -> AppCommand {
    shell(ShellCommand::CloseModal(window))
}

pub fn viewport_resized(window: WindowId, size: Size) -> AppCommand {
    shell(ShellCommand::ViewportResized(window, size))
}

/// Hand a place to the seated shell (`himark::himark::Shell`) —
/// the knife between app logic and placement: everything above
/// this call is logic that stays in Rust; what the place becomes
/// on screen is the shell's answer.
fn navigate(
    store: &mut Store,
    ui: &UiCtx,
    window: WindowId,
    place: impl hikit::navigation::Place,
    fx: &mut AppFx<'_>,
) {
    let origin = himark::bridge::ViewBridgeId::from_raw(window.raw());
    fx.scope(AppCommand::Verb, |fx| {
        navigate_origin(store, ui, origin, place, fx)
    });
}

pub use himark::shell::navigate_origin;

/// A drained verb back into the app stream: shell payloads unwrap
/// (an `AppCommand`, or a deferred dynamic ask that takes the
/// draining window); everything else rides the Verb arm.
#[doc(hidden)]
pub fn verb_command(window: WindowId, verb: Verb) -> Option<AppCommand> {
    match verb {
        Verb::Shell(payload) => {
            let Some(payload) = payload.take() else {
                eprintln!("[app] a spent shell verb was drained again — dropped");
                return None;
            };
            match payload.downcast::<AppCommand>() {
                Ok(command) => Some(*command),
                Err(payload) => {
                    match payload.downcast::<std::sync::Arc<dyn himark::commands::HimarkCommand>>() {
                        Ok(command) => Some(windowed(window, *command)),
                        Err(_) => {
                            eprintln!("[app] an unknown shell verb payload was dropped");
                            None
                        }
                    }
                }
            }
        }
        verb => Some(AppCommand::Verb(verb)),
    }
}

/// The window ceremony — one shell command, run to completion.
/// Recursions re-enter through `run_app`, so a shell command that
/// raises another runs inline, exactly as the one big match always
/// did.
fn run(store: &mut Store, ui: &UiCtx, command: ShellCommand, fx: &mut AppFx<'_>) {
    match command {
        ShellCommand::App(command) => run_app(store, ui, command, fx),
        ShellCommand::Content(window, command) => {
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            fx.scope(
                move |command| content(window, command),
                |fx| entity.perform(store, ui, command, fx),
            );

            let panel_request = entity.take_panel_request();

            let side_request = entity.take_side_request();
            let dock_request = entity.take_dock_request();

            let toolbar_request = entity.take_toolbar_request();

            let request = entity.take_modal_request();
            match request {
                Some(ModalRequest::Close) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    ::workbench::window::Windows::put(store, window, entity);
                }
                Some(ModalRequest::Perform(verb)) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    ::workbench::window::Windows::put(store, window, entity);
                    if let Some(command) = verb_command(window, verb) {
                        run_app(store, ui, command, fx);
                    }
                }
                Some(ModalRequest::OpenAt {
                    location,
                    target,
                    focus,
                }) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    ::workbench::window::Windows::put(store, window, entity);
                    navigate(
                        store,
                        ui,
                        window,
                        himark::places::DocumentPlace {
                            location,
                            target,
                            focus,
                        },
                        fx,
                    );
                }
                Some(ModalRequest::ShowDocument(document)) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    ::workbench::window::Windows::put(store, window, entity);
                    navigate(
                        store,
                        ui,
                        window,
                        himark::places::OpenDocumentPlace {
                            document,
                            target: None,
                            focus: false,
                        },
                        fx,
                    );
                }
                Some(ModalRequest::OpenLocations(locations)) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    ::workbench::window::Windows::put(store, window, entity);
                    navigate(
                        store,
                        ui,
                        window,
                        himark::places::LocationsPlace(locations),
                        fx,
                    );
                }
                Some(ModalRequest::SelectWidget(widget)) => {
                    fx.scope(
                        move |command| content(window, command),
                        |fx| entity.dismiss_modal(store, fx),
                    );
                    entity.mount_focused(store, widget);
                    ::workbench::window::Windows::put(store, window, entity);
                }
                None => {
                    ::workbench::window::Windows::put(store, window, entity);
                }
            }
            if let Some(request) = side_request {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| content(window, command),
                    |fx| entity.dismiss_side_panel(store, fx),
                );
                match request {
                    ModalRequest::Close => {
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                    ModalRequest::Perform(verb) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        if let Some(command) = verb_command(window, verb) {
                            run_app(store, ui, command, fx);
                        }
                    }
                    ModalRequest::OpenAt {
                        location,
                        target,
                        focus,
                    } => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::DocumentPlace {
                                location,
                                target,
                                focus,
                            },
                            fx,
                        );
                    }
                    ModalRequest::ShowDocument(document) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::OpenDocumentPlace {
                                document,
                                target: None,
                                focus: false,
                            },
                            fx,
                        );
                    }
                    ModalRequest::OpenLocations(locations) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::LocationsPlace(locations),
                            fx,
                        );
                    }
                    ModalRequest::SelectWidget(widget) => {
                        entity.mount_focused(store, widget);
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                }
            }
            if let Some(request) = dock_request {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                match request {
                    ModalRequest::Close => {
                        fx.scope(
                            move |command| content(window, command),
                            |fx| entity.dismiss_dock(store, fx),
                        );
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                    ModalRequest::Perform(verb) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        if let Some(command) = verb_command(window, verb) {
                            run_app(store, ui, command, fx);
                        }
                    }
                    ModalRequest::OpenAt {
                        location,
                        target,
                        focus,
                    } => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::DocumentPlace {
                                location,
                                target,
                                focus,
                            },
                            fx,
                        );
                    }
                    ModalRequest::ShowDocument(document) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::OpenDocumentPlace {
                                document,
                                target: None,
                                focus: false,
                            },
                            fx,
                        );
                    }
                    ModalRequest::OpenLocations(locations) => {
                        ::workbench::window::Windows::put(store, window, entity);
                        navigate(
                            store,
                            ui,
                            window,
                            himark::places::LocationsPlace(locations),
                            fx,
                        );
                    }
                    ModalRequest::SelectWidget(widget) => {
                        entity.mount_focused(store, widget);
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                }
            }
            match panel_request {
                Some(hikit::panel::PanelRequest::OpenLocations(locations)) => {
                    navigate(
                        store,
                        ui,
                        window,
                        himark::places::LocationsPlace(locations),
                        fx,
                    );
                }
                Some(hikit::panel::PanelRequest::OpenDiff(old_side, new_side)) => {
                    navigate(
                        store,
                        ui,
                        window,
                        himark::places::DiffPlace {
                            old: old_side,
                            new: new_side,
                        },
                        fx,
                    );
                }
                Some(hikit::panel::PanelRequest::OpenAt(location, target)) => {
                    run_app(
                        store,
                        ui,
                        windowed(
                            window,
                            Arc::new(himark::diff_canvas::OpenCanvasFile { location, target }),
                        ),
                        fx,
                    );
                }
                Some(hikit::panel::PanelRequest::Perform(command)) => {
                    run_app(store, ui, AppCommand::Verb(Verb::Dynamic(command)), fx);
                }
                Some(hikit::panel::PanelRequest::Shell(payload)) => {
                    match payload.downcast_ref::<Arc<dyn himark::commands::HimarkCommand>>() {
                        Some(command) => {
                            let command = command.clone();
                            run_app(store, ui, windowed(window, command), fx);
                        }
                        None => eprintln!("[app] an unknown panel shell ask was dropped"),
                    }
                }
                None => {}
            }
            match toolbar_request {
                Some(::workbench::toolbar::ToolbarRequest::Command(id)) => {
                    if let Some(command) = himark::commands::Commands::of(store).find(id).cloned() {
                        run_app(store, ui, windowed(window, command), fx);
                    }
                }
                None => {}
            }

            for request in himark::commands::AppRequests::drain(store) {
                run_app(store, ui, windowed(window, request), fx);
            }
            for request in imba::command::Requests::drain(store) {
                run_app(store, ui, AppCommand::Verb(Verb::Dynamic(request)), fx);
            }
        }
        ShellCommand::OpenAsync {
            window,
            name,
            primary,
            location,
            build,
        } => {
            let Some(build) = build.take() else {
                eprintln!("[shell] an open-async build was already spent — dropped");
                return;
            };
            // The one synchronous moment of this road: bind the
            // gesture's session here, before anything is in flight.
            let documents = crate::workspace::session_state(store, window)
                .expect("a document opens into a window with a session")
                .documents();
            fx.push(open_effect(
                window, documents, name, primary, location, build,
            ));
        }
        ShellCommand::OpenPanel(window, panel) => {
            let Some(panel) = panel.take() else {
                eprintln!("[shell] an open-panel ask was already spent — dropped");
                return;
            };
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            entity.open_panel(store, ui, panel, fx);
            ::workbench::window::Windows::put(store, window, entity);
        }
        ShellCommand::OpenModal(window, modal) => {
            let Some(modal) = modal.take() else {
                eprintln!("[shell] an open-modal ask was already spent — dropped");
                return;
            };
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            fx.scope(
                move |command| content(window, command),
                |fx| entity.show_modal(store, modal, fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
        }
        ShellCommand::CloseModal(window) => {
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            fx.scope(
                move |command| content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
        }
        ShellCommand::ViewportResized(window, size) => {
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            entity.set_viewport_size(size);
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
}
