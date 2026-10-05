// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::DocumentsCommand;
use imba::command::{Addressed, Verb};

use std::sync::Arc;
use std::sync::OnceLock;

use imba::{arena::Arena, constraints::Constraints, effect::{Effect, Effects}, event::{Event, EventResult}, scroll::ScrollView, store::Store, thunk_ext::ThunkExt, Thunk, ui::UiCtx, View, Widget};
use skia_safe::{Canvas, Rect, Size};
use text::text::Text;

use documents::lifecycle::mount_editor;
use documents::entity_view::EditorIdView;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use documents::OpenDocuments;
use ::workbench::workbench_node::Panel;
use ::workbench::window::Window;
use ::workbench::window::WindowId;
use ::workbench::window::Windows;
use ::workbench::workbench::Workbench;
use ::workbench::workbench_node::WorkbenchNode;
use editor::document::Document;
use editor::markup::Markup;

use crate::stats::{Stats, StatsCommand};

pub struct Application {
    /// THE store — one, global, always live. Hosts, Windows and
    /// Servers are residents like everything else; a batch borrows
    /// it, nothing is projected in or filed back.
    store: Store,

    pub(crate) ui: std::rc::Rc<UiCtx>,

    stats: Stats,
    pub(crate) ui_arena: Arena,

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    effects: crate::effects::EffectLauncher,

    handlers: Arc<crate::effects::Handlers>,

    workshop: Arc<::editor::env::Workshop>,

    pending_file_events: Vec<documents::watch::Subscription>,

    /// A perform raised the settle bit (`Effects::settle`): run the
    /// synchronous `Event::Settle` pulse before the next paint so
    /// viewport corrections land in the SAME frame
    /// (docs/editor/viewport-preservation.md §3.2).
    settle_requested: bool,
    settling: bool,
}

pub struct OpenedDocument {
    /// The collection the open was launched FOR — stamped at the
    /// gesture, so the landing files the document into the session the
    /// user acted in, not whatever the window shows by then.
    pub documents: imba::store::Id<OpenDocuments>,

    pub name: String,
    pub document: Document,

    pub location: Option<editor::location::ResourceLocation>,

    pub primary: bool,

    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,

    /// Move the keyboard to the opened editor (a deliberate jump —
    /// a click or Enter) — or just show it, leaving the keyboard
    /// where it stands (a selection move browsing results).
    pub focus: bool,
}

pub struct AppFonts {
    source: ::editor::FontSource,
}

pub type DocumentBuild = Box<
    dyn FnOnce(
            &imba::store::Store,
            &UiCtx,
            &skia_safe::textlayout::FontCollection,
            &::editor::theme::Theme,
        ) -> Document
        + Send
        + Sync,
>;

pub enum AppCommand {
    Content(WindowId, ::workbench::window::WindowCommand),

    Windowed(WindowId, std::sync::Arc<dyn crate::commands::WindowedCommand>),

    /// The app-level erased vocabulary (imba::command) — windowless:
    /// addressed entity commands, dynamic commands and one-shot
    /// landings that know collections by id and never a window.
    Verb(imba::command::Verb),

    Register(std::sync::Arc<dyn crate::commands::WindowedCommand>),
    Stats(StatsCommand),

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



/// Wrap a shell command for the verb lane — the opaque escape a kit
/// request rides when it must name the application (a window-coupled
/// gesture). Interpreted by the drains below, never by `Verb::run`.
pub fn shell_verb(command: AppCommand) -> Verb {
    Verb::Shell(Box::new(command))
}

/// A drained verb back into the app stream: shell payloads unwrap
/// (an `AppCommand`, or a deferred dynamic ask that takes the
/// draining window); everything else rides the Verb arm.
pub(crate) fn verb_command(window: WindowId, verb: Verb) -> Option<AppCommand> {
    match verb {
        Verb::Shell(payload) => match payload.downcast::<AppCommand>() {
            Ok(command) => Some(*command),
            Err(payload) => match payload.downcast::<std::sync::Arc<dyn crate::commands::WindowedCommand>>() {
                Ok(command) => Some(AppCommand::Windowed(window, *command)),
                Err(_) => {
                    eprintln!("[app] an unknown shell verb payload was dropped");
                    None
                }
            },
        },
        verb => Some(AppCommand::Verb(verb)),
    }
}

impl AppCommand {
    /// The one addressed-command constructor: every launch stamp and
    /// every landing re-wrap goes through here.
    pub fn at<T: imba::store::Entity>(id: imba::store::Id<T>, command: T::Command) -> AppCommand {
        AppCommand::At(Addressed::at(id, command))
    }
}

pub type AppEffects = imba::effect::Batch<AppCommand>;

pub type AppFx<'a> = Effects<'a, AppCommand>;



impl Application {
    pub fn ui_ctx(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }
}

impl AppFonts {
    pub fn new(source: ::editor::FontSource) -> Self {
        Self { source }
    }

    pub fn platform() -> Self {
        Self::new(hikit::fonts::source())
    }

    pub fn embedded() -> Self {
        Self::new(::editor::embedded_fonts::source())
    }

