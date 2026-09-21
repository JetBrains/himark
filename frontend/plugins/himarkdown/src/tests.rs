// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn headers_align_right_center_left_by_level() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = test_fonts();
    let theme = test_theme();
    let source = "# One\n\n## Two\n\n### Three\n";
    let (document, _) = markdown_document(source, store, ui, &fonts, &theme);
    let resolved = |line: &str| {
        let start = source.find(line).expect("line") as u32;
        let range = start..start + line.len() as u32;
        document.markup().block_marks_in(range).resolved(&theme)
    };

    let h1 = resolved("# One");
    assert_eq!(h1.alignment, Some(himark::TextAlignment::Right));
    let h2 = resolved("## Two");
    assert_eq!(h2.alignment, Some(himark::TextAlignment::Center));
    let h3 = resolved("### Three");
    assert_eq!(h3.alignment, None, "deeper levels read left");
    assert!(
        h1.font_size.unwrap_or(0.0) > 1.5 * h3.font_size.unwrap_or(f32::MAX),
        "the H1 face is VERY large: h1={:?} h3={:?}",
        h1.font_size,
        h3.font_size
    );
}

fn test_fonts() -> skia_safe::textlayout::FontCollection {
    himark::embedded_fonts::source()()
}

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

fn test_languages() -> std::sync::Arc<himark::SyntaxLanguages> {
    std::sync::Arc::new(markdown_languages(himark::SyntaxLanguages::new()))
}

use himark::{EditorCommand, EditorId, EditorIdView, ReparseOutcome, ReparseWork};
use imba::{store::Store, View};

fn test_cx() -> std::sync::Arc<himark::Workshop> {
    std::sync::Arc::new(himark::Workshop::new(
        himark::embedded_fonts::source(),
        test_theme(),
    ))
}

fn seed_complete(
    store: &mut Store,
    ui: &imba::UiCtx,
    mut document: Document,
    width: f32,
) -> (himark::DocumentId, EditorIdView, EditorId) {
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));

    store.put(himark::env::Enrichers(std::sync::Arc::new(
        markdown_enrichers(himark::Enrichers::new()),
    )));
    let editor = document.add_editor(
        width,
        None,
        himark::EditorBuild::Complete,
        &[],
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let document_id =
        himark::OpenDocuments::register(store, document.clone(), None, "test".to_owned(), 0);
    (document_id, EditorIdView::new(document_id, editor), editor)
}

fn resize_entity(
    store: &mut Store,
    ui: &imba::UiCtx,
    entity: EditorIdView,
    width: f32,
    anchor: u32,
) -> imba::effect::Batch<EditorCommand> {
    let mut document = himark::OpenDocuments::document(store, entity.document()).expect("document");
    let mut batch = imba::effect::Batch::new();
    document.resize(
        entity.editor(),
        width,
        anchor,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut batch.effects(),
    );
    himark::OpenDocuments::put_document(store, entity.document(), document);
    batch
}

fn perform_pumped(store: &mut Store, view: EditorIdView, command: EditorCommand) {
    let cx = test_cx();
    let ui = himark::test_document::test_ui();
    let mut node = view;
    let mut batch = imba::effect::Batch::new();
    let _ = node.perform(store, &ui, command, &mut batch.effects());
    let mut pending: Vec<EditorCommand> = himark::test_support::surviving_launches(batch)
        .into_iter()
        .map(|effect| himark::test_support::handle_effect(effect, &cx))
        .collect();
    while let Some(command) = pending.pop() {
        let mut more = imba::effect::Batch::new();
        node.perform(store, &ui, command, &mut more.effects());
        pending.extend(
            himark::test_support::surviving_launches(more)
                .into_iter()
                .map(|effect| himark::test_support::handle_effect(effect, &cx)),
        );
    }
}

fn apply_outcome(store: &mut Store, document_id: himark::DocumentId, outcome: ReparseOutcome) {
    let ui = himark::test_document::test_ui();
    let mut document = himark::OpenDocuments::document(store, document_id).expect("document");
    let mut batch = imba::effect::Batch::new();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut batch.effects(),
    );
    himark::OpenDocuments::put_document(store, document_id, document);

    let Some(editor) = himark::OpenDocuments::document_ref(&store, document_id)
        .and_then(|document| document.editor_ids().next())
    else {
        return;
    };
    let mut node = EditorIdView::new(document_id, editor);
    let cx = test_cx();
    let ui = himark::test_document::test_ui();
    let mut pending: Vec<EditorCommand> = himark::test_support::surviving_launches(batch)
        .into_iter()
        .map(|effect| himark::test_support::handle_effect(effect, &cx))
        .collect();
    while let Some(command) = pending.pop() {
        let mut more = imba::effect::Batch::new();
        node.perform(store, &ui, command, &mut more.effects());
        pending.extend(
            himark::test_support::surviving_launches(more)
                .into_iter()
                .map(|effect| himark::test_support::handle_effect(effect, &cx)),
        );
    }
}

#[test]
fn worker_repairs_are_quantized_and_heal_the_viewport_first() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let sample = include_str!("../../demo/sample.md");
    let source = sample.repeat(120);
    let document = document_from_markdown(&source, store, ui, &test_fonts(), &test_theme());

    let mut store = Store::new();
    let (document_id, entity_id, editor) = seed_complete(&mut store, &ui, document, 900.0);

    let effects = resize_entity(&mut store, &ui, entity_id, 620.0, 0);
    assert_eq!(effects.len(), 1, "the tail rides one repair effect");

    let height = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .content_height(editor);
    let top = height * 0.7;
    let bottom = top + 800.0;
    let mut node = entity_id;
    let ui = himark::test_document::test_ui();
    let anchor = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .first_visible_byte(editor, top);
    let mut batch = imba::effect::Batch::new();
    node.perform(
        &mut store,
        &ui,
        EditorCommand::Viewport {
            width: 620.0,
            top,
            bottom,
            anchor,
        },
        &mut batch.effects(),
    );

    let cx = test_cx();
    let effect = himark::test_support::surviving_launches(batch)
        .pop()
        .expect("the viewport report queues the repair chain");
    let command = himark::test_support::handle_effect(effect, &cx);
    node.perform(
        &mut store,
        &ui,
        command,
        &mut imba::effect::Batch::new().effects(),
    );
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert!(
        !document.visible_damage(editor, top, bottom),
        "the landing healed the viewport band first"
    );
    assert!(
        document.probe_state(editor).0.is_some(),
        "one run must NOT repair the whole document — quantization is the fix"
    );
}

