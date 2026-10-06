// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The editor pane's services, behind the workbench's one face
//! (`workbench::services::PaneServices`): the FIND BAR and the
//! completion popups (LSP in code, `@`-paths in markdown). The
//! workbench holds the state as an opaque box and routes events; what
//! the box IS — and every ahp consult it makes — lives here.

use std::sync::Arc;

use imba::dyn_view::DynCommand;
use imba::effect::Effects;
use imba::store::Store;
use imba::thunk_ext::ThunkExt as _;
use imba::ui::UiCtx;
use workbench::services::{PaneServices, ServiceState, ServiceTarget};
use workbench::workbench_node::{PaneSlot, PanelCommand};

use ahp_chat::completion::{Completion, CompletionFound};
use findbar::{FindBar, FindCommand};

/// One leaf's services: the bar when ⌘F stood one, the completion
/// always ready to pop.
#[derive(Clone)]
#[doc(hidden)]
pub struct EditorServices {
    pub find: Option<FindBar>,
    pub completion: Completion,
}

impl Default for EditorServices {
    fn default() -> Self {
        Self {
            find: None,
            completion: Completion::new(),
        }
    }
}

impl EditorServices {
    /// The leaf's seat, minted on demand — then downcast to the one
    /// state this shell installs. The himark-side drivers (⌘F, the
    /// completion trigger) reach the state through here.
    /// The read side — no minting, no store.
    #[doc(hidden)]
    pub fn of_ref(slot: &PaneSlot) -> Option<&EditorServices> {
        slot.services
            .as_ref()?
            .state
            .downcast_ref::<EditorServices>()
    }

    pub fn of<'s>(slot: &'s mut PaneSlot, store: &Store) -> Option<&'s mut EditorServices> {
        if !slot.ensure_services(store) {
            return None;
        }
        slot.services
            .as_mut()?
            .state
            .downcast_mut::<EditorServices>()
    }
}

/// The service commands, erased into `PanelCommand::Service`.
#[derive(Clone)]
pub(crate) enum ServiceCommand {
    Find(FindCommand),
    CompletionFound(CompletionFound),
}

impl std::fmt::Display for ServiceCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceCommand::Find(command) => command.fmt(out),
            ServiceCommand::CompletionFound(_) => out.write_str("completion found"),
        }
    }
}

fn wrap_editor(command: ::editor::editor_view::EditorCommand) -> PanelCommand {
    PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
}

fn wrap_find(command: FindCommand) -> PanelCommand {
    PanelCommand::Service(DynCommand::new(ServiceCommand::Find(command)))
}

fn wrap_found(found: CompletionFound) -> PanelCommand {
    PanelCommand::Service(DynCommand::new(ServiceCommand::CompletionFound(found)))
}

/// The installed face.
pub(crate) struct EditorServicesFace;

impl PaneServices for EditorServicesFace {
    fn mint(&self) -> ServiceState {
        Box::new(EditorServices::default())
    }

    fn clone_state(&self, state: &ServiceState) -> ServiceState {
        match state.downcast_ref::<EditorServices>() {
            Some(services) => Box::new(services.clone()),
            None => Box::new(EditorServices::default()),
        }
    }

    fn focus_data<'w>(
        &self,
        state: &'w ServiceState,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> Option<imba::focus::FocusData<'w, DynCommand>> {
        let services = state.downcast_ref::<EditorServices>()?;
        let find = services.find.as_ref().filter(|find| find.focused)?;
        Some(
            find.focus_data(store, ui)
                .map(|command| DynCommand::new(ServiceCommand::Find(command))),
        )
    }

    fn command(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut Effects<'_, PanelCommand>,
    ) {
        let Some(services) = state.downcast_mut::<EditorServices>() else {
            return;
        };
        match command.downcast::<ServiceCommand>() {
            Some(ServiceCommand::Find(command)) => {
                services.perform_find(store, ui, target, command, fx)
            }
            Some(ServiceCommand::CompletionFound(found)) => {
                services.land_completion(store, ui, target, found)
            }
            None => {}
        }
    }

    fn intercept(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        command: PanelCommand,
        fx: &mut Effects<'_, PanelCommand>,
    ) -> Option<PanelCommand> {
        let services = state.downcast_mut::<EditorServices>()?;
        services.intercept_completion(store, ui, target, command, fx)
    }

