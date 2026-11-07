// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub use himark::app::*;

use himark::commands::Commands;
use imba::focus::FocusData;

use documents::DocumentsCommand;

use std::sync::OnceLock;

use gfx::{Canvas, Rect, Size};
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effect,
    event::Event,
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
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
pub use himark::himark::StoreMut;
pub use himark::himark::{AppFonts, Himark};
pub(crate) use himark::perform::{command_label, AppEffects};
pub use himark::perform::{shell_verb, AppCommand, AppFx};
pub use crate::shell::verb_command;
pub use crate::shell::{
    close_modal, content, open_async, open_modal, open_panel, opened, register, shell,
    viewport_resized, windowed, OneShot, ShellCommand,
};

pub use himark::opening::OpenedDocument;

pub(crate) type DocumentBuild =
    Box<dyn FnOnce(&imba::store::Store, &UiCtx, &::editor::theme::Theme) -> Document + Send + Sync>;

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

impl himark::commands::HimarkCommand for EnterFreshSession {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
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

    /// The workbench SHELL's boot: the bare engine plus everything
/// window-shaped — the windows catalog, the windowed commands,
/// the workbench registry roads. A shell that composes its own
/// frame boots `Himark::bare` and mounts the views it needs.
pub fn workbench_app(fonts: AppFonts) -> Himark {
    let mut app = Himark::bare(
        fonts,
        std::sync::Arc::new(MountFrontend {
            next: WorkbenchFrontend,
        }),
    );
    // The boot only COMPOSES: every registration lives in a
    // plugin's install. The workbench set goes in here (production
    // lists no longer pass it to `install_all`); the AHP session
    // half is the once-guarded belt for boots that install no
    // plugins (the shell tests) — `AhpPlugin` seats it too.
    let cx = himark::plugin::PluginContext {
        languages: std::sync::Arc::new(::editor::reparse::SyntaxLanguages::new()),
        diff_policy: std::sync::Arc::new(myersdiff::Myers),
    };
    himark::plugin::Plugin::install(&crate::ShellPlugin, &mut app, &cx);
    hiahp::install_session_half(&mut app);
    app
}

/// The workbench FURNITURE — everything the shell app registers at
/// boot, as the plugin install it always was in spirit: the shell
/// entity and builtins, the windowed navigators, the registry
/// closures, the windows catalog, the origin-bindings resolver,
/// the composer button, the batch tail, the open road. Idempotent:
/// a second install is a no-op.
/// Answers false when the furniture already stood (the caller's
/// own follow-ups skip too).
pub(crate) fn install_shell_furniture(app: &mut Himark) -> bool {
    #[derive(Clone)]
    struct ShellSeated;
    if app.store().get::<ShellSeated>().is_some() {
        return false;
    }
    app.setup(|store| {
        store.put(ShellSeated);
        store.put_entity(crate::shell::shell_id(), crate::shell::WorkbenchShell);
        crate::builtins::register_builtins(store);
        ::workbench::navigation::Navigators::register_windowed(
            store,
            crate::workspace::EditorNavigator,
        );
        ::workbench::navigation::Navigators::register_windowed(
            store,
            crate::diff_navigator::DiffNavigator,
        );
        ::workbench::registry::Registry::update(store, |registry| {
            registry.run_command = Some(std::sync::Arc::new(|store, id| {
                if let Some(command) = himark::commands::Commands::of(store).find(id).cloned() {
                    himark::commands::AppRequests::push(store, command);
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
        store.put(::workbench::window::Windows::default());
        // The workbench answers origin bindings for the origins
        // that ride a window's raw id: the REAL walk, not just
        // the root facts — a windowed mint resolves the same
        // bindings a palette capture would, the focused document
        // among them.
        store.put(himark::perform::OriginBindings(std::sync::Arc::new(
            |store, ui, origin| {
                let window = ::workbench::window::WindowId::from_raw(origin.raw());
                ::workbench::window::Windows::window_ref(store, window)?;
                crate::app::window_focus_data(store, ui, window)
                    .map(|data| data.bindings)
                    .or_else(|| Some(crate::shell::window_bindings(store, window)))
            },
        )));
    });
    // The composer button is WORKBENCH furniture, not a feature's.
    app.register_toolbar_button(::workbench::toolbar::composer_button());
    // The workbench's batch tail: slot reaping, pane validation,
    // the dock tree-follow — registered like any frame layer's.
    app.register_batch_tail(|app: &mut Himark, label: &str, _theme_changed: bool| {
        app.batch_tail(label)
    });
    // The workbench's own open road (the build-closure flavor,
    // `open_async`): its handler rides the shared workshop.
    let workshop = std::sync::Arc::clone(app.workshop());
    app.register_handler::<OpenEffect>(OpenHandler(workshop));
    // The drawer ROADS are standing shell wiring (the chat roads,
    // and the fold-back after a host lands — only a WINDOW gesture
    // re-opens the side slot; a mounted origin no-ops). The agents
    // BUTTON is `AgentsPlugin`'s, installed by shipping lists.
    app.setup(|store| {
        crate::higent::chat_roads::install_shell_roads(store);
        hiahp::higent::flows::AgentFlows::install_refold(
            store,
            std::sync::Arc::new(crate::higent::drawer::DrawerRefold),
        );
    });
    true
}

/// The workbench COMPOSITION over the engine: windows as
/// bridges, the window-id adapter, the batch tail. An extension
/// trait so every shell and test keeps method syntax.
pub trait WorkbenchShell {
    fn windows(&self) -> &::workbench::window::Windows;
    fn viewport_stale(&self, window: WindowId, size: Size) -> bool;
    fn window_store(&self, window: WindowId) -> Store;
    fn ensure_window_slot(&mut self, window: WindowId) -> himark::bridge::ViewBridgeId;
    fn window_txn(&mut self, window: WindowId, mutate: impl FnOnce(&mut Store));
    fn window_ids(&self) -> Vec<WindowId>;
    fn window_viewport(&self, window: WindowId) -> Option<Size>;
    fn designate_local_host(&mut self, host: ahp_wire::client::HostId);
    fn add_window(&mut self) -> WindowId;
    fn register_windowed_navigator<N: ::workbench::navigation::WindowedNavigator>(
        &mut self,
        navigator: N,
    );
    fn register_navigator<N: hikit::navigation::Navigator>(&mut self, navigator: N);
    fn register_toolbar_button(&mut self, button: ::workbench::toolbar::ToolbarButton);
    fn register_row_minter(&mut self, minter: std::sync::Arc<hikit::panel::RowMinter>);
    fn dock_owner_for_tests(&self, window: WindowId) -> Option<&'static str>;
    fn dispatch_paint(
        &mut self,
        window: WindowId,
        canvas: &Canvas<'_>,
        size: Size,
    ) -> bool;
    fn dispatch_timed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
        event_started_at: f64,
    ) -> bool;
    fn dispatch(&mut self, window: WindowId, event: Event<'_>, size: Size) -> bool;
    fn dispatch_pointed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
    ) -> (bool, imba::event::PointerShape);
    fn window_origin(
        &mut self,
        window: WindowId,
        size: Size,
    ) -> (himark::bridge::ViewBridgeId, Option<Size>);
    fn batch_tail(&mut self, label: &str);
    fn sync_viewport_window(
        &mut self,
        window: ::workbench::window::WindowId,
        size: gfx::Size,
    );
    fn draw_window(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
    ) -> bool;
    fn draw_window_sized(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
        size: gfx::Size,
    ) -> bool;
    #[cfg(any(test, feature = "test-support"))]
    fn draw_window_profiled(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
        size: gfx::Size,
    ) -> std::time::Duration;

}

impl WorkbenchShell for Himark {


    /// The windows catalog, read in place — a boot resident.
    fn windows(&self) -> &::workbench::window::Windows {
        self.store()
            .get::<::workbench::window::Windows>()
            .expect("the windows catalog is a boot resident")
    }

    fn viewport_stale(&self, window: WindowId, size: Size) -> bool {
        self.windows()
            .entity(window)
            .is_some_and(|entity| entity.viewport_stale(size))
    }

    fn window_store(&self, window: WindowId) -> Store {
        let _ = window;
        self.store().clone()
    }

    /// The window's bridge slot, minted on first touch — windows
    /// ride the ONE bridge road (`mounted_frame`, `handle_mounted`,
    /// `draw_mounted`); this adapter is the only party that knows a
    /// window id lives inside.
    fn ensure_window_slot(&mut self, window: WindowId) -> himark::bridge::ViewBridgeId {
        let id = himark::bridge::ViewBridgeId::from_raw(window.raw());
        self.mount_as(id, std::sync::Arc::new(WorkbenchView { window }));
        id
    }


    fn window_txn(&mut self, window: WindowId, mutate: impl FnOnce(&mut Store)) {
        let _ = window;
        self.with_store(mutate);
        self.sweep();
    }

    fn window_ids(&self) -> Vec<WindowId> {
        self.windows().ids()
    }

    fn window_viewport(&self, window: WindowId) -> Option<Size> {
        self.windows()
            .entity(window)
            .map(|entity| entity.viewport_size())
    }

    fn designate_local_host(&mut self, host: ahp_wire::client::HostId) {
        hiahp::engine::set_local_host(self, host);
        self.with_store(|store| {
            store.update::<::workbench::window::Windows>(|windows| {
                windows.adopt_workspaces_all(&crate::workspace::AdoptLocalHost(host))
            })
        });
    }

    fn add_window(&mut self) -> WindowId {
        let ui = self.ui_ctx();
        let mut discarded = AppEffects::new();
        let window = self.with_store(|store| {
            let workspace = ahp_wire::SessionId::local_default(store);
            let state = ahp_session::session::state::Hosts::ensure_state(store, &workspace);
            let editors = fresh_workbench_root(store, &state, &ui, &mut discarded.effects());
            Windows::add(
                store,
                Window::new(
                    editors,
                    crate::workspace::SessionWorkspace::boxed(workspace, state),
                ),
            )
        });
        // The window IS a bridge from birth — one road for frames,
        // events, IME and the fan-outs (no lazy window special case).
        self.ensure_window_slot(window);
        self.invalidate_frames();
        window
    }

    fn register_windowed_navigator<N: ::workbench::navigation::WindowedNavigator>(
        &mut self,
        navigator: N,
    ) {
        self.setup(|store| {
            ::workbench::navigation::Navigators::register_windowed(store, navigator)
        });
    }

    fn register_navigator<N: hikit::navigation::Navigator>(&mut self, navigator: N) {
        self.setup(|store| ::workbench::navigation::Navigators::register(store, navigator));
    }

    fn register_toolbar_button(&mut self, button: ::workbench::toolbar::ToolbarButton) {
        self.setup(|store| ::workbench::toolbar::ToolbarButtons::register(store, button));
    }

    fn register_row_minter(&mut self, minter: std::sync::Arc<hikit::panel::RowMinter>) {
        self.setup(|store| ::workbench::rows::RowMinters::register(store, minter));
    }

    /// TEST SUPPORT: the fronting dock's owner id, if any.
    #[doc(hidden)]
    fn dock_owner_for_tests(&self, window: WindowId) -> Option<&'static str> {
        let store = self.window_store(window);
        ::workbench::window::Windows::window_ref(&store, window)
            .and_then(|entity| entity.dock_owner())
    }

    fn dispatch_paint(
        &mut self,
        window: WindowId,
        canvas: &Canvas<'_>,
        size: Size,
    ) -> bool {
        let (id, live) = self.window_origin(window, size);
        let reconciled = self.draw_mounted(id, canvas, size);
        if live.is_some() {
            // The stats HUD paints OVER the window's tree; per-frame
            // numbers never ride a store walk.
            let theme = ::editor::env::Themes::of(self.store());
            self.stats_mut().paint(
                canvas,
                Rect::from_size(size),
                &theme.part(himark::stats_chrome::StatsChrome::KEY),
            );
        }
        reconciled
    }

    fn dispatch_timed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
        event_started_at: f64,
    ) -> bool {
        if self.viewport_stale(window, size) {
            self.perform_batch(vec![crate::shell::viewport_resized(window, size)]);
        }
        let (id, live) = self.window_origin(window, size);
        self.dispatch_bridge_timed(id, event, live, event_started_at)
    }

    fn dispatch(&mut self, window: WindowId, event: Event<'_>, size: Size) -> bool {
        self.dispatch_pointed(window, event, size).0
    }

    fn dispatch_pointed(
        &mut self,
        window: WindowId,
        event: Event<'_>,
        size: Size,
    ) -> (bool, imba::event::PointerShape) {
        let (id, live) = self.window_origin(window, size);
        self.dispatch_bridge(id, event, live)
    }

    /// The window adapter's fork, in ONE place: a live window
    /// ensures its bridge slot and dispatches at the host's size; a
    /// mounted id riding the same raw id dispatches at its standing
    /// size (the passed size is the window road's business).
    fn window_origin(
        &mut self,
        window: WindowId,
        size: Size,
    ) -> (himark::bridge::ViewBridgeId, Option<Size>) {
        if self.windows().entity(window).is_some() {
            (self.ensure_window_slot(window), Some(size))
        } else {
            (himark::bridge::ViewBridgeId::from_raw(window.raw()), None)
        }
    }

    /// The workbench shell's batch TAIL — the window-shaped
    /// reactions to a finished engine batch (registered in
    /// `Himark::new`; the engine's own fan-outs ran already): slot
    /// reaping, pane validation, the dock tree-follow.
    fn batch_tail(&mut self, label: &str) {
        let ui = self.ui_ctx();
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
        self.retain_bridges(|id| id.is_mounted() || live.contains(&id.raw()));
        if validate_enabled() {
            for id in self.windows().ids() {
                let Some(window) = Windows::window_ref(self.store(), id) else {
                    continue;
                };
                validate_panes(label, self.store(), &window.workbench().root);

                for (_session, stashed) in window.stashed_workbenches() {
                    validate_panes(label, self.store(), &stashed.root);
                }
            }
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
            let followed = crate::app::window_focus_data(&store, ui.as_ref(), window)
                .and_then(|mut data| himark::focus::focused_location(&mut data));
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

    fn sync_viewport_window(
        &mut self,
        window: ::workbench::window::WindowId,
        size: gfx::Size,
    ) {
        if self.viewport_stale(window, size) {
            self.perform_batch(vec![crate::shell::viewport_resized(window, size)]);
        }
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    fn draw_window(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
    ) -> bool {
        let size = canvas.base_layer_size();
        self.draw_window_sized(
            window,
            canvas,
            gfx::Size::new(size.width as f32, size.height as f32),
        )
    }

    fn draw_window_sized(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
        size: gfx::Size,
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
    #[cfg(any(test, feature = "test-support"))]
    fn draw_window_profiled(
        &mut self,
        window: ::workbench::window::WindowId,
        canvas: &gfx::Canvas<'_>,
        size: gfx::Size,
    ) -> std::time::Duration {
        self.stats_mut().begin_frame();
        self.sync_viewport_window(window, size);
        let paint_started = std::time::Instant::now();
        let _ = self.dispatch_paint(window, canvas, size);
        paint_started.elapsed()
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
        let theme = self.0.theme();
        let document = self
            .0
            .with_ctx(|store, ui| (effect.build)(store, ui, &theme));
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

pub(crate) fn trace_reconcile(source: &str, commands: &[AppCommand]) {
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

impl himark::himark::Shell for MountFrontend {
    fn navigate(
        &self,
        origin: himark::bridge::ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        if !origin.is_mounted() {
            return self.next.navigate(origin, place, store, ui, fx);
        }
        let _rides = ::workbench::window::WindowId::from_raw(origin.raw());
        if let Some(place) = place.place::<himark::places::DocumentPlace>() {
            let session = hiahp::sessions::session_of(store, origin);
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
                    origin,
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
        if let Some(place) = place.place::<himark::places::DiffPlace>() {
            let Some(session) = hiahp::sessions::session_of(store, origin) else {
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
                    origin,
                    documents,
                    old,
                    new,
                })
                .map(|command| imba::command::Verb::shell(Box::new(command))),
            );
            return true;
        }
        if let Some(batch) = place.place::<himark::places::LocationsPlace>() {
            // v1 of the mounted batch: the FIRST location opens; a
            // results screen takes this seat later.
            let Some(location) = batch.0.first().cloned() else {
                return true;
            };
            return self.navigate(
                origin,
                hikit::navigation::NavigationLocation::new(himark::places::DocumentPlace {
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

/// The workbench AS the shell (`himark::himark::Shell`): placement
/// answered inline — panes, docks, splits — with the store and fx
/// in hand. This is the placement half of the old window arms,
/// relocated behind the trait; a native shell decorates it and
/// takes places over one kind at a time.
pub struct WorkbenchFrontend;

impl himark::himark::Shell for WorkbenchFrontend {
    fn navigate(
        &self,
        origin: himark::bridge::ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        let window = ::workbench::window::WindowId::from_raw(origin.raw());
        if let Some(place) = place.place::<himark::places::DocumentPlace>() {
            let Some(state) = crate::workspace::session_state(store, window) else {
                return false;
            };
            // The landing is an AppCommand; the verb lane carries
            // it home through the shell-payload road.
            fx.push(
                crate::workspace::open_by_location_effect(
                    himark::bridge::ViewBridgeId::from_raw(window.raw()),
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
        if let Some(place) = place.place::<himark::places::OpenDocumentPlace>() {
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
        if let Some(place) = place.place::<himark::places::DiffPlace>() {
            // Resolve both sides on the UI thread — an open side
            // hands over its live registry snapshot, so the diff is
            // against the live buffer and rebases if it moves
            // (docs/no-diff-on-ui-thread). The landing navigates
            // back with the READY `DiffViewPlace`.
            let Some(state) = crate::workspace::session_state(store, window) else {
                return false;
            };
            let documents = state.documents();
            let old =
                documents::diff_views::DiffSideInput::resolve(store, documents, place.old.clone());
            let new =
                documents::diff_views::DiffSideInput::resolve(store, documents, place.new.clone());
            fx.push(
                imba::effect::AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
                    origin: himark::bridge::ViewBridgeId::from_raw(window.raw()),
                    documents,
                    old,
                    new,
                })
                .map(|command| imba::command::Verb::shell(Box::new(command))),
            );
            return true;
        }
        if let Some(place) = place.place::<himark::places::DiffViewPlace>() {
            // The READY pair: open the standalone split-diff panel
            // over it — the workbench twin of the native shell's
            // pushed screen.
            let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
                return false;
            };
            let panel = ::canvas::diff_pane::DiffPanelView::over(place.documents, place.view);
            let opened =
                entity.open_panel(store, ui, Box::new(hikit::modal::Scrolled::new(panel)), fx);
            ::workbench::window::Windows::put(store, window, entity);
            return opened;
        }
        if let Some(place) = place.place::<hiterminal::TerminalViewPlace>() {
            // The READY terminal: a panel over the filed session —
            // a refused panel hangs the pty up and unfiles it, the
            // old open_panel contract.
            let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
                return false;
            };
            let opened = entity.open_panel(
                store,
                ui,
                Box::new(terminals::pane::TerminalView::new(place.terminals, place.id)),
                fx,
            );
            ::workbench::window::Windows::put(store, window, entity);
            if !opened {
                if let Some(session) =
                    terminals::Terminals::session(store, place.terminals, place.id)
                {
                    session.hangup();
                }
                terminals::Terminals::remove(store, place.terminals, place.id);
            }
            return opened;
        }
        if let Some(place) = place.place::<himark::places::LocationsPlace>() {
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
        // Every other place rides the window's walk-stack navigation
        // — the windowed navigator registry answers per place kind
        // (the canvas among them). Unknown places decline here, same
        // as they always did there.
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return false;
        };
        let navigated = entity.navigate(store, ui, window, &place, fx);
        ::workbench::window::Windows::put(store, window, entity);
        navigated
    }
}

/// The locations batch as a windowed command — the follow-up
/// vehicle `WorkbenchFrontend` rides from the windowless lane.
struct OpenLocationsAsk {
    locations: Vec<editor::location::ResourceLocation>,
}

impl himark::commands::HimarkCommand for OpenLocationsAsk {
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
        bound: &imba::focus::Bindings,
        fx: &mut AppFx<'_>,
    ) {
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
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


impl himark::bridge::BridgeView for WorkbenchView {
    /// The frame's store: the live store plus the FOCUSED SEAT
    /// (the semantic walk's answer), so editors derive their
    /// selections-visible bit from STATE at build time — one
    /// viewport build per frame, no focused upgrade at paint.
    fn frame_store(&self, store: &Store, ui: &UiCtx) -> Store {
        let mut store = store.clone();
        let client = crate::app::window_focus_data(&store, ui, self.window)
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
        crate::app::window_focus_data(store, ui, self.window)
    }

    /// The window's entity tracks the host viewport — the engine's
    /// IME fold rebuilds at it, exactly as the window dispatch does.
    fn live_viewport(&self, store: &Store) -> Option<Size> {
        ::workbench::window::Windows::window_ref(store, self.window)
            .map(|entity| entity.viewport_size())
    }

    /// A window root paints its own ground; the surface stays
    /// opaque.
    fn clears_backdrop(&self) -> bool {
        false
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
        // A closed window's slot may still hear a fan-out before the
        // tail reaps it — it lays out EMPTY, never panics.
        let Some(entity) = ::workbench::window::Windows::window_ref(store, window) else {
            return imba::ThunkBox::new(arena, imba::container::container(arena, size));
        };
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



fn trace_resize_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RESIZE").is_some())
}

/// The semantic focus chain of a window — a STATE WALK over the view
/// tree. No arena, no layout, no realize: focus is state, and the
/// views carry it (imba `View::focus_data`).
pub fn window_focus_data<'a>(
    store: &'a imba::store::Store,
    ui: &'a imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
) -> Option<FocusData<'a, AppCommand>> {
    let entity = ::workbench::window::Windows::window_ref(store, window)?;
    let data = imba::View::focus_data(entity, store, ui)
        .map(move |command| crate::shell::content(window, command));
    // The window's own facts ride the walk's ROOT — the generic
    // bindings commands read instead of a window parameter. Bound
    // last: an inner view that binds its own session shadows these.
    let mut data = data.bind(window);
    data = data.bind(crate::workspace::entity_session(entity));
    if let Some(state) = crate::workspace::session_state(store, window) {
        data = data.bind(state);
    }
    Some(data)
}


pub fn palette_commands(
    store: &Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
) -> Vec<imba::PresentableCommand<AppCommand>> {
    // A STATE WALK over the views — no tree is built for the palette.
    let data = crate::app::window_focus_data(store, ui, window);
    let bound = data.as_ref().map(|data| data.bindings.clone());
    let bound = bound.map(|bound| {
        bound.bind(himark::commands::BoundOrigin(
            himark::bridge::ViewBridgeId::from_raw(window.raw()),
        ))
    });
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


/// The chrome prelude: the window this gesture came from, or a loud
/// drop — the dock toggles and pane openers keep needing one.
pub fn bound_window(
    command: &str,
    bound: &imba::focus::Bindings,
) -> Option<::workbench::window::WindowId> {
    let window = bound.bound::<::workbench::window::WindowId>().copied();
    if window.is_none() {
        eprintln!(
            "[shell] a window-taking command landed with no window bound — dropped: {command}"
        );
    }
    window
}


