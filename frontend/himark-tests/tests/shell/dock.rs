#![allow(unused_imports)]
use super::*;
use std::sync::{Arc, Mutex};


use imba::anim::AnimationClock;

use imba::constraints::Constraints;

use imba::event::{Event, EventResult, Key};

use imba::leaf::leaf;

use imba::store::Store;

use imba::thunk_ext::ThunkExt as _;

use imba::{ui::UiCtx, View};


use himark::test_driver;

use himark::app::AppCommand;

use himark::app_ext::AppExt;

use himark::app::AppFonts;

use himark::app::Application;

use hikit::modal::ModalRequest;

use hikit::modal::ModalView;


#[derive(Clone, Debug)]
enum StubCommand {
    Close,
    Ask,
}


impl std::fmt::Display for StubCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}


#[derive(Clone)]
struct DockStub {
    label: &'static str,
    request: Arc<Mutex<Option<ModalRequest>>>,
}


impl DockStub {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            request: Arc::new(Mutex::new(None)),
        }
    }
}


impl View for DockStub {
    type Command = StubCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, StubCommand> {
        imba::focus::FocusData {
            on_key: Some(Box::new(|key, _mods| match key {
                Key::Escape => EventResult::Command(StubCommand::Close),
                Key::Enter => EventResult::Command(StubCommand::Ask),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        }
    }

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: StubCommand,
        _fx: &mut imba::effect::Effects<'_, StubCommand>,
    ) {
        let request = match command {
            StubCommand::Close => ModalRequest::Close,
            StubCommand::Ask => ModalRequest::OpenLocations(vec![located("asked.md")]),
        };
        *self.request.lock().unwrap() = Some(request);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, StubCommand> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, constraints: Constraints| {
                let size = constraints.max;
                leaf::<StubCommand>(size.width, size.height).event(|_arena, event, _size| {
                    match event {
                        Event::KeyDown {
                            key: Key::Escape, ..
                        } => EventResult::Command(StubCommand::Close),
                        Event::KeyDown {
                            key: Key::Enter, ..
                        } => EventResult::Command(StubCommand::Ask),
                        _ => EventResult::Ignored,
                    }
                })
            },
        )
    }
}


impl ModalView for DockStub {
    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.lock().unwrap().take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }
}


struct ShowStubDock {
    label: &'static str,
    owner: &'static str,
}


impl himark::commands::WindowedCommand for ShowStubDock {
    fn id(&self) -> &'static str {
        "test.dock-show"
    }

    fn name(&self) -> String {
        "Show Test Dock".to_owned()
    }

    fn perform(
    &self,
    store: &mut Store,
    _ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    fx: &mut himark::app::AppFx<'_>,
) {
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        fx.scope(
            move |command| AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(DockStub::new(self.label)), self.owner, fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}


#[test]
fn the_dock_opens_as_a_split_and_narrows_the_workbench() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let before =
        ::workbench::workbench::panel_width(app.store(), entity(&app).workbench().root.focused_pane())
            .expect("the scratch pane is an editor");

    show_dock(&mut app, "files", "test.files");
    assert!(entity(&app).has_dock(), "the dock is up");
    assert_eq!(entity(&app).dock_owner(), Some("test.files"));
    settle(&mut app, &mut surface);

    assert_eq!(
        entity(&app).dock_target_width(),
        ::workbench::dock::DOCK_WIDTH,
        "the dock settled at the default width"
    );
    let after =
        ::workbench::workbench::panel_width(app.store(), entity(&app).workbench().root.focused_pane())
            .expect("still an editor");
    assert!(
        before - after > ::workbench::dock::DOCK_WIDTH * 0.5,
        "the workbench narrowed for the split (was {before}, now {after})"
    );
}


#[test]
fn escape_closes_the_dock_after_the_slide_settles() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    assert!(test_driver::key(&mut app, Key::Escape, Default::default()));
    assert!(entity(&app).has_dock(), "still sliding out");
    assert_eq!(
        entity(&app).dock_owner(),
        None,
        "a closing dock reads as closed"
    );
    settle(&mut app, &mut surface);
    assert!(
        !entity(&app).has_dock(),
        "the settled slide filed the close"
    );
}