    fn sync(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        inserted: Option<&str>,
        fx: &mut Effects<'_, PanelCommand>,
    ) {
        let Some(services) = state.downcast_mut::<EditorServices>() else {
            return;
        };
        services.sync_find(store, ui, target, fx);
        services.sync_completion(store, ui, target, inserted, fx);
    }

    fn defocus(&self, state: &mut ServiceState) {
        if let Some(find) = state
            .downcast_mut::<EditorServices>()
            .and_then(|services| services.find.as_mut())
        {
            find.focused = false;
        }
    }

    fn bar<'a>(
        &self,
        state: &'a ServiceState,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> Option<(f32, imba::ThunkBox<'a, DynCommand>)> {
        let services = state.downcast_ref::<EditorServices>()?;
        let find = services.find.as_ref()?;
        let chrome = ::editor::env::Themes::of(store).ui().search.clone();
        let height = FindBar::height(&chrome);
        let widget = imba::ThunkBox::new(
            arena,
            find.layout(arena, store, ui, width)
                .map(|command| DynCommand::new(ServiceCommand::Find(command))),
        );
        Some((height, widget))
    }
}

/// Stand the face in the registry — the boot ceremony's one line.
pub fn install(store: &mut Store) {
    ::workbench::registry::Registry::update(store, |registry| {
        registry.pane_services = Some(Arc::new(EditorServicesFace));
    });
}

impl EditorServices {
    pub(crate) fn sync_find(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        target: ServiceTarget,
        fx: &mut Effects<'_, PanelCommand>,
    ) {
        let Some(documents) = target.documents else {
            return;
        };
        let Some(find) = &mut self.find else {
            return;
        };
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        fx.scope(wrap_editor, |fx| {
            find.sync(store, documents, target.target, ui, &fonts, &theme, fx)
        });
        fx.scope(wrap_find, |fx| {
            find.launch(store, documents, target.target, fx, FindCommand::Scanned)
        });
    }

    fn perform_find(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        target: ServiceTarget,
        command: FindCommand,
        fx: &mut Effects<'_, PanelCommand>,
    ) {
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        match command {
            FindCommand::Input(command) => {
                if let Some(find) = &mut self.find {
                    fx.scope(wrap_find, |fx| find.perform_input(store, ui, command, fx));
                }
                self.sync_find(store, ui, target, fx);
            }
            FindCommand::Next | FindCommand::Previous => {
                let forward = matches!(command, FindCommand::Next);
                self.sync_find(store, ui, target, fx);
                let Some(documents) = target.documents else {
                    return;
                };
                if let Some(find) = &mut self.find {
                    fx.scope(wrap_editor, |fx| {
                        find.step(store, documents, forward, ui, &fonts, &theme, fx)
                    });
                }
            }
            FindCommand::Close => {
                let Some(documents) = target.documents else {
                    return;
                };
                if let Some(mut find) = self.find.take() {
                    fx.scope(wrap_editor, |fx| {
                        find.uninstall(store, documents, ui, &fonts, &theme, fx)
                    });
                }
            }
            FindCommand::Scanned(landed) => {
                let Some(documents) = target.documents else {
                    return;
                };
                if let Some(find) = &mut self.find {
                    fx.scope(wrap_editor, |fx| {
                        find.adopt(
                            store,
                            documents,
                            target.target,
                            &landed,
                            ui,
                            &fonts,
                            &theme,
                            fx,
                        )
                    });
                }
            }
        }
    }

