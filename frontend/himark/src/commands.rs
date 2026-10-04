// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

use crate::app::AppCommand;

pub fn palette_commands(
    store: &Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
) -> Vec<imba::PresentableCommand<AppCommand>> {
    // A STATE WALK over the views — no tree is built for the palette.
    let mut commands: Vec<imba::PresentableCommand<AppCommand>> =
        crate::focus::window_focus_data(store, ui, window)
            .map(|data| data.commands)
            .unwrap_or_default();
    commands.extend(Commands::of(store).iter().map(|command| {
        imba::PresentableCommand::new(
            command.id(),
            command.name(),
            AppCommand::Windowed(window, command.clone()),
        )
    }));
    commands
}

/// The WINDOWED command: a palette entry or a deferred landing that
/// acts in a window and speaks the app's command stream. The
/// windowless kind is `imba::command::DynamicCommand` — collections
/// and views ride that one; this trait is shell-side only and
/// shrinks away as gestures go windowless (content-addressed opens
/// are the recorded road).
pub trait WindowedCommand: Send + Sync {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    );
}

#[derive(Clone, Default)]
pub struct Commands(pub(crate) Vec<Arc<dyn WindowedCommand>>);

impl Commands {
    pub fn of(store: &Store) -> Commands {
        crate::registry::Registry::of(store)
            .map(|registry| registry.commands.clone())
            .unwrap_or_default()
    }