    pub fn source(&self) -> ::editor::FontSource {
        self.source.clone()
    }
}


pub(crate) fn fresh_workbench_root(
    store: &mut Store,
    state: &ahp_session::session::state::SessionState,
    ui: &UiCtx,
    fx: &mut AppFx<'_>,
) -> WorkbenchNode {
    let mut scratch = markdown_scratch();

    let documents = state.documents();
    let location = documents::next_scratch_location(store, state.scratch_names());
    let name = location.name().to_owned();
    ahp_chat::recents::RecentLocations::touch(store, state.recents(), &location);
    let scratch_id =
        OpenDocuments::register(store, documents, scratch.clone(), Some(location), name, 0);
    let width = ::workbench::workbench::fallback_pane_editor_width(store);
    let editor_id = fx.scope(
        move |command| AppCommand::at(documents, DocumentsCommand::Editor(scratch_id, command)),
        |fx| mount_editor(store, ui, &mut scratch, width, None, fx),
    );
    documents::scroll_stripes::enable_scroll_stripes(
        store,
        documents,
        scratch_id,
        &mut scratch,
        editor_id,
    );
    OpenDocuments::put_document(store, documents, scratch_id, scratch);
    WorkbenchNode::editor_leaf(ScrollView::new(
        EditorIdView::new(documents, scratch_id, editor_id).with_gutter(),
    ))
}

pub fn switch_session(
    store: &mut Store,
    window: WindowId,
    target: ahp_wire::SessionId,
    fx: &mut AppFx<'_>,
) {
    let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
        return;
    };
    if entity.current_session() == target {
        return;
    }
    fx.scope(
        move |command| AppCommand::Content(window, command),
        |fx| entity.dismiss_modal(store, fx),
    );
    fx.scope(
        move |command| AppCommand::Content(window, command),
        |fx| entity.dismiss_side_panel(store, fx),
    );

    // Session ENTRY: the one legitimate catalog consult — the bundle
    // is wired into the window's record here and read from it after.
    let state = ahp_session::session::state::Hosts::ensure_state(store, &target);
    let owed = entity.switch_to(target, state);
    ::workbench::window::Windows::put(store, window, entity);
    if let Some(previous) = owed {
        fx.follow_up(AppCommand::Windowed(
            window,
            std::sync::Arc::new(EnterFreshSession { previous }),
        ));
    }
}

struct EnterFreshSession {
    previous: ahp_wire::SessionId,
}

