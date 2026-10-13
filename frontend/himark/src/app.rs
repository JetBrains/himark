// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::DocumentsCommand;

use std::sync::OnceLock;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effect,
    event::{Event, EventResult},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use skia_safe::{Canvas, Rect, Size};
use text::text::Text;

use ::workbench::window::Window;
use ::workbench::window::WindowId;
use ::workbench::window::Windows;
use ::workbench::workbench::Workbench;
use ::workbench::workbench_node::Panel;
use ::workbench::workbench_node::WorkbenchNode;
use documents::entity_view::EditorIdView;
use documents::lifecycle::mount_editor;
use documents::OpenDocuments;
use editor::document::Document;
use editor::markup::Markup;

#[cfg(any(test, feature = "test-support"))]
pub use crate::himark::StoreMut;
pub use crate::himark::{AppFonts, Himark};
pub(crate) use crate::perform::{command_label, AppEffects};
pub use crate::perform::{shell_verb, verb_command, AppCommand, AppFx};
pub use crate::shell::{
    close_modal, content, open_async, open_modal, open_panel, opened, register, shell,
    viewport_resized, windowed, OneShot, ShellCommand,
};

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
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

pub(crate) type DocumentBuild = Box<
    dyn FnOnce(
            &imba::store::Store,
            &UiCtx,
            &skia_safe::textlayout::FontCollection,
            &::editor::theme::Theme,
        ) -> Document
        + Send
        + Sync,
>;

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
    recents::RecentLocations::touch(store, state.recents(), &location);
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
    if crate::workspace::entity_session(&entity) == target {
        return;
    }
    fx.scope(
        move |command| crate::shell::content(window, command),
        |fx| entity.dismiss_modal(store, fx),
    );
    fx.scope(
        move |command| crate::shell::content(window, command),
        |fx| entity.dismiss_side_panel(store, fx),
    );

    // Session ENTRY: the one legitimate catalog consult — the bundle
    // is wired into the window's record here and read from it after.
    let state = ahp_session::session::state::Hosts::ensure_state(store, &target);
    let owed = entity.switch_to(crate::workspace::SessionWorkspace::boxed(target, state));
    ::workbench::window::Windows::put(store, window, entity);
    if let Some(previous) = owed {
        fx.follow_up(crate::shell::windowed(
            window,
            std::sync::Arc::new(EnterFreshSession {
                previous: std::sync::Mutex::new(Some(previous)),
            }),
        ));
    }
}

struct EnterFreshSession {
    previous: std::sync::Mutex<Option<Box<dyn ::workbench::window::Workspace>>>,
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
        let Some(previous) = self.previous.lock().expect("fresh workspace").take() else {
            return;
        };
        entity.install_fresh(previous, Workbench::new(WorkbenchNode::vacant()));
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn markdown_scratch() -> Document {
    Document::new(Text::from_string_exact(""), Markup::new()).with_syntax(
        ::editor::markup::Syntax::new("markdown", None, Markup::new()),
        &[],
    )
}

impl Himark {
    /// The workbench SHELL's boot: the bare engine plus everything
    /// window-shaped — the windows catalog, the windowed commands,
    /// the workbench registry roads. A shell that composes its own
    /// frame boots `Himark::bare` and mounts the views it needs.
    pub fn new(fonts: AppFonts) -> Self {
        let mut app = Himark::bare(
            fonts,
            std::sync::Arc::new(MountFrontend {
                next: WorkbenchFrontend,
            }),
        );
        app.setup(|store| {
            store.put_entity(crate::shell::shell_id(), crate::shell::WorkbenchShell);
            crate::commands::register_builtins(store);
            ::workbench::navigation::Navigators::register_windowed(
                store,
                crate::workspace::EditorNavigator,
            );
            ::workbench::registry::Registry::update(store, |registry| {
                registry.run_command = Some(std::sync::Arc::new(|store, id| {
                    if let Some(command) = crate::commands::Commands::of(store).find(id).cloned() {
                        crate::commands::AppRequests::push(store, command);
                    }
                }));
                registry.outline = Some(std::sync::Arc::new(
                    |store, ui, documents, document, location, window| {
                        let jump = std::sync::Arc::new(move |place| {
                            hikit::modal::ModalRequest::Perform(crate::app::shell_verb(
                                crate::shell::windowed(
                                    window,
                                    std::sync::Arc::new(crate::toc::NavigateToPlace { place }),
                                ),
                            ))
                        });
                        Box::new(toc::OutlineView::new(
                            store, ui, documents, document, location, jump,
                        )) as Box<dyn hikit::modal::ModalView>
                    },
                ));
                registry.drawer_button = Some(crate::toc::toolbar_button());
            });
            crate::higent::chat_roads::install_shell_roads(store);
            store.put(::workbench::window::Windows::default());
        });
        app
    }

