// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench's BUILT-IN commands — panes, navigation, themes,
//! the chat composer: the registered set `register_builtins`
//! installs at boot. Window-shaped by nature; the engine only ever
//! sees them as `Arc<dyn HimarkCommand>`.

use std::sync::Arc;

use himark::app::{AppCommand, AppFx};
use himark::commands::{Commands, HimarkCommand};
use imba::store::Store;

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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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

pub use himark::form::{theme_is_dark, Form};

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
        let next = Form::of(store).theme(!theme_is_dark(store));
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
        let theme = Form::of(store).theme(self.dark);
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
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
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
        let Some(entity) = ::workbench::window::Windows::window_ref(store, window) else {
            return;
        };
        let current = crate::workspace::entity_session(&entity);
        if ahp_wire::client::Servers::client(store, current.host).is_none() {
            return;
        }
        crate::folder_grant_windows::AddSessionFolders {
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
    ::workbench::navigation::Navigators::register(store, ::canvas::faces::ScrolledCanvasNavigator);
    ::workbench::rows::RowMinters::register(store, ::terminals::pane::terminal_row_minter());

    Commands::register(
        store,
        Arc::new(crate::higent::open_session::OpenNewSession { host: None }),
    );
}
