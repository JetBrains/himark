#![allow(unused_imports)]
use super::*;

#[test]
fn typing_and_backspace_keep_utf8_text_and_layout_in_sync() {
    let mut pane = TestPane::new(plain_document(""), 420.0);

    pane.perform(EditorCommand::InsertText {
        text: "aβc".to_owned(),
    });
    pane.perform(EditorCommand::Move {
        motion: editor::editor_view::Motion::Left,
        select: false,
    });
    pane.perform(EditorCommand::Backspace);

    assert_eq!(text_string(&pane), "ac");
    assert_eq!(pane.gathered().caret_byte(), 1);
    assert_eq!(pane.document().text().byte_count(), 2);
    assert!(!pane.gathered().document_layout().is_empty());
    assert!(pane.gathered().document_layout().height() > 0.0);
}

#[test]
fn view_refresh_matches_committed_store() {
    let mut pane = TestPane::new(plain_document("hello"), 420.0);

    pane.perform(EditorCommand::InsertText {
        text: "well, ".to_owned(),
    });

    let gathered = EditorIdView::new(test_docs(), pane.view.document(), pane.view.editor())
        .gathered(&pane.store)
        .expect("committed editor");
    assert_eq!(text_string(&pane), "well, hello");
    assert_eq!(gathered.caret_byte(), pane.gathered().caret_byte());
    assert_eq!(
        gathered.document_layout().height(),
        pane.gathered().document_layout().height()
    );
}

#[test]
fn editors_sharing_a_document_see_each_others_edits() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let mut document = plain_document("shared alpha beta gamma delta epsilon");
    let left_editor = document.add_editor(
        420.0,
        None,
        ::editor::document::EditorBuild::Complete,
        &[],
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
        &mut imba::effect::Batch::new().effects(),
    );
    let right_editor = document.add_editor(
        200.0,
        None,
        ::editor::document::EditorBuild::Complete,
        &[],
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_caret(right_editor, "shared".len() as u32);
    let document_id = documents::OpenDocuments::register(
        &mut store,
        test_docs(),
        document,
        None,
        "test".to_owned(),
        0,
    );

    let mut left_view = EditorIdView::new(test_docs(), document_id, left_editor);
    {
        let mut batch = imba::effect::Batch::new();
        left_view.perform(
            &mut store,
            ::editor::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "typed ".to_owned(),
            },
            &mut batch.effects(),
        );
    };

    let right_view = EditorIdView::new(test_docs(), document_id, right_editor)
        .gathered(&store)
        .expect("right editor");
    let mut text = right_view.document.text().view();
    assert_eq!(
        text.byte_string(0, text.byte_count()),
        "typed shared alpha beta gamma delta epsilon"
    );

    let fresh = EditorView::complete(
        right_view.document.clone(),
        200.0,
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );
    assert!(right_view.document_layout().height() > 0.0);
    assert_eq!(
        right_view.document_layout().height(),
        fresh.content_height()
    );

    assert_eq!(
        right_view.caret_byte(),
        ("typed ".len() + "shared".len()) as u32
    );
}

#[test]
fn typing_in_code_block_with_emoji_keeps_layout_ranges_valid() {
    let source = "```rust\nlet x = \"😀😀😀😀😀\"; abcdefghijklmnopqrstuvwxyz\n```\n\noutside";
    let mut pane = TestPane::new(fenced_code_document(source), 220.0);
    pane.set_caret(
        source
            .find("abcdefghijklmnopqrstuvwxyz")
            .expect("sample marker") as u32,
    );

    pane.perform(EditorCommand::InsertText {
        text: "😀 inserted ".to_owned(),
    });

    assert_layout_ranges_are_utf8(&pane);
}

#[test]
fn repeated_typing_at_softline_start_repairs_as_fresh_layout() {
    let source = "- [x] alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu\n";
    let width = 180.0;
    let mut pane = TestPane::new(list_document(source), width);
    let second_line_start = layout_line_starts(&pane)
        .get(1)
        .copied()
        .expect("sample should soft-wrap");
    pane.set_caret(second_line_start);

    for ch in "asdfjaksdfdkfjdd".chars() {
        pane.perform(EditorCommand::InsertText {
            text: ch.to_string(),
        });
    }

    assert_layout_ranges_are_utf8(&pane);
    let fresh = TestPane::new(list_document(&text_string(&pane)), width);
    assert_eq!(layout_ranges(&pane), layout_ranges(&fresh));
}

#[test]
fn inlay_command_updates_interval_view_and_repairs_layout() {
    let mut pane = TestPane::new(plain_document("abc"), 420.0);
    let base_height = pane.gathered().content_height();
    let inlay = Inlay::new(
        InlayMode::Under,
        TestInlay {
            width: 40.0,
            height: 20.0,
        },
    );

    pane.replace_inlay(pane.inlay_key(7), 1..2, inlay);
    let inlay_height = pane.gathered().content_height();

    assert!(inlay_height >= base_height + 20.0);

    pane.perform(EditorCommand::Inlay {
        key: pane.inlay_key(7),
        command: imba::dyn_view::DynCommand::new(TestInlayCommand::Grow),
    });

    assert!(pane.gathered().content_height() >= inlay_height + 10.0);
}

#[test]
fn inline_inlay_height_change_repairs_layout() {
    let source = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
    let width = 260.0;
    let mut pane = TestPane::new(plain_document(source), width);
    let inlay = Inlay::new(
        InlayMode::Left,
        TestInlay {
            width: 40.0,
            height: 80.0,
        },
    );

    pane.replace_inlay(pane.inlay_key(9), 6..10, inlay);
    let before = pane.gathered().content_height();

    pane.perform(EditorCommand::Inlay {
        key: pane.inlay_key(9),
        command: imba::dyn_view::DynCommand::new(TestInlayCommand::Grow),
    });

    assert!(
        pane.gathered().content_height() > before,
        "growing an anchored inline inlay should repair the affected line height"
    );
}

