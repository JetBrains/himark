// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::sync::OnceLock;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::{Effect, Effects},
    event::{Event, EventResult},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Canvas, Rect, Size};
use text::Text;

use crate::{
    deliver, mount_editor, Document, DocumentId, EditorCommand, EditorIdView, Markup, ModalRequest,
    ModalView, OpenDocuments, Panel, Window, WindowId, Windows, Workbench, WorkbenchNode,
};

use crate::stats::{Stats, StatsCommand};

pub struct Application {
    state: crate::AppState,

    committed: Store,

    seats: crate::higent::Servers,

    pub(crate) ui: std::rc::Rc<UiCtx>,

    stats: Stats,
    pub(crate) ui_arena: Arena,

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    effects: crate::effects::EffectLauncher,

    handlers: Arc<crate::effects::Handlers>,

    workshop: Arc<::editor::Workshop>,

    pending_file_events: Vec<crate::watch::Subscription>,

    pending_diff_events: Vec<::editor::diff::DiffId>,

    /// A perform raised the settle bit (`Effects::settle`): run the
    /// synchronous `Event::Settle` pulse before the next paint so
    /// viewport corrections land in the SAME frame
    /// (docs/viewport-preservation.md §3.2).
    settle_requested: bool,
    settling: bool,
}

pub struct OpenedDocument {
    pub name: String,
    pub document: Document,

    pub location: Option<crate::ResourceLocation>,

    pub primary: bool,

    pub target: Option<std::ops::Range<crate::LineCol>>,
}

pub struct AppFonts {
    source: crate::FontSource,
}

pub type DocumentBuild = Box<
    dyn FnOnce(&skia_safe::textlayout::FontCollection, &::editor::theme::Theme) -> Document
        + Send
        + Sync,
>;

pub enum AppCommand {
    Content(WindowId, crate::WindowCommand),

    Dynamic(WindowId, std::sync::Arc<dyn crate::DynamicCommand>),

    Landing(WindowId, Box<dyn crate::LandingCommand>),

    InSession(crate::SessionId, Box<AppCommand>),

    Register(std::sync::Arc<dyn crate::DynamicCommand>),
    Stats(StatsCommand),

    Entity(DocumentId, EditorCommand),

    Opened(WindowId, OpenedDocument),

    BaseLocated {
        document: DocumentId,
        base: Option<crate::ResourceLocation>,
    },

    BaseFetched {
        document: DocumentId,
        base: crate::ResourceLocation,
        text: Option<String>,
    },

    BaseBuilt {
        document: DocumentId,
        base: crate::ResourceLocation,
        built: Document,
    },

    DiffNormalized {
        diff: ::editor::diff::DiffId,
        operation: operation::Operation,
        markup: ::editor::Markup,
        changed: Vec<std::ops::Range<u32>>,
        base_revision: u64,
        target_revision: u64,
    },

    /// A Save All store came home for one document.
    DocumentStored {
        document: DocumentId,
        revision: u64,
        snapshot: ::editor::Text,
        stored: bool,
    },

    FileChanged(crate::watch::Subscription),

    Watched(DocumentId, Option<crate::watch::Subscription>),

    Refetched {
        document: DocumentId,

        serial: u64,
        text: Option<String>,
    },

    RefetchDiffed {
        document: DocumentId,
        base_revision: u64,
        serial: u64,
        rebase: crate::watch::RefetchRebase,
    },

    OpenAsync {
        window: WindowId,
        name: String,
        primary: bool,

        location: Option<crate::ResourceLocation>,
        build: DocumentBuild,
    },

    OpenPanel(WindowId, Box<dyn crate::DynPanelView>),

    OpenModal(WindowId, Box<dyn ModalView>),

    CloseModal(WindowId),

    ViewportResized(WindowId, Size),

    RegisterLanguages(::editor::SyntaxLanguages),

    RegisterEnrichers(::editor::Enrichers),
}

impl AppCommand {
    pub fn dynamic_in(
        session: crate::SessionId,
        window: WindowId,
        command: std::sync::Arc<dyn crate::DynamicCommand>,
    ) -> AppCommand {
        AppCommand::InSession(session, Box::new(AppCommand::Dynamic(window, command)))
    }
}

pub type AppEffects = imba::effect::Batch<AppCommand>;

pub type AppFx<'a> = Effects<'a, AppCommand>;

pub(crate) fn pane_width(store: &Store, pane: &crate::EditorPane) -> Option<f32> {
    let view = pane.content();
    Some(OpenDocuments::document_ref(store, view.document())?.layout_width(view.editor()))
}

