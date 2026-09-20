// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

const TABLE: &str = "| Name | Notes |\n|:-----|------:|\n| a | first<br>second |\n| b\\|c | x |";

fn contents(row: &[CellSource]) -> Vec<&str> {
    row.iter().map(|cell| cell.content.as_str()).collect()
}

#[test]
fn recognizes_a_table_and_unescapes_cells() {
    let table = parse_table(TABLE).expect("a table");
    assert_eq!(table.alignments, vec![CellAlign::Left, CellAlign::Right]);
    assert_eq!(table.rows.len(), 3, "header plus two body rows");
    assert_eq!(contents(&table.rows[0]), vec!["Name", "Notes"]);
    assert_eq!(
        table.rows[1][1].content, "first\nsecond",
        "<br> becomes a newline"
    );
    assert_eq!(
        table.rows[2][0].content, "b|c",
        "escaped pipe stays literal"
    );
}

#[test]
fn cell_spans_address_the_source() {
    let table = parse_table(TABLE).expect("a table");
    for row in &table.rows {
        for cell in row {
            let span = cell.span.start as usize..cell.span.end as usize;
            assert_eq!(&TABLE[span], cell.source, "span points at the source");
        }
    }
}

#[test]
fn source_offsets_walk_the_escape_table() {
    let source = "a\\|b<br>c";
    assert_eq!(source_offset(source, 0), 0);
    assert_eq!(source_offset(source, 1), 1, "before the escaped pipe");
    assert_eq!(source_offset(source, 2), 3, "past the escaped pipe");
    assert_eq!(source_offset(source, 3), 4, "before the <br>");
    assert_eq!(source_offset(source, 4), 8, "past the <br>");
    assert_eq!(source_offset(source, 5), 9);
}

#[test]
fn short_rows_pad_and_long_rows_truncate() {
    let table = parse_table("| a | b |\n|---|---|\n| only |\n| x | y | extra |").expect("a table");
    assert_eq!(contents(&table.rows[1]), vec!["only", ""]);
    assert_eq!(contents(&table.rows[2]), vec!["x", "y"]);
}

#[test]
fn rejects_non_tables() {
    assert!(parse_table("plain paragraph").is_none());
    assert!(parse_table("| pipes | only |\nno delimiter row").is_none());
    assert!(
        parse_table("| a |\n| ~~~ |").is_none(),
        "bad delimiter cells"
    );
}

fn apply(source: &str, operation: &Operation) -> String {
    let mut out = String::new();
    let mut at = 0usize;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                out.push_str(&source[at..at + len as usize]);
                at += len as usize;
            }
            Op::Delete(text) => {
                assert_eq!(
                    &source[at..at + text.len()],
                    text,
                    "the op deletes the bytes it claims"
                );
                at += text.len();
            }
            Op::Insert(text) => out.push_str(&text),
        }
    }
    out.push_str(&source[at..]);
    out
}

fn editor_over(source: &str) -> TableEditor {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    use himark::InlayEditing;
    let fonts = himark::embedded_fonts::collection();
    let theme = himark::Theme::embedded();
    let mut editor = TableEditor::new(parse_table(source).expect("a table"),
                store, ui, &fonts, &theme);
    editor.set_range(0..source.len() as u32);
    editor
}

#[test]
fn structural_edits_write_correct_markdown() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    use himark::InlayEditing;
    let source = "| a | b |\n| --- | :-: |\n| 1 | 2 |\n| 3 | 4 |";
    let fonts = himark::embedded_fonts::collection();
    let theme = himark::Theme::embedded();

    let mut editor = editor_over(source);
    editor.insert_row(3,
                store, ui, &fonts, &theme);
    let appended = apply(source, &editor.take_edit().expect("an op"));
    assert_eq!(appended, format!("{source}\n|   |   |"));

    let mut editor = editor_over(source);
    editor.remove_row(1,
                store, ui, &fonts, &theme);
    let removed = apply(source, &editor.take_edit().expect("an op"));
    assert_eq!(removed, "| a | b |\n| --- | :-: |\n| 3 | 4 |");

    let mut editor = editor_over(source);
    editor.insert_column(0,
                store, ui, &fonts, &theme);
    let widened = apply(source, &editor.take_edit().expect("an op"));
    assert_eq!(
        widened,
        "|  | a | b |\n| --- | --- | :-: |\n|  | 1 | 2 |\n|  | 3 | 4 |"
    );
    assert!(parse_table(&widened).is_some(), "still a table");

    let mut editor = editor_over(source);
    editor.remove_column(0,
                store, ui, &fonts, &theme);
    let narrowed = apply(source, &editor.take_edit().expect("an op"));
    assert_eq!(narrowed, "| b |\n| :-: |\n| 2 |\n| 4 |");
    assert!(parse_table(&narrowed).is_some(), "still a table");
}