#[test]
fn under_inlay_height_change_repairs_end_anchor_line() {
    let source = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
    let width = 180.0;
    let mut pane = TestPane::new(plain_document(source), width);
    assert!(
        layout_element_count(&pane) > 1,
        "sample must wrap so range start and under anchor are on different lines"
    );
    let inlay = Inlay::new(
        InlayMode::Under,
        TestInlay {
            width: 40.0,
            height: 80.0,
        },
    );

    pane.replace_inlay(pane.inlay_key(13), 0..source.len() as u32, inlay);
    let before = pane.gathered().content_height();

    pane.perform(EditorCommand::Inlay {
        key: pane.inlay_key(13),
        command: imba::dyn_view::DynCommand::new(TestInlayCommand::Grow),
    });

    assert!(
        pane.gathered().content_height() > before,
        "growing an under inlay should repair the line containing the interval end"
    );
}

#[test]
fn inline_inlay_width_change_rewraps_layout() {
    let source = "alpha beta gamma delta epsilon";
    let width = 360.0;
    let mut pane = TestPane::new(plain_document(source), width);
    let inlay = Inlay::new(
        InlayMode::Left,
        TestInlay {
            width: 8.0,
            height: 20.0,
        },
    );

    pane.replace_inlay(pane.inlay_key(11), 6..10, inlay);
    let before = layout_element_count(&pane);

    pane.perform(EditorCommand::Inlay {
        key: pane.inlay_key(11),
        command: imba::dyn_view::DynCommand::new(TestInlayCommand::GrowWide),
    });

    assert!(
        layout_element_count(&pane) > before,
        "growing an inline inlay width should rewrap the affected paragraph"
    );
}

#[test]
fn resizing_an_empty_document_leaves_nothing_pending() {
    let mut pane = TestPane::new(plain_document(""), 420.0);
    let effects = pane.resize(300.0, 0);

    assert!(
        pane.gathered().document_layout().repair_pending().is_none(),
        "an empty layout must never be left pending"
    );

    pane.finish(effects);
    assert!(pane.gathered().document_layout().repair_pending().is_none());
}

#[test]
fn deleting_all_text_leaves_nothing_pending() {
    let mut pane = TestPane::new(plain_document("abc"), 420.0);
    pane.set_caret(3);
    for _ in 0..3 {
        pane.perform(EditorCommand::Backspace);
    }
    assert_eq!(text_string(&pane), "");
    assert!(
        pane.gathered().document_layout().repair_pending().is_none(),
        "an emptied layout must never be left pending"
    );
}

#[test]
fn typing_deep_in_a_giant_paragraph_repairs_to_completion() {
    let ui = ::editor::test_document::test_ui();
    let source = "word ".repeat(12_000);
    let mut pane = TestPane::new(plain_document(&source), 200.0);
    pane.set_caret((source.len() / 2) as u32);

    pane.perform(EditorCommand::InsertText {
        text: "deep ".to_owned(),
    });

    let view = pane.gathered();
    assert!(
        view.document_layout().repair_pending().is_none(),
        "the deferred repair heals everything the sync pass left"
    );
    let fresh = EditorView::complete(
        view.document.clone(),
        200.0,
        &pane.store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );
    assert_eq!(view.document_layout().height(), fresh.content_height());
}

#[test]
fn repairs_for_a_repointed_entity_discard_themselves() {
    let source_a = "alpha ".repeat(12_000);
    let mut pane = TestPane::new(plain_document(&source_a), 200.0);

    let stale = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "x".to_owned(),
            },
            &mut batch.effects(),
        );
        batch
    };

    let mut document_b = plain_document("# tiny\n");
    let document_b_id = documents::OpenDocuments::register(
        &mut pane.store,
        test_docs(),
        document_b.clone(),
        None,
        "b".to_owned(),
        0,
    );
    let mut fresh = imba::effect::Batch::new();

    documents::lifecycle::close_editor(
        &mut pane.store,
        pane.view.documents(),
        pane.view.document(),
        pane.view.editor(),
    );
    let new_editor = documents::lifecycle::mount_editor(
        &pane.store,
        ::editor::test_document::test_ui(),
        &mut document_b,
        200.0,
        None,
        &mut fresh.effects(),
    );
    pane.view = EditorIdView::new(test_docs(), document_b_id, new_editor);
    documents::OpenDocuments::put_document(&mut pane.store, test_docs(), document_b_id, document_b);
    assert!(
        himark::test_support::surviving_launches(fresh).is_empty(),
        "the tiny document opens fully repaired"
    );

    for effect in himark::test_support::surviving_launches(stale) {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        let mut discarded = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            command,
            &mut discarded.effects(),
        );
    }
    let view = pane.gathered();
    assert_eq!(
        view.document.text().byte_count(),
        "# tiny\n".len(),
        "the entity projects document B"
    );
    assert_eq!(
        view.find_misaligned_boundary(),
        None,
        "the layout tiles document B exactly"
    );
}

#[test]
fn stale_repairs_discard_and_the_fresh_one_converges() {
    let ui = ::editor::test_document::test_ui();
    let source = "word ".repeat(4_000);
    let mut pane = TestPane::new(plain_document(&source), 200.0);

    let first = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "first ".to_owned(),
            },
            &mut batch.effects(),
        );
        batch
    };
    assert_eq!(first.len(), 1);

    let second = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "second ".to_owned(),
            },
            &mut batch.effects(),
        );
        batch
    };

    for effect in himark::test_support::surviving_launches(first) {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        let mut discarded = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            command,
            &mut discarded.effects(),
        );
    }
    assert!(
        pane.gathered().document_layout().repair_pending().is_some(),
        "a stale repair discards itself; the tail stays pending"
    );

    let mut pending = himark::test_support::surviving_launches(second);
    while let Some(effect) = pending.pop() {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            command,
            &mut batch.effects(),
        );
        pending.extend(himark::test_support::surviving_launches(batch));
    }
    let view = pane.view.gathered(&pane.store).expect("editor");
    assert!(view.document_layout().repair_pending().is_none());
    let fresh = EditorView::complete(
        view.document.clone(),
        200.0,
        &pane.store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );
    assert!(view.document_layout().height() > 0.0);
    assert_eq!(view.document_layout().height(), fresh.content_height());
}