impl crate::commands::WindowedCommand for EnterFreshSession {
    fn id(&self) -> &'static str {
        "session.enter-fresh"
    }
    fn name(&self) -> String {
        "Enter Fresh Session".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        let _ = (ui, fx);
        // A fresh session starts with nothing open: the chat (once it
        // arrives) owns the whole workbench until a panel opens beside
        // it. Scratches are minted on demand (`workbench.new-document`).
        entity.install_fresh(
            self.previous.clone(),
            Workbench::new(WorkbenchNode::vacant()),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) fn markdown_scratch() -> Document {
    Document::new(Text::from_string_exact(""), Markup::new())
        .with_syntax(::editor::markup::Syntax::new("markdown", None, Markup::new()), &[])
}

impl Application {
    pub fn new(fonts: AppFonts) -> Self {
        let total_started = crate::startup_profile::start();
        let mut store = Store::new();

        let theme = ::editor::theme::Theme::embedded();
        store.put(::editor::env::Fonts(fonts.source()));
        store.put(::editor::env::Themes(theme.clone()));

        let ui = std::rc::Rc::new(UiCtx::dont_use_too_slow());
        ui.set(imba::ui::UiFonts((fonts.source())()));

        let scrollbar = &theme.ui().scrollbar;
        ui.set(imba::scroll::ScrollbarStyle {
            color: scrollbar.color.0,
            width: scrollbar.width,
            margin: scrollbar.margin,
            radius: scrollbar.radius,
            min_knob: scrollbar.min_knob,
            track_inset: scrollbar.track_inset,
        });

        let overlay_font = hikit::fonts::ui_text_font(&ui, theme.ui().stats.font_size);

        crate::commands::register_builtins(&mut store);
        // The baseline diff policy; outer edges override via
        // `register_diff_policy` (docs/editor/structural-diff.md).
        store.put(::editor::env::Differ(std::sync::Arc::new(myersdiff::Myers)));
        ::workbench::navigation::Navigators::register_windowed(&mut store, crate::workspace::EditorNavigator);
        ::workbench::registry::Registry::update(&mut store, |registry| {
            registry.run_command = Some(std::sync::Arc::new(|store, id| {
                if let Some(command) = crate::commands::Commands::of(store).find(id).cloned() {
                    crate::commands::AppRequests::push(store, command);
                }
            }));
            registry.outline_jump = Some(std::sync::Arc::new(|window, place| {
                hikit::modal::ModalRequest::Perform(crate::app::shell_verb(
                    crate::app::AppCommand::Windowed(
                        window,
                        std::sync::Arc::new(crate::toc::NavigateToPlace { place }),
                    ),
                ))
            }));
            registry.drawer_button = Some(crate::toc::toolbar_button());
        });
        crate::higent::chat_roads::install_shell_roads(&mut store);
        // The locations wash hook is no longer boot-global: the
        // session ceremony installs one per session, wired with its
        // lists collection (docs/entities.md law 4).

        let workshop = Arc::new(::editor::env::Workshop::new(
            ::editor::env::Fonts::of(&store),
            ::editor::env::Themes::of(&store),
        ));
        let handlers = Arc::new(crate::effects::Handlers::default());
        crate::effects::register_builtins(&handlers, &workshop);
        let mut store = store;
        store.put(ahp_session::session::state::Hosts::default());
        store.put(::workbench::window::Windows::default());
        store.put(ahp_wire::client::Servers::default());
        let application = Self {
            store,
            ui,
            stats: Stats::new(overlay_font),
            #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
            effects: crate::effects::EffectLauncher::new(),
            handlers,
            workshop,
            ui_arena: Arena::default(),
            pending_file_events: Vec::new(),
            settle_requested: false,
            settling: false,
        };
        crate::startup_profile::log("Application::new", total_started);
        application
    }

    /// The empty-session housekeeping, run after every batch — the
    /// one thing the old gather/scatter rite still did.
    fn sweep(&mut self) {
        ahp_session::session::state::Hosts::sweep_empty(&mut self.store);
    }

    /// The windows catalog, read in place — a boot resident.
    fn windows(&self) -> &::workbench::window::Windows {
        self.store
            .get::<::workbench::window::Windows>()
            .expect("the windows catalog is a boot resident")
    }

    pub fn register_client(
        &mut self,
        client: ahp_wire::client::Client,
    ) -> ahp_wire::client::HostId {
        let mut minted = ahp_wire::client::HostId::LOCAL;
        self.store
            .update::<ahp_wire::client::Servers>(|servers| minted = servers.mint(client));
        minted
    }

    pub fn viewport_stale(&self, window: WindowId, size: Size) -> bool {
        self.windows()
            .entity(window)
            .is_some_and(|entity| entity.viewport_stale(size))
    }

    pub fn ui_handle(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }

    pub fn window_store(&self, window: WindowId) -> Store {
        let _ = window;
        self.store.clone()
    }

    /// The frame's store: the window store plus the FOCUSED SEAT
    /// (the semantic walk's answer), so editors derive their
    /// selections-visible bit from STATE at build time — one
    /// viewport build per frame, no focused upgrade at paint.
    pub(crate) fn frame_store(&self, window: WindowId) -> imba::store::Store {
        let mut store = self.window_store(window);
        let client = crate::focus::window_focus_data(&store, self.ui.as_ref(), window)
            .and_then(|mut data| data.seat.take());
        if let Some(client) = client {
            ::editor::env::FrameFocus::set(&mut store, client);
        }
        store
    }

    fn setup(&mut self, mutate: impl FnOnce(&mut Store)) {
        mutate(&mut self.store);
    }

    fn window_txn(&mut self, window: WindowId, mutate: impl FnOnce(&mut Store)) {
        let _ = window;
        mutate(&mut self.store);
        self.sweep();
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.windows().ids()
    }

    pub fn window_viewport(&self, window: WindowId) -> Option<Size> {
        self.windows()
            .entity(window)
            .map(|entity| entity.viewport_size())
    }

    pub fn designate_local_host(&mut self, host: ahp_wire::client::HostId) {
        self.setup(|store| {
            let previous = store
                .get::<ahp_wire::client::LocalHost>()
                .and_then(|local| local.0);
            store.update::<ahp_wire::client::LocalHost>(|local| local.0 = Some(host));
            store.update::<ahp_session::session::state::Hosts>(|hosts| {
                hosts.rekey_local_sessions(previous, host);
            });
            // Families minted under the LOCAL placeholder carried no
            // uri map; now that they live under the real host, stamp
            // its map onto them (docs/entities.md law 4).
            ahp_session::session::state::Hosts::stamp_host_uris(store, host);
        });
        self.store
            .update::<::workbench::window::Windows>(|windows| windows.adopt_local_host_all(host));
    }


    pub fn add_window(&mut self) -> WindowId {
        let workspace = ahp_wire::SessionId::local_default(&self.store);
        let ui = self.ui_ctx();
        let mut discarded = AppEffects::new();
        let state = ahp_session::session::state::Hosts::ensure_state(&mut self.store, &workspace);
        let editors = fresh_workbench_root(&mut self.store, &state, &ui, &mut discarded.effects());
        Windows::add(&mut self.store, Window::new(editors, workspace.clone(), state))
    }

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    pub fn attach_host(
        &mut self,
        dispatcher: crate::effects::EffectDispatcher,
        scheduler: Arc<dyn Fn() + Send + Sync>,
    ) -> crate::effects::BackgroundRunner {
        self.effects
            .attach(dispatcher, scheduler, Arc::clone(&self.handlers))
    }

    pub fn register_handler<E: imba::effect::Effect>(
        &mut self,
        handler: impl imba::effect::EffectHandler<E>,
    ) where
        E::Result: Send + Sync,
    {
        self.handlers.register::<E>(handler);
    }

    pub fn register_windowed_navigator<N: ::workbench::navigation::WindowedNavigator>(
        &mut self,
        navigator: N,
    ) {
        self.setup(|store| ::workbench::navigation::Navigators::register_windowed(store, navigator));
    }

    pub fn register_navigator<N: hikit::navigation::Navigator>(&mut self, navigator: N) {
        self.setup(|store| ::workbench::navigation::Navigators::register(store, navigator));
    }

    pub fn observe_file_changes(&mut self) {
        self.setup(documents::watch::Watching::install);
    }

    /// Install the edge's BASE RESOLVER: working location → base ref,
    /// answered synchronously from store truth at ask time.
    pub fn observe_stripe_bases(
        &mut self,
        resolve: std::sync::Arc<dyn documents::diffs::StripeBaseResolver>,
    ) {
        self.setup(move |store| documents::diffs::StripeBases::install(store, resolve.clone()));
    }

    pub fn register_editor_command(&mut self, command: Arc<dyn editor::dynamic::DynamicEditorCommand>) {
        self.setup(|store| ::editor::dynamic::EditorCommands::register(store, command));
    }

    /// Commands that act on the COLLECTION — handed the pane's ids at
    /// dispatch (docs/entities.md law 3), never resolving an owner.
    pub fn register_document_command(&mut self, command: Arc<dyn documents::dynamic::DocumentCommand>) {
        self.setup(|store| documents::dynamic::DocumentCommands::register(store, command));
    }

    pub fn register_toolbar_button(&mut self, button: ::workbench::toolbar::ToolbarButton) {
        self.setup(|store| ::workbench::toolbar::ToolbarButtons::register(store, button));
    }

    pub fn register_row_minter(&mut self, minter: std::sync::Arc<hikit::panel::RowMinter>) {
        self.setup(|store| ::workbench::rows::RowMinters::register(store, minter));
    }

    pub fn workshop(&self) -> &Arc<::editor::env::Workshop> {
        &self.workshop
    }

    pub fn effect_caller(&self) -> imba::effect::EffectCaller {
        let handlers = Arc::downgrade(&self.handlers);
        imba::effect::EffectCaller::new(Arc::new(move |type_id, payload| {
            handlers.upgrade()?.call_future(type_id, payload)
        }))
    }

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    fn launch(&mut self, effects: AppEffects) {
        self.effects.launch(effects);
    }

    #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
    fn launch(&mut self, batch: AppEffects) {
        use imba::effect::Message;

        let messages = batch.drain();
        let cancelled: std::collections::HashSet<_> = messages
            .iter()
            .filter_map(|message| match message {
                Message::Cancel(token) => Some(*token),

                Message::Relaunch(previous, _, _) => Some(*previous),
                Message::Launch(..) => None,
            })
            .collect();
        for message in messages {
            let (token, effect) = match message {
                Message::Launch(token, effect) => (token, effect),
                Message::Relaunch(_, token, effect) => (token, effect),
                Message::Cancel(_) => continue,
            };
            if cancelled.contains(&token) {
                continue;
            }
            let payload = effect.into_payload();
            let type_id = payload.type_id();
            match self.handlers.dispatch(payload) {
                Err(_) => crate::effects::orphan(type_id),
                Ok((future, lift)) => {
                    if let Some(command) = lift(imba::effect::block_on(future)) {
                        self.perform_batch(vec![command]);
                    }
                }
            }
        }
    }

    pub fn set_chrome_clearance(&mut self, width: f32) {
        self.ui.set(::workbench::toolbar::ChromeClearance(width));
    }

    pub(crate) fn refresh_chrome(&mut self, theme: &::editor::theme::Theme) {
        let scrollbar = &theme.ui().scrollbar;
        self.ui.set(imba::scroll::ScrollbarStyle {
            color: scrollbar.color.0,
            width: scrollbar.width,
            margin: scrollbar.margin,
            radius: scrollbar.radius,
            min_knob: scrollbar.min_knob,
            track_inset: scrollbar.track_inset,
        });
        self.stats.set_font(hikit::fonts::ui_text_font(
            &self.ui,
            theme.ui().stats.font_size,
        ));
    }

    fn propagate_theme_change(&mut self) {
        let theme = ::editor::env::Themes::of(&self.store);
        self.refresh_chrome(&theme);

        self.workshop.set_theme(theme);
        let windows: Vec<(::workbench::window::WindowId, Size)> = self
            .windows()
            .ids()
            .into_iter()
            .filter_map(|id| self.window_viewport(id).map(|size| (id, size)))
            .collect();
        for (window, size) in windows {
            self.dispatch(window, Event::ThemeChanged, size);
        }
    }

    /// TEST SUPPORT: the fronting dock's owner id, if any.
    #[doc(hidden)]
    pub fn dock_owner_for_tests(&self, window: WindowId) -> Option<&'static str> {
        let store = self.window_store(window);
        ::workbench::window::Windows::window_ref(&store, window).and_then(|entity| entity.dock_owner())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn store_mut(&mut self) -> StoreMut<'_> {
        StoreMut { app: self }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn stats_mut(&mut self) -> &mut Stats {
        &mut self.stats
    }

    pub(crate) fn dispatch_paint(&mut self, window: WindowId, canvas: &Canvas, size: Size) -> bool {
        let mut arena = std::mem::take(&mut self.ui_arena);
        arena.reset();

        let store = self.frame_store(window);
        let result = {
            let widget = self.layout(
                window,
                &arena,
                &store,
                self.ui.as_ref(),
                Constraints::tight(size),
            );
            let widget = imba::Thunk::realize(widget, &arena, Rect::from_size(size));
            widget.handle_event(
                &arena,
                &Event::Paint {
                    canvas,
                    focused: true,
                },
                Rect::from_size(size),
            )
        };
        self.ui_arena = arena;

        match result {
            EventResult::Ignored | EventResult::Handled => false,
            EventResult::Command(command) => {
                trace_reconcile("paint", std::slice::from_ref(&command));
                self.perform_batch(vec![command])
            }
            EventResult::Commands(commands) => {
                trace_reconcile("paint", &commands);
                self.perform_batch(commands)
            }

            EventResult::Reveal(_) => false,
        }
    }

    pub fn dispatch_timed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
        event_started_at: f64,
    ) -> bool {
        if self.viewport_stale(window, size) {
            self.perform_batch(vec![AppCommand::ViewportResized(window, size)]);
        }
        let handled = self.dispatch(window, event, size);
        if handled {
            self.stats.mark_latency_event_start(event_started_at);
        }
        handled
    }

    pub fn dispatch(&mut self, window: WindowId, event: Event<'_>, size: Size) -> bool {
        let hit = match &event {
            Event::MouseMove { point } => self.dispatch_event(
                window,
                Event::HitTest {
                    point: *point,
                    miss: false,
                },
                size,
            ),
            _ => false,
        };
        self.dispatch_event(window, event, size) || hit
    }

    fn dispatch_event(&mut self, window: WindowId, event: Event<'_>, size: Size) -> bool {
        let clock = matches!(event, Event::AnimationClock { .. });

        let key_down = match event {
            Event::KeyDown { key, mods } => Some((key, mods)),
            _ => None,
        };
        let store = self.window_store(window);
        let (result, fallback) = match &event {
            // Keyboard input routes through the SEMANTIC focus chain
            // — a state walk over the views. No tree is built for a
            // keystroke.
            Event::KeyDown { .. } | Event::TextInput { .. } => {
                let ui = self.ui.clone();
                let result = {
                    let chain = crate::focus::window_focus_data(&store, ui.as_ref(), window);
                    match (&event, chain) {
                        (Event::KeyDown { key, mods }, Some(mut data)) => data.key(*key, *mods),
                        (Event::TextInput { text }, Some(mut data)) => data.text(text),
                        _ => EventResult::Ignored,
                    }
                };
                let fallback = match (key_down, &result) {
                    (Some((key, mods)), EventResult::Ignored | EventResult::Reveal(_)) => {
                        crate::keymap::Keymaps::binding_of(&store, key, mods).and_then(|id| {
                            crate::focus::window_focus_data(&store, ui.as_ref(), window)
                                .map(|data| data.commands)
                                .unwrap_or_default()
                                .into_iter()
                                .find(|presentable| presentable.id == id.as_ref())
                                .map(|presentable| presentable.command)
                                .or_else(|| {
                                    crate::commands::Commands::of(&store)
                                        .find(id.as_ref())
                                        .map(|command| AppCommand::Windowed(window, command.clone()))
                                })
                        })
                    }
                    _ => None,
                };
                (result, fallback)
            }
            _ => {
                let mut arena = std::mem::take(&mut self.ui_arena);
                arena.reset();
                let store = self.frame_store(window);
                let result = {
                    let widget = self.layout(
                        window,
                        &arena,
                        &store,
                        self.ui.as_ref(),
                        Constraints::tight(size),
                    );
                    let widget = imba::Thunk::realize(widget, &arena, Rect::from_size(size));
                    widget.handle_event(&arena, &event, Rect::from_size(size))
                };
                self.ui_arena = arena;
                (result, None)
            }
        };

        if let Some(command) = fallback {
            return self.perform_batch(vec![command]);
        }

        match result {
            EventResult::Ignored => false,

            EventResult::Handled => !clock,
            EventResult::Command(command) => {
                if clock {
                    trace_reconcile("clock", std::slice::from_ref(&command));
                }
                self.perform_batch(vec![command])
            }
            EventResult::Commands(commands) => {
                if clock {
                    trace_reconcile("clock", &commands);
                }
                self.perform_batch(commands)
            }
            EventResult::Reveal(_) => false,
        }
    }

    pub fn perform_batch(&mut self, commands: Vec<AppCommand>) -> bool {
        if commands.is_empty() {
            return false;
        }
        let probe = std::time::Instant::now();
        let label = command_label(&commands[0]);
        let theme_before = ::editor::env::Themes::of(&self.store);
        let ui = self.ui.clone();
        let mut batch = AppEffects::new();

        let mut store = std::mem::take(&mut self.store);
        let mut queue: std::collections::VecDeque<AppCommand> = commands.into();
        while let Some(command) = queue.pop_front() {
            let mut fx = batch.effects();
            self.perform(&mut store, &ui, command, &mut fx);

            // The performed command's follow-ups run next, in push
            // order — the loop's own lane, not a store note.
            for (index, follow_up) in batch.take_follow_ups().into_iter().enumerate() {
                queue.insert(index, follow_up);
            }
        }

        {
            let mut fx = batch.effects();
            // The resident sessions subscription: hosts connect, list
            // and drain whether or not any view shows them — the
            // catalog stays true at all times (views only dress it).
            fx.scope(AppCommand::Verb, |fx| {
                ahp_session::session::driver::sync_sessions_lane(&mut store, fx)
            });
            // The lanes run over EVERY session: each drains its own
            // pending queue, so a clean session costs map reads — and a
            // landing's session gets its sweep THIS batch whatever the
            // batch's scope (the old tail served only the LAST scope's
            // session, so a cross-session batch starved the others).
            for state in ahp_session::session::state::Hosts::states(&store) {
                let documents = state.documents();
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_lanes(&mut store, documents, fx)
                });
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_scroll_stripe_lanes(&mut store, documents, fx)
                });
                // The DRESSING sweep: any view whose basis lags its
                // pair resyncs NOW, id-routed — a normalize landing
                // and its re-dress share a batch, and no face waits
                // for paint.
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_dressing(&mut store, documents, &self.ui_ctx(), fx)
                });
                // The dock tree views ride the push road too: a
                // changes / history feed landing refreshes a mounted
                // stale view in the SAME batch — no paint probe.
                changesview::changes_view::sync_changes_views(
                    &mut store,
                    state.changes(),
                    &self.ui_ctx(),
                );
                // The canvases sync against the fresh document/diff/
                // changeset state — the SAME batch a feed landed in, a
                // direct lane over the sets that own them. The
                // dressed-views queue drains here: the session's own
                // note (id-routed landings appended mid-batch), taken
                // by the one lane that reads it.
                let dressed = documents::OpenDocuments::take_dressed(&mut store, documents);
                fx.scope(AppCommand::Verb, |fx| {
                    canvas::canvas::sync_canvases(
                        &mut store,
                        state.canvas_router(),
                        &self.ui_ctx(),
                        &dressed,
                        fx,
                    )
                });
                // The comments lane: the model's announce and card
                // notes drain onto the wire — a clean collection
                // costs a map read.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_comments::sync(
                        &mut store,
                        state.comments_wire(),
                        &self.ui_ctx(),
                        fx,
                    )
                });
                // The gesture-ask lanes: the views noted onto their
                // MODELS (grow, commit fetches, refetches); the
                // drivers drain the notes onto the wires here — no
                // view carries a wire or a window for these.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_changes::history::sync(&mut store, state.history_wire(), fx);
                    ahp_changes::changes::sync(&mut store, state.changes_wire(), fx);
                    ahp_locations::driver::sync(
                        &mut store,
                        &self.ui_ctx(),
                        state.locations_wire(),
                        fx,
                    );
                });
            }
        }
        // The safety net for a tail lane's follow-up: the queue loop
        // is over, so perform them here — late but never lost.
        loop {
            let late = batch.take_follow_ups();
            if late.is_empty() {
                break;
            }
            for command in late {
                let mut fx = batch.effects();
                self.perform(&mut store, &ui, command, &mut fx);
            }
        }
        let probe_perform = probe.elapsed();
        self.store = store;
        self.sweep();
        if validate_enabled() {
            for id in self.windows().ids() {
                let Some(window) = Windows::window_ref(&self.store, id) else {
                    continue;
                };
                validate_panes(&label, &self.store, &window.workbench().root);

                for (_session, stashed) in window.stashed_workbenches() {
                    validate_panes(&label, &self.store, &stashed.root);
                }
            }
        }
        let probe_commit = probe.elapsed().saturating_sub(probe_perform);
        if batch.take_settle() {
            self.settle_requested = true;
        }
        self.launch(batch);
        if theme_before.name() != ::editor::env::Themes::of(&self.store).name() {
            self.propagate_theme_change();
        }

        let changed: std::collections::HashSet<documents::watch::Subscription> =
            std::mem::take(&mut self.pending_file_events)
                .into_iter()
                .collect();
        if !changed.is_empty() {
            let event = documents::watch::FilesChanged(std::sync::Arc::new(changed));
            let windows: Vec<(::workbench::window::WindowId, Size)> = self
                .windows()
                .ids()
                .into_iter()
                .filter_map(|id| self.window_viewport(id).map(|size| (id, size)))
                .collect();
            for (window, size) in windows {
                self.dispatch(window, Event::UserEvent(&event), size);
            }
        }

        // The settle loop, at the END of the bit-raising batch — not
        // deferred to paint. The ordering is what buys exactness: a
        // scroll batch pulses BEFORE any later batch performs, so a
        // door never anchors on a top older than the last move; a
        // door batch pulses before its frame, so no wrong frame
        // paints (docs/editor/viewport-preservation.md §3.2). Bounded; the
        // guard keeps the pulse's own performs from recursing.
        if !self.settling {
            self.settling = true;
            let mut rounds = 0;
            while self.settle_requested && rounds < 3 {
                self.settle_requested = false;
                let windows: Vec<(::workbench::window::WindowId, Size)> = self
                    .windows()
                    .ids()
                    .into_iter()
                    .filter_map(|id| self.window_viewport(id).map(|size| (id, size)))
                    .collect();
                for (window, size) in windows {
                    self.dispatch(window, Event::Settle, size);
                }
                rounds += 1;
            }
            self.settling = false;
        }

        // Dock tree-follow: the focused location is a STATE question
        // now — walk the views, note the change. No tree was ever
        // built for this bookkeeping.
        for window in self.windows().ids() {
            let following = self
                .windows()
                .entity(window)
                .is_some_and(|entity| entity.dock_panel().is_some());
            if !following {
                continue;
            }
            let store = self.window_store(window);
            let followed = crate::focus::window_focus_data(&store, ui.as_ref(), window)
                .and_then(|mut data| crate::focus::focused_location(&mut data));
            let Some(location) = followed else {
                continue;
            };
            let changed = self
                .windows()
                .entity(window)
                .is_some_and(|entity| entity.focused_location() != Some(&location));
            if changed {
                self.window_txn(window, |store| {
                    if let Some(mut entity) = Windows::window(store, window) {
                        entity.note_focused_location(location);
                        Windows::put(store, window, entity);
                    }
                });
            }
        }
        let probe_launch = probe
            .elapsed()
            .saturating_sub(probe_perform)
            .saturating_sub(probe_commit);
        if trace_stalls_enabled() && probe.elapsed().as_millis() >= 10 {
            eprintln!(
                "[stall] command {label}: perform={probe_perform:?} \
                 commit={probe_commit:?} launch={probe_launch:?} pick={:?}",
                probe
                    .elapsed()
                    .saturating_sub(probe_perform)
                    .saturating_sub(probe_commit)
                    .saturating_sub(probe_launch)
            );
        }
        true
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct StoreMut<'a> {
    app: &'a mut Application,
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::Deref for StoreMut<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.app.store
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::DerefMut for StoreMut<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.app.store
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for StoreMut<'_> {
    fn drop(&mut self) {
        self.app.sweep();
    }
}