    pub(crate) fn register(store: &mut Store, command: Arc<dyn WindowedCommand>) {
        crate::registry::Registry::update(store, |registry| registry.commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn WindowedCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn WindowedCommand>> {
        self.0.iter()
    }
}

/// A window-coupled deferred ask carried through the kit's panel
/// requests — the drain adds the window.
pub fn shell_ask(command: Arc<dyn WindowedCommand>) -> hikit::panel::PanelRequest {
    hikit::panel::PanelRequest::Shell(Arc::new(command))
}

#[derive(Clone, Default)]
pub struct AppRequests(Vec<Arc<dyn WindowedCommand>>);

impl AppRequests {
    pub fn push(store: &mut Store, request: Arc<dyn WindowedCommand>) {
        store.update::<AppRequests>(|requests| requests.0.push(request));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn drain(store: &mut Store) -> Vec<Arc<dyn WindowedCommand>> {
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

pub(crate) struct NewScratch;

impl WindowedCommand for NewScratch {
    fn id(&self) -> &'static str {
        "workbench.new-document"
    }
    fn name(&self) -> String {
        "New Document".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let state = ::workbench::window::Windows::session_state(store, window)
            .expect("a scratch opens into a window with a session");
        let location = documents::next_scratch_location(store, state.scratch_names());
        fx.push(crate::app::open_effect(
            window,
            state.documents(),
            location.name().to_owned(),
            true,
            Some(location),
            Box::new(|_, _, _, _| crate::app::markdown_scratch()),
        ));
    }
}

pub(crate) struct SplitPane;

impl WindowedCommand for SplitPane {
    fn id(&self) -> &'static str {
        "workbench.split-pane"
    }
    fn name(&self) -> String {
        "Split Pane".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            entity.split_current(store, ui, fx);
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct CloseFocused;

impl WindowedCommand for CloseFocused {
    fn id(&self) -> &'static str {
        "workbench.close"
    }
    fn name(&self) -> String {
        "Close".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            let _ = entity.close_focused_widget(store, ui, window, fx);
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct ClosePane;

impl WindowedCommand for ClosePane {
    fn id(&self) -> &'static str {
        "workbench.close-pane"
    }
    fn name(&self) -> String {
        "Close Pane".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        let _ = entity.close_current(store);
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct NavigateBack;

impl WindowedCommand for NavigateBack {
    fn id(&self) -> &'static str {
        "navigation.back"
    }
    fn name(&self) -> String {
        "Go Back".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            let _ = entity.navigate_back(store, ui, window, fx);
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct NavigateForward;

impl WindowedCommand for NavigateForward {
    fn id(&self) -> &'static str {
        "navigation.forward"
    }
    fn name(&self) -> String {
        "Go Forward".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            let _ = entity.navigate_forward(store, ui, window, fx);
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct ToggleTheme;

impl WindowedCommand for ToggleTheme {
    fn id(&self) -> &'static str {
        "theme.toggle"
    }
    fn name(&self) -> String {
        "Toggle Light/Dark Theme".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let next = match ::editor::env::Themes::of(store).name() {
            "light" => ::editor::theme::Theme::embedded(),
            _ => ::editor::theme::Theme::light(),
        };
        ::editor::env::Themes::set(store, next);
    }
}

/// Selects an explicit theme, unlike [ToggleTheme] — the host calls this to
/// follow the OS appearance (at startup and when the system theme changes).
pub(crate) struct SetTheme {
    pub(crate) dark: bool,
}

impl WindowedCommand for SetTheme {
    fn id(&self) -> &'static str {
        if self.dark {
            "theme.dark"
        } else {
            "theme.light"
        }
    }
    fn name(&self) -> String {
        if self.dark {
            "Use Dark Theme".to_owned()
        } else {
            "Use Light Theme".to_owned()
        }
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let theme = if self.dark {
            ::editor::theme::Theme::embedded()
        } else {
            ::editor::theme::Theme::light()
        };
        ::editor::env::Themes::set(store, theme);
    }
}

pub(crate) struct CompletionTrigger;

impl WindowedCommand for CompletionTrigger {
    fn id(&self) -> &'static str {
        "completion.trigger"
    }
    fn name(&self) -> String {
        "Trigger Completion".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            let Some((id, editor)) = slot.find_target() else {
                ::workbench::window::Windows::put(store, window, entity);
                return;
            };
            let documents = ::workbench::window::Windows::session_state(store, window)
                .expect("completion runs in a window with a session")
                .documents();
            let location = documents::OpenDocuments::location(store, documents, id)
                .filter(|location| !location.is_synthetic());
            let Some(location) = location else {
                ::workbench::window::Windows::put(store, window, entity);
                return;
            };
            let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
                ::workbench::window::Windows::put(store, window, entity);
                return;
            };
            let markdown =
                document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
            if !markdown {
                                let Some(documents) = slot.documents_id() else {
                    return;
                };
                slot.completion.sync_lsp(
                    store,
                    &ui,
                    &mut document,
                    editor,
                    None,
                    true,
                    &location,
                    Some((id, editor)),
                    fx,
                    move |found| {
                        crate::app::AppCommand::Windowed(window, Arc::new(CompletionLanded(found)))
                    },
                    move |command| {
                        AppCommand::at(documents, documents::DocumentsCommand::Editor(id, command))
                    },
                );
            }
            documents::OpenDocuments::put_document(store, documents, id, document);
        }
        ::workbench::window::Windows::put(store, window, entity);
    }
}

struct CompletionLanded(ahp_chat::completion::CompletionFound);

impl WindowedCommand for CompletionLanded {
    fn id(&self) -> &'static str {
        "completion.landed"
    }
    fn name(&self) -> String {
        "Completion Landed".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let _ = fx;
                let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        entity.workbench_mut().root.for_each_slot_mut(&mut |slot| {
            let mut discarded = imba::effect::Batch::new();
            slot.land_completion(store, &ui, self.0.clone(), &mut discarded.effects());
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct FindOpen;

impl WindowedCommand for FindOpen {
    fn id(&self) -> &'static str {
        "find.open"
    }
    fn name(&self) -> String {
        "Find in Document".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            if slot.panel.editor().is_some() {
                let seed = slot.find_target().and_then(|(document_id, editor)| {
                    let documents = slot.documents_id()?;
                    let document =
                        documents::OpenDocuments::document_ref(store, documents, document_id)?;
                    let caret = document.carets(editor).primary();
                    if !caret.has_selection() {
                        return None;
                    }
                    let selection = caret.selection();
                    if selection.end - selection.start > 200 {
                        return None;
                    }
                    let text = document.text().view().substring(selection);
                    (!text.contains('\n')).then_some(text)
                });
                let ui = ui;
                match (&mut slot.find, seed) {
                    (Some(find), Some(seed)) => find.seed(store, ui, &seed),
                    (Some(find), None) => find.refocus(),
                    (None, seed) => {
                        let mut find = ::workbench::find::FindBar::new(store, ui);
                        if let Some(seed) = &seed {
                            find.seed(store, ui, seed);
                        }
                        slot.find = Some(find);
                    }
                }
                find_sync_slot(slot, ui, store, window, fx);
            }
        }
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct FindStep(pub bool);

impl WindowedCommand for FindStep {
    fn id(&self) -> &'static str {
        match self.0 {
            true => "find.next",
            false => "find.previous",
        }
    }
    fn name(&self) -> String {
        match self.0 {
            true => "Find Next",
            false => "Find Previous",
        }
        .to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            let target = slot.find_target();
            let slot_documents = slot.documents_id();
            if let (Some(find), Some((document, _))) = (&mut slot.find, target) {
                let fonts = ::editor::env::ui_collection(store, &ui);
                let theme = ::editor::env::Themes::of(store);
                let forward = self.0;
                let Some(documents) = slot_documents else {
                    return;
                };
                fx.scope(
                    move |command| {
                        crate::app::AppCommand::at(
                            documents,
                            documents::DocumentsCommand::Editor(document, command),
                        )
                    },
                    |fx| {
                        find.sync(store, documents, target, &ui, &fonts, &theme, fx);
                        find.step(store, documents, forward, &ui, &fonts, &theme, fx);
                    },
                );
            }
        }
        ::workbench::window::Windows::put(store, window, entity);
    }
}

fn find_sync_slot(
    slot: &mut ::workbench::workbench_node::PaneSlot,
    ui: &imba::ui::UiCtx,
    store: &mut Store,
    window: ::workbench::window::WindowId,
    fx: &mut crate::app::AppFx<'_>,
) {
    let target = slot.find_target();
    let slot_documents = slot.documents_id();
    let Some(find) = &mut slot.find else {
        return;
    };
    let Some((document, _)) = target else {
        return;
    };
        let fonts = ::editor::env::ui_collection(store, &ui);
    let theme = ::editor::env::Themes::of(store);
    let Some(documents) = slot_documents else {
        return;
    };
    fx.scope(
        move |command| {
            crate::app::AppCommand::at(
                documents,
                documents::DocumentsCommand::Editor(document, command),
            )
        },
        |fx| find.sync(store, documents, target, &ui, &fonts, &theme, fx),
    );

    find.launch(store, documents, target, fx, move |scan| {
        crate::app::AppCommand::Windowed(window, Arc::new(FindScanLanded(scan)))
    });
}

struct FindScanLanded(::workbench::find::Scan);

impl WindowedCommand for FindScanLanded {
    fn id(&self) -> &'static str {
        "find.scan-landed"
    }
    fn name(&self) -> String {
        "Find Scan Landed".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
                let fonts = ::editor::env::ui_collection(store, &ui);
        let theme = ::editor::env::Themes::of(store);
        entity.workbench_mut().root.for_each_slot_mut(&mut |slot| {
            let target = slot.find_target();
            let slot_documents = slot.documents_id();
            let Some(find) = &mut slot.find else {
                return;
            };
            let Some((document, _)) = target else {
                return;
            };
            let Some(documents) = slot_documents else {
                return;
            };
            fx.scope(
                move |command| {
                    crate::app::AppCommand::at(
                        documents,
                        documents::DocumentsCommand::Editor(document, command),
                    )
                },
                |fx| find.adopt(store, documents, target, &self.0, &ui, &fonts, &theme, fx),
            );
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct ChatComposer;

impl WindowedCommand for ChatComposer {
    fn id(&self) -> &'static str {
        "chat.composer"
    }
    fn name(&self) -> String {
        "Chat Input".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            entity.front_chat(store, ui, fx);
        });
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// `session.add-folder`, from the palette: grant the current agent
/// session another working folder — the composer button it replaces
/// is gone.
pub(crate) struct AddFolder;

impl WindowedCommand for AddFolder {
    fn id(&self) -> &'static str {
        "session.add-folder"
    }
    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(entity) = ::workbench::window::Windows::window_ref(store, window) else {
            return;
        };
        let current = entity.current_session();
        if ahp_wire::client::Servers::client(store, current.host).is_none() {
            return;
        }
        crate::higent::folder_grant::AddSessionFolders {
            server: current.host,
            session: current.session,
        }
        .perform(store, ui, window, fx);
    }
}

pub(crate) fn register_builtins(store: &mut Store) {
    Commands::register(store, Arc::new(FindOpen));
    Commands::register(store, Arc::new(CompletionTrigger));
    Commands::register(store, Arc::new(FindStep(true)));
    Commands::register(store, Arc::new(FindStep(false)));
    Commands::register(store, Arc::new(NewScratch));
    Commands::register(store, Arc::new(SplitPane));
    Commands::register(store, Arc::new(CloseFocused));
    Commands::register(store, Arc::new(ClosePane));
    Commands::register(store, Arc::new(NavigateBack));
    Commands::register(store, Arc::new(NavigateForward));
    Commands::register(store, Arc::new(crate::toc::ToggleToc));
    Commands::register(store, Arc::new(ToggleTheme));
    Commands::register(store, Arc::new(SetTheme { dark: true }));
    Commands::register(store, Arc::new(SetTheme { dark: false }));
    Commands::register(store, Arc::new(ChatComposer));
    Commands::register(store, Arc::new(AddFolder));

    // The panels himark itself owns answer navigation walks: a
    // recorded chat/terminal place must be able to walk back.
    ::workbench::navigation::Navigators::register(store, ahp_chat::chats::ChatNavigator);
    ::workbench::navigation::Navigators::register(store, ::terminals::pane::TerminalNavigator);
    ::workbench::rows::RowMinters::register(store, ::terminals::pane::terminal_row_minter());

    Commands::register(
        store,
        Arc::new(crate::new_session::OpenNewSession { host: None }),
    );
}