#[test]
fn focus_moves_between_text_and_inlays() {
    use imba::event::EventResult;
    use skia_safe::Point;

    let mut pane = TestPane::new(plain_document("abc"), 420.0);
    pane.replace_inlay(
        pane.inlay_key(7),
        1..2,
        Inlay::new(
            InlayMode::Under,
            TestInlay {
                width: 40.0,
                height: 20.0,
            },
        ),
    );
    assert_eq!(pane.gathered().focus(), EditorFocus::Text);

    pane.perform(EditorCommand::Inlay {
        key: pane.inlay_key(7),
        command: imba::dyn_view::DynCommand::new(TestInlayCommand::Grow),
    });
    assert_eq!(
        pane.gathered().focus(),
        EditorFocus::Inlay(pane.inlay_key(7))
    );
    let stored = pane.view;
    assert_eq!(
        documents::OpenDocuments::document_ref(&pane.store, stored.documents(), stored.document())
            .expect("document")
            .focus(stored.editor()),
        EditorFocus::Inlay(pane.inlay_key(7))
    );

    {
        let ui = ::editor::test_document::test_ui();
        assert!(matches!(
            imba::View::focus_data(&pane.view, &pane.store, &ui).text("x"),
            EventResult::Ignored
        ));
    }

    pane.perform(EditorCommand::Click {
        kind: editor::editor_view::ClickKind::Set,
        point: Point::new(5.0, 5.0),
    });
    assert_eq!(pane.gathered().focus(), EditorFocus::Text);

    {
        let ui = ::editor::test_document::test_ui();
        assert!(matches!(
            imba::View::focus_data(&pane.view, &pane.store, &ui).text("x"),
            EventResult::Command(EditorCommand::InsertText { .. })
        ));
    }
}

#[test]
fn inlay_modes_anchor_to_interval_offsets() {
    let first = 0..10;
    let second = 10..20;
    let interval = 0..20;

    assert!(inlay_anchors_line(InlayMode::Left, &interval, &first));
    assert!(!inlay_anchors_line(InlayMode::Left, &interval, &second));
    assert!(!inlay_anchors_line(InlayMode::Right, &interval, &first));
    assert!(inlay_anchors_line(InlayMode::Right, &interval, &second));
    assert!(inlay_anchors_line(
        InlayMode::Instead(editor::markup::InsteadKind::FullLine),
        &interval,
        &first
    ));
    assert!(!inlay_anchors_line(
        InlayMode::Instead(editor::markup::InsteadKind::FullLine),
        &interval,
        &second
    ));

    assert!(inlay_anchors_line(InlayMode::Above, &interval, &first));
    assert!(!inlay_anchors_line(InlayMode::Above, &interval, &second));
    assert!(!inlay_anchors_line(InlayMode::Under, &interval, &first));
    assert!(inlay_anchors_line(InlayMode::Under, &interval, &second));
}

#[test]
fn an_inlay_paints_focused_only_while_it_holds_the_editors_focus() {
    use imba::constraints::Constraints;
    use imba::event::{Event, EventResult};
    use imba::Widget;

    let mut pane = TestPane::new(plain_document("abc def ghi"), 420.0);
    let key = pane.inlay_key(7);
    pane.replace_inlay(key, 1..2, Inlay::new(InlayMode::Under, FocusProbe));

    pane.perform(EditorCommand::Inlay {
        key,
        command: imba::dyn_view::DynCommand::new(ProbeCommand::Poke),
    });
    let focus = |pane: &TestPane| {
        let entity = pane.view;
        pane.document().focus(entity.editor())
    };
    assert_eq!(focus(&pane), EditorFocus::Inlay(key));

    let paint = |pane: &TestPane| -> Vec<EditorCommand> {
        let arena = imba::arena::Arena::default();
        let ui = ::editor::test_document::test_ui();
        let view = pane.gathered();
        let widget = imba::layout::Layout::layout(
            View::display(&view, &arena, &pane.store, &ui),
            &arena,
            Constraints {
                min: skia_safe::Size::default(),
                max: skia_safe::Size::new(420.0, f32::MAX),
            },
        );
        let mut surface = skia_safe::surfaces::raster_n32_premul((420, 600)).expect("a surface");
        let viewport = skia_safe::Rect::from_wh(420.0, 600.0);
        let widget = imba::Thunk::realize(widget, &arena, viewport);
        match widget.handle_event(
            &arena,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
            viewport,
        ) {
            EventResult::Command(command) => vec![command],
            EventResult::Commands(commands) => commands,
            _ => Vec::new(),
        }
    };
    let probe_reports = |commands: &[EditorCommand]| {
        commands
            .iter()
            .filter(|command| matches!(command, EditorCommand::Inlay { key: at, .. } if *at == key))
            .count()
    };

    assert_eq!(probe_reports(&paint(&pane)), 0, "a focused paint agrees");

    pane.perform(EditorCommand::Click {
        kind: editor::editor_view::ClickKind::Set,
        point: skia_safe::Point::new(5.0, 5.0),
    });
    assert_eq!(focus(&pane), EditorFocus::Text);

    assert_eq!(
        probe_reports(&paint(&pane)),
        1,
        "the unfocused paint is seen"
    );
}