pub(crate) struct OpenEffect {
    window: WindowId,
    documents: imba::store::Id<OpenDocuments>,
    name: String,
    primary: bool,
    location: Option<editor::location::ResourceLocation>,

    build: DocumentBuild,
}

impl std::fmt::Display for OpenEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "open document {}", self.name)
    }
}

impl Effect for OpenEffect {
    type Result = AppCommand;
}

pub(crate) struct OpenHandler(pub(crate) std::sync::Arc<::editor::env::Workshop>);

impl imba::effect::EffectHandler<OpenEffect> for OpenHandler {
    async fn handle(&self, effect: OpenEffect) -> AppCommand {
        let fonts = self.0.fonts();
        let theme = self.0.theme();
        let document = self
            .0
            .with_ctx(|store, ui| (effect.build)(store, ui, &fonts, &theme));
        AppCommand::Opened(
            effect.window,
            OpenedDocument {
                documents: effect.documents,
                name: effect.name,
                document,
                location: effect.location,
                primary: effect.primary,
                target: None,
                focus: false,
            },
        )
    }
}

pub fn open_effect(
    window: WindowId,
    documents: imba::store::Id<OpenDocuments>,
    name: String,
    primary: bool,
    location: Option<editor::location::ResourceLocation>,
    build: DocumentBuild,
) -> imba::effect::AnyEffect<AppCommand> {
    imba::effect::AnyEffect::new(OpenEffect {
        window,
        documents,
        name,
        primary,
        location,
        build,
    })
}