pub(crate) fn panel_width(store: &Store, panel: &Panel) -> Option<f32> {
    pane_width(store, panel.editor()?)
}

pub(crate) fn fallback_pane_editor_width(store: &Store) -> f32 {
    let theme = ::editor::env::Themes::of(store);
    let ui = theme.ui();
    (ui.window.first_pane_width - ui.editor_gutter.width).max(ui.window.min_editor_width)
}

impl Application {
    pub fn ui_ctx(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }
}

pub fn entity_scope<T>(
    document: DocumentId,
    fx: &mut AppFx<'_>,
    f: impl FnOnce(&mut Effects<'_, EditorCommand>) -> T,
) -> T {
    fx.scope(move |command| AppCommand::Entity(document, command), f)
}

impl AppFonts {
    pub fn new(source: crate::FontSource) -> Self {
        Self { source }
    }

    pub fn platform() -> Self {
        Self::new(crate::fonts::source())
    }

    pub fn embedded() -> Self {
        Self::new(::editor::embedded_fonts::source())
    }

    pub fn source(&self) -> crate::FontSource {
        self.source.clone()
    }
}

pub struct ChromeClearance(pub f32);

pub(crate) fn fresh_workbench_root(store: &mut Store, fx: &mut AppFx<'_>) -> WorkbenchNode {
    let mut scratch = markdown_scratch();

    let location = crate::next_scratch_location(store);
    let name = location.name().to_owned();
    crate::RecentLocations::touch(store, &location);
    let scratch_id = OpenDocuments::register(store, scratch.clone(), Some(location), name, 0);
    let width = fallback_pane_editor_width(store);
    let editor_id = entity_scope(scratch_id, fx, |fx| {
        mount_editor(store, &mut scratch, width, None, fx)
    });
    documents::scroll_stripes::enable_scroll_stripes(store, scratch_id, &mut scratch, editor_id);
    OpenDocuments::put_document(store, scratch_id, scratch);
    WorkbenchNode::editor_leaf(ScrollView::new(
        EditorIdView::new(scratch_id, editor_id).with_gutter(),
    ))
}

pub fn switch_session(
    store: &mut Store,
    window: WindowId,
    target: crate::SessionId,
    fx: &mut AppFx<'_>,
) {
    let Some(mut entity) = crate::Windows::window(store, window) else {
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

    let owed = entity.switch_to(target);
    crate::Windows::put(store, window, entity);
    if let Some(previous) = owed {
        crate::commands::BatchRequests::push(
            store,
            window,
            std::sync::Arc::new(EnterFreshSession { previous }),
        );
    }
}

struct EnterFreshSession {
    previous: crate::SessionId,
}

impl crate::DynamicCommand for EnterFreshSession {
    fn id(&self) -> &'static str {
        "session.enter-fresh"
    }
    fn name(&self) -> String {
        "Enter Fresh Session".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let root = fresh_workbench_root(store, fx);
        entity.install_fresh(self.previous.clone(), Workbench::new(root));
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) fn markdown_scratch() -> Document {
    Document::new(Text::from_string_exact(""), Markup::new())
        .with_syntax(::editor::Syntax::new("markdown", None, Markup::new()), &[])
}

impl Application {
    pub fn new(fonts: AppFonts) -> Self {
        let total_started = crate::startup_profile::start();
        let mut store = Store::new();

        let theme = ::editor::theme::Theme::embedded();
        store.put(::editor::env::Fonts(fonts.source()));
        store.put(::editor::env::Themes(theme.clone()));

        let ui = std::rc::Rc::new(UiCtx::cold());
        ui.set(::editor::env::UiFonts((fonts.source())()));

        let scrollbar = &theme.ui().scrollbar;
        ui.set(imba::scroll::ScrollbarStyle {
            color: scrollbar.color.0,
            width: scrollbar.width,
            margin: scrollbar.margin,
            radius: scrollbar.radius,
            min_knob: scrollbar.min_knob,
            track_inset: scrollbar.track_inset,
        });

        let overlay_font = crate::fonts::ui_text_font(&ui, theme.ui().stats.font_size);

        crate::commands::register_builtins(&mut store);
        crate::Navigators::register(&mut store, crate::navigation::EditorNavigator);

        let workshop = Arc::new(::editor::Workshop::new(
            ::editor::env::Fonts::of(&store),
            ::editor::env::Themes::of(&store),
        ));
        let handlers = Arc::new(crate::effects::Handlers::default());
        crate::effects::register_builtins(&handlers, &workshop);
        let state = crate::AppState::adopt(store);
        let committed = state.gather_seatless(None);
        let application = Self {
            state,
            committed,
            seats: crate::higent::Servers::default(),
            ui,
            stats: Stats::new(overlay_font),
            #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
            effects: crate::effects::EffectLauncher::new(),
            handlers,
            workshop,
            ui_arena: Arena::default(),
            pending_file_events: Vec::new(),
            pending_diff_events: Vec::new(),
            settle_requested: false,
            settling: false,
        };
        crate::startup_profile::log("Application::new", total_started);
        application
    }

    fn commit(&mut self, store: Store) {
        self.state.scatter(store);
        self.refresh_committed();
    }

    fn refresh_committed(&mut self) {
        let window = self.state.windows.primary();
        let scope = window.and_then(|id| self.state.windows.session_of(id));
        self.committed = self.state.gather(window, scope.as_ref(), &self.seats);
    }

    pub fn register_seat(
        &mut self,
        seat: std::sync::Arc<dyn crate::higent::AhpServer>,
    ) -> crate::higent::HostId {
        let minted = self.seats.mint(seat);
        self.refresh_committed();
        minted
    }

    pub fn viewport_stale(&self, window: WindowId, size: Size) -> bool {
        self.state
            .windows
            .entity(window)
            .is_some_and(|entity| entity.viewport_stale(size))
    }

    pub fn ui_handle(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }

    pub fn window_store(&self, window: WindowId) -> Store {
        let scope = self.state.windows.session_of(window);
        self.state.gather(Some(window), scope.as_ref(), &self.seats)
    }

    fn setup(&mut self, mutate: impl FnOnce(&mut Store)) {
        let mut store = self.state.gather(None, None, &self.seats);
        mutate(&mut store);
        self.commit(store);
    }

    fn window_txn(&mut self, window: WindowId, mutate: impl FnOnce(&mut Store)) {
        let scope = self.state.windows.session_of(window);
        let mut store = self.state.gather(Some(window), scope.as_ref(), &self.seats);
        mutate(&mut store);
        self.commit(store);
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.state.windows.ids()
    }

    pub fn window_viewport(&self, window: WindowId) -> Option<Size> {
        self.state
            .windows
            .entity(window)
            .map(|entity| entity.viewport_size())
    }

    pub fn designate_local_host(&mut self, host: crate::higent::HostId) {
        self.setup(|store| {
            let previous = store
                .get::<crate::higent::LocalHost>()
                .and_then(|local| local.0);
            store.update::<crate::higent::LocalHost>(|local| local.0 = Some(host));
            store.update::<crate::higent::Hosts>(|hosts| {
                hosts.rekey_local_families(previous, host);
            });
        });
        self.state.windows.adopt_local_host_all(host);
        self.refresh_committed();
    }

    fn command_scope(
        &self,
        store: &Store,
        command: &AppCommand,
    ) -> (Option<WindowId>, Option<crate::SessionId>) {
        if let AppCommand::InSession(session, inner) = command {
            let (window, _) = self.command_scope(store, inner);
            return (window, Some(session.clone()));
        }
        let window = match command {
            AppCommand::Content(window, _)
            | AppCommand::Dynamic(window, _)
            | AppCommand::Landing(window, _)
            | AppCommand::Opened(window, _)
            | AppCommand::OpenAsync { window, .. }
            | AppCommand::OpenPanel(window, _)
            | AppCommand::OpenModal(window, _)
            | AppCommand::ViewportResized(window, _) => *window,
            AppCommand::CloseModal(window) => *window,

            AppCommand::Entity(document, _)
            | AppCommand::BaseLocated { document, .. }
            | AppCommand::BaseFetched { document, .. }
            | AppCommand::BaseBuilt { document, .. }
            | AppCommand::DocumentStored { document, .. }
            | AppCommand::Watched(document, _)
            | AppCommand::Refetched { document, .. }
            | AppCommand::RefetchDiffed { document, .. } => {
                let document = *document;
                if store
                    .get::<OpenDocuments>()
                    .is_some_and(|documents| documents.contains_id(document))
                {
                    return (None, crate::Gathered::scope(store).cloned());
                }
                return (
                    None,
                    crate::higent::Hosts::session_of_document(store, document),
                );
            }
            AppCommand::FileChanged(subscription) => {
                let subscription = *subscription;
                if store
                    .get::<OpenDocuments>()
                    .is_some_and(|documents| documents.rides_watch(subscription))
                {
                    return (None, crate::Gathered::scope(store).cloned());
                }
                return (
                    None,
                    crate::higent::Hosts::session_of_watch(store, subscription),
                );
            }
            AppCommand::DiffNormalized { diff, .. } => {
                let diff = *diff;
                if store
                    .get::<OpenDocuments>()
                    .is_some_and(|documents| documents.tracks_diff(diff))
                {
                    return (None, crate::Gathered::scope(store).cloned());
                }
                return (None, crate::higent::Hosts::session_of_diff(store, diff));
            }
            _ => return (None, None),
        };

        let session = Windows::window_ref(store, window)
            .map(|entity| entity.current_session())
            .or_else(|| self.state.windows.session_of(window));
        (Some(window), session)
    }

    pub fn add_window(&mut self) -> WindowId {
        let workspace = crate::SessionId::local_default(&self.state.gather_seatless(None));
        let mut store = self.state.gather(None, Some(&workspace), &self.seats);

        let mut discarded = AppEffects::new();
        let editors = fresh_workbench_root(&mut store, &mut discarded.effects());
        let window = Windows::add(&mut store, Window::new(editors, workspace));
        self.commit(store);
        window
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

    pub fn register_navigator<N: crate::Navigator>(&mut self, navigator: N) {
        self.setup(|store| crate::Navigators::register(store, navigator));
    }

    pub fn observe_document_changes(&mut self) {
        self.setup(|store| {
            crate::ChangeObserver::install(store, {
                let cache = std::sync::Mutex::new((u64::MAX, std::sync::Arc::new(Vec::new())));
                std::sync::Arc::new(move |store: &imba::store::Store| {
                    let generation = crate::higent::Hosts::generation(store);
                    let mut held = cache.lock().expect("the folders cache");
                    if held.0 != generation {
                        *held = (
                            generation,
                            std::sync::Arc::new(crate::higent::all_session_folders(store)),
                        );
                    }
                    held.1.as_ref().clone()
                })
            })
        });
    }

    pub fn observe_file_changes(&mut self) {
        self.setup(crate::watch::Watching::install);
    }

    pub fn observe_stripe_bases(&mut self) {
        self.setup(crate::diffs::StripeBases::install);
    }

    pub fn register_editor_command(&mut self, command: Arc<dyn crate::DynamicEditorCommand>) {
        self.setup(|store| ::editor::EditorCommands::register(store, command));
    }

    pub fn register_toolbar_button(&mut self, button: crate::ToolbarButton) {
        self.setup(|store| crate::toolbar::ToolbarButtons::register(store, button));
    }

    pub fn register_overlay_surface(&mut self, surface: crate::OverlaySurface) {
        self.setup(|store| crate::toolbar::OverlaySurfaces::register(store, surface));
    }

    pub fn register_row_minter(&mut self, minter: std::sync::Arc<crate::RowMinter>) {
        self.setup(|store| crate::family_rows::RowMinters::register(store, minter));
    }

    pub fn workshop(&self) -> &Arc<::editor::Workshop> {
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
        self.ui.set(ChromeClearance(width));
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
        self.stats.set_font(crate::fonts::ui_text_font(
            &self.ui,
            theme.ui().stats.font_size,
        ));
    }

    fn propagate_theme_change(&mut self) {
        let theme = ::editor::env::Themes::of(&self.committed);
        self.refresh_chrome(&theme);

        self.workshop.set_theme(theme);
        let windows: Vec<(crate::WindowId, Size)> = self
            .state
            .windows
            .ids()
            .into_iter()
            .filter_map(|id| self.window_viewport(id).map(|size| (id, size)))
            .collect();
        for (window, size) in windows {
            self.dispatch(window, Event::ThemeChanged, size);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn store_mut(&mut self) -> StoreMut<'_> {
        StoreMut { app: self }
    }

    pub fn store(&self) -> &Store {
        &self.committed
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

        let store = self.window_store(window);
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
                        crate::Keymaps::binding_of(&store, key, mods).and_then(|id| {
                            crate::focus::window_focus_data(&store, ui.as_ref(), window)
                                .map(|data| data.commands)
                                .unwrap_or_default()
                                .into_iter()
                                .find(|presentable| presentable.id == id.as_ref())
                                .map(|presentable| presentable.command)
                                .or_else(|| {
                                    crate::commands::Commands::of(&store)
                                        .find(id.as_ref())
                                        .map(|command| AppCommand::Dynamic(window, command.clone()))
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
        let theme_before = ::editor::env::Themes::of(&self.committed);
        let ui = self.ui.clone();
        let mut batch = AppEffects::new();

        let mut scope: (Option<WindowId>, Option<crate::SessionId>) = (None, None);
        let mut store = self.state.gather(scope.0, scope.1.as_ref(), &self.seats);
        let mut queue: std::collections::VecDeque<AppCommand> = commands.into();
        while let Some(command) = queue.pop_front() {
            let next = self.command_scope(&store, &command);
            if next != scope {
                self.state.scatter(store);
                scope = next;
                store = self.state.gather(scope.0, scope.1.as_ref(), &self.seats);
            }
            let mut fx = batch.effects();
            self.perform(&mut store, &ui, command, &mut fx);

            for (index, (window, request)) in crate::commands::BatchRequests::drain(&mut store)
                .into_iter()
                .enumerate()
            {
                queue.insert(index, AppCommand::Dynamic(window, request));
            }
        }

        {
            let mut fx = batch.effects();
            crate::diffs::sync_diff_lanes(&mut store, &mut fx);
            documents::scroll_stripes::sync_scroll_stripe_lanes(
                &mut store,
                &mut fx,
                |document, command| AppCommand::Entity(document, command),
            );
        }
        let probe_perform = probe.elapsed();
        self.commit(store);
        if validate_enabled() {
            for id in self.state.windows.ids() {
                let store = self.window_store(id);
                let Some(window) = Windows::window_ref(&store, id) else {
                    continue;
                };
                validate_panes(label, &store, &window.workbench().root);

                for (session, stashed) in window.stashed_workbenches() {
                    let store = self.state.gather(Some(id), Some(session), &self.seats);
                    validate_panes(label, &store, &stashed.root);
                }
            }
        }
        let probe_commit = probe.elapsed().saturating_sub(probe_perform);
        if batch.take_settle() {
            self.settle_requested = true;
        }
        self.launch(batch);
        if theme_before.name() != ::editor::env::Themes::of(&self.committed).name() {
            self.propagate_theme_change();
        }

        let changed: std::collections::HashSet<crate::Subscription> =
            std::mem::take(&mut self.pending_file_events)
                .into_iter()
                .collect();
        if !changed.is_empty() {
            let event = crate::watch::FilesChanged(std::sync::Arc::new(changed));
            let windows: Vec<(crate::WindowId, Size)> = self
                .state
                .windows
                .ids()
                .into_iter()
                .filter_map(|id| self.window_viewport(id).map(|size| (id, size)))
                .collect();
            for (window, size) in windows {
                self.dispatch(window, Event::UserEvent(&event), size);
            }
        }

        for diff in std::mem::take(&mut self.pending_diff_events) {
            let event = crate::DiffChanged { diff };
            let windows: Vec<(crate::WindowId, Size)> = self
                .state
                .windows
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
        // paints (docs/viewport-preservation.md §3.2). Bounded; the
        // guard keeps the pulse's own performs from recursing.
        if !self.settling {
            self.settling = true;
            let mut rounds = 0;
            while self.settle_requested && rounds < 3 {
                self.settle_requested = false;
                let windows: Vec<(crate::WindowId, Size)> = self
                    .state
                    .windows
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
        for window in self.state.windows.ids() {
            let following = self
                .state
                .windows
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
                .state
                .windows
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
        &self.app.committed
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::DerefMut for StoreMut<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.app.committed
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for StoreMut<'_> {
    fn drop(&mut self) {
        let store = self.app.committed.clone();
        self.app.state.scatter(store);
        self.app.refresh_committed();
    }
}

pub(crate) struct OpenEffect {
    window: WindowId,
    name: String,
    primary: bool,
    location: Option<crate::ResourceLocation>,

    build: Box<
        dyn FnOnce(&skia_safe::textlayout::FontCollection, &::editor::theme::Theme) -> Document
            + Sync
            + Send,
    >,
}

impl Effect for OpenEffect {
    type Result = AppCommand;
}

pub(crate) struct OpenHandler(pub(crate) std::sync::Arc<::editor::Workshop>);

impl imba::effect::EffectHandler<OpenEffect> for OpenHandler {
    async fn handle(&self, effect: OpenEffect) -> AppCommand {
        let document = (effect.build)(&self.0.fonts(), &self.0.theme());
        AppCommand::Opened(
            effect.window,
            OpenedDocument {
                name: effect.name,
                document,
                location: effect.location,
                primary: effect.primary,
                target: None,
            },
        )
    }
}

pub fn open_effect(
    window: WindowId,
    name: String,
    primary: bool,
    location: Option<crate::ResourceLocation>,
    build: DocumentBuild,
) -> imba::effect::AnyEffect<AppCommand> {
    imba::effect::AnyEffect::new(OpenEffect {
        window,
        name,
        primary,
        location,
        build,
    })
}

fn command_label(command: &AppCommand) -> &'static str {
    match command {
        AppCommand::Content(_, crate::WindowCommand::Base(_)) => "pane",
        AppCommand::Content(_, crate::WindowCommand::Toolbar(_)) => "toolbar",
        AppCommand::Content(_, crate::WindowCommand::Side(command)) => {
            match command.downcast_ref::<crate::drawer::DrawerCommand>() {
                Some(crate::drawer::DrawerCommand::Tick(_)) => "side slide-tick",
                Some(crate::drawer::DrawerCommand::Content(_)) => "side content",
                None => "side",
            }
        }
        AppCommand::Content(_, crate::WindowCommand::SideFocusLost) => "side",
        AppCommand::Content(_, crate::WindowCommand::Dock(command)) => {
            match command.downcast_ref::<crate::dock::DockCommand>() {
                Some(crate::dock::DockCommand::Tick(_)) => "dock slide-tick",
                Some(crate::dock::DockCommand::Content(_)) => "dock content",
                _ => "dock",
            }
        }
        AppCommand::Content(_, crate::WindowCommand::Bottom(command)) => {
            match command.downcast_ref::<crate::sheet::SheetCommand>() {
                Some(crate::sheet::SheetCommand::Tick(_)) => "sheet slide-tick",
                Some(crate::sheet::SheetCommand::Content(_)) => "sheet content",
                _ => "sheet",
            }
        }
        AppCommand::Content(_, crate::WindowCommand::Modal(_)) => "modal",
        AppCommand::Content(_, crate::WindowCommand::Focus(_)) => "focus",
        AppCommand::Dynamic(..) => "dynamic",
        AppCommand::Landing(..) => "landing",
        AppCommand::InSession(..) => "landing",
        AppCommand::Register(_) => "register",
        AppCommand::OpenAsync { .. } => "open async",
        AppCommand::OpenPanel(..) => "open panel",
        AppCommand::OpenModal(..) => "open modal",
        AppCommand::CloseModal(_) => "close modal",
        AppCommand::ViewportResized(..) => "viewport",
        AppCommand::RegisterLanguages(_) => "register languages",
        AppCommand::RegisterEnrichers(_) => "register enrichers",
        AppCommand::Stats(_) => "stats",
        AppCommand::Entity(_, EditorCommand::ApplyRepair(_)) => "repair",
        AppCommand::Entity(_, EditorCommand::ApplyReparse(_)) => "reparse",
        AppCommand::Entity(_, EditorCommand::ApplyEnrichment(_)) => "enrich",
        AppCommand::Entity(..) => "entity",
        AppCommand::Opened(..) => "opened",
        AppCommand::BaseLocated { .. } => "base located",
        AppCommand::BaseFetched { .. } => "base fetched",
        AppCommand::BaseBuilt { .. } => "base built",
        AppCommand::DiffNormalized { .. } => "diff normalized",
        AppCommand::DocumentStored { .. } => "document stored",
        AppCommand::FileChanged(..) => "file changed",
        AppCommand::Watched(..) => "watched",
        AppCommand::Refetched { .. } => "refetched",
        AppCommand::RefetchDiffed { .. } => "refetch-diffed",
    }
}

fn trace_reconcile(source: &str, commands: &[AppCommand]) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RECONCILE").is_some()) {
        return;
    }
    let labels: Vec<&str> = commands.iter().map(command_label).collect();
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
                let mut entity = crate::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
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
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        crate::Windows::put(store, window, entity);
                    }
                    Some(ModalRequest::Perform(command)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        crate::Windows::put(store, window, entity);
                        self.perform(store, ui, command, fx);
                    }
                    Some(ModalRequest::ShowDocument(document)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        entity.show_document(store, window, document, None, fx);
                        crate::Windows::put(store, window, entity);

                        crate::watch::sync_document_watches(store, fx);
                        crate::diffs::sync_stripe_bases(store, fx);
                    }
                    Some(ModalRequest::OpenLocations(locations)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        crate::Windows::put(store, window, entity);
                        crate::open_locations(store, window, &locations, fx);
                    }
                    Some(ModalRequest::SelectWidget(widget)) => {
                        fx.scope(
                            move |command| AppCommand::Content(window, command),
                            |fx| entity.dismiss_modal(store, fx),
                        );
                        entity.mount_focused(widget);
                        crate::Windows::put(store, window, entity);
                    }
                    None => {
                        crate::Windows::put(store, window, entity);
                    }
                }
                if let Some(request) = side_request {
                    let mut entity =
                        crate::Windows::window(store, window).expect("the window entity");
                    fx.scope(
                        move |command| AppCommand::Content(window, command),
                        |fx| entity.dismiss_side_panel(store, fx),
                    );
                    match request {
                        ModalRequest::Close => {
                            crate::Windows::put(store, window, entity);
                        }
                        ModalRequest::Perform(command) => {
                            crate::Windows::put(store, window, entity);
                            self.perform(store, ui, command, fx);
                        }
                        ModalRequest::ShowDocument(document) => {
                            entity.show_document(store, window, document, None, fx);
                            crate::Windows::put(store, window, entity);

                            crate::watch::sync_document_watches(store, fx);
                            crate::diffs::sync_stripe_bases(store, fx);
                        }
                        ModalRequest::OpenLocations(locations) => {
                            crate::Windows::put(store, window, entity);
                            crate::open_locations(store, window, &locations, fx);
                        }
                        ModalRequest::SelectWidget(widget) => {
                            entity.mount_focused(widget);
                            crate::Windows::put(store, window, entity);
                        }
                    }
                }
                if let Some(request) = dock_request {
                    let mut entity =
                        crate::Windows::window(store, window).expect("the window entity");
                    match request {
                        ModalRequest::Close => {
                            fx.scope(
                                move |command| AppCommand::Content(window, command),
                                |fx| entity.dismiss_dock(store, fx),
                            );
                            crate::Windows::put(store, window, entity);
                        }
                        ModalRequest::Perform(command) => {
                            crate::Windows::put(store, window, entity);
                            self.perform(store, ui, command, fx);
                        }
                        ModalRequest::ShowDocument(document) => {
                            entity.show_document(store, window, document, None, fx);
                            crate::Windows::put(store, window, entity);
                            crate::watch::sync_document_watches(store, fx);
                            crate::diffs::sync_stripe_bases(store, fx);
                        }
                        ModalRequest::OpenLocations(locations) => {
                            crate::Windows::put(store, window, entity);
                            crate::open_locations(store, window, &locations, fx);
                        }
                        ModalRequest::SelectWidget(widget) => {
                            entity.mount_focused(widget);
                            crate::Windows::put(store, window, entity);
                        }
                    }
                }
                match panel_request {
                    Some(crate::PanelRequest::OpenLocations(locations)) => {
                        crate::open_locations(store, window, &locations, fx);
                    }
                    Some(crate::PanelRequest::Perform(command)) => {
                        self.perform(store, ui, AppCommand::Dynamic(window, command), fx);
                    }
                    None => {}
                }
                match toolbar_request {
                    Some(crate::ToolbarRequest::Command(id)) => {
                        if let Some(command) =
                            crate::commands::Commands::of(store).find(id).cloned()
                        {
                            self.perform(store, ui, AppCommand::Dynamic(window, command), fx);
                        }
                    }
                    Some(crate::ToolbarRequest::Query(raw)) => {
                        crate::toolbar::toolbar_query(store, ui, window, &raw, fx);
                    }
                    None => {}
                }

                for request in crate::commands::AppRequests::drain(store) {
                    self.perform(store, ui, AppCommand::Dynamic(window, request), fx);
                }
            }
            AppCommand::Stats(command) => {
                fx.scope(AppCommand::Stats, |fx| {
                    self.stats.perform(store, ui, command, fx)
                });
            }
            AppCommand::Entity(document, command) => {
                if OpenDocuments::contains(store, document) {
                    entity_scope(document, fx, |fx| deliver(store, ui, document, command, fx));
                }
            }
            AppCommand::Dynamic(window, command) => command.perform(self, store, window, fx),
            AppCommand::Landing(window, command) => command.perform(self, store, window, fx),

            AppCommand::InSession(_, command) => self.perform(store, ui, *command, fx),
            AppCommand::Register(command) => {
                crate::commands::Commands::register(store, command);
            }
            AppCommand::BaseLocated { document, base } => {
                crate::diffs::land_base_located(store, document, base, fx);
            }
            AppCommand::BaseFetched {
                document,
                base,
                text,
            } => {
                let Some(text) = text else {
                    return;
                };
                if !OpenDocuments::contains(store, document) {
                    return;
                }
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::BuildDocumentEffect {
                        location: base.clone(),
                        text,
                        prep: None,
                    })
                    .map(move |built| AppCommand::BaseBuilt {
                        document,
                        base,
                        built: built.document,
                    }),
                );
            }
            AppCommand::BaseBuilt {
                document,
                base,
                built,
            } => {
                crate::diffs::land_base_built(store, document, base, built, fx);
            }
            AppCommand::DiffNormalized {
                diff,
                operation,
                markup,
                changed,
                base_revision,
                target_revision,
            } => {
                if crate::diffs::land_normalized(
                    store,
                    diff,
                    operation,
                    base_revision,
                    target_revision,
                ) {
                    if let Some(handle) = crate::OpenDocuments::diff_handle(store, diff) {
                        entity_scope(handle.target, fx, |fx| {
                            documents::diffs::land_diff_markup(
                                store,
                                diff,
                                markup,
                                changed,
                                target_revision,
                                fx,
                            )
                        });
                    }
                    self.pending_diff_events.push(diff);
                }
            }
            AppCommand::DocumentStored {
                document,
                revision,
                snapshot,
                stored,
            } => match stored {
                true => crate::OpenDocuments::mark_saved(store, document, revision, snapshot),
                false => eprintln!("[himark] store failed for an open document"),
            },
            AppCommand::FileChanged(subscription) => {
                crate::watch::refetch_watched(store, subscription, fx);

                self.pending_file_events.push(subscription);
            }
            AppCommand::Watched(document, subscription) => {
                // The channel may have gone live while the subscribe
                // was in flight: mode one holds — the host watches
                // the file, this subscription is surplus.
                if crate::OpenDocuments::host_synced(store, document) {
                    if let Some(subscription) = subscription {
                        let _ = fx.push(imba::effect::AnyEffect::notification(
                            crate::watch::UnsubscribeEffect { subscription },
                        ));
                    }
                    return;
                }
                crate::OpenDocuments::set_watch(store, document, subscription);
            }
            AppCommand::Refetched {
                document,
                serial,
                text,
            } => {
                crate::watch::apply_refetched(store, document, serial, text, fx);
            }
            AppCommand::RefetchDiffed {
                document,
                base_revision,
                serial,
                rebase,
            } => {
                let documents::watch::RefetchRebase {
                    operation,
                    fetched,
                    fetched_source,
                    synced,
                    ..
                } = rebase;
                let retry = entity_scope(document, fx, |fx| {
                    crate::OpenDocuments::absorb_refetched(
                        store,
                        document,
                        base_revision,
                        serial,
                        &operation,
                        fetched,
                        synced,
                        fx,
                    )
                });
                if retry {
                    documents::watch::rediff(
                        store,
                        document,
                        serial,
                        fetched_source,
                        fx,
                        |document, base_revision, serial, rebase| AppCommand::RefetchDiffed {
                            document,
                            base_revision,
                            serial,
                            rebase,
                        },
                    );
                }
            }
            AppCommand::Opened(window, opened) => {
                let document = opened.document;
                let saved_revision = document.revision();

                let document_id = match opened
                    .location
                    .as_ref()
                    .and_then(|location| OpenDocuments::by_location(store, location))
                {
                    Some(existing) => existing,
                    None => OpenDocuments::register(
                        store,
                        document.clone(),
                        opened.location,
                        opened.name,
                        saved_revision,
                    ),
                };

                crate::watch::sync_document_watches(store, fx);
                crate::diffs::sync_stripe_bases(store, fx);

                if opened.primary {
                    if let Some(mut entity) = crate::Windows::window(store, window) {
                        entity.show_document(store, window, document_id, opened.target, fx);
                        crate::Windows::put(store, window, entity);
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
                fx.push(open_effect(window, name, primary, location, build));
            }
            AppCommand::OpenPanel(window, panel) => {
                let mut entity = crate::Windows::window(store, window).expect("the window entity");
                entity.open_panel(store, panel, fx);
                crate::Windows::put(store, window, entity);
            }
            AppCommand::OpenModal(window, modal) => {
                let mut entity = crate::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.show_modal(store, modal, fx),
                );
                crate::Windows::put(store, window, entity);
            }
            AppCommand::CloseModal(window) => {
                let mut entity = crate::Windows::window(store, window).expect("the window entity");
                fx.scope(
                    move |command| AppCommand::Content(window, command),
                    |fx| entity.dismiss_modal(store, fx),
                );
                crate::Windows::put(store, window, entity);
            }
            AppCommand::ViewportResized(window, size) => {
                let mut entity = crate::Windows::window(store, window).expect("the window entity");
                entity.set_viewport_size(size);
                crate::Windows::put(store, window, entity);
            }
            AppCommand::RegisterLanguages(languages) => {
                store.put(::editor::env::Parsers(std::sync::Arc::new(languages)));
            }
            AppCommand::RegisterEnrichers(enrichers) => {
                store.put(::editor::env::Enrichers(std::sync::Arc::new(enrichers)));
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
        let entity = crate::Windows::window_ref(store, window).expect("the window entity");
        let content = imba::Layout::layout(entity.display(arena, store, ui), arena, constraints);
        let stats = imba::Layout::layout(self.stats.display(arena, store, ui), arena, constraints);

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