    /// The windows catalog, read in place — a boot resident.
    pub(crate) fn windows(&self) -> &::workbench::window::Windows {
        self.store
            .get::<::workbench::window::Windows>()
            .expect("the windows catalog is a boot resident")
    }

    pub fn viewport_stale(&self, window: WindowId, size: Size) -> bool {
        self.windows()
            .entity(window)
            .is_some_and(|entity| entity.viewport_stale(size))
    }

    pub fn window_store(&self, window: WindowId) -> Store {
        let _ = window;
        self.store.clone()
    }

    /// The window's current frame. The frame machinery itself is
    /// window-blind: the engine asks the BRIDGE, and the workbench
    /// adapter is the only party that knows a window id lives
    /// inside.
    pub(crate) fn ensure_frame(
        &mut self,
        window: WindowId,
        size: Size,
    ) -> &mut crate::bridge::Frame {
        let id = crate::bridge::ViewBridgeId::from_raw(window.raw());
        let revision = self.revision;
        let slot = self.bridges.entry(id).or_insert_with(|| {
            crate::himark::Mounted::new(std::rc::Rc::new(WorkbenchView { window }))
        });
        let fresh = slot.frame.is_some() && slot.revision == revision && slot.size == size;
        if !fresh {
            if let Some(frame) = slot.frame.take() {
                slot.spare_arena = Some(frame.recycle());
            }
            let arena = slot.spare_arena.take().unwrap_or_default();
            // The pipeline, staged in the open: lay the window out
            // at its size, realize the WHOLE rect — a window fills
            // its viewport; a native scroller holds the layout and
            // realizes slices instead.
            let layout = slot
                .bridge
                .layout(&self.store, &self.ui, Constraints::tight(size), arena);
            slot.frame = Some(layout.realize(Rect::from_size(size)));
            slot.revision = revision;
            slot.size = size;
        }
        slot.frame.as_mut().expect("the frame just derived")
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
            .update::<::workbench::window::Windows>(|windows| {
                windows.adopt_workspaces_all(&crate::workspace::AdoptLocalHost(host))
            });
        self.invalidate_frames();
    }

    pub fn add_window(&mut self) -> WindowId {
        let workspace = ahp_wire::SessionId::local_default(&self.store);
        let ui = self.ui_ctx();
        let mut discarded = AppEffects::new();
        let state = ahp_session::session::state::Hosts::ensure_state(&mut self.store, &workspace);
        let editors = fresh_workbench_root(&mut self.store, &state, &ui, &mut discarded.effects());
        let window = Windows::add(
            &mut self.store,
            Window::new(
                editors,
                crate::workspace::SessionWorkspace::boxed(workspace.clone(), state),
            ),
        );
        self.invalidate_frames();
        window
    }

    pub(crate) fn register_windowed_navigator<N: ::workbench::navigation::WindowedNavigator>(
        &mut self,
        navigator: N,
    ) {
        self.setup(|store| {
            ::workbench::navigation::Navigators::register_windowed(store, navigator)
        });
    }

