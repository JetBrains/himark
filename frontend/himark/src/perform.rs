// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's command vocabulary and interpreter: `AppCommand`
//! and the `Performer` that runs one. The window/workbench arms
//! are tenants on notice — the generalization ahead addresses them
//! to the Windows entity over the `At` road (docs/himark-core.md,
//! commands section) and this enum shrinks toward `Verb` + `At`.

use imba::command::{Addressed, Verb};

use std::sync::Arc;

use imba::{effect::Effects, store::Store, ui::UiCtx, View};
use skia_safe::Size;

use ::workbench::window::WindowId;
use documents::OpenDocuments;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;

use crate::app::{open_effect, OpenedDocument, DocumentBuild};

pub enum AppCommand {
    Content(WindowId, ::workbench::window::WindowCommand),

    Windowed(
        WindowId,
        std::sync::Arc<dyn crate::commands::WindowedCommand>,
    ),

    /// The app-level erased vocabulary (imba::command) — windowless:
    /// addressed entity commands, dynamic commands and one-shot
    /// landings that know collections by id and never a window.
    Verb(imba::command::Verb),

    Register(std::sync::Arc<dyn crate::commands::WindowedCommand>),

    /// THE one command road (docs/entities.md law 5): any collection,
    /// addressed by its id, stamped at launch, carried through the
    /// whole landing chain — nothing re-derives a scope that was never
    /// lost. Built by `AppCommand::at`; the erasure is what keeps the
    /// enum from growing an arm per collection.
    At(Addressed),

    Opened(WindowId, OpenedDocument),

    FileChanged(documents::watch::Subscription),

    OpenAsync {
        window: WindowId,
        name: String,
        primary: bool,

        location: Option<editor::location::ResourceLocation>,
        build: DocumentBuild,
    },

    OpenPanel(WindowId, Box<dyn hikit::panel::DynPanelView>),

    OpenModal(WindowId, Box<dyn ModalView>),

    CloseModal(WindowId),

    ViewportResized(WindowId, Size),

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

pub(crate) type AppEffects = imba::effect::Batch<AppCommand>;
pub type AppFx<'a> = Effects<'a, AppCommand>;

/// Wrap a shell command for the verb lane — the opaque escape a kit
/// request rides when it must name the application (a window-coupled
/// gesture). Interpreted by the drains below, never by `Verb::run`.
pub fn shell_verb(command: AppCommand) -> Verb {
    Verb::Shell(Box::new(command))
}

/// A drained verb back into the app stream: shell payloads unwrap
/// (an `AppCommand`, or a deferred dynamic ask that takes the
/// draining window); everything else rides the Verb arm.
#[doc(hidden)]
pub fn verb_command(window: WindowId, verb: Verb) -> Option<AppCommand> {
    match verb {
        Verb::Shell(payload) => match payload.downcast::<AppCommand>() {
            Ok(command) => Some(*command),
            Err(payload) => {
                match payload.downcast::<std::sync::Arc<dyn crate::commands::WindowedCommand>>() {
                    Ok(command) => Some(AppCommand::Windowed(window, *command)),
                    Err(_) => {
                        eprintln!("[app] an unknown shell verb payload was dropped");
                        None
                    }
                }
            }
        },
        verb => Some(AppCommand::Verb(verb)),
    }
}

pub(crate) fn command_label(command: &AppCommand) -> std::borrow::Cow<'static, str> {
    // The addressed commands print themselves (`Display` through the
    // erased `Addressed`) — the type is erased by the time the trace
    // reads one.
    if let AppCommand::At(addressed) = command {
        return addressed.to_string().into();
    }
    std::borrow::Cow::Borrowed(match command {
        AppCommand::Content(_, ::workbench::window::WindowCommand::Base(_)) => "pane",
        AppCommand::Content(_, ::workbench::window::WindowCommand::Toolbar(_)) => "toolbar",
        AppCommand::Content(_, ::workbench::window::WindowCommand::Side(command)) => {
            match command.downcast_ref::<::workbench::drawer::DrawerCommand>() {
                Some(::workbench::drawer::DrawerCommand::Tick(_)) => "side slide-tick",
                Some(::workbench::drawer::DrawerCommand::Content(_)) => "side content",
                None => "side",
            }
        }
        AppCommand::Content(_, ::workbench::window::WindowCommand::SideFocusLost) => "side",
        AppCommand::Content(_, ::workbench::window::WindowCommand::Dock(command)) => {
            match command.downcast_ref::<::workbench::dock::DockCommand>() {
                Some(::workbench::dock::DockCommand::Tick(_)) => "dock slide-tick",
                Some(::workbench::dock::DockCommand::Content(_)) => "dock content",
                _ => "dock",
            }
        }
        AppCommand::Content(_, ::workbench::window::WindowCommand::Modal(_)) => "modal",
        AppCommand::Content(_, ::workbench::window::WindowCommand::Focus(_)) => "focus",
        AppCommand::Windowed(..) => "windowed",
        AppCommand::Verb(..) => "verb",
        AppCommand::Register(_) => "register",
        AppCommand::OpenAsync { .. } => "open async",
        AppCommand::OpenPanel(..) => "open panel",
        AppCommand::OpenModal(..) => "open modal",
        AppCommand::CloseModal(_) => "close modal",
        AppCommand::ViewportResized(..) => "viewport",
        AppCommand::RegisterLanguages(_) => "register languages",
        AppCommand::RegisterDiffPolicy(_) => "register diff policy",
        AppCommand::RegisterEnrichers(_) => "register enrichers",
        AppCommand::At(..) => unreachable!(),
        AppCommand::Opened(..) => "opened",
        AppCommand::FileChanged(..) => "file changed",
    })
}