#[test]
fn a_scroll_into_a_resize_tail_heals_synchronously() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let sample = include_str!("../../demo/sample.md");
    let source = sample.repeat(120);
    let document = document_from_markdown(&source, store, ui, &test_fonts(), &test_theme());

    let mut store = Store::new();
    let (document_id, entity_id, editor) = seed_complete(&mut store, &ui, document, 900.0);
    let _ = resize_entity(&mut store, &ui, entity_id, 620.0, 0);

    let height = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .content_height(editor);
    let top = height * 0.6;
    let bottom = top + 800.0;
    assert!(
        himark::OpenDocuments::document_ref(&store, document_id)
            .expect("document")
            .visible_damage(editor, top, bottom),
        "the deep band is a resize tail (still stale)"
    );

    let mut node = entity_id;
    let ui = himark::test_document::test_ui();
    let anchor = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .first_visible_byte(editor, top);

    node.perform(
        &mut store,
        &ui,
        EditorCommand::Viewport {
            width: 620.0,
            top,
            bottom,
            anchor,
        },
        &mut imba::effect::Batch::new().effects(),
    );
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert!(
        !document.visible_damage(editor, top, bottom),
        "the band the user scrolled into re-shaped synchronously"
    );
}

#[test]
fn resize_repair_timing_probe() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let sample = include_str!("../../demo/sample.md");
    let source = sample.repeat(1409);

    let started = std::time::Instant::now();
    let document = document_from_markdown(&source, store, ui, &test_fonts(), &test_theme());
    eprintln!("document_from_markdown: {:?}", started.elapsed());

    let mut store = Store::new();
    let started = std::time::Instant::now();
    let (_, editor_id, _) = seed_complete(&mut store, &ui, document, 900.0);
    eprintln!("seed_complete(900): {:?}", started.elapsed());

    let started = std::time::Instant::now();
    let effects = resize_entity(&mut store, &ui, editor_id, 609.0, 2_000_000);
    eprintln!("Editor::resize sync part: {:?}", started.elapsed());
    imba::perf::record(
        "monster-resize",
        "sync_ms",
        started.elapsed().as_secs_f64() * 1000.0,
    );
    assert_eq!(effects.len(), 1);

    let mut node = editor_id;
    let cx = test_cx();
    for effect in himark::test_support::surviving_launches(effects) {
        let started = std::time::Instant::now();
        let command = himark::test_support::handle_effect(effect, &cx);
        eprintln!("repair effect run: {:?}", started.elapsed());
        imba::perf::record(
            "monster-resize",
            "repair_effect_ms",
            started.elapsed().as_secs_f64() * 1000.0,
        );
        node.perform(
            &mut store,
            himark::test_document::test_ui(),
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }

    let started = std::time::Instant::now();
    let effects = resize_entity(&mut store, &ui, editor_id, 500.0, 0);
    eprintln!(
        "Editor::resize(anchor 0) sync part: {:?}",
        started.elapsed()
    );
    assert_eq!(
        himark::test_support::surviving_launches(effects).len(),
        1,
        "a top-anchored resize must leave the tail as an effect"
    );
}

#[test]
fn content_only_edits_rebuild_inline_markup() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    let document = document_from_markdown(
        "some **bold** words\n",
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 400.0);

    let strong_spans = |store: &Store| {
        let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        document
            .markup()
            .marks_inline_hidden_in(0..19, &mut inline, &mut hidden);
        inline
            .iter()
            .filter(|decoration| decoration.id == StyleId::Strong)
            .count()
    };
    assert!(strong_spans(&store) > 0, "the strong span is decorated");

    let mut store = store;
    for _ in 0..6 {
        let mut view = editor;
        let _ = view.perform(
            &mut store,
            himark::test_document::test_ui(),
            EditorCommand::Move {
                motion: himark::Motion::Right,
                select: false,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    }
    let mut view = editor;
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::Backspace,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        {
            let mut text_view = himark::OpenDocuments::document_ref(&store, document_id)
                .expect("document")
                .text()
                .view();
            let byte_count = text_view.byte_count();
            text_view.byte_string(0, byte_count)
        },
        "some *bold** words\n",
        "one `*` was deleted"
    );

    let outcome = ReparseWork::capture(
        himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("document has a parse")
    .run_reparse();
    assert!(
        !outcome.invalidated().is_empty(),
        "a content-only edit must invalidate the edited block"
    );
    let mut store = store;
    apply_outcome(&mut store, document_id, outcome);

    assert_eq!(
        strong_spans(&store),
        0,
        "the strong decoration is gone after removing one `*`"
    );
}

#[test]
fn a_focused_table_cell_presents_structural_commands() {
    let ui = himark::test_document::test_ui();
    use imba::View;
    let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, entity, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let view = entity;
    let ui = himark::test_document::test_ui();
    assert!(
        imba::focus::frame_commands(&view, &store, &ui)
            .iter()
            .all(|presentable| presentable.id.starts_with("editor.")),
        "before the click only the editor's caret commands are on offer"
    );

    let mut view = entity;
    let _ = view.perform(
        &mut store,
        &ui,
        EditorCommand::Inlay {
            key,
            command: Box::new(table::TableCommand::Cell {
                row: 1,
                col: 0,
                command: EditorCommand::Click {
                    kind: himark::ClickKind::Set,
                    point: skia_safe::Point::new(2.0, 2.0),
                },
            }) as himark::InlayCommand,
        },
        &mut imba::effect::Batch::new().effects(),
    );

    let commands = imba::focus::frame_commands(&view, &store, &ui);
    let ids: Vec<&str> = commands.iter().map(|command| command.id).collect();
    assert!(ids.contains(&"table.insert-row-below"), "ids: {ids:?}");
    assert!(ids.contains(&"table.insert-row-above"), "ids: {ids:?}");
    assert!(ids.contains(&"table.remove-column"), "ids: {ids:?}");
    assert!(
        !ids.contains(&"table.remove-row"),
        "the only body row must not offer removal: {ids:?}"
    );

    let insert = commands
        .into_iter()
        .find(|command| command.id == "table.insert-row-below")
        .expect("the insert");
    let _ = view.perform(
        &mut store,
        &ui,
        insert.command,
        &mut imba::effect::Batch::new().effects(),
    );
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let text = document.text().byte_string(0, document.text().byte_count());
    assert_eq!(text, "| a | b |\n|---|---|\n| 1 | 2 |\n|   |   |\n");
}

#[test]
fn nested_task_items_each_get_their_checkbox() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let source = "* [ ] Licensing #P0\n\t* [ ] product code\n\t* [x] license delivery\n* [ ] Performance\n\t* [ ] diagnostics latency\n\t    * [x] deep nesting\n";
    let document = document_from_markdown(source, store, ui, &test_fonts(), &test_theme());
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    assert_eq!(
        inlays.len(),
        6,
        "one checkbox inlay per task item, nested included"
    );

    let (_, blocks) = markdown_document(source, store, ui, &test_fonts(), &test_theme());
    let items: Vec<_> = blocks
        .iter()
        .filter(|block| block.marks.list_item)
        .collect();
    assert_eq!(items.len(), 6, "six list-item blocks");
    for block in items {
        assert!(
            block.checkbox.is_some(),
            "every item carries its marker: {:?}",
            block.range
        );
    }
}