#[test]
fn the_dock_resizes_by_dragging_its_edge() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    let edge = 800.0 - ::workbench::dock::DOCK_WIDTH;
    assert!(test_driver::click(&mut app, edge, 300.0, 800.0, 600.0));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(test_driver::drag(&mut app, 100.0, 300.0));
    assert!(test_driver::mouse_up(&mut app, 100.0, 300.0));
    assert_eq!(
        entity(&app).dock_target_width(),
        800.0 * 0.6,
        "the drag clamps at the wide cap"
    );

    app.draw_window(app.sole_window(), surface.canvas());
    let edge = 800.0 - 800.0 * 0.6;
    assert!(test_driver::click(&mut app, edge, 300.0, 800.0, 600.0));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(test_driver::drag(&mut app, 780.0, 300.0));
    assert!(test_driver::mouse_up(&mut app, 780.0, 300.0));
    assert_eq!(
        entity(&app).dock_target_width(),
        ::workbench::dock::DOCK_MIN_WIDTH,
        "the drag clamps at the floor"
    );
}


#[test]
fn an_editor_drag_still_selects_while_the_dock_is_up() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(test_driver::type_text(
        &mut app,
        "alpha beta gamma delta epsilon zeta"
    ));
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    assert!(test_driver::click(&mut app, 160.0, 120.0, 800.0, 600.0));
    app.draw_window(app.sole_window(), surface.canvas());
    test_driver::drag(&mut app, 380.0, 130.0);
    test_driver::mouse_up(&mut app, 380.0, 130.0);
    let (_, held) = documents::OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .next()
        .expect("the scratch document");
    let document = held.document();
    let editor = document.editor_ids().next().expect("its editor");
    let selection = document.carets(editor).primary().selection();
    assert!(
        !selection.is_empty(),
        "the drag selected nothing — the dock swallowed it"
    );
}


#[test]
fn dock_picks_keep_the_panel_up() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    assert!(test_driver::key(&mut app, Key::Enter, Default::default()));
    assert!(entity(&app).has_dock(), "the pick kept the panel");
    assert_eq!(entity(&app).dock_owner(), Some("test.files"));
}


#[test]
fn dock_header_buttons_dispatch_their_commands() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Mark {
        hits: Arc<AtomicUsize>,
    }
    impl himark::commands::WindowedCommand for Mark {
        fn id(&self) -> &'static str {
            "test.dock-mark"
        }
        fn name(&self) -> String {
            "Dock Mark".to_owned()
        }
        fn perform(
    &self,
    _store: &mut Store,
    _ui: &imba::ui::UiCtx,
    _window: ::workbench::window::WindowId,
    _fx: &mut himark::app::AppFx<'_>,
) {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let hits = Arc::new(AtomicUsize::new(0));
    {
        let mut store = app.store_mut();
        himark::commands::Commands::register(
            &mut store,
            Arc::new(Mark {
                hits: Arc::clone(&hits),
            }),
        );
    }
    app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
        command: "test.dock-mark",
        order: 10.0,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|_canvas, _rect, _color| {}),
    });
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    let chrome = ::editor::env::Themes::of(app.store()).ui().toolbar.clone();
    let dock_width = entity(&app).dock_target_width();
    let edge = 800.0 - dock_width;
    let x = edge + dock_width - chrome.button_inset - chrome.button_size * 0.5;
    assert!(
        test_driver::click(&mut app, x, chrome.height * 0.5, 800.0, 600.0),
        "the dock header button answers the click"
    );
    settle(&mut app, &mut surface);
    assert_eq!(
        hits.load(Ordering::Relaxed),
        1,
        "the dock header button ran its command"
    );
}


#[test]
fn the_dock_swaps_content_in_place() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    show_dock(&mut app, "changes", "test.changes");
    assert_eq!(entity(&app).dock_owner(), Some("test.changes"));
    let label = entity(&app)
        .dock_panel()
        .expect("the dock is up")
        .as_any()
        .downcast_ref::<DockStub>()
        .expect("the stub")
        .label;
    assert_eq!(label, "changes", "the content swapped in place");
    assert_eq!(
        entity(&app).dock_target_width(),
        ::workbench::dock::DOCK_WIDTH,
        "the width stood through the swap"
    );

    {
        let window = app.sole_window();
        let mut entity = ::workbench::window::Windows::window(app.store(), window).expect("window");
        entity.roll_away_dock();
        ::workbench::window::Windows::put(&mut app.store_mut(), window, entity);
    }
    assert_eq!(entity(&app).dock_owner(), None);
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);
    assert_eq!(entity(&app).dock_owner(), Some("test.files"));
    assert!(entity(&app).has_dock());
}