fn command_label(command: &AppCommand) -> std::borrow::Cow<'static, str> {
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
        AppCommand::Stats(_) => "stats",
        AppCommand::At(..) => unreachable!(),
        AppCommand::Opened(..) => "opened",
        AppCommand::FileChanged(..) => "file changed",
    })
}

fn trace_reconcile(source: &str, commands: &[AppCommand]) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RECONCILE").is_some()) {
        return;
    }
    let labels: Vec<_> = commands.iter().map(command_label).collect();
    eprintln!("[reconcile] {source} reported: {}", labels.join(", "));
}

pub fn trace_stalls_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_STALLS").is_some())
}

fn validate_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let enabled = std::env::var_os("HIMARK_VALIDATE").is_some();
        if enabled {
            eprintln!(
                "HIMARK_VALIDATE is on: every commit walks the full layout of every pane \
                 (hundreds of milliseconds per keystroke on large documents). \
                 This is a debugging tripwire — unset it for normal use."
            );
        }
        enabled
    })
}

fn validate_panes(context: &str, store: &Store, editors: &WorkbenchNode) {
    if !validate_enabled() {
        return;
    }
    let mut index = 0usize;
    let check = |index: usize, view: &EditorIdView| {
        let boundary = view
            .gathered(store)
            .and_then(|view| view.find_misaligned_boundary());
        if let Some(boundary) = boundary {
            panic!(
                "{context}: pane {index} layout boundary at byte {boundary} \
                 splits a UTF-8 character"
            );
        }
    };
    editors.for_each_pane(&mut |panel| {
        match panel {
            Panel::Editor(pane) => check(index, pane.content()),

            Panel::Plugin(_) => {}
        }
        index += 1;
    });
}