#[test]
fn inlay_commands_route_by_layer() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let mut document = document_from_markdown(source, store, ui, &test_fonts(), &test_theme());
    let byte_count = document.text().byte_count() as u32;
    let test_markup = himark::MarkupId::mint();
    document.ensure_document_markup(test_markup);
    let user_key = document.push_inlay(
        test_markup,
        0..1,
        himark::Inlay::new(
            himark::InlayMode::Above,
            himark::EditorView::input(100.0, store, ui, himark::embedded_fonts::source()),
        ),
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let keys: Vec<_> = document
        .all_inlays_in(0..byte_count)
        .into_iter()
        .map(|interval| interval.key)
        .collect();
    assert_eq!(keys.len(), 2, "the table and the host inlay");
    assert!(keys.contains(&user_key));
    let unique: std::collections::HashSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(), "scoped keys are distinct");
}

#[test]
fn adding_a_table_row_keeps_the_inlay_covering_the_block() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nafter\n";
    let block_len = source.find("\n\n").expect("blank line") as u32 + 1;
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);

    let inlay_range = |store: &Store| {
        himark::OpenDocuments::document_ref(&store, document_id)
            .expect("document")
            .all_inlays_in(0..u32::MAX / 2)
            .into_iter()
            .next()
            .expect("the table inlay")
            .range
    };
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;
    assert_eq!(inlay_range(&store), 0..block_len, "covers the block");

    let send = |store: &mut Store, command: table::TableCommand| {
        let mut view = editor;
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(command) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };

    send(&mut store, table::TableCommand::InsertRow(2));
    let text = |store: &Store| {
        let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
        document.text().byte_string(0, document.text().byte_count())
    };
    assert_eq!(
        text(&store),
        "| a | b |\n|---|---|\n| 1 | 2 |\n|   |   |\n| 3 | 4 |\n\nafter\n"
    );
    let grown = inlay_range(&store);
    assert_eq!(grown.start, 0);
    assert_eq!(
        grown.end,
        block_len + "|   |   |\n".len() as u32,
        "the interval grew with the middle insert"
    );

    send(&mut store, table::TableCommand::InsertRow(4));
    assert_eq!(
        text(&store),
        "| a | b |\n|---|---|\n| 1 | 2 |\n|   |   |\n| 3 | 4 |\n|   |   |\n\nafter\n"
    );
    let appended = inlay_range(&store);
    assert_eq!(
        appended.end - appended.start,
        block_len + 2 * "|   |   |\n".len() as u32,
        "the interval grew with the appended row"
    );

    let parsers = std::sync::Arc::new(markdown_languages(himark::SyntaxLanguages::new()));
    let document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    let outcome = himark::ReparseWork::capture(&document, parsers)
        .expect("reparse")
        .run_reparse();
    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    document.apply_reparse_outcome(
        outcome,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    himark::OpenDocuments::put_document(&mut store, document_id, document);
    let landed = inlay_range(&store);
    assert_eq!(
        landed.end - landed.start,
        block_len + 2 * "|   |   |\n".len() as u32,
        "the landed interval still covers the whole block"
    );
}

#[test]
fn adding_a_table_row_keeps_the_blocks_below_it() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n## Tail Heading\n\n| x | y |\n|---|---|\n| 3 | 4 |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the first table inlay")
        .key;

    let mut view = editor;
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::Inlay {
            key,
            command: Box::new(table::TableCommand::InsertRow(2)) as himark::InlayCommand,
        },
        &mut imba::effect::Batch::new().effects(),
    );

    let parsers = std::sync::Arc::new(markdown_languages(himark::SyntaxLanguages::new()));
    let document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    let outcome = himark::ReparseWork::capture(&document, parsers)
        .expect("reparse")
        .run_reparse();
    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    document.apply_reparse_outcome(
        outcome,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let text = document.text().byte_string(0, document.text().byte_count());
    let byte_count = document.text().byte_count() as u32;
    let tables = document
        .all_inlays_in(0..byte_count)
        .into_iter()
        .filter(|interval| matches!(interval.inlay.mode(), himark::InlayMode::Instead(_)))
        .count();
    assert_eq!(tables, 2, "both tables survive the landing: {text}");

    let heading = text.find("## Tail Heading").expect("heading") as u32;
    let marks = document.markup().block_marks_in(heading..heading + 15);
    assert!(
        marks
            .ids()
            .iter()
            .any(|id| matches!(id, himark::StyleId::Header(_))),
        "the heading below the table keeps its block style"
    );
    let rule = text.find("\n---\n").expect("rule") as u32 + 1;
    let marks = document.markup().block_marks_in(rule..rule + 3);
    assert!(
        marks
            .ids()
            .iter()
            .any(|id| matches!(id, himark::StyleId::HorizontalLine)),
        "the rule below the table keeps its block style"
    );
}

#[test]
fn multibyte_typing_in_a_cell_stays_utf8() {
    let ui = himark::test_document::test_ui();
    let source = "| Unicode | emoji and combining marks |\n|---|---|\n| a | b |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);

    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let send = |store: &mut Store, command: EditorCommand| {
        let mut view = editor;
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 0,
                    col: 1,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };

    send(
        &mut store,
        EditorCommand::Click {
            kind: himark::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        },
    );
    for _ in 0..15 {
        send(
            &mut store,
            EditorCommand::Move {
                motion: himark::Motion::Right,
                select: false,
            },
        );
    }

    let burst = ["ф", "ж", "д", " ", "ы", "в", " ", "а", "ф", "ы"];
    for (i, typed) in burst.iter().enumerate() {
        send(
            &mut store,
            EditorCommand::InsertText {
                text: (*typed).to_owned(),
            },
        );

        if i == 4 {
            let document =
                himark::OpenDocuments::document_ref(&store, document_id).expect("document");
            if let Some(work) = ReparseWork::capture(&document, test_languages()) {
                let outcome = work.run_reparse();
                let mut view = editor;
                let _ = view.perform(
                    &mut store,
                    himark::test_document::test_ui(),
                    EditorCommand::ApplyReparse(outcome),
                    &mut imba::effect::Batch::new().effects(),
                );
            }
        }

        let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
        let bytes = document.text().byte_string(0, document.text().byte_count());
        assert!(
            std::str::from_utf8(bytes.as_bytes()).is_ok(),
            "document corrupted after keystroke {i} ({typed:?}): {:?}",
            bytes.as_bytes()
        );
    }

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let text = document.text().byte_string(0, document.text().byte_count());
    assert!(
        text.contains("emoji and combiфжд ыв афыning marks"),
        "typed burst lands contiguously: {text}"
    );
}