    pub fn register_navigator<N: hikit::navigation::Navigator>(&mut self, navigator: N) {
        self.setup(|store| ::workbench::navigation::Navigators::register(store, navigator));
    }

    pub fn register_toolbar_button(&mut self, button: ::workbench::toolbar::ToolbarButton) {
        self.setup(|store| ::workbench::toolbar::ToolbarButtons::register(store, button));
    }

    pub fn register_row_minter(&mut self, minter: std::sync::Arc<hikit::panel::RowMinter>) {
        self.setup(|store| ::workbench::rows::RowMinters::register(store, minter));
    }

    pub fn set_chrome_clearance(&mut self, width: f32) {
        self.ui.set(::workbench::toolbar::ChromeClearance(width));
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
        ::workbench::window::Windows::window_ref(&store, window)
            .and_then(|entity| entity.dock_owner())
    }

    pub(crate) fn dispatch_paint(&mut self, window: WindowId, canvas: &Canvas, size: Size) -> bool {
        if self.windows().entity(window).is_none() {
            return self.draw_mounted(
                crate::bridge::ViewBridgeId::from_raw(window.raw()),
                canvas,
                size,
            );
        }
        let result = self.ensure_frame(window, size).handle(
            &Event::Paint {
                canvas,
                focused: true,
            },
            Rect::from_size(size),
        );
        let theme = ::editor::env::Themes::of(&self.store);
        self.stats
            .paint(canvas, Rect::from_size(size), &theme.ui().stats);

        match result {
            EventResult::Ignored => false,
            // The WINDOW road: its scroller consumed reveals en
            // route; one that reached the top dies as it always
            // did — the commands still run.
            EventResult::Handled(handled) => match handled.commands.is_empty() {
                true => false,
                false => {
                    trace_reconcile("paint", &handled.commands);
                    self.perform_batch(handled.commands)
                }
            },
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
            self.perform_batch(vec![crate::shell::viewport_resized(window, size)]);
        }
        let handled = self.dispatch(window, event, size);
        if handled {
            self.stats.mark_latency_event_start(event_started_at);
        }
        handled
    }

    pub fn dispatch(&mut self, window: WindowId, event: Event<'_>, size: Size) -> bool {
        self.dispatch_pointed(window, event, size).0
    }

    /// Dispatch, and beside the redraw flag the shape the pointer
    /// takes: a move hit-tests first and the hit's answer IS the
    /// shape — derived per event; the shell applies it and nothing
    /// stores it.
    pub fn dispatch_pointed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
    ) -> (bool, imba::event::PointerShape) {
        let (hit, shape) = match &event {
            Event::MouseMove { point, mods } => self.dispatch_event(
                window,
                Event::HitTest {
                    point: *point,
                    miss: false,
                    mods: *mods,
                },
                size,
            ),
            _ => (false, imba::event::PointerShape::Default),
        };
        let (changed, _) = self.dispatch_event(window, event, size);
        (changed || hit, shape)
    }