impl Application {
    fn perform(&mut self, store: &mut Store, ui: &UiCtx, command: AppCommand, fx: &mut AppFx<'_>) {
        match command {
            AppCommand::Content(window, command) => {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
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
                        if let Some(state) = Windows::session_state(store, window) {
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

                        if let Some(state) = Windows::session_state(store, window) {
                            fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_document_watches(store, state.documents(), fx)
            });
                            fx.scope(AppCommand::Verb, |fx| {
                                documents::lanes::sync_stripe_bases(store, state.documents(), ui, fx)
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
                    let mut entity =
                        ::workbench::window::Windows::window(store, window).expect("the window entity");
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
                            if let Some(state) = Windows::session_state(store, window) {
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

                            if let Some(state) = Windows::session_state(store, window) {
                                fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_document_watches(store, state.documents(), fx)
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
                    let mut entity =
                        ::workbench::window::Windows::window(store, window).expect("the window entity");
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
                            if let Some(state) = Windows::session_state(store, window) {
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
                            if let Some(state) = Windows::session_state(store, window) {
                                fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_document_watches(store, state.documents(), fx)
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
                        match payload.downcast_ref::<std::sync::Arc<dyn crate::commands::WindowedCommand>>() {
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
            AppCommand::Stats(command) => {
                fx.scope(AppCommand::Stats, |fx| {
                    self.stats.perform(store, ui, command, fx)
                });
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
                let documents = Windows::session_state(store, window)
                    .expect("a document opens into a window with a session")
                    .documents();
                fx.push(open_effect(
                    window, documents, name, primary, location, build,
                ));
            }
            AppCommand::OpenPanel(window, panel) => {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
                entity.open_panel(store, ui, panel, fx);
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::OpenModal(window, modal) => {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.show_modal(store, modal, fx),
                );
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::CloseModal(window) => {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.dismiss_modal(store, fx),
                );
                ::workbench::window::Windows::put(store, window, entity);
            }
            AppCommand::ViewportResized(window, size) => {
                let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
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

    pub(crate) fn layout<'a>(
        &'a self,
        window: WindowId,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, AppCommand> + 'a {
        let size = constraints.max;
        let entity = ::workbench::window::Windows::window_ref(store, window).expect("the window entity");
        let content = imba::layout::Layout::layout(entity.display(arena, store, ui), arena, constraints);
        let stats = imba::layout::Layout::layout(self.stats.display(arena, store, ui), arena, constraints);

        let mut container = imba::container::container(arena, size);
        container.place(
            0.0,
            0.0,
            content
                .map(move |command| AppCommand::Content(window, command))
                .overlay_host(imba::overlay::WINDOW),
        );

        container.place(0.0, 0.0, stats.map(AppCommand::Stats));
        container
    }
}

impl Application {
    pub fn sync_viewport_window(&mut self, window: ::workbench::window::WindowId, size: skia_safe::Size) {
        if self.viewport_stale(window, size) {
            self.perform_batch(vec![AppCommand::ViewportResized(window, size)]);
        }
    }

    pub fn draw_window(&mut self, window: ::workbench::window::WindowId, canvas: &skia_safe::Canvas) -> bool {
        let size = canvas.base_layer_size();
        self.draw_window_sized(
            window,
            canvas,
            skia_safe::Size::new(size.width as f32, size.height as f32),
        )
    }

    pub fn draw_window_sized(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &skia_safe::Canvas,
        size: skia_safe::Size,
    ) -> bool {
        self.stats_mut().begin_frame();

        if trace_resize_enabled() {
            let entity =
                ::workbench::window::Windows::window_ref(self.store(), window).expect("the window entity");
            if entity.viewport_stale(size) {
                let current = entity.viewport_size();
                eprintln!(
                    "[resize himark] draw canvas={}x{} previous_viewport={}x{}",
                    size.width, size.height, current.width, current.height
                );
            }
        }
        self.sync_viewport_window(window, size);

        let render_started = std::time::Instant::now();

        let reconciled = self.dispatch_paint(window, canvas, size);
        self.stats_mut().record_reconcile(reconciled);
        self.stats_mut()
            .record_render_sample(render_started.elapsed());
        reconciled
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn draw_window_profiled(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &skia_safe::Canvas,
        size: skia_safe::Size,
    ) -> std::time::Duration {
        self.stats_mut().begin_frame();
        self.sync_viewport_window(window, size);
        let paint_started = std::time::Instant::now();
        let _ = self.dispatch_paint(window, canvas, size);
        paint_started.elapsed()
    }
}

fn trace_resize_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RESIZE").is_some())
}