#[test]
fn a_stale_reparse_landing_mid_burst_must_not_revert_cell_state() {
    let ui = himark::test_document::test_ui();
    let source = "| head | combining |\n|---|---|\n| a | b |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);

    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let send = |store: &mut Store, command: EditorCommand| {
        let mut view = editor;
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 0,
                    col: 1,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };

    send(
        &mut store,
        EditorCommand::Click {
            kind: himark::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        },
    );
    for _ in 0..5 {
        send(
            &mut store,
            EditorCommand::Move {
                motion: himark::Motion::Right,
                select: false,
            },
        );
    }

    send(&mut store, EditorCommand::InsertText { text: "ф".into() });
    let in_flight = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();

    send(&mut store, EditorCommand::InsertText { text: "ы".into() });
    send(&mut store, EditorCommand::InsertText { text: "в".into() });

    let mut view = editor;
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::ApplyReparse(in_flight),
        &mut imba::effect::Batch::new().effects(),
    );

    send(&mut store, EditorCommand::InsertText { text: "а".into() });

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let text = document.text().byte_string(0, document.text().byte_count());
    assert!(
        std::str::from_utf8(text.as_bytes()).is_ok(),
        "document corrupted: {:?}",
        text.as_bytes()
    );
    assert!(
        text.contains("combiфыва"),
        "all four keystrokes land contiguously: {text}"
    );
}

#[test]
fn click_placed_carets_in_multibyte_cells_stay_on_boundaries() {
    let ui = himark::test_document::test_ui();
    let source = "| head | фывафыва прол джлол |\n|---|---|\n| a | b |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);

    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let send = |store: &mut Store, command: EditorCommand| {
        let mut view = editor;
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 0,
                    col: 1,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };

    for x in [0.0f32, 7.0, 13.0, 26.0, 41.0, 55.0, 90.0, 160.0, 400.0] {
        send(
            &mut store,
            EditorCommand::Click {
                kind: himark::ClickKind::Set,
                point: skia_safe::Point::new(x, 8.0),
            },
        );
        send(
            &mut store,
            EditorCommand::InsertText {
                text: "ы".to_owned(),
            },
        );
        let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
        let bytes = document.text().byte_string(0, document.text().byte_count());
        assert!(
            std::str::from_utf8(bytes.as_bytes()).is_ok(),
            "corrupted after click at x={x}: {:?}",
            bytes.as_bytes()
        );
    }
}

#[test]
fn typing_in_a_cell_writes_through_and_survives_the_reparse() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| left | right |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 700.0);

    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let mut view = editor;
    let cell_command = |store: &mut Store, command: EditorCommand| {
        let mut view = editor;
        view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 0,
                    col: 1,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        )
    };
    let _ = cell_command(
        &mut store,
        EditorCommand::Click {
            kind: himark::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        },
    );
    let _ = cell_command(
        &mut store,
        EditorCommand::InsertText {
            text: "x|y\nz".to_owned(),
        },
    );

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let text = document.text().byte_string(0, document.text().byte_count());
    assert!(
        text.contains("x\\|y<br>z"),
        "cell edit reaches the source escaped: {text}"
    );

    let outcome = ReparseWork::capture(&document, test_languages())
        .expect("reparse pending")
        .run_reparse();
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::ApplyReparse(outcome),
        &mut imba::effect::Batch::new().effects(),
    );

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    assert_eq!(inlays.len(), 1, "still exactly the table");
    assert_eq!(
        inlays[0].key, key,
        "adoption keeps the key — focus routing survives"
    );

    let _ = {
        let mut view = editor;
        view.perform(
            &mut store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 0,
                    col: 1,
                    command: EditorCommand::InsertText {
                        text: "!".to_owned(),
                    },
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        )
    };
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let text = document.text().byte_string(0, document.text().byte_count());
    assert!(
        text.contains("x\\|y<br>z!"),
        "the caret survived the reparse: {text}"
    );
}

#[test]
fn insert_table_pads_blank_lines_and_becomes_an_inlay() {
    let ui = himark::test_document::test_ui();
    use himark::DynamicEditorCommand;

    let location = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        vec!["notes.md".to_owned()],
    );

    let mut store = Store::new();
    let document =
        document_from_markdown("alpha\n\nbeta", &store, ui, &test_fonts(), &test_theme());
    let (document_id, _, editor) = seed_complete(&mut store, &ui, document, 700.0);
    {
        let mut document = himark::OpenDocuments::document(&store, document_id).expect("document");
        document.set_caret(editor, 2);
        let mut batch = imba::effect::Batch::new();
        table::InsertTable.perform(
            &mut store,
            &ui,
            &mut document,
            editor,
            &location,
            None,
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, document_id, document);
    }
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert_eq!(
        document.text().byte_string(0, document.text().byte_count()),
        "al\n\n|   |   |\n| --- | --- |\n|   |   |\n\npha\n\nbeta",
        "the block separates from both neighbors"
    );

    let mut store = Store::new();
    let document = document_from_markdown("gamma\n\n", &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, editor) = seed_complete(&mut store, &ui, document, 700.0);
    {
        let mut document = himark::OpenDocuments::document(&store, document_id).expect("document");
        document.set_caret(editor, 7);
        let mut batch = imba::effect::Batch::new();
        table::InsertTable.perform(
            &mut store,
            &ui,
            &mut document,
            editor,
            &location,
            None,
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, document_id, document);
    }
    let text = {
        let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
        document.text().byte_string(0, document.text().byte_count())
    };
    assert_eq!(text, "gamma\n\n|   |   |\n| --- | --- |\n|   |   |");

    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();
    perform_pumped(&mut store, view, EditorCommand::ApplyReparse(outcome));
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    assert_eq!(inlays.len(), 1, "the template became a table inlay");
    let table = inlays[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("a table view");
    assert_eq!(table.cell_content(0, 0), "", "2 columns, empty header");
    assert_eq!(table.cell_content(1, 1), "", "and an empty body row");

    let mut store = Store::new();
    let plain = Document::new(
        text::Text::from_string_exact("plain"),
        himark::Markup::builder().finish(),
    );
    let (document_id, _, editor) = seed_complete(&mut store, &ui, plain, 700.0);
    {
        let mut document = himark::OpenDocuments::document(&store, document_id).expect("document");
        let mut batch = imba::effect::Batch::new();
        table::InsertTable.perform(
            &mut store,
            &ui,
            &mut document,
            editor,
            &location,
            None,
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, document_id, document);
    }
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert_eq!(
        document.text().byte_string(0, document.text().byte_count()),
        "plain",
        "non-markdown documents stay untouched"
    );
}

#[test]
fn enter_in_a_cell_becomes_a_br_and_survives_the_next_letter() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| one | two |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let mut view = view;
    let mut cell = |store: &mut Store, command: EditorCommand| {
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 1,
                    col: 0,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };
    cell(
        &mut store,
        EditorCommand::Click {
            kind: himark::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        },
    );
    cell(&mut store, EditorCommand::Enter { soft: false });
    let text_of = |store: &Store| {
        let document = himark::OpenDocuments::document_ref(store, document_id).expect("document");
        document.text().byte_string(0, document.text().byte_count())
    };
    assert!(
        text_of(&store).contains("<br>one"),
        "Enter wrote through as <br>: {}",
        text_of(&store)
    );

    cell(
        &mut store,
        EditorCommand::InsertText {
            text: "x".to_owned(),
        },
    );
    assert!(
        text_of(&store).contains("<br>xone"),
        "the letter followed the break: {}",
        text_of(&store)
    );

    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::ApplyReparse(outcome),
        &mut imba::effect::Batch::new().effects(),
    );
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let table = document.all_inlays_in(0..byte_count)[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("the table view");
    assert_eq!(
        table.cell_content(1, 0),
        "\nxone",
        "the break lives in the cell across the reparse"
    );
}