#[test]
fn resize_relayout_round_trips_through_the_effect() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    use imba::effect::{block_on, EffectHandler};

    let fonts = himark::embedded_fonts::collection();
    let theme = himark::Theme::embedded();
    let source = "| title | body |\n| --- | --- |\n| a | some long prose that wraps at narrow widths and keeps wrapping |";
    let mut editor = editor_over(source);
    editor.relay_all(600.0,
                store, ui, &fonts, &theme);
    assert!(!editor.needs_relay(600.0), "just laid — nothing to report");
    assert!(
        editor.needs_relay(280.0),
        "a narrower pane moves the columns"
    );

    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let mut batch = imba::effect::Batch::new();
    imba::View::perform(
        &mut editor,
        &mut store,
        &ui,
        TableCommand::Relayout { width: 280.0 },
        &mut batch.effects(),
    );
    let launched = himark::test_support::surviving_launches(batch);
    assert_eq!(launched.len(), 1, "one relay launched");
    assert!(launched[0].is::<TableRelayoutEffect>());
    let mut repeat = imba::effect::Batch::new();
    imba::View::perform(
        &mut editor,
        &mut store,
        &ui,
        TableCommand::Relayout { width: 280.0 },
        &mut repeat.effects(),
    );
    assert!(
        himark::test_support::surviving_launches(repeat).is_empty(),
        "the same wanted width relaunches nothing"
    );

    let workshop = std::sync::Arc::new(himark::Workshop::new(
        himark::embedded_fonts::source(),
        theme.clone(),
    ));
    let handler = TableRelayoutHandler(workshop);
    let (value, _lift) = launched
        .into_iter()
        .next()
        .expect("launched")
        .into_payload()
        .split();
    let effect = value
        .downcast::<TableRelayoutEffect>()
        .expect("the relay effect");
    let relaid = block_on(Box::pin(async move { handler.handle(*effect).await }));

    let mut expected = editor.clone();
    expected.relay_all(280.0, &store, &ui, &fonts, &theme);
    imba::View::perform(
        &mut editor,
        &mut store,
        &ui,
        relaid,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        editor.laid_widths(),
        expected.laid_widths(),
        "the landing adopted the worker's lay"
    );
    assert!(!editor.needs_relay(280.0), "settled at the new width");
}

#[test]
fn a_relaid_landing_over_moved_content_discards_itself() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    use himark::InlayEditing;

    let fonts = himark::embedded_fonts::collection();
    let theme = himark::Theme::embedded();
    let source =
        "| a | b |\n| --- | --- |\n| one | a very long cell that wraps when squeezed hard |";
    let mut editor = editor_over(source);
    editor.relay_all(600.0,
                store, ui, &fonts, &theme);

    let stale = {
        let mut clone = editor.clone();
        clone.relay_all(300.0,
                store, ui, &fonts, &theme);
        clone
    };
    editor.write_through_insert(0, 0, "typed");
    let _ = editor.take_edit();
    let before = editor.laid_widths().to_vec();

    let mut store = Store::new();
    imba::View::perform(
        &mut editor,
        &mut store,
        &UiCtx::dont_use_too_slow(),
        TableCommand::Relaid(Box::new(stale)),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        editor.laid_widths(),
        &before[..],
        "a relay against overwritten content never lands"
    );
    assert_eq!(
        editor.cell_content(0, 0),
        "typeda",
        "the keystroke survives"
    );
}

