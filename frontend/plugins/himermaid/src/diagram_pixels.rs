use super::*;

#[test]
fn the_diagram_paints_styled_nodes_and_labels() {
    let view = MermaidView::render("flowchart TD\n    A[alpha] --> B[beta]\n");
    assert!(view.is_diagram());
    let size = view.scaled(400.0);
    let (w, h) = (size.width.ceil() as i32, size.height.ceil() as i32);
    let mut surface = skia_safe::surfaces::raster_n32_premul((w, h)).expect("surface");
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::WHITE);
    let store = imba::Store::new();
    let ui = imba::UiCtx::new();
    let arena = imba::arena::Arena::default();
    let widget = imba::View::layout(
        &view,
        &arena,
        &store,
        &ui,
        imba::constraints::Constraints {
            min: Size::default(),
            max: Size::new(400.0, f32::MAX),
        },
    );
    let widget = imba::Thunk::realize(
        widget,
        &arena,
        skia_safe::Rect::from_wh(size.width, size.height),
    );
    imba::Widget::handle_event(
        &widget,
        &arena,
        &imba::event::Event::Paint {
            focused: false,
            canvas,
        },
        skia_safe::Rect::from_wh(size.width, size.height),
    );
    let image = surface.image_snapshot();
    let pixels = image.peek_pixels().expect("pixels");
    let mut colors = std::collections::HashSet::new();
    let mut tinted = 0usize;
    let mut black = 0usize;
    for y in 0..h {
        for x in 0..w {
            let color: skia_safe::Color = pixels.get_color((x, y));
            colors.insert((color.r() / 16, color.g() / 16, color.b() / 16));
            if color.b() > color.r().saturating_add(8) {
                tinted += 1;
            }
            if color.r() < 30 && color.g() < 30 && color.b() < 30 {
                black += 1;
            }
        }
    }
    let total = (w * h) as usize;
    assert!(
        colors.len() > 6,
        "a styled diagram has more than boxes: {} colors",
        colors.len()
    );
    assert!(
        tinted > total / 100,
        "node fills carry their themed tint: {tinted}/{total}"
    );
    assert!(
        black < total / 3,
        "not a wall of unstyled black: {black}/{total}"
    );
}