    fn intercept_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        target: ServiceTarget,
        command: PanelCommand,
        fx: &mut Effects<'_, PanelCommand>,
    ) -> Option<PanelCommand> {
        use imba::scroll::ScrollCommand;
        let PanelCommand::Editor(ScrollCommand::Content(
            ::editor::editor_view::EditorCommand::Inlay {
                key,
                command: inlay,
            },
        )) = command
        else {
            return Some(command);
        };
        let rewrap = |inlay| {
            PanelCommand::Editor(ScrollCommand::Content(
                ::editor::editor_view::EditorCommand::Inlay {
                    key,
                    command: inlay,
                },
            ))
        };
        if Some(key) != self.completion.inlay_key() {
            return Some(rewrap(inlay));
        }
        let popup = match inlay.downcast_ref::<ahp_chat::completion::CompletionCommand>() {
            Some(_) => inlay
                .downcast::<ahp_chat::completion::CompletionCommand>()
                .expect("probed above"),
            None => return Some(rewrap(inlay)),
        };
        use ahp_chat::completion::CompletionCommand;
        let Some((id, editor)) = self.completion.installed() else {
            return None;
        };
        let Some(documents) = target.documents else {
            return None;
        };
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
            self.completion.clear();
            return None;
        };
        match popup {
            CompletionCommand::Select(delta) => {
                self.completion.select(store, &mut document, editor, delta);
            }
            CompletionCommand::PickCursor => {
                let row = self.completion.selected();
                let _ = self.completion.apply_pick(
                    store,
                    ui,
                    &mut document,
                    editor,
                    row,
                    fx,
                    wrap_editor,
                );
            }
            CompletionCommand::Rows(rows) => {
                let picked = self
                    .completion
                    .rows_command(store, ui, &mut document, editor, rows);
                if let Some(row) = picked {
                    let _ = self.completion.apply_pick(
                        store,
                        ui,
                        &mut document,
                        editor,
                        row,
                        fx,
                        wrap_editor,
                    );
                }
            }
            CompletionCommand::Close => {
                self.completion
                    .drop_state(&mut document, store, ui, fx, wrap_editor);
            }
        }
        documents::OpenDocuments::put_document(store, documents, id, document);
        None
    }

    fn sync_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        target: ServiceTarget,
        inserted: Option<&str>,
        fx: &mut Effects<'_, PanelCommand>,
    ) {
        let Some(documents) = target.documents else {
            return;
        };
        if let Some(installed) = self.completion.installed() {
            if target.target != Some(installed) {
                match documents::OpenDocuments::document(store, documents, installed.0) {
                    Some(mut old) => {
                        self.completion
                            .drop_state(&mut old, store, ui, fx, wrap_editor);
                        documents::OpenDocuments::put_document(store, documents, installed.0, old);
                    }
                    None => self.completion.clear(),
                }
            }
        }
        let Some((id, editor)) = target.target else {
            return;
        };
        if !self.completion.open() && inserted.is_none() {
            return;
        }
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
            return;
        };

        let markdown = document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
        if markdown {
            // The lists collection is the documents' sibling — a
            // catalog consult at a pane border, the save.rs/fsroute
            // debt class; the gating keeps this one boot-global.
            let Some((session, state)) =
                ahp_session::session::state::Hosts::home_of_documents(store, documents)
            else {
                documents::OpenDocuments::put_document(store, documents, id, document);
                return;
            };
            let typed_at =
                (inserted == Some("@")).then(|| document.caret_byte(editor).saturating_sub(1));
            self.completion.sync_path(
                store,
                ui,
                &mut document,
                editor,
                typed_at,
                std::sync::Arc::new(ahp_session::session::folders::session_folders(
                    store, &session,
                )),
                state.recents(),
                Some((id, editor)),
                fx,
                wrap_found,
                wrap_editor,
            );
        } else {
            match documents::OpenDocuments::location(store, documents, id) {
                Some(location) if !location.is_synthetic() => {
                    self.completion.sync_lsp(
                        store,
                        ui,
                        &mut document,
                        editor,
                        inserted,
                        false,
                        &location,
                        Some((id, editor)),
                        fx,
                        wrap_found,
                        wrap_editor,
                    );
                }
                _ if self.completion.open() => {
                    self.completion
                        .drop_state(&mut document, store, ui, fx, wrap_editor);
                }
                _ => {}
            }
        }
        documents::OpenDocuments::put_document(store, documents, id, document);
    }

    pub(crate) fn land_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        target: ServiceTarget,
        found: CompletionFound,
    ) {
        let Some((id, editor)) = self.completion.installed() else {
            return;
        };
        let Some(documents) = target.documents else {
            return;
        };
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
            self.completion.clear();
            return;
        };
        self.completion
            .land(store, ui, &mut document, editor, found);
        documents::OpenDocuments::put_document(store, documents, id, document);
    }
}