#[test]
fn paint_reports_relayout_while_the_width_lags() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    use imba::event::{Event, EventResult};

    let fonts = himark::embedded_fonts::collection();
    let theme = himark::Theme::embedded();
    let source = "| a | b |\n| --- | --- |\n| x | prose long enough that no pane fits it unwrapped, and then some more of it |";
    let mut editor = editor_over(source);
    editor.relay_all(600.0,
                store, ui, &fonts, &theme);

    let arena = Arena::default();
    let store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let constraints = Constraints {
        min: Size::default(),
        max: Size::new(900.0, f32::MAX),
    };
    let mut surface = skia_safe::surfaces::raster_n32_premul((1024, 768)).expect("a surface");
    let paint_commands = |editor: &TableEditor, surface: &mut skia_safe::Surface| {
        let widget = imba::Layout::layout(
            imba::View::display(editor, &arena, &store, &ui),
            &arena,
            constraints,
        );
        let viewport = Rect::from_size(imba::Thunk::size(&widget));
        let widget = imba::Thunk::realize(widget, &arena, viewport);
        let result = imba::Widget::handle_event(
            &widget,
            &arena,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: false,
            },
            viewport,
        );
        match result {
            EventResult::Command(command) => vec![command],
            EventResult::Commands(commands) => commands,
            _ => Vec::new(),
        }
    };

    let strip = editor.chrome.control_size + 6.0;
    let trailing = editor.chrome.control_size * 0.5 + 2.0;
    let expected = (900.0 - strip - trailing).max(60.0);
    assert!(
        paint_commands(&editor, &mut surface)
            .iter()
            .any(|command| matches!(
                command,
                TableCommand::Relayout { width } if (width - expected).abs() < 0.5
            )),
        "a painted frame at a lagging width reports the relayout"
    );

    editor.relay_all(expected, &store, &ui, &fonts, &theme);
    assert!(
        !paint_commands(&editor, &mut surface)
            .iter()
            .any(|command| matches!(command, TableCommand::Relayout { .. })),
        "a settled table reports nothing"
    );
}

#[test]
fn stretches_to_fill_when_everything_fits() {
    let widths = column_widths(
        &[
            ColumnIntrinsics {
                min: 40.0,
                max: 100.0,
            },
            ColumnIntrinsics {
                min: 40.0,
                max: 200.0,
            },
        ],
        620.0,
        &himark::Theme::embedded().ui().table.clone(),
    );

    let total: f32 = widths.iter().sum();
    assert!((total - 620.0).abs() < 0.5, "fills the width: {total}");

    assert!(
        (widths[1] / widths[0] - 2.0).abs() < 0.05,
        "proportional: {widths:?}"
    );
}

#[test]
fn squeeze_distributes_by_compressibility() {
    let columns = [
        ColumnIntrinsics {
            min: 100.0,
            max: 200.0,
        },
        ColumnIntrinsics {
            min: 100.0,
            max: 400.0,
        },
    ];
    let chrome = himark::Theme::embedded().ui().table.clone();
    let widths = column_widths(&columns, 400.0, &chrome);

    let total: f32 = widths.iter().sum();
    assert!(
        (total - 400.0).abs() <= chrome.quantum * 2.0,
        "total {total}"
    );

    for (width, column) in widths.iter().zip(&columns) {
        assert!(*width >= column.min - chrome.quantum);
        assert!(*width <= column.max.min(400.0 * chrome.column_cap) + chrome.quantum);
    }

    assert!(widths[1] > widths[0]);
}

#[test]
fn hard_squeeze_falls_back_to_minimums() {
    let widths = column_widths(
        &[
            ColumnIntrinsics {
                min: 300.0,
                max: 500.0,
            },
            ColumnIntrinsics {
                min: 300.0,
                max: 500.0,
            },
        ],
        400.0,
        &himark::Theme::embedded().ui().table.clone(),
    );
    assert_eq!(
        widths,
        vec![240.0, 240.0],
        "mins, capped at 60% of available"
    );
}

#[test]
fn floor_applies_and_the_cap_bounds_dominance() {
    let chrome = himark::Theme::embedded().ui().table.clone();
    let widths = column_widths(
        &[
            ColumnIntrinsics { min: 2.0, max: 4.0 },
            ColumnIntrinsics {
                min: 10.0,
                max: 10_000.0,
            },
        ],
        620.0,
        &chrome,
    );

    assert!(widths[0] >= chrome.column_floor);

    let total: f32 = widths.iter().sum();
    assert!((total - 620.0).abs() < 0.5, "fills the width: {total}");
    assert!(widths[1] > widths[0]);
    assert!(
        widths[0] > chrome.column_floor,
        "cap left room for column 0"
    );
}