#[test]
fn unmapped_cell_mutations_are_swallowed_not_diverged() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| one | two |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let mut view = view;
    let mut cell = |store: &mut Store, command: EditorCommand| {
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            EditorCommand::Inlay {
                key,
                command: Box::new(table::TableCommand::Cell {
                    row: 1,
                    col: 0,
                    command,
                }) as himark::InlayCommand,
            },
            &mut imba::effect::Batch::new().effects(),
        );
    };
    cell(
        &mut store,
        EditorCommand::Click {
            kind: himark::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        },
    );
    cell(&mut store, EditorCommand::DeleteForward);
    cell(&mut store, EditorCommand::Undo);

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert_eq!(
        document.text().byte_string(0, document.text().byte_count()),
        source,
        "the source never moved"
    );
    let byte_count = document.text().byte_count() as u32;
    let table = document.all_inlays_in(0..byte_count)[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("the table view");
    assert_eq!(
        table.cell_content(1, 0),
        "one",
        "the cell never diverged from it"
    );
}

#[test]
fn breaking_the_delimiter_dissolves_the_widget() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    assert_eq!(
        himark::OpenDocuments::document_ref(&store, document_id)
            .expect("document")
            .all_inlays_in(0..source.len() as u32)
            .len(),
        1,
        "the builder's pass installed the widget"
    );

    {
        let mut document =
            himark::OpenDocuments::document(&mut store, document_id).expect("document");
        let mut batch = imba::effect::Batch::new();
        document.edit(
            &operation::Operation::from_ops([
                operation::Op::Retain(10),
                operation::Op::Delete("|---|---|".to_owned()),
                operation::Op::Insert("not a delimiter".to_owned()),
            ]),
            &store,
            ui,
            &test_fonts(),
            &test_theme(),
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, document_id, document);
    }
    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();
    perform_pumped(&mut store, view, EditorCommand::ApplyReparse(outcome));

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    assert!(
        document.all_inlays_in(0..byte_count).is_empty(),
        "the dissolved table's widget left with the landing"
    );
}

#[test]
fn an_external_edit_inside_a_table_reaches_the_cells_after_the_reparse() {
    let ui = himark::test_document::test_ui();
    use operation::{Op, Operation};

    let source = "| a | b |\n|---|---|\n| left | right |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let at = source.find("left").expect("the cell") as u32;
    {
        let mut document = himark::OpenDocuments::document(&store, document_id).expect("document");
        let mut batch = imba::effect::Batch::new();
        document.edit(
            &Operation::from_ops([
                Op::Retain(at),
                Op::Delete("left".to_owned()),
                Op::Insert("outside".to_owned()),
            ]),
            &store,
            ui,
            &test_fonts(),
            &test_theme(),
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, document_id, document);
    }

    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();
    perform_pumped(&mut store, view, EditorCommand::ApplyReparse(outcome));

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    assert_eq!(inlays.len(), 1, "still exactly the table");
    assert_eq!(inlays[0].key, key, "adoption keeps the key");
    let table = inlays[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("the table view");
    assert_eq!(
        table.cell_content(1, 0),
        "outside",
        "the rebuild replaced the stale live view"
    );
}

#[test]
fn undoing_a_cell_edit_rebuilds_the_table_at_the_next_reparse() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| one | two |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let mut view = view;
    let mut send = |store: &mut Store, command: EditorCommand| {
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };

    send(
        &mut store,
        EditorCommand::Inlay {
            key,
            command: Box::new(table::TableCommand::Cell {
                row: 1,
                col: 0,
                command: EditorCommand::Click {
                    kind: himark::ClickKind::Set,
                    point: skia_safe::Point::new(1.0, 1.0),
                },
            }) as himark::InlayCommand,
        },
    );
    send(
        &mut store,
        EditorCommand::Inlay {
            key,
            command: Box::new(table::TableCommand::Cell {
                row: 1,
                col: 0,
                command: EditorCommand::InsertText {
                    text: "X".to_owned(),
                },
            }) as himark::InlayCommand,
        },
    );
    let text_of = |store: &Store| {
        let document = himark::OpenDocuments::document_ref(store, document_id).expect("document");
        document.text().byte_string(0, document.text().byte_count())
    };
    assert!(
        text_of(&store).contains("Xone"),
        "the keystroke wrote through"
    );

    send(&mut store, EditorCommand::Undo);
    assert!(
        !text_of(&store).contains("Xone"),
        "undo reverted the source: {}",
        text_of(&store)
    );

    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();

    perform_pumped(&mut store, view, EditorCommand::ApplyReparse(outcome));

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    let table = inlays[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("the table view");
    assert_eq!(
        table.cell_content(1, 0),
        "one",
        "the undone source reached the cells"
    );
}

#[test]
fn a_faithful_reparse_still_carries_the_live_table() {
    let ui = himark::test_document::test_ui();
    let source = "| a | b |\n|---|---|\n| one | two |\n";
    let mut store = Store::new();
    let document = document_from_markdown(source, &store, ui, &test_fonts(), &test_theme());
    let (document_id, view, _) = seed_complete(&mut store, &ui, document, 700.0);
    let key = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .next()
        .expect("the table inlay")
        .key;

    let mut view = view;
    let mut send = |store: &mut Store, command: EditorCommand| {
        let _ = view.perform(
            store,
            himark::test_document::test_ui(),
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };
    send(
        &mut store,
        EditorCommand::Inlay {
            key,
            command: Box::new(table::TableCommand::Cell {
                row: 1,
                col: 0,
                command: EditorCommand::Click {
                    kind: himark::ClickKind::Set,
                    point: skia_safe::Point::new(1.0, 1.0),
                },
            }) as himark::InlayCommand,
        },
    );
    let type_in_cell = |text: &str| EditorCommand::Inlay {
        key,
        command: Box::new(table::TableCommand::Cell {
            row: 1,
            col: 0,
            command: EditorCommand::InsertText {
                text: text.to_owned(),
            },
        }) as himark::InlayCommand,
    };

    send(&mut store, type_in_cell("X"));
    let outcome = ReparseWork::capture(
        &himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("reparse pending")
    .run_reparse();
    send(&mut store, type_in_cell("Y"));
    send(&mut store, EditorCommand::ApplyReparse(outcome));

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let byte_count = document.text().byte_count() as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    let table = inlays[0]
        .inlay
        .view_as::<table::TableEditor>()
        .expect("the table view");
    assert_eq!(
        table.cell_content(1, 0),
        "XYone",
        "the live view's local novelty survived the landing"
    );
}

#[test]
fn a_table_replaces_all_of_its_source_lines() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let two_rows = "before\n\n| x | q |\n|---|---|\n| z | w |\n\nafter\n";
    let six_rows = "before\n\n| x | q |\n|---|---|\n| z | w |\n| z | w |\n| z | w |\n| z | w |\n| z | w |\n\nafter\n";

    let height = |source: &str| {
        himark::EditorView::complete(
            document_from_markdown(source, store, ui, &test_fonts(), &test_theme()),
            700.0,
            store,
            ui,
            &test_fonts(),
            &test_theme(),
        )
        .content_height()
    };
    let base = height(two_rows);
    let grown = height(six_rows);

    let per_row = (grown - base) / 4.0;
    assert!(per_row > 20.0, "rows must occupy space (got {per_row}/row)");
    assert!(
        per_row < 100.0,
        "continuation lines leak under the table ({per_row}/row)"
    );
}

#[test]
fn a_markdown_table_becomes_an_instead_inlay() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let source = "before\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter\n";
    let document = document_from_markdown(source, store, ui, &test_fonts(), &test_theme());
    let byte_count = document.text().byte_count() as u32;

    let table_start = source.find('|').expect("table") as u32;
    let inlays = document.all_inlays_in(0..byte_count);
    assert_eq!(inlays.len(), 1, "exactly the table");
    let inlay = &inlays[0];
    assert_eq!(
        inlay.inlay.mode(),
        himark::InlayMode::Instead(himark::InsteadKind::FullLine)
    );
    assert!(inlay.range.start >= table_start.saturating_sub(1));
    assert!(inlay.range.end > inlay.range.start);

    let plain = document_from_markdown(
        "a | b | c\njust prose\n",
        store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let count = plain.all_inlays_in(0..64).len();
    assert_eq!(count, 0);
}

#[test]
fn late_reparse_outcomes_rebase_over_typing() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    let document = document_from_markdown(
        "plain words here\n",
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 400.0);

    let mut view = editor;
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::InsertText {
            text: "# ".to_owned(),
        },
        &mut imba::effect::Batch::new().effects(),
    );
    let in_flight = ReparseWork::capture(
        himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("document has a parse")
    .run_reparse();

    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::InsertText {
            text: "big ".to_owned(),
        },
        &mut imba::effect::Batch::new().effects(),
    );

    apply_outcome(&mut store, document_id, in_flight);
    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    let line_end = "# big plain words here".len() as u32;
    assert_eq!(
        document
            .markup()
            .block_marks_in(0..line_end)
            .ids()
            .contains(&StyleId::Header(1))
            .then_some(1u8),
        Some(1),
        "the rebased reparse styles the header at its shifted position"
    );
}