    fn dispatch_event(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
    ) -> (bool, imba::event::PointerShape) {
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
                let id = crate::bridge::ViewBridgeId::from_raw(window.raw());
                let result = {
                    // The chain comes from the BRIDGE's view: the
                    // workbench walks its window, a mounted view
                    // walks itself.
                    let chain = self
                        .bridges
                        .get(&id)
                        .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))
                        .or_else(|| crate::focus::window_focus_data(&store, ui.as_ref(), window));
                    match (&event, chain) {
                        (Event::KeyDown { key, mods }, Some(mut data)) => data.key(*key, *mods),
                        (Event::TextInput { text }, Some(mut data)) => data.text(text),
                        _ => EventResult::Ignored,
                    }
                };
                if std::env::var_os("HIMARK_TRACE_KEYS").is_some() {
                    if let Some((key, mods)) = key_down {
                        eprintln!(
                            "[keys] {key:?} {mods:?}: chain says {}; binding {:?}",
                            match &result {
                                EventResult::Ignored => "ignored",
                                EventResult::Handled(handled) => {
                                    match (handled.commands.is_empty(), handled.reveal.is_some()) {
                                        (true, true) => "reveal",
                                        (true, false) => "handled",
                                        (false, _) => "command",
                                    }
                                }
                            },
                            crate::keymap::Keymaps::binding_of(&store, key, mods)
                        );
                    }
                }
                let empty_handed = match &result {
                    EventResult::Ignored => true,
                    EventResult::Handled(handled) => handled.commands.is_empty(),
                };
                let fallback = match (key_down, empty_handed) {
                    (Some((key, mods)), true) => {
                        crate::keymap::Keymaps::binding_of(&store, key, mods).and_then(|binding| {
                            // The chain source mirrors the dispatch
                            // above: a mounted view's own focus, a
                            // window's walk — bound EDITING keys
                            // (backspace, enter, arrows) resolve
                            // through presentables either way.
                            self.bridges
                                .get(&id)
                                .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))
                                .or_else(|| {
                                    crate::focus::window_focus_data(&store, ui.as_ref(), window)
                                })
                                .map(|data| data.commands)
                                .unwrap_or_default()
                                .into_iter()
                                .find(|presentable| presentable.id == binding.as_ref())
                                .map(|presentable| presentable.command)
                                .or_else(|| {
                                    // The global registry speaks
                                    // WINDOWED commands — a window
                                    // road only.
                                    if self.windows().entity(window).is_none() {
                                        return None;
                                    }
                                    crate::commands::Commands::of(&store)
                                        .find(binding.as_ref())
                                        .map(|command| {
                                            crate::shell::windowed(window, command.clone())
                                        })
                                })
                        })
                    }
                    _ => None,
                };
                (result, fallback)
            }
            _ => {
                let result = if self.windows().entity(window).is_some() {
                    self.ensure_frame(window, size)
                        .handle(&event, Rect::from_size(size))
                } else {
                    // A mounted view routes against its STANDING
                    // frame at its own size; the host's passed size
                    // is the window road's business.
                    self.handle_mounted(crate::bridge::ViewBridgeId::from_raw(window.raw()), &event)
                };
                (result, None)
            }
        };

        if let Some(command) = fallback {
            if std::env::var_os("HIMARK_TRACE_KEYS").is_some() {
                eprintln!("[keys] fallback performs the bound command");
            }
            return (
                self.perform_batch(vec![command]),
                imba::event::PointerShape::Default,
            );
        }

        let changed = match result {
            EventResult::Ignored => return (false, imba::event::PointerShape::Default),
            EventResult::Handled(handled) => {
                let shape = handled.pointer.unwrap_or_default();
                // A mounted view's reveal is the HOST's to scroll —
                // park it for `take_reveal`; a window's dies here as
                // it always did (its scroller consumes en route).
                let mut parked = false;
                if let Some(reveal) = handled.reveal {
                    let bridge = crate::bridge::ViewBridgeId::from_raw(window.raw());
                    if bridge.is_mounted() {
                        if std::env::var_os("HIMARK_TRACE_REVEAL").is_some() {
                            let slot = self.bridges.get(&bridge);
                            eprintln!(
                                "[reveal] parked (mounted dispatch) bridge {} {:?} band {:?} size {:?}",
                                bridge.raw(),
                                reveal,
                                slot.and_then(|slot| slot.stage_viewport),
                                slot.map(|slot| slot.size),
                            );
                        }
                        self.park_reveal(bridge, reveal);
                        parked = true;
                    }
                }
                let changed = match handled.commands.is_empty() {
                    // An empty answer on a clock tick is NOT a
                    // change — but a parked reveal is.
                    true => parked || !clock,
                    false => {
                        if clock {
                            trace_reconcile("clock", &handled.commands);
                        }
                        self.perform_batch(handled.commands) || parked
                    }
                };
                return (changed, shape);
            }
        };
        #[allow(unreachable_code)]
        (changed, imba::event::PointerShape::Default)
    }

    /// The workbench shell's batch TAIL — every window-shaped
    /// reaction to a finished engine batch (`Himark::perform_batch`
    /// calls back here once the store is committed and the effects
    /// launched): pane validation, theme fan-out, file-event
    /// delivery, the settle rounds, the dock tree-follow.
    pub(crate) fn batch_tail(&mut self, label: &str, theme_changed: bool) {
        let ui = self.ui.clone();
        // Invalidation is lazy (a revision bump), so a window that
        // closed this batch must drop its bridge here or the slot's
        // last frame — and the store snapshot it pins — lingers.
        // MOUNTED slots are not window-claimed: the host owns their
        // lifetime (`unmount`), so the reap must spare them.
        let live: std::collections::HashSet<u64> = self
            .windows()
            .ids()
            .into_iter()
            .map(|window| window.raw())
            .collect();
        self.bridges
            .retain(|id, _| id.is_mounted() || live.contains(&id.raw()));
        if validate_enabled() {
            for id in self.windows().ids() {
                let Some(window) = Windows::window_ref(&self.store, id) else {
                    continue;
                };
                validate_panes(label, &self.store, &window.workbench().root);

                for (_session, stashed) in window.stashed_workbenches() {
                    validate_panes(label, &self.store, &stashed.root);
                }
            }
        }
        if theme_changed {
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
                // The MOUNTED bridges hear the pulse too: the canvas
                // re-aims its row editors off Settle — a deaf mount
                // shaped only each row's birth viewport and painted
                // black below it.
                for id in self.mounted_ids() {
                    self.dispatch(
                        ::workbench::window::WindowId::from_raw(id.raw()),
                        Event::Settle,
                        Size::default(),
                    );
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
                let location_for_tree = location.clone();
                self.window_txn(window, |store| {
                    if let Some(mut entity) = Windows::window(store, window) {
                        entity.note_focused_location(location);
                        Windows::put(store, window, entity);
                    }
                });
                // The EXPLICIT follow push — the same TreeCommand the
                // native host sends through `himark_tree_follow`; the
                // probe that read window focus per paint is gone
                // (docs/mounted-parity.md: one API, both shells).
                let (owner, generation) = {
                    let store = self.window_store(window);
                    let entity = Windows::window_ref(&store, window);
                    (
                        entity.and_then(|entity| entity.dock_owner()),
                        entity.map(|entity| entity.focus_generation()).unwrap_or(0),
                    )
                };
                if owner == Some("files.tree") {
                    self.perform_batch(vec![crate::shell::content(
                        window,
                        ::workbench::window::WindowCommand::Dock(imba::dyn_view::DynCommand::new(
                            ::workbench::dock::DockCommand::Content(
                                imba::dyn_view::DynCommand::new(
                                    imba::scroll::ScrollCommand::Content(
                                        filetree::TreeCommand::Follow {
                                            location: location_for_tree,
                                            generation,
                                        },
                                    ),
                                ),
                            ),
                        )),
                    )]);
                }
            }
        }
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
        crate::shell::opened(
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

fn trace_reconcile(source: &str, commands: &[AppCommand]) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RECONCILE").is_some()) {
        return;
    }
    let labels: Vec<_> = commands.iter().map(command_label).collect();
    eprintln!("[reconcile] {source} reported: {}", labels.join(", "));
}

