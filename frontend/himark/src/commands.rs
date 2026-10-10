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

    pub fn register(store: &mut Store, command: Arc<dyn WindowedCommand>) {
        crate::registry::Registry::update(store, |registry| registry.commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn WindowedCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn WindowedCommand>> {
        self.0.iter()
    }
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
        let state = crate::workspace::session_state(store, window)
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(AppCommand::Verb, |fx| {
            entity.front_chat(store, ui, fx);
        });
        // Fronting alone leaves the view on whatever area it last had —
        // ⌘I means "type here", so point it at the composer.
        if let Some(chat) = entity.workbench().chat() {
            if let ::workbench::workbench_node::Panel::Plugin(view) = chat.panel() {
                if let Some(pane) = view.as_any().downcast_ref::<ahp_chat::chats::ChatPane>() {
                    pane.focus_composer(store);
                }
            }
        }
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
        let current = crate::workspace::entity_session(&entity);
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
        Arc::new(crate::higent::open_session::OpenNewSession { host: None }),
    );
}
