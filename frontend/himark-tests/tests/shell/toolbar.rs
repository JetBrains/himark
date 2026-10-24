use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use imba::store::Store;

use himark::app::AppFonts;
use himark::app::Himark;
use himark::test_driver;

struct Mark {
    id: &'static str,
    hits: Arc<AtomicUsize>,
}

impl himark::commands::HimarkCommand for Mark {
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
        _bound: &imba::focus::Bindings,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
}

fn button(
    id: &'static str,
    label: &'static str,
    order: f32,
    side: ::workbench::toolbar::ToolbarSide,
) -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: id,
        label,
        order,
        side,
        glyph: Arc::new(|_canvas, _rect, _color| {}),
    }
}

/// A header WORD's advance in its rank — the test lays the trailing
/// group the way `hikit::header::ColumnHeader` does: right to left
/// from the trailing inset, each word its gap before it.
fn word_width(app: &Himark, style: &hikit::ui::TextStyle, label: &str) -> f32 {
    let _ = app;
    style.shaper.advance(&style.font, label)
}

/// The redesign has no icon cluster: a registered button is a WORD
/// in a column header. A Left button is one of the leftmost header's
/// leading TABS, right after the semaphore (dim while it does not
/// stand); a Right button is a side-column tab, shown dim at the same
/// header's trailing edge while the dock is closed — and both answer
/// a press.
#[test]
fn a_right_button_press_dispatches_its_own_command() {
    let mut app = Himark::new(AppFonts::embedded());
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
    app.register_toolbar_button(button(
        "test.left-mark",
        "Sessions",
        10.0,
        ::workbench::toolbar::ToolbarSide::Left,
    ));
    app.register_toolbar_button(button(
        "test.right-mark",
        "Tab",
        10.0,
        ::workbench::toolbar::ToolbarSide::Right,
    ));
    let (width, height) = (1400.0f32, 600.0f32);
    let mut surface =
        skia_safe::surfaces::raster_n32_premul((width as i32, height as i32)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let chrome = ::editor::env::Themes::of(app.store()).ui().toolbar.clone();
    let styles = hikit::header::HeaderStyles::of(app.store(), &app.ui_ctx());
    let y = chrome.title_baseline - chrome.title_size * 0.4;

    // The leftmost header (no chat column stands in a fresh window:
    // the tree carries the tabs), as `workbench::tree_header`
    // composes it. LEADING: the Left words as tabs from the lead
    // inset, dim while they do not stand. TRAILING: the drawer word
    // (Contents) in the quiet rank, then every Right tab dim; a
    // word's gap stands BEFORE it — quiet words `button_text_gap`,
    // dim words `tab_gap`; the first word none. Laid right to left
    // from the trailing inset.
    assert!(
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .workbench()
            .chat()
            .is_none(),
        "a fresh window has no chat column"
    );
    let buttons = ::workbench::toolbar::ToolbarButtons::of(app.store());
    let drawer = ::workbench::registry::Registry::of(app.store())
        .and_then(|registry| registry.drawer_button.clone());
    let mut words: Vec<(&str, &str, &hikit::ui::TextStyle, f32)> = Vec::new();
    if let Some(drawer) = &drawer {
        words.push((drawer.command, drawer.label, &styles.button, chrome.button_text_gap));
    }
    for tab in buttons.tabs() {
        words.push((tab.command, tab.label, &styles.dim, chrome.tab_gap));
    }
    let mut centers = std::collections::HashMap::new();
    let mut right_edge = width - chrome.trailing_inset;
    for (index, (command, label, style, gap)) in words.iter().enumerate().rev() {
        let advance = word_width(&app, style, label);
        let left = right_edge - advance;
        centers.insert(*command, left + advance * 0.5);
        right_edge = left - if index > 0 { *gap } else { 0.0 };
    }
    let tab_x = centers["test.right-mark"];
    assert!(test_driver::click(&mut app, tab_x, y, width, height));
    assert_eq!(
        right_hits.load(Ordering::Relaxed),
        1,
        "the closed side column's tab fires from the header"
    );

    // The first leading tab starts at the lead inset: past the
    // semaphore clearance, never short of the code text edge.
    let window = ::editor::env::Themes::of(app.store()).ui().window.clone();
    let lead = chrome.lead_inset.max(window.code_text_edge);
    let word_x = lead + word_width(&app, &styles.dim, "Sessions") * 0.5;
    assert!(test_driver::click(&mut app, word_x, y, width, height));
    assert_eq!(
        left_hits.load(Ordering::Relaxed),
        1,
        "the header's leading tab fired"
    );
}