pub(crate) fn trace_stalls_enabled() -> bool {
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

/// The MOUNTED-origin resolver in the shell chain: places that
/// need OPENING (a located document, a diff of two locations) are
/// the engine's to resolve — it launches the same open effects the
/// workbench does, bound to the mount's session, with the mounted
/// id riding where a window id would. The landings re-navigate
/// with READY places (`OpenDocumentPlace`, `DiffViewPlace`), which
/// a native shell claims and mints synchronously. Window origins
/// pass straight through.
pub struct MountFrontend {
    pub next: WorkbenchFrontend,
}

impl crate::himark::Shell for MountFrontend {
    fn navigate(
        &self,
        origin: crate::bridge::ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        if !origin.is_mounted() {
            return self.next.navigate(origin, place, store, ui, fx);
        }
        let rides = ::workbench::window::WindowId::from_raw(origin.raw());
        if let Some(place) = place.place::<crate::places::DocumentPlace>() {
            let session = crate::mounts::session_of(store, origin);
            if std::env::var_os("HIMARK_TRACE_NAV").is_some() {
                eprintln!(
                    "[nav] MountFrontend DocumentPlace '{}' origin {} session? {}",
                    place.location.name(),
                    origin.raw(),
                    session.is_some()
                );
            }
            let Some(session) = session else {
                return false;
            };
            let documents =
                ahp_session::session::state::Hosts::ensure_state(store, &session).documents();
            fx.push(
                crate::workspace::open_by_location_effect(
                    rides,
                    documents,
                    place.location.clone(),
                    true,
                    place.focus,
                    place.target.clone(),
                )
                .map(|command| imba::command::Verb::shell(Box::new(command))),
            );
            return true;
        }
        if let Some(place) = place.place::<crate::places::DiffPlace>() {
            let Some(session) = crate::mounts::session_of(store, origin) else {
                return false;
            };
            let documents =
                ahp_session::session::state::Hosts::ensure_state(store, &session).documents();
            let old =
                documents::diff_views::DiffSideInput::resolve(store, documents, place.old.clone());
            let new =
                documents::diff_views::DiffSideInput::resolve(store, documents, place.new.clone());
            fx.push(
                imba::effect::AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
                    window: rides,
                    documents,
                    old,
                    new,
                })
                .map(|command| imba::command::Verb::shell(Box::new(command))),
            );
            return true;
        }
        if let Some(batch) = place.place::<crate::places::LocationsPlace>() {
            // v1 of the mounted batch: the FIRST location opens; a
            // results screen takes this seat later.
            let Some(location) = batch.0.first().cloned() else {
                return true;
            };
            return self.navigate(
                origin,
                hikit::navigation::NavigationLocation::new(crate::places::DocumentPlace {
                    location,
                    target: None,
                    focus: true,
                }),
                store,
                ui,
                fx,
            );
        }
        false
    }
}