#[test]
fn resize_repairs_the_viewport_synchronously_and_the_rest_as_an_effect() {
    let ui = ::editor::test_document::test_ui();
    let source = "word ".repeat(4_000);
    let mut pane = TestPane::new(plain_document(&source), 200.0);

    let reference = EditorView::complete(
        plain_document(&source),
        420.0,
        &pane.store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );

    let anchor = (source.len() / 2) as u32;
    let effects = pane.resize(420.0, anchor);
    assert_eq!(
        effects.len(),
        1,
        "the whole document cannot re-lay out within the budget"
    );

    pane.finish(effects);

    assert_eq!(pane.gathered().layout_width(), 420.0);
    assert!(
        (pane.gathered().content_height() - reference.content_height()).abs() < 0.5,
        "the repaired layout converges to a from-scratch layout at 420px"
    );
}

#[test]
fn a_repair_from_before_a_resize_discards_itself() {
    let source = "word ".repeat(4_000);
    let mut pane = TestPane::new(plain_document(&source), 200.0);

    let edit_effects = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::InsertText {
                text: "first ".to_owned(),
            },
            &mut batch.effects(),
        );
        batch
    };
    assert_eq!(edit_effects.len(), 1);

    let _ = pane.resize(420.0, 0);

    let before = pane.gathered().document_layout().height();
    for effect in himark::test_support::surviving_launches(edit_effects) {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        let mut discarded = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            command,
            &mut discarded.effects(),
        );
    }
    assert_eq!(
        pane.gathered().document_layout().height(),
        before,
        "a repair captured at the old width must discard itself"
    );
}

#[test]
fn opening_a_document_lays_out_the_viewport_and_repairs_the_rest() {
    let ui = ::editor::test_document::test_ui();
    let source = "word ".repeat(4_000);
    let mut document = plain_document(&source);
    let mut store = Store::new();
    let document_id = documents::OpenDocuments::register(
        &mut store,
        test_docs(),
        document.clone(),
        None,
        "test".to_owned(),
        0,
    );
    let mut open_batch = imba::effect::Batch::new();
    let editor_id = documents::lifecycle::mount_editor(
        &store,
        &ui,
        &mut document,
        200.0,
        None,
        &mut open_batch.effects(),
    );
    documents::OpenDocuments::put_document(&mut store, test_docs(), document_id, document.clone());
    let effects = himark::test_support::surviving_launches(open_batch);

    let reference = EditorView::complete(
        document.clone(),
        200.0,
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );
    let opened = EditorIdView::new(test_docs(), document_id, editor_id)
        .gathered(&store)
        .expect("entity");
    assert!(opened.content_height() < reference.content_height());
    assert_eq!(opened.find_misaligned_boundary(), None);
    assert_eq!(effects.len(), 1, "the tail repairs in background");

    let mut node = EditorIdView::new(test_docs(), document_id, editor_id);
    let mut chain = Vec::new();
    for effect in effects {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        chain.extend({
            let mut batch = imba::effect::Batch::new();
            node.perform(
                &mut store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            himark::test_support::surviving_launches(batch)
        });
    }
    while let Some(effect) = chain.pop() {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        chain.extend({
            let mut batch = imba::effect::Batch::new();
            node.perform(
                &mut store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            himark::test_support::surviving_launches(batch)
        });
    }

    let opened = node.gathered(&store).expect("entity");
    assert!(
        (opened.content_height() - reference.content_height()).abs() < 0.5,
        "the background repair converges to the complete layout"
    );
}

#[test]
fn ime_composition_reaches_the_focused_editor() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let set = app.with_ime_client(app.sole_window(), |client| {
        client.set_marked_text("\u{306B}\u{307B}", (2, 0), None);
    });
    assert!(set.is_some(), "the startup editor answers the IME event");
    let (marked, range) = app
        .with_ime_client(app.sole_window(), |client| {
            (client.has_marked_text(), client.marked_range())
        })
        .expect("still focused");
    assert!(marked, "the composition is live");
    assert_eq!(range, Some((0, 2)), "two UTF-16 units marked");
    let rect = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(0, 2))
        .expect("still focused");
    assert!(rect.is_some(), "the candidate window has a caret rect");

    let _ = app.with_ime_client(app.sole_window(), |client| client.unmark_text());
    let marked = app
        .with_ime_client(app.sole_window(), |client| client.has_marked_text())
        .expect("still focused");
    assert!(!marked, "unmark commits the composition");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("\u{306B}\u{307B}"),
        "the committed text is in the document"
    );
}

#[test]
fn ime_hit_test_answers_only_over_the_focused_text() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("hello", None);
    });
    app.draw_window(app.sole_window(), surface.canvas());

    let (x, y, _, height) = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(0, 0))
        .expect("focused")
        .expect("a caret rect");
    let inside = app
        .with_ime_client(app.sole_window(), |client| {
            client.char_index_at(x + 1.0, y + height * 0.5)
        })
        .expect("focused");
    assert!(inside.is_some(), "the caret's own point hits text");

    let below = app
        .with_ime_client(app.sole_window(), |client| {
            client.char_index_at(x + 1.0, y + 4000.0)
        })
        .expect("focused");
    assert!(below.is_none(), "past the painted band is not text");

    let left = app
        .with_ime_client(app.sole_window(), |client| {
            client.char_index_at(-64.0, y + height * 0.5)
        })
        .expect("focused");
    assert!(left.is_none(), "the gutter is not text");
}

