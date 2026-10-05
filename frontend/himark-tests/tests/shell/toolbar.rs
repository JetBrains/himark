use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use imba::store::Store;

use himark::test_driver;
use himark::app::AppFonts;
use himark::app::Application;

struct Mark {
    id: &'static str,
    hits: Arc<AtomicUsize>,
}

impl himark::commands::WindowedCommand for Mark {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> String {
        self.id.to_owned()
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

fn button(id: &'static str, order: f32, side: ::workbench::toolbar::ToolbarSide) -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: id,
        order,
        side,
        glyph: Arc::new(|_canvas, _rect, _color| {}),
    }
}

#[test]
fn a_right_button_press_dispatches_its_own_command() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let left_hits = Arc::new(AtomicUsize::new(0));
    let right_hits = Arc::new(AtomicUsize::new(0));
    {
        let mut store = app.store_mut();
        himark::commands::Commands::register(
            &mut store,
            Arc::new(Mark {
                id: "test.left-mark",
                hits: Arc::clone(&left_hits),
            }),
        );
        himark::commands::Commands::register(
            &mut store,
            Arc::new(Mark {
                id: "test.right-mark",
                hits: Arc::clone(&right_hits),
            }),
        );
    }
    app.register_toolbar_button(button("test.left-mark", 10.0, ::workbench::toolbar::ToolbarSide::Left));
    app.register_toolbar_button(button("test.right-mark", 10.0, ::workbench::toolbar::ToolbarSide::Right));
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let chrome = ::editor::env::Themes::of(app.store()).ui().toolbar.clone();
    // LEFT buttons live in the global cluster, top-left.
    let cluster_index = ::workbench::toolbar::ToolbarButtons::of(app.store())
        .iter()
        .filter(|button| {
            matches!(
                button.side,
                ::workbench::toolbar::ToolbarSide::Left | ::workbench::toolbar::ToolbarSide::Well
            )
        })
        .position(|button| button.command == "test.left-mark")
        .expect("the left button is in the cluster");
    let x = chrome.button_inset
        + cluster_index as f32 * chrome.button_size
        + chrome.button_size * 0.5;
    assert!(test_driver::click(
        &mut app,
        x,
        chrome.height * 0.5,
        800.0,
        600.0
    ));
    assert_eq!(
        left_hits.load(Ordering::Relaxed),
        1,
        "the cluster button fired"
    );
    // RIGHT buttons ride the DOCK CLUSTER top-right — the mirror
    // of the global cluster, present even with the dock closed.
    let right_x = 800.0 - chrome.button_inset - chrome.button_size * 0.5;
    assert!(test_driver::click(
        &mut app,
        right_x,
        chrome.height * 0.5,
        800.0,
        600.0
    ));
    assert_eq!(
        right_hits.load(Ordering::Relaxed),
        1,
        "the dock cluster fires with the dock closed"
    );
}