/// The workbench AS the shell (`crate::himark::Shell`): placement
/// answered inline — panes, docks, splits — with the store and fx
/// in hand. This is the placement half of the old window arms,
/// relocated behind the trait; a native shell decorates it and
/// takes places over one kind at a time.
pub struct WorkbenchFrontend;

impl crate::himark::Shell for WorkbenchFrontend {
    fn navigate(
        &self,
        origin: crate::bridge::ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        let window = ::workbench::window::WindowId::from_raw(origin.raw());
        if let Some(place) = place.place::<crate::places::DocumentPlace>() {
            let Some(state) = crate::workspace::session_state(store, window) else {
                return false;
            };
            // The landing is an AppCommand; the verb lane carries
            // it home through the shell-payload road.
            fx.push(
                crate::workspace::open_by_location_effect(
                    window,
                    state.documents(),
                    place.location.clone(),
                    true,
                    place.focus,
                    place.target.clone(),
                )
                .map(|command| imba::command::Verb::shell(Box::new(command))),
            );
            return true;
        }
        if let Some(place) = place.place::<crate::places::OpenDocumentPlace>() {
            let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
                return false;
            };
            entity.show_document(
                store,
                ui,
                window,
                place.document,
                place.target.clone(),
                place.focus,
                fx,
            );
            ::workbench::window::Windows::put(store, window, entity);
            if let Some(state) = crate::workspace::session_state(store, window) {
                documents::lanes::sync_document_watches(store, state.documents(), fx);
                documents::lanes::sync_stripe_bases(store, state.documents(), ui, fx);
            }
            return true;
        }
        if let Some(place) = place.place::<crate::places::DiffPlace>() {
            // A windowed command performs the pane work; it rides
            // the follow-up lane so it runs with its own scope.
            fx.follow_up(imba::command::Verb::shell(Box::new(
                crate::shell::windowed(
                    window,
                    std::sync::Arc::new(crate::hichanges::OpenDiffForPair {
                        old: place.old.clone(),
                        new: place.new.clone(),
                    }),
                ),
            )));
            return true;
        }
        if let Some(place) = place.place::<crate::places::LocationsPlace>() {
            fx.follow_up(imba::command::Verb::shell(Box::new(
                crate::shell::windowed(
                    window,
                    std::sync::Arc::new(OpenLocationsAsk {
                        locations: place.0.clone(),
                    }),
                ),
            )));
            return true;
        }
        false
    }
}