#[test]
fn ime_selection_sets_reads_back_and_answers_rects() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("hello brave world", None);
    });
    app.draw_window(app.sole_window(), surface.canvas());
    let length = app
        .with_ime_client(app.sole_window(), |client| client.document_length())
        .expect("focused");
    assert_eq!(length, 17, "the document's UTF-16 length");

    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.set_selected_range(6, 5);
    });
    let selected = app
        .with_ime_client(app.sole_window(), |client| client.selected_range())
        .expect("focused");
    assert_eq!(selected, Some((6, 11)), "the selection round-trips");
    assert_eq!(
        app.with_ime_client(app.sole_window(), |client| client.substring_utf16(6, 5))
            .expect("focused")
            .as_deref(),
        Some("brave"),
        "the selected text is what was asked for"
    );

    let rects = app
        .with_ime_client(app.sole_window(), |client| client.selection_rects(6, 5))
        .expect("focused");
    assert_eq!(
        rects.len(),
        1,
        "a one-line selection is one rect: {rects:?}"
    );
    let (x, y, width, height) = rects[0];
    assert!(width > 0.0 && height > 0.0, "a real rect: {rects:?}");
    let caret = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(6, 0))
        .expect("focused")
        .expect("the caret rect");
    assert!(
        (x - caret.0).abs() < 2.0,
        "the rect starts at the selection's caret x: {x} vs {}",
        caret.0
    );
    assert!(
        (y - caret.1).abs() < height,
        "and on its line: {y} vs {}",
        caret.1
    );

    assert!(app
        .with_ime_client(app.sole_window(), |client| client.selection_rects(6, 0))
        .expect("focused")
        .is_empty());
}

#[test]
fn ime_popup_positions_in_window_coordinates_and_hides() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let mut text = String::new();
    for line in 0..80 {
        text.push_str(&format!("line {line} with some words on it\n"));
    }
    assert!(himark::test_driver::type_text(&mut app, &text));
    app.draw_window(app.sole_window(), surface.canvas());

    let _ = himark::test_driver::scroll_at(&mut app, 400.0, 300.0, -100_000.0);
    app.draw_window(app.sole_window(), surface.canvas());
    let (click_x, click_y) = (140.0, 160.0);
    let _ = himark::test_driver::click(&mut app, click_x, click_y, 800.0, 600.0);
    app.draw_window(app.sole_window(), surface.canvas());

    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.set_marked_text("\u{306B}", (1, 0), None);
    });
    let rect = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(0, 1))
        .expect("the clicked editor answers the IME")
        .expect("the composition has a caret rect");
    let (x, y, _w, h) = rect;
    assert!(h > 4.0 && h < 120.0, "a line-sized caret rect, got {h}");

    assert!(
        (x - click_x).abs() < 40.0,
        "the popup x must sit at the click (window coords): rect x {x}, clicked {click_x}"
    );

    assert!(
        y <= click_y && click_y < y + h * 2.0,
        "the click must fall inside the caret line (window coords): rect y {y} h {h}, clicked {click_y}"
    );

    let delta = 120.0;
    assert!(himark::test_driver::scroll_at(
        &mut app, 400.0, 300.0, delta
    ));
    app.draw_window(app.sole_window(), surface.canvas());
    let scrolled = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(0, 1))
        .expect("still focused")
        .expect("still composing");
    assert!(
        ((y - scrolled.1) - delta).abs() < 2.0,
        "the rect follows the scroll: was y {y}, now {}, scrolled by {delta}",
        scrolled.1
    );

    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("\u{306B}", None)
    });
    let marked = app
        .with_ime_client(app.sole_window(), |client| client.has_marked_text())
        .expect("still focused");
    assert!(
        !marked,
        "the commit ended the composition — the popup hides"
    );
}

#[test]
fn accent_popup_replaces_the_held_character() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    assert!(himark::test_driver::type_text(&mut app, "so"));
    assert!(himark::test_driver::type_text(&mut app, "u"));
    assert_eq!(app.focused_document_text().as_deref(), Some("sou"));

    let replaced = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("\u{fc}", Some((2, 1)));
    });
    assert!(replaced.is_some(), "the focused editor answers");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("so\u{fc}"),
        "the accent replaces the base character"
    );

    let _ = app.with_ime_client(app.sole_window(), |client| client.insert_text("!", None));
    assert_eq!(app.focused_document_text().as_deref(), Some("so\u{fc}!"));
}

#[test]
fn typing_into_the_empty_startup_document_inserts() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(
        app.with_ime_client(app.sole_window(), |_| ()).is_some(),
        "the startup editor should have text focus"
    );
    let handled = himark::test_driver::type_text(&mut app, "a");
    assert!(handled, "typing into the empty document must be handled");
    assert_eq!(app.focused_document_text().as_deref(), Some("a"));
}

#[test]
fn retheme_reshapes_the_viewport_synchronously_and_the_tail_in_repairs() {
    let ui = ::editor::test_document::test_ui();
    use ::editor::theme::Theme;

    let source: String = (0..2500)
        .map(|index| format!("paragraph {index} with a handful of words in it\n"))
        .collect();
    let mut pane = TestPane::new(plain_document(&source), 420.0);
    let fonts = ::editor::test_document::test_fonts_collection();
    let light = Theme::light();

    let entity = pane.view;
    let total_dark = pane
        .document()
        .document_layout(entity.editor())
        .unwrap()
        .height();
    let top = total_dark * 0.5;
    let bottom = top + 600.0;
    pane.perform(EditorCommand::Viewport {
        width: 420.0,
        top,
        bottom,
        anchor: 0,
    });

    let band = |layout: &::editor::document_layout::DocumentLayout, visible: &(u32, u32)| {
        layout.height_before(visible.1) - layout.height_before(visible.0)
    };
    let (visible, dark_band) = {
        let document = pane.document();
        let layout = document.document_layout(entity.editor()).unwrap();
        let visible = (layout.byte_at_y(top), layout.byte_at_y(bottom));
        (visible, band(layout, &visible))
    };
    let visible_start = visible.0;

    ::editor::env::Themes::set(&mut pane.store, light.clone());
    let effects = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::Retheme {
                top,
                bottom,
                anchor: visible_start,
            },
            &mut batch.effects(),
        );
        batch
    };
    let document =
        documents::OpenDocuments::document(&pane.store, entity.documents(), entity.document())
            .expect("document");
    assert!(!effects.is_empty(), "the tail defers as repair effects");
    assert!(
        document
            .document_layout(entity.editor())
            .unwrap()
            .repair_pending()
            .is_some(),
        "the reshape is BOUNDED: work past the viewport stays pending"
    );

    let fresh_light =
        EditorView::complete(document.clone(), 420.0, &pane.store, ui, &fonts, &light);
    let switched_band = band(document.document_layout(entity.editor()).unwrap(), &visible);
    let fresh_band = band(fresh_light.document_layout(), &visible);
    assert!(
        (switched_band - fresh_band).abs() < 0.5,
        "the visible band is light-shaped immediately: {switched_band} vs fresh {fresh_band}"
    );

    assert!(
        (switched_band - dark_band).abs() < 0.5,
        "identical geometry across themes ({switched_band} vs {dark_band})"
    );

    let mut pending = himark::test_support::surviving_launches(effects);
    while let Some(effect) = pending.pop() {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(light.clone()),
        );
        pending.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            himark::test_support::surviving_launches(batch)
        });
    }
    let converged = pane.document();
    let layout = converged.document_layout(entity.editor()).unwrap();
    assert!(layout.repair_pending().is_none(), "the tail repaired");
    assert!(
        (layout.height() - fresh_light.document_layout().height()).abs() < 0.5,
        "the repaired document IS the fresh light layout: {} vs {}",
        layout.height(),
        fresh_light.document_layout().height()
    );
}