#[test]
fn demo_open_pipeline_applies_markup_and_layout() {
    let ui = himark::test_document::test_ui();
    let source = format!("# heading\n\n{}\n", "words ".repeat(40_000));
    let text = text::Text::from_string_exact(&source);
    let mut document = himark::Document::new(text.clone(), himark::Markup::new());

    let mut store = Store::new();
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    let document_id =
        himark::OpenDocuments::register(&mut store, document.clone(), None, "test".to_owned(), 0);
    let mut batch = imba::effect::Batch::new();
    let editor_id = himark::mount_editor(
        &store,
        &ui,
        &mut document,
        600.0,
        None,
        &mut batch.effects(),
    );
    let entity_id = EditorIdView::new(document_id, editor_id);
    himark::OpenDocuments::put_document(&mut store, document_id, document.clone());

    let mut effects = himark::test_support::surviving_launches(std::mem::replace(
        &mut batch,
        imba::effect::Batch::new(),
    ));
    assert_eq!(effects.len(), 1, "the open must defer its tail");
    let capped = entity_id.gathered(&store).expect("entity").content_height();
    let reference = himark::EditorView::complete(
        document.clone(),
        600.0,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    assert!(
        capped < reference.content_height(),
        "the open is bounded: {capped} < {}",
        reference.content_height()
    );

    let command =
        himark::test_support::handle_effect(effects.pop().expect("repair effect"), &test_cx());
    assert!(
        matches!(command, EditorCommand::ApplyRepair(ref items) if !items.is_empty()),
        "the repair effect must yield a non-empty ApplyRepair"
    );

    let mut view = entity_id;
    view.perform(
        &mut store,
        himark::test_document::test_ui(),
        command,
        &mut batch.effects(),
    );
    let mut pending = himark::test_support::surviving_launches(std::mem::replace(
        &mut batch,
        imba::effect::Batch::new(),
    ));
    while let Some(effect) = pending.pop() {
        let command = himark::test_support::handle_effect(effect, &test_cx());
        view.perform(
            &mut store,
            himark::test_document::test_ui(),
            command,
            &mut batch.effects(),
        );
        pending.extend(himark::test_support::surviving_launches(std::mem::replace(
            &mut batch,
            imba::effect::Batch::new(),
        )));
    }
    let gathered = view.gathered(&store).expect("entity");
    assert!(
        gathered.find_misaligned_boundary().is_none(),
        "the repaired layout tiles the text"
    );
    assert_eq!(
        gathered.content_height(),
        reference.content_height(),
        "the applied repair completes the plain layout"
    );

    let tree = parse_markdown(&text);
    let markup = markup_builder_from_tree(&text, &tree, &test_fonts(), &test_theme()).finish();
    let sites = MarkdownLanguage.sites_impl(&text, &tree);
    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    let mut parse_batch = imba::effect::Batch::new();
    document.apply_syntax(
        himark::Syntax::new("markdown", Some(Box::new(TsTree(tree))), markup),
        &sites,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut parse_batch.effects(),
    );
    himark::OpenDocuments::put_document(&mut store, document_id, document);

    let styled = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert_eq!(
        styled
            .markup()
            .block_marks_in(0..9)
            .ids()
            .contains(&StyleId::Header(1))
            .then_some(1u8),
        Some(1),
        "install_parse must apply the syntax markup"
    );

    let styled_reference = himark::EditorView::complete(
        styled.clone(),
        600.0,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let cx = test_cx();
    let mut pending: Vec<EditorCommand> = himark::test_support::surviving_launches(parse_batch)
        .into_iter()
        .map(|effect| himark::test_support::handle_effect(effect, &cx))
        .collect();
    while let Some(command) = pending.pop() {
        view.perform(
            &mut store,
            himark::test_document::test_ui(),
            command,
            &mut batch.effects(),
        );
        pending.extend(
            himark::test_support::surviving_launches(std::mem::replace(
                &mut batch,
                imba::effect::Batch::new(),
            ))
            .into_iter()
            .map(|effect| himark::test_support::handle_effect(effect, &cx)),
        );
    }
    let gathered = view.gathered(&store).expect("entity");
    assert_eq!(
        gathered.content_height(),
        styled_reference.content_height(),
        "the styled layout converges after the parse's repair"
    );
}

#[test]
fn late_results_do_not_disturb_typing_at_a_soft_line_end() {
    let ui = himark::test_document::test_ui();
    let source = format!("# heading\n\n{}\n", "words ".repeat(40_000));
    let mut store = Store::new();
    let document = document_from_markdown(&source, &store, ui, &test_fonts(), &test_theme());
    let (_, editor, _) = seed_complete(&mut store, &ui, document.clone(), 200.0);

    let mut view = editor;
    let boundary = {
        let gathered = view.gathered(&store).expect("editor");
        let ranges = gathered.document_layout().element_byte_ranges();
        ranges[ranges.len() / 2].start
    };
    {
        let entity = editor;
        let mut document =
            himark::OpenDocuments::document(&store, entity.document()).expect("document");
        document.set_caret(entity.editor(), boundary);

        document.refresh_unhide(entity.editor(), &store, ui, &test_fonts(), &test_theme());
        himark::OpenDocuments::put_document(&mut store, entity.document(), document);
    }

    let caret_line_y = |store: &Store| {
        let gathered = editor.gathered(store).expect("editor");
        let caret = gathered.caret_byte();
        (gathered.document_layout().height_before(caret), caret)
    };

    let mut late: Vec<EditorCommand> = Vec::new();
    let mut saw_late_result = false;
    let (mut last_y, _) = caret_line_y(&store);
    for step in 0..12 {
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            himark::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "x".to_owned(),
            },
            &mut batch.effects(),
        );
        let effects = himark::test_support::surviving_launches(batch);

        for command in late.drain(..) {
            let mut followup_batch = imba::effect::Batch::new();
            view.perform(
                &mut store,
                himark::test_document::test_ui(),
                command,
                &mut followup_batch.effects(),
            );

            for effect in himark::test_support::surviving_launches(followup_batch) {
                let command = himark::test_support::handle_effect(effect, &test_cx());
                view.perform(
                    &mut store,
                    himark::test_document::test_ui(),
                    command,
                    &mut imba::effect::Batch::new().effects(),
                );
            }
        }
        saw_late_result |= !effects.is_empty();
        late = effects
            .into_iter()
            .map(|effect| himark::test_support::handle_effect(effect, &test_cx()))
            .collect();

        let gathered = view.gathered(&store).expect("editor");
        assert_eq!(
            gathered.find_misaligned_boundary(),
            None,
            "step {step}: layout must stay byte-aligned with the text"
        );
        let (y, caret) = caret_line_y(&store);
        assert!(
            y >= last_y,
            "step {step}: the caret's line jumped backward: y {y} < {last_y} (caret {caret})"
        );
        last_y = y;
    }
    assert!(
        saw_late_result,
        "the scenario must actually produce late background results"
    );
}