/// The locations batch as a windowed command — the follow-up
/// vehicle `WorkbenchFrontend` rides from the windowless lane.
struct OpenLocationsAsk {
    locations: Vec<editor::location::ResourceLocation>,
}

impl crate::commands::WindowedCommand for OpenLocationsAsk {
    fn id(&self) -> &'static str {
        "workspace.open-locations"
    }

    fn name(&self) -> String {
        "Open Locations".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        crate::workspace::open_locations(store, ui, window, &self.locations, fx);
    }
}

/// The workbench window as a REGULAR mounted view — all that
/// remains of "window" at the frame machinery. The adapter reads
/// its entity from the store, stamps the focused seat into the
/// frame snapshot, and wraps the entity's commands with its window
/// id. The stats HUD is painted OVER the tree by the paint
/// dispatch; per-frame numbers never ride a store walk.
pub(crate) struct WorkbenchView {
    window: WindowId,
}

impl crate::bridge::BridgeView for WorkbenchView {
    /// The frame's store: the live store plus the FOCUSED SEAT
    /// (the semantic walk's answer), so editors derive their
    /// selections-visible bit from STATE at build time — one
    /// viewport build per frame, no focused upgrade at paint.
    fn frame_store(&self, store: &Store, ui: &UiCtx) -> Store {
        let mut store = store.clone();
        let client = crate::focus::window_focus_data(&store, ui, self.window)
            .and_then(|mut data| data.seat.take());
        if let Some(client) = client {
            ::editor::env::FrameFocus::set(&mut store, client);
        }
        store
    }

    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        crate::focus::window_focus_data(store, ui, self.window)
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, AppCommand> {
        let size = constraints.max;
        let window = self.window;
        let entity =
            ::workbench::window::Windows::window_ref(store, window).expect("the window entity");
        let content =
            imba::layout::Layout::layout(entity.display(arena, store, ui), arena, constraints);

        let mut container = imba::container::container(arena, size);
        container.place(
            0.0,
            0.0,
            content
                .map(move |command| crate::shell::content(window, command))
                .overlay_host(imba::overlay::WINDOW),
        );
        imba::ThunkBox::new(arena, container)
    }
}

impl Himark {
    pub(crate) fn sync_viewport_window(
        &mut self,
        window: ::workbench::window::WindowId,
        size: skia_safe::Size,
    ) {
        if self.viewport_stale(window, size) {
            self.perform_batch(vec![crate::shell::viewport_resized(window, size)]);
        }
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn draw_window(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &skia_safe::Canvas,
    ) -> bool {
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
            let entity = ::workbench::window::Windows::window_ref(self.store(), window)
                .expect("the window entity");
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

    /// TEST SUPPORT: no production caller outside this crate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
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