/// The perform CLOSURE: what the command interpreter touches besides
/// the store — borrowed from the application for one batch, so a
/// perform never holds the application itself.
pub(crate) struct Performer<'app> {
    pub(crate) pending_file_events: &'app mut Vec<documents::watch::Subscription>,
    pub(crate) workshop: &'app Arc<::editor::env::Workshop>,
}

impl Performer<'_> {
    pub(crate) fn perform(&mut self, store: &mut Store, ui: &UiCtx, command: AppCommand, fx: &mut AppFx<'_>) {
        match command {
            AppCommand::Content(window, command) => {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.perform(store, ui, command, fx),
                );

                let panel_request = entity.take_panel_request();

                let side_request = entity.take_side_request();
                let dock_request = entity.take_dock_request();

                let toolbar_request = entity.take_toolbar_request();
                let dock_command = entity.take_dock_command();

                let request = entity.take_modal_request();
                match request {
                    Some(ModalRequest::Close) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                    Some(ModalRequest::Perform(verb)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        ::workbench::window::Windows::put(store, window, entity);
                        if let Some(command) = verb_command(window, verb) {
                            self.perform(store, ui, command, fx);
                        }
                    }
                    Some(ModalRequest::OpenAt {
                        location,
                        target,
                        focus,
                    }) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        ::workbench::window::Windows::put(store, window, entity);
                        if let Some(state) = crate::workspace::session_state(store, window) {
                            fx.push(crate::workspace::open_by_location_effect(
                                window,
                                state.documents(),
                                location,
                                true,
                                focus,
                                target,
                            ));
                        }
                    }
                    Some(ModalRequest::ShowDocument(document)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        fx.scope(AppCommand::Verb, |fx| {
                            entity.show_document(store, ui, window, document, None, false, fx);
                        });
                        ::workbench::window::Windows::put(store, window, entity);

                        if let Some(state) = crate::workspace::session_state(store, window) {
                            fx.scope(crate::app::AppCommand::Verb, |fx| {
                                documents::lanes::sync_document_watches(
                                    store,
                                    state.documents(),
                                    fx,
                                )
                            });
                            fx.scope(AppCommand::Verb, |fx| {
                                documents::lanes::sync_stripe_bases(
                                    store,
                                    state.documents(),
                                    ui,
                                    fx,
                                )
                            });
                        }
                    }
                    Some(ModalRequest::OpenLocations(locations)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        ::workbench::window::Windows::put(store, window, entity);
                        crate::workspace::open_locations(store, ui, window, &locations, fx);
                    }
                    Some(ModalRequest::SelectWidget(widget)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
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
                    let mut entity = ::workbench::window::Windows::window(store, window)
                        .expect("the window entity");
                    fx.scope(
                        move |command| AppCommand::Content(window, command),
                        |fx| entity.dismiss_side_panel(store, fx),
                    );
                    match request {
                        ModalRequest::Close => {
                            ::workbench::window::Windows::put(store, window, entity);
                        }
                        ModalRequest::Perform(verb) => {
                            ::workbench::window::Windows::put(store, window, entity);
                            if let Some(command) = verb_command(window, verb) {
                                self.perform(store, ui, command, fx);
                            }
                        }
                        ModalRequest::OpenAt {
                            location,
                            target,
                            focus,
                        } => {
                            ::workbench::window::Windows::put(store, window, entity);
                            if let Some(state) = crate::workspace::session_state(store, window) {
                                fx.push(crate::workspace::open_by_location_effect(
                                    window,
                                    state.documents(),
                                    location,
                                    true,
                                    focus,
                                    target,
                                ));
                            }
                        }
                        ModalRequest::ShowDocument(document) => {
                            fx.scope(AppCommand::Verb, |fx| {
                                entity.show_document(store, ui, window, document, None, false, fx);
                            });
                            ::workbench::window::Windows::put(store, window, entity);

                            if let Some(state) = crate::workspace::session_state(store, window) {
                                fx.scope(crate::app::AppCommand::Verb, |fx| {
                                    documents::lanes::sync_document_watches(
                                        store,
                                        state.documents(),
                                        fx,
                                    )
                                });
                                fx.scope(AppCommand::Verb, |fx| {
                                    documents::lanes::sync_stripe_bases(
                                        store,
                                        state.documents(),
                                        ui,
                                        fx,
                                    )
                                });
                            }
                        }
                        ModalRequest::OpenLocations(locations) => {
                            ::workbench::window::Windows::put(store, window, entity);
                            crate::workspace::open_locations(store, ui, window, &locations, fx);
                        }
                        ModalRequest::SelectWidget(widget) => {
                            entity.mount_focused(store, widget);
                            ::workbench::window::Windows::put(store, window, entity);
                        }
                    }
                }
                if let Some(request) = dock_request {
                    let mut entity = ::workbench::window::Windows::window(store, window)
                        .expect("the window entity");
                    match request {
                        ModalRequest::Close => {
                            fx.scope(
                                move |command| AppCommand::Content(window, command),
                                |fx| entity.dismiss_dock(store, fx),
                            );
                            ::workbench::window::Windows::put(store, window, entity);
                        }
                        ModalRequest::Perform(verb) => {
                            ::workbench::window::Windows::put(store, window, entity);
                            if let Some(command) = verb_command(window, verb) {
                                self.perform(store, ui, command, fx);
                            }
                        }
                        ModalRequest::OpenAt {
                            location,
                            target,
                            focus,
                        } => {
                            ::workbench::window::Windows::put(store, window, entity);
                            if let Some(state) = crate::workspace::session_state(store, window) {
                                fx.push(crate::workspace::open_by_location_effect(
                                    window,
                                    state.documents(),
                                    location,
                                    true,
                                    focus,
                                    target,
                                ));
                            }
                        }
                        ModalRequest::ShowDocument(document) => {
                            fx.scope(AppCommand::Verb, |fx| {
                                entity.show_document(store, ui, window, document, None, false, fx);
                            });
                            ::workbench::window::Windows::put(store, window, entity);
                            if let Some(state) = crate::workspace::session_state(store, window) {
                                fx.scope(crate::app::AppCommand::Verb, |fx| {
                                    documents::lanes::sync_document_watches(
                                        store,
                                        state.documents(),
                                        fx,
                                    )
                                });
                                fx.scope(AppCommand::Verb, |fx| {
                                    documents::lanes::sync_stripe_bases(
                                        store,
                                        state.documents(),
                                        ui,
                                        fx,
                                    )
                                });
                            }
                        }
                        ModalRequest::OpenLocations(locations) => {
                            ::workbench::window::Windows::put(store, window, entity);
                            crate::workspace::open_locations(store, ui, window, &locations, fx);
                        }
                        ModalRequest::SelectWidget(widget) => {
                            entity.mount_focused(store, widget);
                            ::workbench::window::Windows::put(store, window, entity);
                        }
                    }
                }
                match panel_request {
                    Some(hikit::panel::PanelRequest::OpenLocations(locations)) => {
                        crate::workspace::open_locations(store, ui, window, &locations, fx);
                    }
                    Some(hikit::panel::PanelRequest::OpenDiff(old_side, new_side)) => {
                        self.perform(
                            store,
                            ui,
                            AppCommand::Windowed(
                                window,
                                std::sync::Arc::new(crate::hichanges::OpenDiffForPair {
                                    old: old_side,
                                    new: new_side,
                                }),
                            ),
                            fx,
                        );
                    }
                    Some(hikit::panel::PanelRequest::OpenAt(location, target)) => {
                        self.perform(
                            store,
                            ui,
                            AppCommand::Windowed(
                                window,
                                std::sync::Arc::new(crate::diff_canvas::OpenCanvasFile {
                                    location,
                                    target,
                                }),
                            ),
                            fx,
                        );
                    }
                    Some(hikit::panel::PanelRequest::Perform(command)) => {
                        self.perform(store, ui, AppCommand::Verb(Verb::Dynamic(command)), fx);
                    }
                    Some(hikit::panel::PanelRequest::Shell(payload)) => {
                        match payload
                            .downcast_ref::<std::sync::Arc<dyn crate::commands::WindowedCommand>>()
                        {
                            Some(command) => {
                                let command = command.clone();
                                self.perform(store, ui, AppCommand::Windowed(window, command), fx);
                            }
                            None => eprintln!("[app] an unknown panel shell ask was dropped"),
                        }
                    }
                    None => {}
                }
                match toolbar_request {
                    Some(::workbench::toolbar::ToolbarRequest::Command(id)) => {
                        if let Some(command) =
                            crate::commands::Commands::of(store).find(id).cloned()
                        {
                            self.perform(store, ui, AppCommand::Windowed(window, command), fx);
                        }
                    }
                    None => {}
                }
                if let Some(id) = dock_command {
                    if let Some(command) = crate::commands::Commands::of(store).find(id).cloned() {
                        self.perform(store, ui, AppCommand::Windowed(window, command), fx);
                    }
                }

                for request in crate::commands::AppRequests::drain(store) {
                    self.perform(store, ui, AppCommand::Windowed(window, request), fx);
                }
                for request in imba::command::Requests::drain(store) {
                    self.perform(store, ui, AppCommand::Verb(Verb::Dynamic(request)), fx);
                }
            }
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
            AppCommand::Windowed(window, command) => {
                command.perform(store, &ui, window, fx);
                // Deferred requests must not wait for the next CONTENT
                // command — an idle window (nothing painted, nothing
                // clicked) would starve them forever.
                for request in crate::commands::AppRequests::drain(store) {
                    self.perform(store, ui, AppCommand::Windowed(window, request), fx);
                }
                for request in imba::command::Requests::drain(store) {
                    self.perform(store, ui, AppCommand::Verb(Verb::Dynamic(request)), fx);
                }
            }
            AppCommand::Register(command) => {
                crate::commands::Commands::register(store, command);
            }
            AppCommand::FileChanged(subscription) => {
                // The border road: the event names only a subscription;
                // its documents collection is found once, by content.
                if let Some(documents) =
                    ahp_session::session::state::Hosts::documents_of_watch(store, subscription)
                {
                    fx.scope(crate::app::AppCommand::Verb, |fx| {
                        documents::lanes::refetch_watched(store, documents, subscription, fx)
                    });
                }

                self.pending_file_events.push(subscription);
            }
            AppCommand::Opened(window, opened) => {
                // The landing files into the collection stamped at
                // launch — never the window's CURRENT session, which
                // may have switched while the open was in flight.
                let documents = opened.documents;
                let document = opened.document;
                let saved_revision = document.revision();

                let document_id = match opened
                    .location
                    .as_ref()
                    .and_then(|location| OpenDocuments::by_location(store, documents, location))
                {
                    Some(existing) => existing,
                    None => OpenDocuments::register(
                        store,
                        documents,
                        document.clone(),
                        opened.location,
                        opened.name,
                        saved_revision,
                    ),
                };

                fx.scope(crate::app::AppCommand::Verb, |fx| {
                    documents::lanes::sync_document_watches(store, documents, fx)
                });
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_stripe_bases(store, documents, ui, fx)
                });

                if opened.primary {
                    if let Some(mut entity) = ::workbench::window::Windows::window(store, window) {
                        fx.scope(AppCommand::Verb, |fx| {
                            entity.show_document(
                                store,
                                ui,
                                window,
                                document_id,
                                opened.target,
                                opened.focus,
                                fx,
                            )
                        });
                        ::workbench::window::Windows::put(store, window, entity);
                    }
                }
            }
            AppCommand::OpenAsync {
                window,
                name,
                primary,
                location,
                build,
            } => {
                // The one synchronous moment of this road: bind the
                // gesture's session here, before anything is in flight.
                let documents = crate::workspace::session_state(store, window)
                    .expect("a document opens into a window with a session")
                    .documents();
                fx.push(open_effect(
                    window, documents, name, primary, location, build,
                ));
            }
            AppCommand::OpenPanel(window, panel) => {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                entity.open_panel(store, ui, panel, fx);
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::OpenModal(window, modal) => {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.show_modal(store, modal, fx),
                );
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::CloseModal(window) => {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.dismiss_modal(store, fx),
                );
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::ViewportResized(window, size) => {
                let mut entity =
                    ::workbench::window::Windows::window(store, window).expect("the window entity");
                entity.set_viewport_size(size);
                ::workbench::window::Windows::put(store, window, entity);
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
