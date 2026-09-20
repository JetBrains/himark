// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn policy_store() -> Store {
    let mut store = Store::new();
    // Production stores carry the edge-installed diff policy; a bare
    // store degrades to ReplaceAll and every diff becomes one hunk.
    store.put(::editor::env::Differ(std::sync::Arc::new(myersdiff::Myers)));
    store
}

fn resolved(before: &str, after: &str) -> Cell {
    let mut store = policy_store();
    let ui = UiCtx::dont_use_too_slow();
    let (mut cell, _) = Cell::pending_diff(
        &store,
        DiffHeader {
            title: "sample.md".to_owned(),
            added: Some(1),
            removed: Some(1),
        },
        640.0,
    );
    let mut batch = imba::effect::Batch::new();
    cell.perform(
        &mut store,
        &ui,
        CellCommand::ResolveDiff(Ok(crate::higent::FileEditContents {
            before: Some(before.to_owned()),
            after: Some(after.to_owned()),
        })),
        &mut batch.effects(),
    );
    cell
}

fn paint_cell(
    cell: &Cell,
    store: &Store,
    ui: &UiCtx,
    width: f32,
    path: &str,
) -> (f32, Vec<CellCommand>) {
    let arena = Arena::default();
    let thunk = imba::Layout::layout(
        imba::View::display(cell, &arena, store, ui),
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(width, f32::MAX),
        },
    );
    let height = imba::Thunk::size(&thunk).height;
    let viewport = Rect::from_wh(width, height);
    let widget = imba::Thunk::realize(thunk, &arena, viewport);
    let mut surface =
        skia_safe::surfaces::raster_n32_premul((width as i32, height.ceil() as i32 + 4))
            .expect("raster surface");
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::from_argb(0xff, 0x10, 0x12, 0x18));
    let result = imba::Widget::handle_event(
        &widget,
        &arena,
        &Event::Paint {
            canvas,
            focused: false,
        },
        viewport,
    );
    let commands = match result {
        EventResult::Command(command) => vec![command],
        EventResult::Commands(commands) => commands,
        _ => Vec::new(),
    };
    if let Ok(dir) = std::env::var("HIMARK_SHOT") {
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(format!("{dir}/{path}"), data.as_bytes()).expect("write png");
    }
    (height, commands)
}

fn edited_sources() -> (String, String) {
    let mut before = String::new();
    let mut after = String::new();
    for line in 0..30 {
        let text = format!("line {line:02} of the quiet unchanged context\n");
        before.push_str(&text);
        if line == 2 {
            after.push_str("line 02 was rewritten in place\n");
        } else {
            after.push_str(&text);
        }
    }
    before.push_str("old tail one\nold tail two\nold tail three\n");
    after.push_str("new tail\n");
    (before, after)
}

#[test]
fn a_diff_cell_lays_out_sane_heights_and_settles_its_rewrap() {
    let mut store = policy_store();
    let ui = UiCtx::dont_use_too_slow();
    let (before, after) = edited_sources();
    let mut cell = resolved(&before, &after);

    let sane = |height: f32| height.is_finite() && (40.0..100_000.0).contains(&height);
    let (h1, _) = paint_cell(&cell, &store, &ui, 640.0, "cell_at_build_width.png");
    assert!(sane(h1), "the built cell's laid height is sane: {h1}");
    // The app path: the panel is wider than the build width, so the
    // first paint asks for a rewrap; apply it and paint again — the
    // rewrap must SETTLE (no rewrap storm frame over frame).
    let (_, commands) = paint_cell(&cell, &store, &ui, 900.0, "cell_before_rewrap.png");
    let mut batch = imba::effect::Batch::new();
    for command in commands {
        cell.perform(&mut store, &ui, command, &mut batch.effects());
    }
    let (h2, again) = paint_cell(&cell, &store, &ui, 900.0, "cell_after_rewrap.png");
    assert!(sane(h2), "the rewrapped cell's laid height is sane: {h2}");
    assert!(
        !again
            .iter()
            .any(|command| matches!(command, CellCommand::Rewrap(_))),
        "the rewrap settled after one round"
    );
}

#[test]
fn expanded_before_cards_are_born_full_size() {
    // The deleted-code cards are born 1px tall and GROW on animation
    // ticks — the ticks must reach them through the overlay host they
    // render on.
    use imba::anim::AnimationClock;

    let mut store = policy_store();
    let ui = UiCtx::dont_use_too_slow();
    let (before, after) = edited_sources();
    let mut cell = resolved(&before, &after);

    let height_of = |cell: &Cell, store: &Store, ui: &UiCtx| {
        let arena = Arena::default();
        let thunk = imba::Layout::layout(
            imba::View::display(cell, &arena, store, ui),
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(640.0, f32::MAX),
            },
        );
        let height = imba::Thunk::size(&thunk).height;
        drop(thunk);
        height
    };
    let born = height_of(&cell, &store, &ui);

    // The app's frame loop: lay, realize, tick the clock, perform
    // whatever the widgets ask.
    for tick in 0..40u32 {
        let commands = {
            let arena = Arena::default();
            let thunk = imba::Layout::layout(
                imba::View::display(&cell, &arena, &store, &ui),
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(640.0, f32::MAX),
                },
            );
            let height = imba::Thunk::size(&thunk).height;
            let viewport = Rect::from_wh(640.0, height);
            let widget = imba::Thunk::realize(thunk, &arena, viewport);
            match imba::Widget::handle_event(
                &widget,
                &arena,
                &Event::AnimationClock {
                    now: AnimationClock::from_millis(tick as f64 * 16.0),
                },
                viewport,
            ) {
                EventResult::Command(command) => vec![command],
                EventResult::Commands(commands) => commands,
                _ => Vec::new(),
            }
        };
        let mut batch = imba::effect::Batch::new();
        for command in commands {
            cell.perform(&mut store, &ui, command, &mut batch.effects());
        }
    }

    let after_ticks = height_of(&cell, &store, &ui);
    assert!(
        (after_ticks - born).abs() < 0.5,
        "programmatic cards are born FULL SIZE — no growth to run: \
         born {born}, after the clock {after_ticks}"
    );
    paint_cell(&cell, &store, &ui, 640.0, "cell_with_cards.png");
    let CellBody::Diff { view, .. } = &cell.body else {
        panic!("the resolve lands the diff face");
    };
    let inline = view.inline_editor.expect("the inline face is minted");
    let cards = view.split.right.document.before_inlay_views(inline);
    assert!(!cards.is_empty(), "the deleted lines ride before-cards");
    assert!(
        cards.iter().all(|(_, card)| !card.is_appearing()),
        "programmatic mounts never animate"
    );
}