#[test]
fn a_stale_theme_repair_landing_discards_itself() {
    let store = &imba::store::Store::new();
    let ui = ::editor::test_document::test_ui();
    use ::editor::theme::Theme;
    let source: String = (0..2000)
        .map(|index| {
            format!(
                "line {index} with a few more words
"
            )
        })
        .collect();
    let mut pane = TestPane::new(plain_document(&source), 420.0);
    let fonts = ::editor::test_document::test_fonts_collection();
    let light = Theme::light();
    let entity = pane.view;

    pane.perform(EditorCommand::Viewport {
        width: 420.0,
        top: 0.0,
        bottom: 600.0,
        anchor: 0,
    });
    ::editor::env::Themes::set(&mut pane.store, light.clone());
    let stale_round = {
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            EditorCommand::Retheme {
                top: 0.0,
                bottom: 600.0,
                anchor: 0,
            },
            &mut batch.effects(),
        );
        batch
    };
    assert!(!stale_round.is_empty(), "the tail defers as repair effects");
    let pending = |pane: &TestPane| {
        documents::OpenDocuments::document_ref(&pane.store, entity.documents(), entity.document())
            .unwrap()
            .document_layout(entity.editor())
            .unwrap()
            .repair_pending()
    };
    assert!(pending(&pane).is_some(), "the tail is pending");

    let mut requeued = Vec::new();
    for effect in himark::test_support::surviving_launches(stale_round) {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(Theme::embedded()),
        );
        requeued.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            himark::test_support::surviving_launches(batch)
        });
    }
    assert!(
        pending(&pane).is_some(),
        "a stale-theme landing must not install: the damage stays pending"
    );

    let mut pending_effects = requeued;
    while let Some(effect) = pending_effects.pop() {
        let command = himark::test_support::handle_effect(
            effect,
            &himark::test_support::test_workshop(light.clone()),
        );
        pending_effects.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            himark::test_support::surviving_launches(batch)
        });
    }
    let converged =
        documents::OpenDocuments::document(&pane.store, entity.documents(), entity.document())
            .expect("document");
    let layout = converged.document_layout(entity.editor()).unwrap();
    assert!(layout.repair_pending().is_none(), "the tail repaired");
    assert_eq!(layout.shaped_theme(), "light");
    let fresh = ::editor::editor_view::EditorView::complete(
        converged.clone(),
        420.0,
        &store,
        ui,
        &fonts,
        &light,
    );
    assert!(
        (layout.height() - fresh.document_layout().height()).abs() < 0.5,
        "the repaired document IS the fresh light layout: {} vs {}",
        layout.height(),
        fresh.document_layout().height()
    );
}

#[test]
fn theme_toggle_swaps_the_theme_and_reshapes_the_view() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(
        &mut app,
        "# A title\n\nSome body text to shape, long enough to wrap around.",
    ));
    app.draw_window(app.sole_window(), surface.canvas());
    let height_dark = app.focused_pane_content_height();
    assert!(height_dark > 0.0);

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));
    assert_eq!(
        ::editor::env::Themes::of(app.store()).name(),
        "light",
        "the store's theme swapped"
    );
    app.draw_window(app.sole_window(), surface.canvas());
    let height_light = app.focused_pane_content_height();

    assert!(
        (height_light - height_dark).abs() < 0.5,
        "identical geometry across themes: {height_light} vs {height_dark}"
    );

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));
    assert_eq!(::editor::env::Themes::of(app.store()).name(), "dark");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(
        (app.focused_pane_content_height() - height_dark).abs() < 0.5,
        "toggling back restores the dark geometry"
    );
}

#[test]
fn caret_commands_glide_the_pane_to_the_caret() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::test_driver;
    use imba::anim::AnimationClock;
    use imba::event::{Key, Modifiers};

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );
    let body: String = (0..300).map(|index| format!("line {index}\n")).collect();
    assert!(app.add_document(
        app.sole_window(),
        plain_document(&body),
        "tall".to_owned(),
        true
    ));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(app.focused_pane_scroll_y(), 0.0);

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Modifiers::default()
        },
    ));

    let mut moved_ticks = 0;
    for tick in 0..200 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let busy = test_driver::animate(&mut app, AnimationClock::from_millis(tick as f64 * 16.0));
        app.draw_window(app.sole_window(), surface.canvas());
        if busy {
            moved_ticks += 1;
        } else if tick > 0 {
            break;
        }
    }

    let scroll_y = app.focused_pane_scroll_y();
    let content = app.focused_pane_content_height();
    assert!(
        scroll_y > content * 0.5,
        "the pane glided deep toward the caret (scroll {scroll_y} of {content})"
    );
    assert!(
        moved_ticks > 2,
        "the glide eased over several ticks ({moved_ticks})"
    );
}