#[test]
fn the_dock_and_the_floating_drawer_coexist() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    show_dock(&mut app, "files", "test.files");
    settle(&mut app, &mut surface);

    {
        let window = app.sole_window();
        let mut entity = ::workbench::window::Windows::window(app.store(), window).expect("window");
        let mut batch = imba::effect::Batch::new();
        let mut store = app.store().clone();
        entity.show_side_panel(
            &mut store,
            Box::new(DockStub::new("drawer")),
            &mut batch.effects(),
        );
        ::workbench::window::Windows::put(&mut app.store_mut(), window, entity);
    }
    assert!(entity(&app).has_side_panel(), "the drawer is up");
    assert!(entity(&app).has_dock(), "the dock stayed");
}


#[test]
fn the_dock_and_the_families_ride_their_session_across_switches() {
    let fonts = himark::app::AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    show_dock(&mut app, "changes", "test.owner");
    settle(&mut app, &mut surface);
    struct NullBackend;
    impl terminals::TerminalBackend for NullBackend {
        fn write(&self, _bytes: &[u8]) {}
        fn resize(&self, _cols: u16, _rows: u16, _w: f32, _h: f32) {}
        fn hangup(&self) {}
    }
    let hidden = terminals::TerminalId::mint();
    let session = terminals::Session::new(Box::new(NullBackend));
    let home = app.sole_window_session();
    let terminals =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).terminals();
    terminals::Terminals::put(&mut app.store_mut(), terminals, hidden, session);
    let entity = |app: &Application| {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("the window entity")
            .clone()
    };
    assert!(entity(&app).has_dock());
    assert!(
        terminals::Terminals::session_ref(app.store(), terminals, hidden).is_some(),
        "the state row is in the session"
    );

    struct Switch(std::sync::Arc<Mutex<Option<ahp_wire::SessionId>>>);
    impl himark::commands::WindowedCommand for Switch {
        fn id(&self) -> &'static str {
            "test.switch"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
    &self,
    store: &mut Store,
    _ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    fx: &mut himark::app::AppFx<'_>,
) {
            let target = match self.0.lock().unwrap().clone() {
                Some(target) => target,
                None => ahp_wire::SessionId::mint_scratch(store),
            };
            *self.0.lock().unwrap() = Some(target.clone());
            himark::app::switch_session(store, window, target, fx);
        }
    }
    let first = himark::workspace::entity_session(&entity(&app));
    let minted = std::sync::Arc::new(Mutex::new(None));
    assert!(app.perform_command(AppCommand::Windowed(
        window,
        Arc::new(Switch(std::sync::Arc::clone(&minted))),
    )));
    assert!(
        entity(&app).dock_panel().is_none(),
        "a fresh session has no dock"
    );
    let fresh = minted.lock().unwrap().clone().expect("the minted session");
    assert!(
        ahp_session::session::state::Hosts::state(app.store(), &fresh)
            .map(|state| terminals::Terminals::list(app.store(), state.terminals()))
            .unwrap_or_default()
            .is_empty(),
        "a fresh session has no state rows of its own"
    );
    assert!(
        terminals::Terminals::session_ref(app.store(), terminals, hidden).is_some(),
        "and the first session's row stayed WITH it — never borrowed, never dropped"
    );

    let back = std::sync::Arc::new(Mutex::new(Some(first)));
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(Switch(back)),)));
    let restored = entity(&app);
    assert!(restored.dock_panel().is_some(), "the dock rode its session");
    assert!(
        restored.dock_target_width() > 0.0,
        "and reads open, not closing"
    );
    assert!(
        terminals::Terminals::session_ref(app.store(), terminals, hidden).is_some(),
        "the state row rode along"
    );

    settle(&mut app, &mut surface);
    assert!(
        !app.draw_window(app.sole_window(), surface.canvas()),
        "the restored dock mounts settled — no slide, no reconcile"
    );
}


pub(super) fn located(name: &str) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("test"),
        vec!["project".to_owned(), name.to_owned()],
    )
}


pub(super) fn show_dock(app: &mut Application, label: &'static str, owner: &'static str) {
    let window = app.sole_window();
    assert!(app.perform_command(AppCommand::Windowed(
        window,
        Arc::new(ShowStubDock { label, owner }),
    )));
}


pub(super) fn settle(app: &mut Application, surface: &mut skia_safe::Surface) {
    for tick in 0..60 {
        app.draw_window(app.sole_window(), surface.canvas());
        let busy = test_driver::animate(app, AnimationClock::from_millis(tick as f64 * 32.0));
        if !busy && tick > 1 {
            break;
        }
    }
}


pub(super) fn entity(app: &Application) -> &::workbench::window::Window {
    ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("the window")
}
