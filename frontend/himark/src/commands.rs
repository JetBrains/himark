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
    let data = crate::focus::window_focus_data(store, ui, window);
    let bound = data.as_ref().map(|data| data.bindings.clone());
    let mut commands: Vec<imba::PresentableCommand<AppCommand>> =
        data.map(|data| data.commands).unwrap_or_default();
    commands.extend(Commands::of(store).iter().map(|command| {
        imba::PresentableCommand::new(
            command.id(),
            command.name(),
            // The walk's facts ride the mint; without a walk (no
            // window entity) the windowed lane resolves them at
            // dispatch — the mounted fork binds the origin's session.
            match &bound {
                Some(bound) => crate::shell::bound(bound.clone(), command.clone()),
                None => crate::shell::windowed(window, command.clone()),
            },
        )
    }));
    commands
}

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

/// The chrome prelude: the window this gesture came from, or a loud
/// drop — the dock toggles and pane openers keep needing one.
pub fn bound_window(
    command: &str,
    bound: &imba::focus::Bindings,
) -> Option<::workbench::window::WindowId> {
    let window = bound.bound::<::workbench::window::WindowId>().copied();
    if window.is_none() {
        eprintln!("[shell] a window-taking command landed with no window bound — dropped: {command}");
    }
    window
}

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
        .or_else(|| {
            bound
                .bound::<::workbench::window::WindowId>()
                .map(|window| crate::bridge::ViewBridgeId::from_raw(window.raw()))
        });
    if origin.is_none() {
        eprintln!("[shell] a navigating command landed with no origin bound — dropped: {command}");
    }
    origin
}

/// The session prelude: the session state the UI bound, or a loud
/// drop.
pub fn bound_session(
    command: &str,
    bound: &imba::focus::Bindings,
) -> Option<ahp_session::session::state::SessionState> {
    let state = bound
        .bound::<ahp_session::session::state::SessionState>()
        .cloned();
    if state.is_none() {
        eprintln!("[shell] a session command landed with no session bound — dropped: {command}");
    }
    state
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

    pub(crate) fn drain(store: &mut Store) -> Vec<Arc<dyn HimarkCommand>> {
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

impl HimarkCommand for NewScratch {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let state = crate::workspace::session_state(store, window)
            .expect("a scratch opens into a window with a session");
        let location = documents::next_scratch_location(store, state.scratch_names());
        fx.push(crate::app::open_effect(
            window,
            state.documents(),
            location.name().to_owned(),
            true,
            Some(location),
            Box::new(|_, _, _| crate::app::markdown_scratch()),
        ));
    }
}

pub(crate) struct SplitPane;

impl HimarkCommand for SplitPane {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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

impl HimarkCommand for CloseFocused {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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

impl HimarkCommand for ClosePane {
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
        bound: &imba::focus::Bindings,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        let _ = entity.close_current(store);
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct NavigateBack;

impl HimarkCommand for NavigateBack {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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

impl HimarkCommand for NavigateForward {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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

impl HimarkCommand for ToggleTheme {
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
        _bound: &imba::focus::Bindings,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let next = match ::editor::env::Themes::of(store).name() {
            "light" => ::editor::theme::embedded(),
            _ => ::editor::theme::light(),
        };
        ::editor::env::Themes::set(store, next);
    }
}

/// Selects an explicit theme, unlike [ToggleTheme] — the host calls this to
/// follow the OS appearance (at startup and when the system theme changes).
pub(crate) struct SetTheme {
    pub(crate) dark: bool,
}

impl HimarkCommand for SetTheme {
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
        _bound: &imba::focus::Bindings,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let theme = if self.dark {
            ::editor::theme::embedded()
        } else {
            ::editor::theme::light()
        };
        ::editor::env::Themes::set(store, theme);
    }
}

pub(crate) struct ChatComposer;

impl HimarkCommand for ChatComposer {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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

impl HimarkCommand for AddFolder {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
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
        .perform(store, ui, bound, fx);
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
    // The WINDOWED canvas: wrapped in the shared top scroller — a
    // BUILTIN, so every windowed shell (the tests' included) seats
    // a canvas that can scroll; the bare navigator is the mounted
    // road's (docs/mounted-parity.md).
    ::workbench::navigation::Navigators::register(store, ::canvas::canvas::ScrolledCanvasNavigator);
    ::workbench::rows::RowMinters::register(store, ::terminals::pane::terminal_row_minter());

    Commands::register(
        store,
        Arc::new(crate::higent::open_session::OpenNewSession { host: None }),
    );
}