#[test]
fn a_manual_scroll_cancels_the_reveal_in_flight() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::test_driver;
    use imba::anim::AnimationClock;
    use imba::event::{Key, Modifiers};

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );
    let body: String = (0..300).map(|index| format!("line {index}\n")).collect();
    assert!(app.add_document(
        app.sole_window(),
        plain_document(&body),
        "tall".to_owned(),
        true
    ));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Modifiers::default()
        },
    ));

    for tick in 0..3 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        test_driver::animate(&mut app, AnimationClock::from_millis(tick as f64 * 16.0));
        app.draw_window(app.sole_window(), surface.canvas());
    }
    assert!(app.focused_pane_scroll_y() > 0.0, "the glide started");

    assert!(test_driver::scroll(&mut app, -200.0));
    let after_wheel = app.focused_pane_scroll_y();
    for tick in 3..40 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        test_driver::animate(&mut app, AnimationClock::from_millis(tick as f64 * 16.0));
        app.draw_window(app.sole_window(), surface.canvas());
    }
    assert!(
        (app.focused_pane_scroll_y() - after_wheel).abs() < 1.0,
        "the reveal never fought the wheel (scroll {} vs {after_wheel})",
        app.focused_pane_scroll_y()
    );
}

#[test]
fn opening_with_a_target_lands_the_caret_revealed() {
    use documents::text_ext::LineCol;
    use himark::app::AppCommand;
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::app::OpenedDocument;
    use himark::app_ext::AppExt;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();

    let source: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let target = LineCol { line: 150, col: 5 }..LineCol { line: 150, col: 7 };
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "big.md".to_owned(),
            document: plain_document(&source),
            location: None,
            primary: true,
            target: Some(target),
            focus: false,
        },
    )));

    let expected = source
        .lines()
        .take(150)
        .map(|line| line.len() + 1)
        .sum::<usize>() as u32
        + 5;
    assert_eq!(
        app.focused_caret_byte(),
        Some(expected),
        "the line/col target resolved against the opened rope"
    );
    assert_eq!(
        app.focused_reveal_pending(),
        Some(true),
        "the scroll-to-caret is armed on mount"
    );

    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "plain.md".to_owned(),
            document: plain_document("hello"),
            location: None,
            primary: true,
            target: None,
            focus: false,
        },
    )));
    assert_eq!(app.focused_caret_byte(), Some(0));
    assert_eq!(app.focused_reveal_pending(), Some(false));
}

#[test]
fn clipboard_reaches_the_focused_editor() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());
    let size = skia_safe::Size::new(800.0, 600.0);

    app.dispatch(
        window,
        imba::event::Event::TextInput {
            text: "hello world",
        },
        size,
    );

    let copied = app
        .with_clipboard_client(window, |client| client.copy().map(|content| content.text))
        .expect("the focused editor answers");
    assert_eq!(copied, None, "no selection, nothing to copy");

    app.dispatch(
        window,
        imba::event::Event::KeyDown {
            key: imba::event::Key::Char('a'),
            mods: imba::event::Modifiers {
                command: true,
                ..Default::default()
            },
        },
        size,
    );
    let copied = app
        .with_clipboard_client(window, |client| client.copy().map(|content| content.text))
        .expect("still focused");
    assert_eq!(copied.as_deref(), Some("hello world"));
    assert_eq!(app.focused_document_text().as_deref(), Some("hello world"));

    let cut = app
        .with_clipboard_client(window, |client| client.cut().map(|content| content.text))
        .expect("still focused");
    assert_eq!(cut.as_deref(), Some("hello world"));
    assert_eq!(app.focused_document_text().as_deref(), Some(""));

    for _ in 0..2 {
        let pasted = app
            .with_clipboard_client(window, |client| {
                client.paste(&imba::clipboard::ClipboardContent {
                    text: "again ".to_owned(),
                })
            })
            .expect("still focused");
        assert!(pasted);
    }
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("again again "),
        "each paste inserted at the caret"
    );
}

#[test]
fn double_and_triple_click_select_word_and_line() {
    let store = &imba::store::Store::new();
    let ui = ::editor::test_document::test_ui();
    use editor::editor_view::ClickKind;
    use imba::{
        arena::Arena,
        constraints::Constraints,
        event::{Event, EventResult},
        Widget,
    };
    use skia_safe::{Point, Rect, Size};

    let text = "alpha  beta gamma\nsecond line\n";
    let mut pane = TestPane::new(plain_document(text), 420.0);
    let fonts = ::editor::test_document::test_fonts_collection();
    let theme = ::editor::theme::Theme::embedded();
    let point_at = |pane: &TestPane, byte: u32| -> Point {
        let document = pane.document();
        let (x, y, _, h) = document
            .caret_content_rect(pane.view.editor(), byte, &store, ui, &fonts, &theme)
            .expect("a caret rect for the byte");
        Point::new(x + 1.0, y + h / 2.0)
    };
    let primary = |pane: &TestPane| pane.document().carets(pane.view.editor()).primary();

    let in_beta = point_at(&pane, 9);
    pane.perform(EditorCommand::Click {
        kind: ClickKind::Word,
        point: in_beta,
    });
    assert_eq!(
        primary(&pane).selection(),
        7..11,
        "the word around the point"
    );

    pane.perform(EditorCommand::Click {
        kind: ClickKind::Line,
        point: in_beta,
    });
    assert_eq!(
        primary(&pane).selection(),
        0..18,
        "the hard line with its trailing newline"
    );

    let on_space = point_at(&pane, 6);
    pane.perform(EditorCommand::Click {
        kind: ClickKind::Word,
        point: on_space,
    });
    assert!(
        !primary(&pane).has_selection(),
        "whitespace double: caret only"
    );
    assert_eq!(primary(&pane).offset(), 6);

    let arena = Arena::default();
    let ui = ::editor::test_document::test_ui();
    let click = |count: u8, alt: bool| -> Option<ClickKind> {
        let mut event = Event::MouseDown {
            point: in_beta,
            button: imba::event::MouseButton::Left,
            mods: imba::event::Modifiers {
                alt,
                ..Default::default()
            },
            count,
        };
        match imba::Thunk::realize(
            imba::layout::Layout::layout(
                pane.view.display(&arena, &pane.store, &ui),
                &arena,
                Constraints::tight(Size::new(420.0, 400.0)),
            ),
            &arena,
            Rect::from_wh(420.0, 400.0),
        )
        .handle_event(&arena, &mut event, Rect::from_wh(420.0, 400.0))
        {
            EventResult::Command(EditorCommand::Click { kind, .. }) => Some(kind),
            _ => None,
        }
    };
    assert_eq!(click(1, false), Some(ClickKind::Set));
    assert_eq!(click(2, false), Some(ClickKind::Word));
    assert_eq!(click(3, false), Some(ClickKind::Line));
    assert_eq!(
        click(4, false),
        Some(ClickKind::Line),
        "a run keeps selecting lines"
    );
    assert_eq!(
        click(2, true),
        Some(ClickKind::Add),
        "alt outranks the count"
    );
}