#[test]
fn a_scrolled_turn_of_cells_paints_and_keeps_its_extent() {
    // The app shape: cells in a turn's list, the turn in a scroll,
    // painted mid-scroll — the embedded editors must translate with
    // the viewport.
    use imba::scroll::ScrollView;

    let store = policy_store();
    let ui = UiCtx::dont_use_too_slow();
    let (before, after) = edited_sources();
    let mut cells: Vec<(Cell, f32)> = Vec::new();
    let mut batch = imba::effect::Batch::new();
    let (text_cell, text_height) = Cell::build(
        &store,
        &ui,
        CellKind::Agent,
        "A short agent message above the edit.",
        640.0,
        &mut batch.effects(),
    );
    cells.push((text_cell, text_height));
    for _ in 0..2 {
        let cell = resolved(&before, &after);
        let arena = Arena::default();
        let height = imba::Thunk::size(&imba::Layout::layout(
            imba::View::display(&cell, &arena, &store, &ui),
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(640.0, f32::MAX),
            },
        ))
        .height;
        cells.push((cell, height));
    }
    let declared: f32 = cells.iter().map(|(_, height)| height).sum();
    let turn = crate::higent::TurnView::new("turn", 640.0, cells);
    let mut scroll = ScrollView::new(turn);
    let width = 640.0f32;
    let view_h = 700.0f32;

    for (scroll_y, path) in [
        (0.0, "turn_scroll_0.png"),
        (137.5, "turn_scroll_137.png"),
        (400.0, "turn_scroll_400.png"),
    ] {
        scroll.set_scroll_y(scroll_y);
        let arena = Arena::default();
        let thunk = imba::Layout::layout(
            imba::View::display(&scroll, &arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(width, view_h)),
        );
        let viewport = Rect::from_wh(width, view_h);
        let widget = imba::Thunk::realize(thunk, &arena, viewport);
        let mut surface =
            skia_safe::surfaces::raster_n32_premul((width as i32, view_h as i32)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::from_argb(0xff, 0x10, 0x12, 0x18));
        imba::Widget::handle_event(
            &widget,
            &arena,
            &Event::Paint {
                canvas,
                focused: false,
            },
            viewport,
        );
        if let Ok(dir) = std::env::var("HIMARK_SHOT") {
            let image = surface.image_snapshot();
            let data = image
                .encode(None, skia_safe::EncodedImageFormat::PNG, None)
                .expect("png");
            std::fs::write(format!("{dir}/{path}"), data.as_bytes()).expect("write png");
        }
    }
    // The turn's laid extent is exactly the sum of its cells'
    // declared heights — the list contract the scroll rides on.
    let arena = Arena::default();
    let turn_height = imba::Thunk::size(&imba::Layout::layout(
        imba::View::display(scroll.content(), &arena, &store, &ui),
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(width, f32::MAX),
        },
    ))
    .height;
    assert!(
        (turn_height - declared).abs() < 1.5,
        "the turn spans its declared cells: laid {turn_height} vs declared {declared}"
    );
}

#[test]
fn a_resolved_edit_lands_as_an_inline_diff_with_folds() {
    let mut before = String::new();
    let mut after = String::new();
    for line in 0..40 {
        let text = format!("line {line:02} of the quiet unchanged context\n");
        before.push_str(&text);
        after.push_str(&text);
    }
    before.push_str("old tail\n");
    after.push_str("new tail\n");
    let cell = resolved(&before, &after);

    let CellBody::Diff { view, .. } = &cell.body else {
        panic!("the resolve lands the diff face");
    };
    assert_eq!(view.layout, crate::DiffLayout::Inline, "inline from birth");
    let inline = view.inline_editor.expect("the inline face is minted");

    // The forty untouched lines hide behind a fold strip...
    let (_, right_marks) = view.split.state.mark_markups();
    let extras = view
        .split
        .right
        .document
        .feature_markup(right_marks)
        .expect("the pane extras ride the document");
    assert!(
        !extras.all_inlays_in(0..u32::MAX).is_empty(),
        "a fold strip stands in the unchanged context"
    );
    // ...and the folded inline face is far shorter than the text.
    let folded = view.split.right.document.content_height(inline);
    let rows = view
        .split
        .right
        .document
        .content_height(view.split.right.editor)
        / 41.0;
    assert!(
        folded < rows * 20.0,
        "the context is collapsed: {folded} vs {rows} per row"
    );

    // THE diff markup washes the rewritten tail.
    let hunks = view
        .split
        .right
        .document
        .feature_markup(view.split.state.hunk_markup_oracle())
        .expect("THE diff markup rides the document");
    assert!(
        !crate::set_diff(None, hunks).is_empty(),
        "the hunk is washed"
    );

    let (_, oracle) = cell.oracle();
    assert!(oracle.contains("new tail"), "the after side shows");
}