#[test]
fn interleaved_edits_reparses_and_repairs_keep_boundaries_char_aligned() {
    let ui = himark::test_document::test_ui();
    let mut seed = 0x243f6a8885a308d3u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let source = "# Título 🚀 émphasis\n\npáragraph with **böld** and 😀 emoji\n\n```\ncodé blöck 🧬\n```\n\n> quöte with 🎯 target\n\n## Anöther séction 🌍\n\nfinal wörds 🏁\n"
        .repeat(8);
    let mut store = Store::new();
    let mut document = document_from_markdown(&source, &store, ui, &test_fonts(), &test_theme());
    let narrow_editor = document.add_editor(
        160.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let wide_editor = document.add_editor(
        420.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let document_id =
        himark::OpenDocuments::register(&mut store, document.clone(), None, "test".to_owned(), 0);
    let narrow = EditorIdView::new(document_id, narrow_editor);
    let wide = EditorIdView::new(document_id, wide_editor);

    let texts = ["#", "# ", "é", "😀", "\n\n", "**x**", "```\n", "> ", "xyz"];
    let mut pending_reparses: Vec<ReparseOutcome> = Vec::new();

    let mut pending_commands: Vec<EditorCommand> = Vec::new();

    for step in 0..2000u32 {
        let editor = if rand() % 2 == 0 { narrow } else { wide };
        let mut batch: imba::effect::Batch<EditorCommand> = imba::effect::Batch::new();
        match rand() % 8 {
            0..=3 => {
                let mut view = editor;
                let command = EditorCommand::InsertText {
                    text: texts[(rand() % texts.len() as u64) as usize].to_owned(),
                };
                view.perform(
                    &mut store,
                    himark::test_document::test_ui(),
                    command,
                    &mut batch.effects(),
                );
            }
            4 => {
                let mut view = editor;
                view.perform(
                    &mut store,
                    himark::test_document::test_ui(),
                    EditorCommand::Backspace,
                    &mut batch.effects(),
                );
            }
            5 => {
                let mut view = editor;
                let y = (rand() % 4000) as f32;
                view.perform(
                    &mut store,
                    himark::test_document::test_ui(),
                    EditorCommand::Click {
                        kind: himark::ClickKind::Set,
                        point: skia_safe::Point::new((rand() % 300) as f32, y),
                    },
                    &mut batch.effects(),
                );
            }
            6 => {
                if let Some(work) = ReparseWork::capture(
                    himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
                    test_languages(),
                ) {
                    pending_reparses.push(work.run_reparse());
                }
            }
            _ => {
                if !pending_reparses.is_empty() && rand() % 2 == 0 {
                    let index = (rand() % pending_reparses.len() as u64) as usize;
                    let outcome = pending_reparses.swap_remove(index);
                    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
                        .expect("document")
                        .clone();
                    let mut local = imba::effect::Batch::new();
                    document.apply_reparse_outcome(
                        outcome,
                        &store,
                        ui,
                        &test_fonts(),
                        &test_theme(),
                        &mut local.effects(),
                    );
                    himark::OpenDocuments::put_document(&mut store, document_id, document);
                    pending_commands.extend(
                        himark::test_support::surviving_launches(local)
                            .into_iter()
                            .map(|effect| himark::test_support::handle_effect(effect, &test_cx())),
                    );
                } else if !pending_commands.is_empty() {
                    let index = (rand() % pending_commands.len() as u64) as usize;
                    let command = pending_commands.swap_remove(index);
                    let mut view = editor;
                    view.perform(
                        &mut store,
                        himark::test_document::test_ui(),
                        command,
                        &mut batch.effects(),
                    );
                }
            }
        }
        for effect in himark::test_support::surviving_launches(batch) {
            pending_commands.push(himark::test_support::handle_effect(effect, &test_cx()));
        }

        for (name, id) in [("narrow", narrow), ("wide", wide)] {
            let view = id.gathered(&store).expect("editor");
            if let Some(boundary) = view.find_misaligned_boundary() {
                panic!(
                    "step {step}: {name} layout boundary at byte {boundary}                          splits a UTF-8 character"
                );
            }
        }
    }
}

#[test]
fn typing_a_hash_becomes_a_header_after_the_reparse_lands() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    let document =
        document_from_markdown("hello\n\nworld", &store, ui, &test_fonts(), &test_theme());
    let (document_id, editor, _) = seed_complete(&mut store, &ui, document.clone(), 400.0);

    let mut view = editor;
    let _ = view.perform(
        &mut store,
        himark::test_document::test_ui(),
        EditorCommand::InsertText {
            text: "# ".to_owned(),
        },
        &mut imba::effect::Batch::new().effects(),
    );
    let plain_height = editor.gathered(&store).expect("editor").content_height();
    assert_eq!(
        himark::OpenDocuments::document_ref(&store, document_id)
            .expect("document")
            .markup()
            .block_marks_in(0..7)
            .ids()
            .contains(&StyleId::Header(1))
            .then_some(1u8),
        None,
        "before the reparse the line is still a paragraph"
    );

    let outcome = ReparseWork::capture(
        himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("document has a tree")
    .run_reparse();
    assert!(!outcome.invalidated().is_empty());
    let mut store = store;
    apply_outcome(&mut store, document_id, outcome);

    let document = himark::OpenDocuments::document_ref(&store, document_id).expect("document");
    assert_eq!(
        document
            .markup()
            .block_marks_in(0..7)
            .ids()
            .contains(&StyleId::Header(1))
            .then_some(1u8),
        Some(1),
        "the reparsed markup makes the line a header"
    );
    let header_height = editor.gathered(&store).expect("editor").content_height();
    assert!(
        header_height > plain_height,
        "the projection re-laid the header line out (H1 is taller)"
    );

    let idle = ReparseWork::capture(
        himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
        test_languages(),
    )
    .expect("tree advanced")
    .run_reparse();
    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
        .expect("document")
        .clone();
    let mut idle_batch = imba::effect::Batch::new();
    document.apply_reparse_outcome(
        idle,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut idle_batch.effects(),
    );
    assert!(
        idle_batch.is_empty(),
        "an idle reparse must not write anything"
    );
}

#[test]
fn enter_and_tab_assist_through_the_command_path() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    store.put(himark::env::Themes(test_theme()));
    store.put(himark::env::Parsers(test_languages()));
    let languages = test_languages();
    let mut document = Document::from_language(
        text::Text::from_string_exact("- one"),
        "markdown",
        &languages,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let editor = document.add_editor(
        600.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let ui = himark::test_document::test_ui();
    let perform = |document: &mut Document, store: &mut Store, command: EditorCommand| {
        document.perform(
            store,
            &ui,
            editor,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };

    let reparse = |document: &mut Document| {
        let outcome = ReparseWork::capture(document, test_languages())
            .expect("document has a parse")
            .run_reparse();
        document.apply_reparse_outcome(
            outcome,
            &imba::store::Store::new(),
            himark::test_document::test_ui(),
            &test_fonts(),
            &test_theme(),
            &mut imba::effect::Batch::new().effects(),
        );
    };
    let text_now = |document: &Document| {
        let mut view = document.text().view();
        let count = view.byte_count();
        view.byte_string(0, count)
    };

    document.set_caret(editor, 5);
    perform(
        &mut document,
        &mut store,
        EditorCommand::Enter { soft: false },
    );
    assert_eq!(text_now(&document), "- one\n- ");
    assert_eq!(document.caret_byte(editor), 8);

    reparse(&mut document);
    perform(&mut document, &mut store, EditorCommand::Indent);
    assert_eq!(text_now(&document), "- one\n  - ");
    assert_eq!(document.caret_byte(editor), 10);

    reparse(&mut document);
    perform(&mut document, &mut store, EditorCommand::Outdent);
    assert_eq!(text_now(&document), "- one\n- ");
    assert_eq!(document.caret_byte(editor), 8);
}

#[test]
fn multi_caret_enter_continues_every_item() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    store.put(himark::env::Themes(test_theme()));
    store.put(himark::env::Parsers(test_languages()));
    let languages = test_languages();
    let mut document = Document::from_language(
        text::Text::from_string_exact("- a\n- b"),
        "markdown",
        &languages,
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let editor = document.add_editor(
        600.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_carets(
        editor,
        himark::MultiCaret::normalized(vec![himark::Caret::at(3), himark::Caret::at(7)], 0),
    );
    let ui = himark::test_document::test_ui();
    document.perform(
        &mut store,
        &ui,
        editor,
        EditorCommand::Enter { soft: false },
        &mut imba::effect::Batch::new().effects(),
    );
    let mut view = document.text().view();
    let count = view.byte_count();
    assert_eq!(view.byte_string(0, count), "- a\n- \n- b\n- ");
}

#[test]
fn sections_emit_outline_items_at_parse() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let source = "# One\ntext\n## Two\nmore\n# Three\ntail\n";
    let languages = test_languages();
    let mut document = Document::from_language(
        text::Text::from_string_exact(source),
        "markdown",
        &languages,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let outcome = himark::ReparseWork::capture(&document, languages)
        .expect("markdown reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let items = document.outline_items();
    let shape: Vec<(u32, String)> = items
        .iter()
        .map(|(_, _, range, item)| (range.start, item.title.clone()))
        .collect();
    assert_eq!(
        shape,
        vec![
            (0, "One".to_owned()),
            (source.find("## Two").unwrap() as u32, "Two".to_owned()),
            (source.find("# Three").unwrap() as u32, "Three".to_owned()),
        ],
        "sections in order, titled by the heading inline"
    );

    let one = &items[0].2;
    let two = &items[1].2;
    let three = &items[2].2;
    assert!(
        one.start <= two.start && two.end <= one.end,
        "Two inside One"
    );
    assert!(three.start >= one.end, "Three is One's sibling");
    assert!(document.has_outline(), "the drawer gate sees the channel");
}

#[test]
fn document_from_markdown_populates_the_full_outline() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let titles = |src: &str| -> Vec<String> {
        let document = document_from_markdown(src, store, ui, &test_fonts(), &test_theme());
        document
            .outline_items()
            .into_iter()
            .map(|(_, _, _, item)| item.title)
            .collect()
    };
    assert_eq!(
        titles("# One\n\n## Two\n\n### Three\n\ntext\n"),
        vec!["One", "Two", "Three"],
        "every header level, no fences — not just H1"
    );
    assert_eq!(
        titles("# One\n\n## Two\n\n```rust\nfn x() {}\n```\n\n## Three\n"),
        vec!["One", "Two", "Three"],
        "headers with a fence present — not just the fence"
    );

    let languages = test_languages();
    let mut document = document_from_markdown(
        "# One\n\n## Two\n\n### Three\n",
        store,
        ui,
        &test_fonts(),
        &test_theme(),
    );
    let outcome = himark::ReparseWork::capture(&document, languages)
        .expect("reparse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let after: Vec<String> = document
        .outline_items()
        .into_iter()
        .map(|(_, _, _, item)| item.title)
        .collect();
    assert_eq!(after, vec!["One", "Two", "Three"], "the reparse keeps them");
}