#[test]
fn drag_extends_selection_by_the_press_unit() {
    let store = &imba::store::Store::new();
    let ui = ::editor::test_document::test_ui();
    use editor::editor_view::ClickKind;
    use skia_safe::Point;
    let text = "alpha  beta gamma\nsecond line\n";
    let mut pane = TestPane::new(plain_document(text), 420.0);
    let fonts = ::editor::test_document::test_fonts_collection();
    let theme = ::editor::theme::Theme::embedded();
    let point_at = |pane: &TestPane, byte: u32| -> Point {
        let document = pane.document();
        let (x, y, _, h) = document
            .caret_content_rect(pane.view.editor(), byte, &store, ui, &fonts, &theme)
            .expect("a caret rect for the byte");
        Point::new(x + 1.0, y + h / 2.0)
    };
    let primary = |pane: &TestPane| pane.document().carets(pane.view.editor()).primary();

    let press = point_at(&pane, 2);
    pane.perform(EditorCommand::Click {
        kind: ClickKind::Set,
        point: press,
    });
    let to = point_at(&pane, 9);
    pane.perform(EditorCommand::Drag { point: to });
    assert_eq!(
        primary(&pane).selection(),
        2..9,
        "char drag from the press byte"
    );
    assert_eq!(primary(&pane).offset(), 9, "head at the pointer");

    let back = point_at(&pane, 0);
    pane.perform(EditorCommand::Drag { point: back });
    assert_eq!(primary(&pane).selection(), 0..2, "the anchor held");
    assert_eq!(primary(&pane).offset(), 0);

    pane.perform(EditorCommand::DragEnd);
    pane.perform(EditorCommand::Drag { point: to });
    assert_eq!(primary(&pane).selection(), 0..2, "no origin, no drag");

    pane.perform(EditorCommand::Click {
        kind: ClickKind::Word,
        point: point_at(&pane, 9),
    });
    pane.perform(EditorCommand::Drag {
        point: point_at(&pane, 14),
    });
    assert_eq!(
        primary(&pane).selection(),
        7..17,
        "word drag takes whole words on both ends"
    );

    pane.perform(EditorCommand::Drag {
        point: point_at(&pane, 2),
    });
    assert_eq!(
        primary(&pane).selection(),
        0..11,
        "backwards word drag keeps the origin word"
    );
    pane.perform(EditorCommand::DragEnd);

    pane.perform(EditorCommand::Click {
        kind: ClickKind::Set,
        point: point_at(&pane, 2),
    });
    pane.perform(EditorCommand::Click {
        kind: ClickKind::Add,
        point: point_at(&pane, 20),
    });
    pane.perform(EditorCommand::Drag {
        point: point_at(&pane, 24),
    });
    let carets = pane.document().carets(pane.view.editor());
    assert_eq!(carets.len(), 2, "both carets live");
    assert_eq!(
        carets.primary().selection(),
        20..24,
        "the added caret dragged"
    );
    assert_eq!(
        carets.carets()[0].offset(),
        2,
        "the first caret never moved"
    );
}

#[test]
fn ime_hit_test_covers_an_empty_document() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(
        app.with_ime_client(app.sole_window(), |client| client.document_length()),
        Some(0),
        "the startup document is empty"
    );

    for y in [100.0, 300.0, 500.0] {
        assert_eq!(
            app.with_ime_client(app.sole_window(), |client| client.char_index_at(400.0, y))
                .expect("focused"),
            Some(0),
            "the empty pane is editable at y={y}"
        );
    }

    assert!(app
        .with_ime_client(app.sole_window(), |client| client
            .char_index_at(60.0, 300.0))
        .expect("focused")
        .is_none());
}

#[test]
fn ime_hit_test_rejects_chrome_over_a_scrolled_pane() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let mut text = String::new();
    for line in 0..80 {
        text.push_str(&format!("line number {line}\n"));
    }
    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text(&text, None);
    });
    app.draw_window(app.sole_window(), surface.canvas());

    let hit = |app: &mut Application, y: f32| {
        app.with_ime_client(app.sole_window(), |client| client.char_index_at(400.0, y))
            .expect("focused")
    };
    for scroll in [-100_000.0, 900.0] {
        let _ = himark::test_driver::scroll_at(&mut app, 400.0, 300.0, scroll);
        app.draw_window(app.sole_window(), surface.canvas());
        for y in [10.0, 30.0, 60.0] {
            assert!(
                hit(&mut app, y).is_none(),
                "the toolbar strip is chrome at y={y} (scroll {scroll})"
            );
        }
        assert!(
            hit(&mut app, 300.0).is_some(),
            "the pane's own text still answers (scroll {scroll})"
        );
    }
}
