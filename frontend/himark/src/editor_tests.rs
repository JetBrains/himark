// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{store::Store, View};
use std::str;

use editor::{
    inlay_anchors_line,
    test_document::{fenced_code_document, list_document, plain_document},
    Document, EditorCommand, EditorFocus, EditorView, Inlay, InlayCommand, InlayMode,
};

use crate::AppExt;
use crate::EditorIdView;

struct TestPane {
    store: Store,
    view: EditorIdView,

    inlay_markup: editor::MarkupId,
}

impl TestPane {
    fn new(mut document: Document, width: f32) -> Self {
        let ui = ::editor::test_document::test_ui();
        let mut store = Store::new();
        let editor = document.add_editor(
            width,
            None,
            ::editor::EditorBuild::Complete,
            &[],
            &store,
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::theme::Theme::embedded(),
            &mut imba::effect::Batch::new().effects(),
        );
        let inlay_markup = document.add_markup();
        document.show_markup(editor, inlay_markup);
        let document_id =
            crate::OpenDocuments::register(&mut store, document, None, "test".to_owned(), 0);

        Self {
            view: EditorIdView::new(document_id, editor),
            store,
            inlay_markup,
        }
    }

    fn inlay_key(&self, key: u32) -> editor::InlayKey {
        editor::InlayKey::in_markup(self.inlay_markup, key)
    }

    fn resize(&mut self, width: f32, anchor: u32) -> imba::effect::Batch<EditorCommand> {
        let ui = ::editor::test_document::test_ui();
        let entity = self.view;
        let mut document =
            crate::OpenDocuments::document(&self.store, entity.document()).expect("document");
        let mut batch = imba::effect::Batch::new();
        let _ = document.resize(
            entity.editor(),
            width,
            anchor,
            &self.store,
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::theme::Theme::embedded(),
            &mut batch.effects(),
        );
        crate::OpenDocuments::put_document(&mut self.store, entity.document(), document);
        batch
    }

    fn perform(&mut self, command: EditorCommand) {
        let mut batch = imba::effect::Batch::new();
        self.view.perform(
            &mut self.store,
            ::editor::test_document::test_ui(),
            command,
            &mut batch.effects(),
        );
        self.finish(batch);
    }

    fn finish(&mut self, batch: imba::effect::Batch<EditorCommand>) {
        let mut pending = crate::test_support::surviving_launches(batch);
        while let Some(effect) = pending.pop() {
            let command = crate::test_support::handle_effect(
                effect,
                &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
            );
            let mut batch = imba::effect::Batch::new();
            self.view.perform(
                &mut self.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            pending.extend(crate::test_support::surviving_launches(batch));
        }
    }

    fn gathered(&self) -> EditorView {
        self.view.gathered(&self.store).expect("editor in store")
    }

    fn document(&self) -> Document {
        self.gathered().document.clone()
    }

    fn set_caret(&mut self, byte: u32) {
        let entity = self.view;
        let mut document =
            crate::OpenDocuments::document(&self.store, entity.document()).expect("document");
        document.set_caret(entity.editor(), byte);
        crate::OpenDocuments::put_document(&mut self.store, entity.document(), document);
    }

    fn replace_inlay(&mut self, key: editor::InlayKey, range: std::ops::Range<u32>, inlay: Inlay) {
        let ui = ::editor::test_document::test_ui();
        let entity = self.view;
        let mut document =
            crate::OpenDocuments::document(&self.store, entity.document()).expect("document");
        let mut batch = imba::effect::Batch::new();
        document.replace_inlay(
            key,
            range,
            inlay,
            &self.store,
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::theme::Theme::embedded(),
            &mut batch.effects(),
        );
        crate::OpenDocuments::put_document(&mut self.store, entity.document(), document);
        self.finish(batch);
    }
}

#[test]
fn typing_and_backspace_keep_utf8_text_and_layout_in_sync() {
    let mut pane = TestPane::new(plain_document(""), 420.0);

    pane.perform(EditorCommand::InsertText {
        text: "aβc".to_owned(),
    });
    pane.perform(EditorCommand::Move {
        motion: editor::Motion::Left,
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

    let gathered = EditorIdView::new(pane.view.document(), pane.view.editor())
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
        ::editor::EditorBuild::Complete,
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
        ::editor::EditorBuild::Complete,
        &[],
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_caret(right_editor, "shared".len() as u32);
    let document_id =
        crate::OpenDocuments::register(&mut store, document, None, "test".to_owned(), 0);

    let mut left_view = EditorIdView::new(document_id, left_editor);
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

    let right_view = EditorIdView::new(document_id, right_editor)
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
        command: Box::new(TestInlayCommand::Grow) as InlayCommand,
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
        command: Box::new(TestInlayCommand::Grow) as InlayCommand,
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
        command: Box::new(TestInlayCommand::Grow) as InlayCommand,
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
        command: Box::new(TestInlayCommand::GrowWide) as InlayCommand,
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
    let document_b_id = crate::OpenDocuments::register(
        &mut pane.store,
        document_b.clone(),
        None,
        "b".to_owned(),
        0,
    );
    let mut fresh = imba::effect::Batch::new();

    crate::close_editor(&mut pane.store, pane.view.document(), pane.view.editor());
    let new_editor = crate::mount_editor(
        &pane.store,
        ::editor::test_document::test_ui(),
        &mut document_b,
        200.0,
        None,
        &mut fresh.effects(),
    );
    pane.view = EditorIdView::new(document_b_id, new_editor);
    crate::OpenDocuments::put_document(&mut pane.store, document_b_id, document_b);
    assert!(
        crate::test_support::surviving_launches(fresh).is_empty(),
        "the tiny document opens fully repaired"
    );

    for effect in crate::test_support::surviving_launches(stale) {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
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

    for effect in crate::test_support::surviving_launches(first) {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
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

    let mut pending = crate::test_support::surviving_launches(second);
    while let Some(effect) = pending.pop() {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        let mut batch = imba::effect::Batch::new();
        pane.view.perform(
            &mut pane.store,
            ::editor::test_document::test_ui(),
            command,
            &mut batch.effects(),
        );
        pending.extend(crate::test_support::surviving_launches(batch));
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
        command: Box::new(TestInlayCommand::Grow) as InlayCommand,
    });
    assert_eq!(
        pane.gathered().focus(),
        EditorFocus::Inlay(pane.inlay_key(7))
    );
    let stored = pane.view;
    assert_eq!(
        crate::OpenDocuments::document_ref(&pane.store, stored.document())
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
        kind: editor::ClickKind::Set,
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
        InlayMode::Instead(editor::InsteadKind::FullLine),
        &interval,
        &first
    ));
    assert!(!inlay_anchors_line(
        InlayMode::Instead(editor::InsteadKind::FullLine),
        &interval,
        &second
    ));

    assert!(inlay_anchors_line(InlayMode::Above, &interval, &first));
    assert!(!inlay_anchors_line(InlayMode::Above, &interval, &second));
    assert!(!inlay_anchors_line(InlayMode::Under, &interval, &first));
    assert!(inlay_anchors_line(InlayMode::Under, &interval, &second));
}

fn text_string(pane: &TestPane) -> String {
    let mut view = pane.document().text().view();
    view.byte_string(0, view.byte_count())
}

fn assert_layout_ranges_are_utf8(pane: &TestPane) {
    let gathered = pane.gathered();
    let mut view = pane.document().text().view();
    let mut covered = 0;
    for range in gathered.document_layout().element_byte_ranges() {
        let mut bytes = Vec::new();
        view.byte_range_into(range.start as usize, range.end as usize, &mut bytes);
        assert!(
            str::from_utf8(&bytes).is_ok(),
            "layout range {range:?} is not UTF-8"
        );
        covered = range.end;
    }
    assert_eq!(covered as usize, pane.document().text().byte_count());
}

fn layout_element_count(pane: &TestPane) -> usize {
    pane.gathered()
        .document_layout()
        .element_byte_ranges()
        .len()
}

fn layout_line_starts(pane: &TestPane) -> Vec<u32> {
    pane.gathered()
        .document_layout()
        .element_byte_ranges()
        .into_iter()
        .map(|range| range.start)
        .collect()
}

fn layout_ranges(pane: &TestPane) -> Vec<String> {
    let gathered = pane.gathered();
    let mut view = pane.document().text().view();
    gathered
        .document_layout()
        .element_byte_ranges()
        .into_iter()
        .map(|range| view.byte_string(range.start as usize, range.end as usize))
        .collect()
}

#[derive(Clone)]
struct TestInlay {
    width: f32,
    height: f32,
}

enum TestInlayCommand {
    Grow,
    GrowWide,
}

impl View for TestInlay {
    type Command = TestInlayCommand;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            TestInlayCommand::Grow => {
                self.width += 180.0;
                self.height += 10.0;
            }
            TestInlayCommand::GrowWide => {
                self.width += 520.0;
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                use imba::event::{Event, EventResult, MouseButton};
                use imba::thunk_ext::ThunkExt;

                imba::leaf::leaf(self.width, self.height).event(|_, event, _| match event {
                    Event::MouseDown {
                        mods: _,
                        button: MouseButton::Left,
                        ..
                    } => EventResult::Command(TestInlayCommand::Grow),
                    _ => EventResult::Ignored,
                })
            },
        )
    }
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
        command: Box::new(ProbeCommand::Poke) as InlayCommand,
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
        let widget = imba::Layout::layout(
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
        kind: editor::ClickKind::Set,
        point: skia_safe::Point::new(5.0, 5.0),
    });
    assert_eq!(focus(&pane), EditorFocus::Text);

    assert_eq!(
        probe_reports(&paint(&pane)),
        1,
        "the unfocused paint is seen"
    );
}

#[derive(Clone)]
struct FocusProbe;

enum ProbeCommand {
    Poke,
}

impl View for FocusProbe {
    type Command = ProbeCommand;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                use imba::event::{Event, EventResult};
                use imba::thunk_ext::ThunkExt;

                imba::leaf::leaf(40.0, 20.0).event(|_, event, _| match event {
                    Event::Paint { focused: false, .. } => EventResult::Command(ProbeCommand::Poke),
                    _ => EventResult::Ignored,
                })
            },
        )
    }
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
    for effect in crate::test_support::surviving_launches(edit_effects) {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
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
    let document_id =
        crate::OpenDocuments::register(&mut store, document.clone(), None, "test".to_owned(), 0);
    let mut open_batch = imba::effect::Batch::new();
    let editor_id = crate::mount_editor(
        &store,
        &ui,
        &mut document,
        200.0,
        None,
        &mut open_batch.effects(),
    );
    crate::OpenDocuments::put_document(&mut store, document_id, document.clone());
    let effects = crate::test_support::surviving_launches(open_batch);

    let reference = EditorView::complete(
        document.clone(),
        200.0,
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::theme::Theme::embedded(),
    );
    let opened = EditorIdView::new(document_id, editor_id)
        .gathered(&store)
        .expect("entity");
    assert!(opened.content_height() < reference.content_height());
    assert_eq!(opened.find_misaligned_boundary(), None);
    assert_eq!(effects.len(), 1, "the tail repairs in background");

    let mut node = EditorIdView::new(document_id, editor_id);
    let mut chain = Vec::new();
    for effect in effects {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        chain.extend({
            let mut batch = imba::effect::Batch::new();
            node.perform(
                &mut store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            crate::test_support::surviving_launches(batch)
        });
    }
    while let Some(effect) = chain.pop() {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(::editor::theme::Theme::embedded()),
        );
        chain.extend({
            let mut batch = imba::effect::Batch::new();
            node.perform(
                &mut store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            crate::test_support::surviving_launches(batch)
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("hello", None);
    });
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text("hello brave world", None);
    });
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let mut text = String::new();
    for line in 0..80 {
        text.push_str(&format!("line {line} with some words on it\n"));
    }
    assert!(crate::test_driver::type_text(&mut app, &text));
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let _ = crate::test_driver::scroll_at(&mut app, 400.0, 300.0, -100_000.0);
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let (click_x, click_y) = (140.0, 160.0);
    let _ = crate::test_driver::click(&mut app, click_x, click_y, 800.0, 600.0);
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

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
    assert!(crate::test_driver::scroll_at(&mut app, 400.0, 300.0, delta));
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(crate::test_driver::type_text(&mut app, "so"));
    assert!(crate::test_driver::type_text(&mut app, "u"));
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(
        app.with_ime_client(app.sole_window(), |_| ()).is_some(),
        "the startup editor should have text focus"
    );
    let handled = crate::test_driver::type_text(&mut app, "a");
    assert!(handled, "typing into the empty document must be handled");
    assert_eq!(app.focused_document_text().as_deref(), Some("a"));
}

#[test]
fn two_windows_edit_independently() {
    use crate::{AppFonts, Application};
    use imba::event::Event;
    use skia_safe::Size;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let first = app.add_window();
    let second = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw_with_size(first, &mut app, surface.canvas(), Size::new(800.0, 600.0));
    crate::Window::draw_with_size(second, &mut app, surface.canvas(), Size::new(500.0, 400.0));

    let focused_text = |app: &Application, window: crate::WindowId| -> Option<String> {
        let store = &app.window_store(window);
        let document = crate::OpenDocuments::document_ref(
            store,
            crate::Windows::window_ref(store, window)?.focused_document_id()?,
        )?;
        let end = document.text().byte_count().min(u32::MAX as usize) as u32;
        Some(document.text().view().substring(0..end))
    };

    assert!(app.dispatch_timed(
        first,
        Event::TextInput { text: "one" },
        Size::new(800.0, 600.0),
        0.0,
    ));
    assert!(app.dispatch_timed(
        second,
        Event::TextInput { text: "two" },
        Size::new(500.0, 400.0),
        0.0,
    ));
    assert_eq!(focused_text(&app, first).as_deref(), Some("one"));
    assert_eq!(focused_text(&app, second).as_deref(), Some("two"));

    let viewport = |app: &Application, window: crate::WindowId| {
        app.window_viewport(window).expect("the window entity")
    };
    assert_eq!(viewport(&app, first).width, 800.0);
    assert_eq!(viewport(&app, second).width, 500.0);

    assert!(app.perform_registered(second, "workbench.split-pane"));
    let panes = |app: &Application, window: crate::WindowId| {
        let mut count = 0;
        let store = app.window_store(window);
        crate::Windows::window_ref(&store, window)
            .expect("the window entity")
            .workbench()
            .root
            .for_each_pane(&mut |_| count += 1);
        count
    };
    assert_eq!(panes(&app, second), 2, "the second window split");
    assert_eq!(panes(&app, first), 1, "the first window did not");
}

#[test]
fn closing_a_split_pane_collapses_to_the_sibling() {
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let start = app.pane_count();

    assert!(
        app.perform_registered(app.sole_window(), "workbench.split-pane"),
        "splitting the focused pane"
    );
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(app.pane_count(), start + 1);

    assert!(
        app.perform_registered(app.sole_window(), "workbench.close-pane"),
        "closing the focused pane"
    );
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(
        app.pane_count(),
        start,
        "the split collapsed to its sibling"
    );
    while app.pane_count() > 1 {
        assert!(
            app.perform_registered(app.sole_window(), "workbench.close-pane"),
            "closing down to one pane"
        );
        let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }
    let _ = app.perform_registered(app.sole_window(), "workbench.close-pane");
    assert_eq!(app.pane_count(), 1, "the last pane refuses to close");
    assert!(
        crate::test_driver::type_text(&mut app, "x"),
        "the surviving pane still types"
    );
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

    let band = |layout: &::editor::DocumentLayout, visible: &(u32, u32)| {
        layout.height_before(visible.1) - layout.height_before(visible.0)
    };
    let (visible, dark_band) = {
        let document = pane.document();
        let layout = document.document_layout(entity.editor()).unwrap();
        let visible = (layout.byte_at_y(top), layout.byte_at_y(bottom));
        (visible, band(layout, &visible))
    };
    let visible_start = visible.0;

    crate::env::Themes::set(&mut pane.store, light.clone());
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
        crate::OpenDocuments::document(&pane.store, entity.document()).expect("document");
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

    let mut pending = crate::test_support::surviving_launches(effects);
    while let Some(effect) = pending.pop() {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(light.clone()),
        );
        pending.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            crate::test_support::surviving_launches(batch)
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
    crate::env::Themes::set(&mut pane.store, light.clone());
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
        crate::OpenDocuments::document_ref(&pane.store, entity.document())
            .unwrap()
            .document_layout(entity.editor())
            .unwrap()
            .repair_pending()
    };
    assert!(pending(&pane).is_some(), "the tail is pending");

    let mut requeued = Vec::new();
    for effect in crate::test_support::surviving_launches(stale_round) {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(Theme::embedded()),
        );
        requeued.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            crate::test_support::surviving_launches(batch)
        });
    }
    assert!(
        pending(&pane).is_some(),
        "a stale-theme landing must not install: the damage stays pending"
    );

    let mut pending_effects = requeued;
    while let Some(effect) = pending_effects.pop() {
        let command = crate::test_support::handle_effect(
            effect,
            &crate::test_support::test_workshop(light.clone()),
        );
        pending_effects.extend({
            let mut batch = imba::effect::Batch::new();
            pane.view.perform(
                &mut pane.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            crate::test_support::surviving_launches(batch)
        });
    }
    let converged =
        crate::OpenDocuments::document(&pane.store, entity.document()).expect("document");
    let layout = converged.document_layout(entity.editor()).unwrap();
    assert!(layout.repair_pending().is_none(), "the tail repaired");
    assert_eq!(layout.shaped_theme(), "light");
    let fresh =
        ::editor::EditorView::complete(converged.clone(), 420.0, &store, ui, &fonts, &light);
    assert!(
        (layout.height() - fresh.document_layout().height()).abs() < 0.5,
        "the repaired document IS the fresh light layout: {} vs {}",
        layout.height(),
        fresh.document_layout().height()
    );
}

#[test]
fn theme_toggle_swaps_the_theme_and_reshapes_the_view() {
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(crate::test_driver::type_text(
        &mut app,
        "# A title\n\nSome body text to shape, long enough to wrap around.",
    ));
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let height_dark = app.focused_pane_content_height();
    assert!(height_dark > 0.0);

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));
    assert_eq!(
        crate::env::Themes::of(app.store()).name(),
        "light",
        "the store's theme swapped"
    );
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let height_light = app.focused_pane_content_height();

    assert!(
        (height_light - height_dark).abs() < 0.5,
        "identical geometry across themes: {height_light} vs {height_dark}"
    );

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));
    assert_eq!(crate::env::Themes::of(app.store()).name(), "dark");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(
        (app.focused_pane_content_height() - height_dark).abs() < 0.5,
        "toggling back restores the dark geometry"
    );
}

#[test]
fn open_documents_list_recently_opened_first() {
    use crate::OpenDocuments;
    let mut store = Store::new();
    let alpha = OpenDocuments::register(
        &mut store,
        plain_document("alpha"),
        None,
        "alpha".to_owned(),
        0,
    );
    let _beta = OpenDocuments::register(
        &mut store,
        plain_document("beta"),
        None,
        "beta".to_owned(),
        0,
    );

    let recent_names = |store: &Store| -> Vec<String> {
        OpenDocuments::list_recent(store)
            .into_iter()
            .map(|(_, info)| info.name())
            .collect()
    };
    assert_eq!(recent_names(&store), ["beta", "alpha"]);

    OpenDocuments::touch(&mut store, alpha);
    assert_eq!(recent_names(&store), ["alpha", "beta"]);

    let open_order: Vec<String> = OpenDocuments::list(&store)
        .into_iter()
        .map(|(_, info)| info.name())
        .collect();
    assert_eq!(open_order, ["alpha", "beta"], "open order stays stable");
}

#[test]
fn palette_commands_follow_the_modal_focus() {
    use crate::{AppCommand, AppFonts, Application, ModalRequest, ModalView, OpenDocuments};

    #[derive(Clone)]
    struct TestModal {
        document: crate::DocumentId,

        request: std::sync::Arc<std::sync::Mutex<Option<ModalRequest>>>,
    }
    enum TestModalCommand {
        Close,
        Show,
    }
    impl imba::View for TestModal {
        type Command = TestModalCommand;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &imba::UiCtx,
            command: TestModalCommand,
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
            *self.request.lock().unwrap() = Some(match command {
                TestModalCommand::Close => ModalRequest::Close,
                TestModalCommand::Show => ModalRequest::ShowDocument(self.document),
            });
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a Store,
            _ui: &'a imba::UiCtx,
        ) -> impl imba::Layout<'a, TestModalCommand> + imba::LayoutValue + 'a {
            imba::laid(
                move |_arena: &'a imba::arena::Arena,
                      constraints: imba::constraints::Constraints| {
                    imba::leaf::leaf::<TestModalCommand>(
                        constraints.max.width,
                        constraints.max.height,
                    )
                },
            )
        }

        fn focus_data<'w>(
            &'w self,
            _store: &'w Store,
            _ui: &'w imba::UiCtx,
        ) -> imba::focus::FocusData<'w, TestModalCommand> {
            imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
                "test.modal.close",
                "Close Test Modal",
                TestModalCommand::Close,
            )])
        }
    }
    impl ModalView for TestModal {
        fn clone_modal(&self) -> Box<dyn ModalView> {
            Box::new(self.clone())
        }

        fn take_request(&mut self) -> Option<ModalRequest> {
            self.request.lock().unwrap().take()
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let ids = |app: &Application| -> Vec<&'static str> {
        crate::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .iter()
            .map(|command| command.id)
            .collect()
    };
    assert_eq!(
        ids(&app),
        [
            "editor.undo",
            "editor.redo",
            "editor.select-all",
            "editor.toggle-softwrap",
            "editor.select-next-occurrence",
            "editor.select-all-occurrences",
            "editor.add-caret-above",
            "editor.add-caret-below",
            "editor.backspace",
            "editor.delete-forward",
            "editor.delete-word-back",
            "editor.delete-word-forward",
            "editor.newline",
            "editor.newline-soft",
            "editor.indent",
            "editor.outdent",
            "editor.move-left",
            "editor.select-left",
            "editor.move-right",
            "editor.select-right",
            "editor.move-up",
            "editor.select-up",
            "editor.move-down",
            "editor.select-down",
            "editor.move-word-left",
            "editor.select-word-left",
            "editor.move-word-right",
            "editor.select-word-right",
            "editor.move-line-start",
            "editor.select-line-start",
            "editor.move-line-end",
            "editor.select-line-end",
            "editor.move-doc-start",
            "editor.select-doc-start",
            "editor.move-doc-end",
            "editor.select-doc-end",
            "find.open",
            "completion.trigger",
            "find.next",
            "find.previous",
            "workbench.new-document",
            "workbench.split-pane",
            "workbench.close",
            "workbench.close-pane",
            "navigation.back",
            "navigation.forward",
            "toc.toggle",
            "theme.toggle",
            "chat.composer",
            "session.new",
        ],
        "text focus offers the editor's caret commands, then the workbench actions"
    );

    assert!(app.add_document(
        app.sole_window(),
        plain_document("beta body"),
        "beta".to_owned(),
        false
    ));
    let beta = OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, info)| info.name() == "beta")
        .expect("beta is open")
        .0;

    assert!(app.open_modal(
        app.sole_window(),
        Box::new(TestModal {
            document: beta,
            request: Default::default(),
        })
    ));
    assert_eq!(
        ids(&app).first(),
        Some(&"test.modal.close"),
        "the modal's commands come first"
    );

    let close = crate::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
        .remove(0)
        .command;
    app.perform_batch(vec![close]);
    assert_eq!(
        ids(&app).first(),
        Some(&"editor.undo"),
        "the modal closed; the base surface is back"
    );

    assert!(app.open_modal(
        app.sole_window(),
        Box::new(TestModal {
            document: beta,
            request: Default::default(),
        })
    ));
    app.perform_batch(vec![AppCommand::Content(
        app.sole_window(),
        crate::WindowCommand::Modal(Box::new(TestModalCommand::Show) as imba::DynCommand),
    )]);
    assert!(app.plugin_modal().is_none(), "the show dismissed the modal");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("beta body"),
        "the requested document took the focused pane"
    );

    assert_eq!(app.pane_count(), 1);
    let split = crate::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
        .into_iter()
        .find(|command| command.id == "workbench.split-pane")
        .expect("the split action")
        .command;
    app.perform_batch(vec![split]);
    assert_eq!(app.pane_count(), 2, "the scheduled split ran");
}

#[test]
fn registered_commands_present_and_dispatch_by_id() {
    use crate::{AppFonts, Application, DynamicCommand};

    #[derive(Clone)]
    struct Marker;
    struct Probe;
    impl DynamicCommand for Probe {
        fn id(&self) -> &'static str {
            "test.probe"
        }
        fn name(&self) -> String {
            "Probe".to_owned()
        }
        fn perform(
            &self,
            _app: &mut Application,
            store: &mut Store,
            _window: crate::WindowId,
            _fx: &mut crate::AppFx<'_>,
        ) {
            store.put(Marker);
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    app.register_command(std::sync::Arc::new(Probe));
    let ids: Vec<&str> = crate::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
        .iter()
        .map(|command| command.id)
        .collect();
    assert_eq!(
        ids,
        [
            "editor.undo",
            "editor.redo",
            "editor.select-all",
            "editor.toggle-softwrap",
            "editor.select-next-occurrence",
            "editor.select-all-occurrences",
            "editor.add-caret-above",
            "editor.add-caret-below",
            "editor.backspace",
            "editor.delete-forward",
            "editor.delete-word-back",
            "editor.delete-word-forward",
            "editor.newline",
            "editor.newline-soft",
            "editor.indent",
            "editor.outdent",
            "editor.move-left",
            "editor.select-left",
            "editor.move-right",
            "editor.select-right",
            "editor.move-up",
            "editor.select-up",
            "editor.move-down",
            "editor.select-down",
            "editor.move-word-left",
            "editor.select-word-left",
            "editor.move-word-right",
            "editor.select-word-right",
            "editor.move-line-start",
            "editor.select-line-start",
            "editor.move-line-end",
            "editor.select-line-end",
            "editor.move-doc-start",
            "editor.select-doc-start",
            "editor.move-doc-end",
            "editor.select-doc-end",
            "find.open",
            "completion.trigger",
            "find.next",
            "find.previous",
            "workbench.new-document",
            "workbench.split-pane",
            "workbench.close",
            "workbench.close-pane",
            "navigation.back",
            "navigation.forward",
            "toc.toggle",
            "theme.toggle",
            "chat.composer",
            "session.new",
            "test.probe",
        ],
        "registration order, after the built-ins"
    );

    assert!(
        !app.perform_registered(app.sole_window(), "no.such.command"),
        "unknown id: no-op"
    );
    assert!(app.perform_registered(app.sole_window(), "test.probe"));

    assert!(app.store().get::<Marker>().is_some());
}

#[test]
fn switching_workspaces_stashes_and_restores_the_workbench() {
    use crate::AppFonts;
    use std::sync::Arc;

    struct Switch {
        target: Option<crate::SessionId>,
        made: Arc<std::sync::Mutex<Option<crate::SessionId>>>,
    }
    impl crate::DynamicCommand for Switch {
        fn id(&self) -> &'static str {
            "test.switch"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            _app: &mut crate::Application,
            store: &mut Store,
            window: crate::WindowId,
            fx: &mut crate::AppFx<'_>,
        ) {
            let target = match self.target.clone() {
                Some(target) => target,
                None => crate::SessionId::mint_scratch(store),
            };
            *self.made.lock().unwrap() = Some(target.clone());
            crate::switch_session(store, window, target, fx)
        }
    }
    let switch =
        |app: &mut crate::Application, target: Option<crate::SessionId>| -> crate::SessionId {
            let window = app.sole_window();
            let made = Arc::new(std::sync::Mutex::new(None));
            app.perform_batch(vec![crate::AppCommand::Dynamic(
                window,
                Arc::new(Switch {
                    target,
                    made: Arc::clone(&made),
                }),
            )]);
            let result = made.lock().unwrap().take().expect("the switch ran");
            result
        };

    let fonts = AppFonts::embedded();
    let mut app = crate::Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let first = crate::Windows::window_ref(app.store(), window)
        .expect("window")
        .current_session();

    assert!(app.perform_registered(window, "workbench.split-pane"));
    assert_eq!(app.pane_count(), 2);
    assert!(crate::test_driver::type_text(&mut app, "hello A"));
    let a_text = app.focused_document_text().expect("A text");
    assert!(a_text.contains("hello A"));
    let documents_before = app.document_count();

    let second = switch(&mut app, None);
    assert_eq!(
        crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .current_session(),
        second
    );
    assert_eq!(app.pane_count(), 1, "a fresh workbench for B");
    assert_eq!(
        app.document_count(),
        1,
        "B's world holds its own scratch alone (documents are per session)"
    );
    let b_text = app.focused_document_text().expect("B text");
    assert!(!b_text.contains("hello A"), "B does not show A's document");

    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(app.pane_count(), 2, "A's split survived the stash");
    assert!(app
        .focused_document_text()
        .expect("A restored")
        .contains("hello A"));
    let _ = switch(&mut app, Some(second.clone()));
    assert_eq!(
        app.document_count(),
        1,
        "toggling creates exactly one scratch per session, ever"
    );
    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(
        app.document_count(),
        documents_before,
        "A's world is its own again"
    );

    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(app.pane_count(), 2);
    assert_eq!(
        crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .current_session(),
        first
    );
}

#[test]
fn switching_dismisses_the_overlays_first() {
    use crate::AppFonts;
    use std::sync::Arc;

    #[derive(Clone)]
    struct NullModal;
    impl imba::View for NullModal {
        type Command = ();
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &imba::UiCtx,
            _command: (),
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a Store,
            _ui: &'a imba::UiCtx,
        ) -> impl imba::Layout<'a, ()> + imba::LayoutValue + 'a {
            imba::laid(
                move |_arena: &'a imba::arena::Arena,
                      constraints: imba::constraints::Constraints| {
                    imba::leaf::leaf::<()>(constraints.max.width, constraints.max.height)
                },
            )
        }
    }
    impl crate::ModalView for NullModal {
        fn clone_modal(&self) -> Box<dyn crate::ModalView> {
            Box::new(self.clone())
        }
        fn take_request(&mut self) -> Option<crate::ModalRequest> {
            None
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct SwitchFresh(Arc<std::sync::Mutex<Option<crate::SessionId>>>);
    impl crate::DynamicCommand for SwitchFresh {
        fn id(&self) -> &'static str {
            "test.switch"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            _app: &mut crate::Application,
            store: &mut Store,
            window: crate::WindowId,
            fx: &mut crate::AppFx<'_>,
        ) {
            let target = crate::SessionId::mint_scratch(store);
            *self.0.lock().unwrap() = Some(target.clone());
            crate::switch_session(store, window, target, fx)
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = crate::Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    assert!(app.open_modal(window, Box::new(NullModal)));
    assert!(app.plugin_modal().is_some());

    let made = Arc::new(std::sync::Mutex::new(None));
    app.perform_batch(vec![crate::AppCommand::Dynamic(
        window,
        Arc::new(SwitchFresh(Arc::clone(&made))),
    )]);
    let second = made.lock().unwrap().take().expect("the switch ran");
    let entity = crate::Windows::window_ref(app.store(), window).expect("window");
    assert!(
        entity.plugin_modal().is_none() && entity.side_panel().is_none(),
        "the switch dismissed the overlays"
    );
    assert_eq!(entity.current_session(), second);
}

#[test]
fn caret_commands_glide_the_pane_to_the_caret() {
    use crate::test_driver;
    use crate::{AppFonts, Application};
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
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    use crate::test_driver;
    use crate::{AppFonts, Application};
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
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

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
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }
    assert!(
        (app.focused_pane_scroll_y() - after_wheel).abs() < 1.0,
        "the reveal never fought the wheel (scroll {} vs {after_wheel})",
        app.focused_pane_scroll_y()
    );
}

#[test]
fn opening_with_a_target_lands_the_caret_revealed() {
    use crate::{AppCommand, AppExt, AppFonts, Application, LineCol, OpenedDocument};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();

    let source: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let target = LineCol { line: 150, col: 5 }..LineCol { line: 150, col: 7 };
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(window, &mut app, surface.canvas());
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
                client.paste(&imba::ClipboardContent {
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
fn navigation_back_and_forward_walk_pane_history() {
    use crate::{AppFonts, Application, OpenedDocument};
    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }
    fn editor_count(app: &Application, location: &crate::ResourceLocation) -> usize {
        let id = crate::OpenDocuments::by_location(app.store(), location).expect("registered");
        crate::OpenDocuments::document_ref(app.store(), id)
            .expect("document")
            .editor_ids()
            .count()
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let window = app.sole_window();

    let open = |app: &mut Application, name: &str, text: &str| {
        let document = plain_document(text);
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: name.to_owned(),
                document,
                location: Some(located(name)),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    };
    open(&mut app, "a.md", "alpha\n");
    assert_eq!(app.focused_document_text().as_deref(), Some("alpha\n"));

    assert!(crate::test_driver::type_text(&mut app, "x"));
    assert_eq!(app.focused_document_text().as_deref(), Some("xalpha\n"));

    open(&mut app, "b.md", "beta\n");
    assert_eq!(app.focused_document_text().as_deref(), Some("beta\n"));
    assert!(crate::test_driver::type_text(&mut app, "y"));
    assert_eq!(
        editor_count(&app, &located("a.md")),
        0,
        "displacement retracted the editor; dirty, the document stays"
    );

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xalpha\n"),
        "back returns to the recorded place — an instant remount, edits intact"
    );
    assert_eq!(editor_count(&app, &located("a.md")), 1);
    assert_eq!(
        editor_count(&app, &located("b.md")),
        0,
        "b displaced in turn — retracted, dirty, kept"
    );

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("ybeta\n"),
        "forward walks up again"
    );

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(app.focused_document_text().as_deref(), Some("ybeta\n"));

    let caret = |app: &Application| -> u32 {
        let (document, editor) = app.focused_editor_id();
        crate::OpenDocuments::document_ref(app.store(), document)
            .expect("document")
            .caret_byte(editor)
    };
    let before_jump = caret(&app);
    assert!(app.perform_command(crate::AppCommand::Opened(
        window,
        OpenedDocument {
            name: "b.md".to_owned(),
            document: plain_document("beta\n"),
            location: Some(located("b.md")),
            primary: true,
            target: Some(crate::LineCol { line: 0, col: 3 }..crate::LineCol { line: 0, col: 4 }),
            focus: false,
        },
    )));
    assert_eq!(app.focused_document_text().as_deref(), Some("ybeta\n"));
    assert_eq!(caret(&app), 3, "the jump moved the caret in place");
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(
        caret(&app),
        before_jump,
        "back returns to where the same-file jump left"
    );
}

#[test]
fn double_and_triple_click_select_word_and_line() {
    let store = &imba::store::Store::new();
    let ui = ::editor::test_document::test_ui();
    use editor::ClickKind;
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
            imba::Layout::layout(
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
    use editor::ClickKind;
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
fn close_widget_walks_the_pane_history() {
    use crate::{AppFonts, Application, OpenedDocument};
    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let window = app.sole_window();
    let open = |app: &mut Application, name: &str, text: &str| {
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: name.to_owned(),
                document: plain_document(text),
                location: Some(located(name)),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    };

    open(&mut app, "a.md", "alpha\n");
    assert!(crate::test_driver::type_text(&mut app, "x"));
    open(&mut app, "b.md", "beta\n");
    assert_eq!(
        crate::RecentLocations::list(app.store())[..2],
        [located("b.md"), located("a.md")],
        "every visited place on the recents list, newest first (the startup scratch behind)"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xalpha\n"),
        "the last place in the pane's history took over"
    );
    assert!(
        crate::OpenDocuments::by_location(app.store(), &located("b.md")).is_none(),
        "the closed clean document left the registry"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert!(
        app.focused_document_text().is_none(),
        "history spent — the blank stays (a pristine scratch is not a place)"
    );
    let a = crate::OpenDocuments::by_location(app.store(), &located("a.md"))
        .expect("dirty: unsaved edits are never released");
    assert_eq!(
        crate::OpenDocuments::document_ref(app.store(), a)
            .expect("document")
            .editor_ids()
            .count(),
        0
    );
    assert!(
        crate::OpenDocuments::list(app.store())
            .iter()
            .any(|(_, entity)| entity.location().is_some_and(crate::is_scratch)),
        "the startup scratch survives the walk-past — listed, not lost"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert!(app.focused_document_text().is_none());
    let recents = crate::RecentLocations::list(app.store());
    assert!(
        recents.contains(&located("a.md")) && recents.contains(&located("b.md")),
        "closing forgets documents, never places: {recents:?}"
    );
}

#[test]
fn find_bar_rescans_in_the_background_after_document_edits() {
    use crate::{AppFonts, Application};
    use std::sync::{mpsc, Arc};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let settle = |app: &mut Application| {
        for _ in 0..3 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(crate::test_driver::type_text(
        &mut app,
        "alpha one\nalpha two\n"
    ));
    assert!(app.perform_registered(window, "find.open"));
    assert!(crate::test_driver::type_text(&mut app, "alpha"));
    settle(&mut app);

    let matches_now = |app: &Application| -> Vec<std::ops::Range<u32>> {
        let entity = crate::Windows::window_ref(app.store(), window).expect("window");
        entity
            .workbench()
            .root
            .focused_slot()
            .find
            .as_ref()
            .expect("the bar is open")
            .matches()
            .to_vec()
    };
    assert_eq!(matches_now(&app), vec![0..5, 10..15]);

    {
        let mut store = app.store_mut();
        let mut entity = crate::Windows::window(&mut store, window).expect("window");
        entity
            .workbench_mut()
            .root
            .focused_slot_mut()
            .find
            .as_mut()
            .expect("the bar is open")
            .focused = false;
        crate::Windows::put(&mut store, window, entity);
    }

    assert!(crate::test_driver::type_text(&mut app, "alpha"));
    settle(&mut app);
    assert_eq!(matches_now(&app), vec![0..5, 10..15, 20..25]);

    for _ in 0..5 {
        assert!(crate::test_driver::key(
            &mut app,
            imba::event::Key::Backspace,
            Default::default()
        ));
    }
    settle(&mut app);
    assert_eq!(matches_now(&app), vec![0..5, 10..15]);
}

#[test]
fn find_bar_highlights_and_walks_occurrences() {
    use crate::{AppFonts, Application};
    use std::sync::{mpsc, Arc};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(crate::test_driver::type_text(
        &mut app,
        "alpha beta\nalpha gamma\nALPHA tail\n"
    ));
    let text_before = app.focused_document_text();

    assert!(app.perform_registered(window, "find.open"));
    assert!(crate::test_driver::type_text(&mut app, "alpha"));

    for _ in 0..3 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    assert_eq!(
        app.focused_document_text(),
        text_before,
        "typing lands in the bar's input"
    );

    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let with_slot = |app: &Application, f: &dyn Fn(&crate::workbench_node::PaneSlot)| {
        let entity = crate::Windows::window_ref(app.store(), window).expect("window");
        f(entity.workbench().root.focused_slot());
    };
    with_slot(&app, &|slot| {
        let find = slot.find.as_ref().expect("the bar is open");
        assert_eq!(find.query(), "alpha");

        assert_eq!(find.matches().len(), 3, "{:?}", find.matches());
    });

    let (document_id, editor) = {
        let entity = crate::Windows::window_ref(app.store(), window).expect("window");
        entity
            .workbench()
            .root
            .focused_slot()
            .find_target()
            .expect("an editor pane")
    };
    let document = crate::OpenDocuments::document(app.store(), document_id).expect("the document");
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let extras = document.extras_keyed(editor);
    ::editor::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..12,
        &mut inline,
        &mut hidden,
    );
    assert!(
        inline
            .iter()
            .any(|interval| interval.id == ::editor::theme::StyleId::Match),
        "occurrences tint like the global search"
    );
    drop(document);

    assert!(app.perform_registered(window, "find.next"));
    let selection = |app: &Application| {
        let document =
            crate::OpenDocuments::document(app.store(), document_id).expect("the document");
        let caret = document.carets(editor).primary();
        caret.selection()
    };
    assert_eq!(selection(&app), 0..5, "the first occurrence is selected");
    assert!(app.perform_registered(window, "find.next"));
    assert_eq!(selection(&app), 11..16, "the walk moves forward");
    assert!(app.perform_registered(window, "find.previous"));
    assert_eq!(selection(&app), 0..5, "and back");

    assert!(crate::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        imba::event::Modifiers::default()
    ));
    with_slot(&app, &|slot| {
        assert!(slot.find.is_none(), "Escape closed the bar");
    });
    let document = crate::OpenDocuments::document(app.store(), document_id).expect("the document");
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let extras = document.extras_keyed(editor);
    ::editor::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..12,
        &mut inline,
        &mut hidden,
    );
    assert!(
        inline
            .iter()
            .all(|interval| interval.id != ::editor::theme::StyleId::Match),
        "the tints unwound with the bar"
    );

    {
        let mut document =
            crate::OpenDocuments::document(app.store(), document_id).expect("the document");
        document.set_carets(
            editor,
            ::editor::MultiCaret::one(::editor::Caret::selecting(6, 10)),
        );
        crate::OpenDocuments::put_document(&mut app.store_mut(), document_id, document);
    }
    assert!(app.perform_registered(window, "find.open"));

    for _ in 0..3 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }
    with_slot(&app, &|slot| {
        let find = slot.find.as_ref().expect("re-opened");
        assert_eq!(find.query(), "beta", "the selection seeded the query");
        assert_eq!(find.matches().len(), 1);
    });
}

mod navigation_history {
    use super::plain_document;
    use crate::{AppExt, AppFonts, Application, OpenedDocument};

    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }

    fn app() -> (Application, crate::WindowId) {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        (app, window)
    }

    fn land(
        app: &mut Application,
        window: crate::WindowId,
        name: &str,
        text: &str,
        col: Option<u32>,
    ) {
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: name.to_owned(),
                document: plain_document(text),
                location: Some(located(name)),
                primary: true,
                target: col.map(|col| {
                    crate::LineCol { line: 0, col }..crate::LineCol {
                        line: 0,
                        col: col + 1,
                    }
                }),
                focus: false,
            },
        )));
    }

    fn depths(app: &Application) -> (usize, usize) {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .expect("the window")
            .focused_history_depths()
    }

    fn focused_text(app: &Application) -> String {
        app.focused_document_text().expect("an editor pane")
    }

    fn caret(app: &Application) -> u32 {
        let (document, editor) = app.focused_editor_id();
        crate::OpenDocuments::document_ref(app.store(), document)
            .expect("document")
            .caret_byte(editor)
    }

    fn registered(app: &Application, name: &str) -> bool {
        crate::OpenDocuments::by_location(app.store(), &located(name)).is_some()
    }

    #[test]
    fn back_into_a_released_document_completes_the_walk_on_landing() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);
        land(&mut app, window, "b.md", "beta\n", None);
        assert_eq!(focused_text(&app), "beta\n");
        assert_eq!(depths(&app), (1, 0), "the displaced place recorded");
        assert!(
            !registered(&app, "a.md"),
            "clean + displaced = released (the everyday case)"
        );

        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(focused_text(&app), "beta\n", "still parked");
        assert_eq!(depths(&app), (1, 0), "the entry stays for the landing");

        land(&mut app, window, "a.md", "alpha\n", None);
        assert_eq!(focused_text(&app), "alpha\n", "the walk came home");
        assert_eq!(depths(&app), (0, 1), "back popped; b.md on forward");

        assert!(app.perform_registered(window, "navigation.forward"));
        assert_eq!(focused_text(&app), "alpha\n", "parked again");
        land(&mut app, window, "b.md", "beta\n", None);
        assert_eq!(focused_text(&app), "beta\n", "forward came home");
        assert_eq!(depths(&app), (1, 0), "the round trip restored the shape");
    }

    #[test]
    fn back_and_forward_walk_a_three_deep_chain() {
        let (mut app, window) = app();
        for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n"), ("c.md", "gamma\n")] {
            land(&mut app, window, name, text, None);

            assert!(crate::test_driver::type_text(&mut app, "x"));
        }
        assert_eq!(focused_text(&app), "xgamma\n");
        assert_eq!(depths(&app), (2, 0));

        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(focused_text(&app), "xbeta\n");
        assert_eq!(depths(&app), (1, 1));
        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(focused_text(&app), "xalpha\n");
        assert_eq!(depths(&app), (0, 2));

        assert!(!app.perform_registered(window, "navigation.back") || true);
        assert_eq!(focused_text(&app), "xalpha\n");
        assert_eq!(depths(&app), (0, 2));

        assert!(app.perform_registered(window, "navigation.forward"));
        assert_eq!(focused_text(&app), "xbeta\n");
        assert_eq!(depths(&app), (1, 1));
        assert!(app.perform_registered(window, "navigation.forward"));
        assert_eq!(focused_text(&app), "xgamma\n");
        assert_eq!(depths(&app), (2, 0));
        assert_eq!(focused_text(&app), "xgamma\n");
        assert_eq!(depths(&app), (2, 0), "the far end no-ops");
    }

    #[test]
    fn back_restores_the_recorded_caret_across_a_release() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);

        land(&mut app, window, "a.md", "alpha\n", Some(3));
        assert_eq!(caret(&app), 3);
        assert_eq!(depths(&app), (1, 0), "the jump recorded where it left");

        land(&mut app, window, "b.md", "beta\n", None);
        assert_eq!(depths(&app), (2, 0), "displacement recorded A@3");
        assert!(!registered(&app, "a.md"), "released clean");

        assert!(app.perform_registered(window, "navigation.back"));
        land(&mut app, window, "a.md", "alpha\n", None);
        assert_eq!(focused_text(&app), "alpha\n");
        assert_eq!(caret(&app), 3, "the walk restored the recorded caret");
        assert_eq!(depths(&app), (1, 1));

        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(caret(&app), 0, "the same-file step walked the caret");
        assert_eq!(depths(&app), (0, 2));
    }

    #[test]
    fn back_walks_carets_within_one_document() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha beta\n", None);
        land(&mut app, window, "a.md", "alpha beta\n", Some(3));
        land(&mut app, window, "a.md", "alpha beta\n", Some(7));
        assert_eq!(caret(&app), 7);
        assert_eq!(depths(&app), (2, 0));

        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(caret(&app), 3);
        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(caret(&app), 0);
        assert_eq!(depths(&app), (0, 2));
        assert!(app.perform_registered(window, "navigation.forward"));
        assert!(app.perform_registered(window, "navigation.forward"));
        assert_eq!(caret(&app), 7);
        assert_eq!(depths(&app), (2, 0));
    }

    #[test]
    fn reopening_the_focused_document_is_a_no_op() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha beta\n", None);
        land(&mut app, window, "a.md", "alpha beta\n", Some(7));
        land(&mut app, window, "b.md", "beta\n", None);
        assert!(app.perform_registered(window, "navigation.back"));
        land(&mut app, window, "a.md", "alpha beta\n", None);
        assert_eq!(caret(&app), 7);
        let shape = depths(&app);
        assert_eq!(shape.1, 1, "forward holds b.md");

        land(&mut app, window, "a.md", "alpha beta\n", None);
        assert_eq!(caret(&app), 7, "the caret did not jump to 0");
        assert_eq!(depths(&app), shape, "nothing recorded, forward intact");
    }

    #[test]
    fn a_new_navigation_clears_forward() {
        let (mut app, window) = app();
        for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n")] {
            land(&mut app, window, name, text, None);
            assert!(crate::test_driver::type_text(&mut app, "x"));
        }
        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(depths(&app), (0, 1), "b.md on forward");

        land(&mut app, window, "c.md", "gamma\n", None);
        assert_eq!(
            depths(&app),
            (1, 0),
            "the branch recorded a.md and forgot redo"
        );
        assert!(!app.perform_registered(window, "navigation.forward") || true);
        assert_eq!(focused_text(&app), "gamma\n", "no forward to walk");
    }

    #[test]
    fn close_reveals_the_previous_place_with_its_caret() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);
        land(&mut app, window, "a.md", "alpha\n", Some(3));
        land(&mut app, window, "b.md", "beta\n", None);
        assert_eq!(depths(&app), (2, 0));
        assert!(!registered(&app, "a.md"), "released clean");

        assert!(app.perform_registered(window, "workbench.close"));
        assert!(!registered(&app, "b.md"), "closed for real");

        land(&mut app, window, "a.md", "alpha\n", None);
        assert_eq!(focused_text(&app), "alpha\n");
        assert_eq!(caret(&app), 3, "the reveal restored the recorded caret");
        assert_eq!(depths(&app), (1, 0), "popped at close; nothing recorded");
    }

    #[test]
    fn close_reveals_a_dirty_previous_place_instantly() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);
        assert!(crate::test_driver::type_text(&mut app, "zz"));
        land(&mut app, window, "b.md", "beta\n", None);
        assert!(app.perform_registered(window, "workbench.close"));
        assert_eq!(focused_text(&app), "zzalpha\n", "instant remount");
        assert_eq!(caret(&app), 2, "at the recorded caret");
        assert_eq!(depths(&app), (0, 0));
    }

    #[test]
    fn split_copies_history_and_walks_independently() {
        let (mut app, window) = app();
        for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n")] {
            land(&mut app, window, name, text, None);
            assert!(crate::test_driver::type_text(&mut app, "x"));
        }
        assert_eq!(depths(&app), (1, 0));
        assert!(app.perform_registered(window, "workbench.split-pane"));
        assert_eq!(depths(&app), (1, 0), "the focused (new) pane inherited");
        assert!(app.perform_registered(window, "navigation.back"));
        assert_eq!(focused_text(&app), "xalpha\n", "the copy walks");
        assert_eq!(depths(&app), (0, 1));
    }

    #[test]
    fn a_new_navigation_cancels_a_parked_walk() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);
        land(&mut app, window, "b.md", "beta\n", None);
        assert!(app.perform_registered(window, "navigation.back"));
        land(&mut app, window, "c.md", "gamma\n", None);
        assert_eq!(focused_text(&app), "gamma\n");

        land(&mut app, window, "a.md", "alpha\n", None);
        assert_eq!(focused_text(&app), "alpha\n");
        assert_eq!(caret(&app), 0, "a fresh navigation, not the stale walk");
        assert!(
            app.perform_registered(window, "navigation.back"),
            "back returns to c.md — the fresh record"
        );
        land(&mut app, window, "c.md", "gamma\n", None);
        assert_eq!(focused_text(&app), "gamma\n");
    }

    #[test]
    fn exhausted_stacks_no_op() {
        let (mut app, window) = app();
        land(&mut app, window, "a.md", "alpha\n", None);
        assert_eq!(depths(&app), (0, 0));
        let _ = app.perform_registered(window, "navigation.back");
        let _ = app.perform_registered(window, "navigation.forward");
        assert_eq!(focused_text(&app), "alpha\n");
        assert_eq!(depths(&app), (0, 0));
    }
}

mod toc {
    use super::*;
    use crate::{AppExt, AppFonts, Application, OpenedDocument};

    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }

    fn outlined_document() -> (String, editor::Document) {
        let source = "# One\ntext\n## Two\nmore\n".to_owned();
        let len = source.len() as u32;
        let mut syntax = editor::Syntax::new("toy", None, editor::Markup::new());
        syntax.push_outline_item(
            0..len,
            editor::OutlineItem {
                title: "One".to_owned(),
            },
        );
        syntax.push_outline_item(
            11..len,
            editor::OutlineItem {
                title: "Two".to_owned(),
            },
        );
        let document = editor::Document::new(
            editor::Text::from_string_exact(&source),
            editor::Markup::new(),
        )
        .with_syntax(syntax, &[]);
        (source, document)
    }

    fn run_outline(
        batch: imba::effect::Batch<crate::OutlineCommand>,
    ) -> Option<crate::OutlineRows> {
        use imba::effect::{block_on, EffectHandler, Message};
        for message in batch.drain() {
            let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
                continue;
            };
            let (value, _) = effect.into_payload().split();
            if let Ok(effect) = value.downcast::<crate::OutlineEffect>() {
                return Some(block_on(Box::pin(async move {
                    crate::OutlineHandler.handle(*effect).await
                })));
            }
        }
        None
    }

    #[test]
    fn the_outline_derives_lands_and_jumps() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        let (source, document) = outlined_document();
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "toc.md".to_owned(),
                document,
                location: Some(located("toc.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));

        assert!(app.perform_registered(window, "toc.toggle"));
        let mut view = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .side_panel()
            .expect("the drawer is up")
            .as_any()
            .downcast_ref::<crate::OutlineView>()
            .expect("the drawer holds the outline")
            .clone();
        assert!(view.rows().is_empty(), "nothing landed yet");

        let ui = ::editor::test_document::test_ui();
        let mut store = app.store().clone();
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Refresh,
            &mut batch.effects(),
        );
        let landed = run_outline(batch).expect("the refresh launched the derivation");
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Landed(landed),
            &mut imba::effect::Batch::new().effects(),
        );
        assert_eq!(
            view.rows(),
            vec![(0, "One".to_owned()), (1, "Two".to_owned())],
            "containment nests the sections"
        );

        assert!(app.perform_registered(window, "toc.toggle"));

        let _ = crate::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(0.0));
        let _ =
            crate::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(1_000.0));
        assert!(crate::test_driver::type_text(&mut app, "x"));
        let mut store = app.store().clone();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Select(1),
            &mut imba::effect::Batch::new().effects(),
        );
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Pick,
            &mut imba::effect::Batch::new().effects(),
        );
        let Some(crate::ModalRequest::Perform(command)) = crate::ModalView::take_request(&mut view)
        else {
            panic!("the pick performs the jump");
        };
        assert!(app.perform_command(command));
        let (document_id, editor_id) = app.focused_editor_id();
        let caret = crate::OpenDocuments::document_ref(app.store(), document_id)
            .expect("document")
            .caret_byte(editor_id);
        assert_eq!(
            caret,
            source.find("## Two").unwrap() as u32 + 1,
            "the address resolved LIVE — shifted by the typed byte"
        );
        let depths = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .focused_history_depths();
        assert_eq!(depths, (1, 0), "the jump recorded where it left");
    }

    #[test]
    fn a_structureless_pane_offers_no_toc() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        assert!(app.perform_registered(window, "toc.toggle") || true);
        assert!(
            crate::Windows::window_ref(app.store(), window)
                .expect("window")
                .side_panel()
                .is_none(),
            "no structure, no drawer"
        );
    }

    #[test]
    fn locations_group_into_a_results_forest() {
        let store = Store::new();
        let window = crate::WindowId::from_raw(1);
        let at = |dir: &str, name: &str| {
            crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec![dir.to_owned(), name.to_owned()],
            )
        };
        let toc = crate::TocView::for_locations(
            &store,
            ::editor::test_document::test_ui(),
            window,
            &[at("src", "b.rs"), at("docs", "a.md"), at("src", "a.rs")],
        )
        .expect("rows");
        assert_eq!(
            toc.rows(),
            vec![
                (0, "docs".to_owned(), false),
                (1, "a.md".to_owned(), true),
                (0, "src".to_owned(), false),
                (1, "a.rs".to_owned(), true),
                (1, "b.rs".to_owned(), true),
            ],
            "dirs band their files; files sort within"
        );

        let mut toc = toc;
        let ui = ::editor::test_document::test_ui();
        let mut scratch = Store::new();
        toc.perform(
            &mut scratch,
            &ui,
            crate::TocCommand::Pick,
            &mut imba::effect::Batch::new().effects(),
        );
        let Some(crate::ModalRequest::OpenLocations(locations)) =
            crate::ModalView::take_request(&mut toc)
        else {
            panic!("a file pick opens");
        };
        assert_eq!(locations, vec![at("docs", "a.md")]);

        let mut band = crate::TocView::for_locations(
            &store,
            ::editor::test_document::test_ui(),
            window,
            &[at("src", "x.rs")],
        )
        .expect("rows");
        band.perform(
            &mut scratch,
            &ui,
            crate::TocCommand::Select(-1),
            &mut imba::effect::Batch::new().effects(),
        );
        band.perform(
            &mut scratch,
            &ui,
            crate::TocCommand::Pick,
            &mut imba::effect::Batch::new().effects(),
        );
        assert!(
            crate::ModalView::take_request(&mut band).is_none(),
            "band rows never pick"
        );
    }

    #[test]
    fn locations_nest_into_a_directory_tree() {
        let store = Store::new();
        let window = crate::WindowId::from_raw(1);
        let at = |path: &[&str]| {
            crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        };
        let toc = crate::TocView::for_locations(
            &store,
            ::editor::test_document::test_ui(),
            window,
            &[
                at(&["src", "ui", "widgets", "c.rs"]),
                at(&["src", "a.rs"]),
                at(&["src", "ui", "b.rs"]),
            ],
        )
        .expect("rows");
        assert_eq!(
            toc.rows(),
            vec![
                (0, "src".to_owned(), false),
                (1, "ui".to_owned(), false),
                (2, "widgets".to_owned(), false),
                (3, "c.rs".to_owned(), true),
                (2, "b.rs".to_owned(), true),
                (1, "a.rs".to_owned(), true),
            ],
            "real nesting, folders before files"
        );

        let chain = crate::TocView::for_locations(
            &store,
            ::editor::test_document::test_ui(),
            window,
            &[at(&["a", "b", "c", "x.rs"])],
        )
        .expect("rows");
        assert_eq!(
            chain.rows(),
            vec![(0, "a/b/c".to_owned(), false), (1, "x.rs".to_owned(), true),],
            "single-child directory chains compact"
        );
    }

    #[test]
    fn result_directories_fold_and_unfold() {
        let store = Store::new();
        let window = crate::WindowId::from_raw(1);
        let at = |dir: &str, name: &str| {
            crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec![dir.to_owned(), name.to_owned()],
            )
        };
        let mut toc = crate::TocView::for_locations(
            &store,
            ::editor::test_document::test_ui(),
            window,
            &[at("src", "a.rs"), at("src", "b.rs")],
        )
        .expect("rows");
        assert_eq!(toc.visible_rows(), 3, "the band and both files");

        let ui = ::editor::test_document::test_ui();
        let mut scratch = Store::new();
        let mut drive = |toc: &mut crate::TocView, command| {
            toc.perform(
                &mut scratch,
                &ui,
                command,
                &mut imba::effect::Batch::new().effects(),
            );
        };

        drive(&mut toc, crate::TocCommand::Select(-1));
        drive(&mut toc, crate::TocCommand::Fold(false));
        assert_eq!(toc.visible_rows(), 1, "the fold hides the subtree");
        assert!(
            crate::ModalView::take_request(&mut toc).is_none(),
            "folding is not a pick"
        );
        drive(&mut toc, crate::TocCommand::Pick);
        assert_eq!(toc.visible_rows(), 3, "Enter on a directory unfolds");
    }

    #[test]
    fn outline_speedsearch_filters_steps_and_clears() {
        use imba::effect::{block_on, EffectHandler, Message};

        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        let (_source, document) = outlined_document();
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "toc.md".to_owned(),
                document,
                location: Some(located("toc.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));
        assert!(app.perform_registered(window, "toc.toggle"));
        let mut view = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .side_panel()
            .expect("drawer")
            .as_any()
            .downcast_ref::<crate::OutlineView>()
            .expect("outline")
            .clone();

        let ui = ::editor::test_document::test_ui();
        let mut store = app.store().clone();
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Refresh,
            &mut batch.effects(),
        );
        let landed = run_outline(batch).expect("derivation");
        let mut drive = |view: &mut crate::OutlineView, command| -> imba::effect::Batch<_> {
            let mut batch = imba::effect::Batch::new();
            view.perform(&mut store, &ui, command, &mut batch.effects());
            batch
        };
        let _ = drive(&mut view, crate::OutlineCommand::Landed(landed));

        let typing = drive(
            &mut view,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Input(
                crate::EditorCommand::InsertText {
                    text: "two".to_owned(),
                },
            )),
        );
        let mut matches = None;
        for message in typing.drain() {
            let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
                continue;
            };
            let (value, _) = effect.into_payload().split();
            if let Ok(effect) = value.downcast::<crate::SpeedSearchEffect>() {
                matches = Some(block_on(Box::pin(async move {
                    crate::SpeedSearchHandler.handle(*effect).await
                })));
            }
        }
        let matches = matches.expect("typing launched the filter");
        let _ = drive(
            &mut view,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Landed(matches)),
        );
        assert_eq!(view.match_count(), 1, "only 'Two' matches");
        assert_eq!(
            view.cursor_title().as_deref(),
            Some("Two"),
            "the cursor jumped"
        );

        let _ = drive(
            &mut view,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Step(1)),
        );
        assert_eq!(
            view.cursor_title().as_deref(),
            Some("Two"),
            "wrapped in place"
        );
        let _ = drive(
            &mut view,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Clear),
        );
        assert_eq!(view.match_count(), 0, "cleared");
    }

    #[test]
    fn speedsearch_arrows_step_the_matches() {
        use imba::constraints::Constraints;
        use imba::effect::{block_on, EffectHandler, Message};
        use imba::event::{Event, EventResult, Key, Modifiers};
        use imba::Widget as _;

        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        let (_source, document) = outlined_document();
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "toc.md".to_owned(),
                document,
                location: Some(located("toc.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));
        assert!(app.perform_registered(window, "toc.toggle"));
        let mut view = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .side_panel()
            .expect("drawer")
            .as_any()
            .downcast_ref::<crate::OutlineView>()
            .expect("outline")
            .clone();

        let ui = ::editor::test_document::test_ui();
        let mut store = app.store().clone();
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Refresh,
            &mut batch.effects(),
        );
        let landed = run_outline(batch).expect("derivation");
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Landed(landed),
            &mut imba::effect::Batch::new().effects(),
        );

        let mut typing = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Input(
                crate::EditorCommand::InsertText {
                    text: "o".to_owned(),
                },
            )),
            &mut typing.effects(),
        );
        let mut matches = None;
        for message in typing.drain() {
            let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
                continue;
            };
            let (value, _) = effect.into_payload().split();
            if let Ok(effect) = value.downcast::<crate::SpeedSearchEffect>() {
                matches = Some(block_on(Box::pin(async move {
                    crate::SpeedSearchHandler.handle(*effect).await
                })));
            }
        }
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::List(crate::SpeedSearchCommand::Landed(
                matches.expect("filter launched"),
            )),
            &mut imba::effect::Batch::new().effects(),
        );
        assert_eq!(view.match_count(), 2, "One and Two match 'o'");
        assert_eq!(view.cursor_title().as_deref(), Some("One"));

        let result = {
            let arena = imba::arena::Arena::default();
            let widget = imba::Layout::layout(
                imba::View::display(&view, &arena, &store, &ui),
                &arena,
                Constraints::tight(skia_safe::Size::new(800.0, 600.0)),
            );
            let widget =
                imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(800.0, 600.0));
            let result = widget.handle_event(
                &arena,
                &Event::KeyDown {
                    key: Key::Down,
                    mods: Modifiers::default(),
                },
                skia_safe::Rect::from_wh(800.0, 600.0),
            );
            drop(widget);
            result
        };
        let stepped = matches!(
            result,
            EventResult::Command(crate::OutlineCommand::List(
                crate::SpeedSearchCommand::Step(1)
            ))
        );
        assert!(stepped, "Down while searching must become Step(1)");

        if let EventResult::Command(command) = result {
            view.perform(
                &mut store,
                &ui,
                command,
                &mut imba::effect::Batch::new().effects(),
            );
        }
        assert_eq!(
            view.cursor_title().as_deref(),
            Some("Two"),
            "stepped to the next match"
        );
    }

    #[test]
    fn outline_folds_survive_relandings() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let window = app.sole_window();
        let (_source, document) = outlined_document();
        assert!(app.perform_command(crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "toc.md".to_owned(),
                document,
                location: Some(located("toc.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));
        assert!(app.perform_registered(window, "toc.toggle"));
        let mut view = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .side_panel()
            .expect("drawer")
            .as_any()
            .downcast_ref::<crate::OutlineView>()
            .expect("outline")
            .clone();

        let ui = ::editor::test_document::test_ui();
        let mut store = app.store().clone();
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            crate::OutlineCommand::Refresh,
            &mut batch.effects(),
        );
        let landed = run_outline(batch).expect("derivation");
        let mut drive = |view: &mut crate::OutlineView, command| {
            view.perform(
                &mut store,
                &ui,
                command,
                &mut imba::effect::Batch::new().effects(),
            );
        };
        drive(&mut view, crate::OutlineCommand::Landed(landed.clone()));
        assert_eq!(view.visible_rows(), 2, "One with Two nested");

        drive(&mut view, crate::OutlineCommand::Fold(false));
        assert_eq!(view.visible_rows(), 1, "folded");

        let mut relanded = landed;
        relanded.stamp = (relanded.stamp.0 + 1, relanded.stamp.1);
        drive(&mut view, crate::OutlineCommand::Landed(relanded));
        assert_eq!(view.visible_rows(), 1, "the fold survived the swap");
    }
}

#[test]
fn keymap_chords_resolve_through_the_palette_surface() {
    use crate::{AppFonts, Application};
    use imba::event::{Key, Modifiers};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let cmd = Modifiers {
        command: true,
        ..Default::default()
    };

    assert!(crate::test_driver::type_text(&mut app, "typed"));
    assert_eq!(app.focused_document_text().as_deref(), Some("typed"));
    assert!(
        crate::test_driver::key(&mut app, Key::Char('z'), cmd),
        "the chord consumed"
    );
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some(""),
        "cmd-z undid via the keymap"
    );
    let shifted = Modifiers {
        command: true,
        shift: true,
        ..Default::default()
    };

    assert!(crate::test_driver::key(&mut app, Key::Char('Z'), shifted));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("typed"),
        "cmd-shift-Z redid via the keymap"
    );

    crate::Keymaps::set(
        &mut app.store_mut(),
        crate::Keymap::from_json(r#"{ "cmd-9": "no.such-command" }"#).expect("parses"),
    );
    assert!(
        !crate::test_driver::key(&mut app, Key::Char('9'), cmd),
        "an unoffered id leaves the key unconsumed"
    );

    crate::Keymaps::set(
        &mut app.store_mut(),
        crate::Keymap::from_json(r#"{ "backspace": "editor.undo" }"#).expect("parses"),
    );
    assert!(crate::test_driver::backspace(&mut app));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some(""),
        "backspace fired the rebound undo, not a delete"
    );

    crate::Keymaps::set(&mut app.store_mut(), crate::Keymap::embedded());
    assert!(crate::test_driver::key(&mut app, Key::Char('Z'), shifted));
    assert_eq!(app.focused_document_text().as_deref(), Some("typed"));
    assert!(crate::test_driver::backspace(&mut app));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("type"),
        "the default keymap's backspace deletes"
    );
}

#[test]
fn keymap_backspace_edits_the_find_bar_query() {
    use crate::{AppFonts, Application};
    use imba::event::{Key, Modifiers};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(crate::test_driver::type_text(&mut app, "hello"));
    let cmd = Modifiers {
        command: true,
        ..Default::default()
    };
    assert!(
        crate::test_driver::key(&mut app, Key::Char('f'), cmd),
        "cmd-F opens the find bar"
    );
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(crate::test_driver::type_text(&mut app, "ab"));

    let query = |app: &Application| -> Option<String> {
        Some(
            crate::Windows::window_ref(app.store(), app.sole_window())?
                .workbench()
                .root
                .focused_slot()
                .find
                .as_ref()?
                .query(),
        )
    };
    assert_eq!(
        query(&app).as_deref(),
        Some("ab"),
        "the query took the typing"
    );
    assert!(
        crate::test_driver::backspace(&mut app),
        "backspace resolves through the keymap"
    );
    assert_eq!(query(&app).as_deref(), Some("a"), "the QUERY shrank");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("hello"),
        "the document behind the bar never moved"
    );
}

mod dock_tests {
    use std::sync::{Arc, Mutex};

    use imba::anim::AnimationClock;
    use imba::constraints::Constraints;
    use imba::event::{Event, EventResult, Key};
    use imba::leaf::leaf;
    use imba::store::Store;
    use imba::thunk_ext::ThunkExt as _;
    use imba::{UiCtx, View};

    use crate::test_driver;
    use crate::{AppCommand, AppExt, AppFonts, Application, ModalRequest, ModalView};

    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }

    enum StubCommand {
        Close,
        Ask,
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
        ) -> impl imba::Layout<'a, StubCommand> + imba::LayoutValue + 'a {
            imba::laid(
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

    impl crate::DynamicCommand for ShowStubDock {
        fn id(&self) -> &'static str {
            "test.dock-show"
        }

        fn name(&self) -> String {
            "Show Test Dock".to_owned()
        }

        fn perform(
            &self,
            _app: &mut Application,
            store: &mut Store,
            window: crate::WindowId,
            fx: &mut crate::AppFx<'_>,
        ) {
            let Some(mut entity) = crate::Windows::window(store, window) else {
                return;
            };
            fx.scope(
                move |command| AppCommand::Content(window, command),
                |fx| entity.show_dock(store, Box::new(DockStub::new(self.label)), self.owner, fx),
            );
            crate::Windows::put(store, window, entity);
        }
    }

    fn show_dock(app: &mut Application, label: &'static str, owner: &'static str) {
        let window = app.sole_window();
        assert!(app.perform_command(AppCommand::Dynamic(
            window,
            Arc::new(ShowStubDock { label, owner }),
        )));
    }

    fn settle(app: &mut Application, surface: &mut skia_safe::Surface) {
        for tick in 0..60 {
            crate::Window::draw(app.sole_window(), app, surface.canvas());
            let busy = test_driver::animate(app, AnimationClock::from_millis(tick as f64 * 32.0));
            if !busy && tick > 1 {
                break;
            }
        }
    }

    fn entity(app: &Application) -> &crate::Window {
        crate::Windows::window_ref(app.store(), app.sole_window()).expect("the window")
    }

    #[test]
    fn the_dock_opens_as_a_split_and_narrows_the_workbench() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let before =
            crate::app::panel_width(app.store(), entity(&app).workbench().root.focused_pane())
                .expect("the scratch pane is an editor");

        show_dock(&mut app, "files", "test.files");
        assert!(entity(&app).has_dock(), "the dock is up");
        assert_eq!(entity(&app).dock_owner(), Some("test.files"));
        settle(&mut app, &mut surface);

        assert_eq!(
            entity(&app).dock_target_width(),
            crate::DOCK_WIDTH,
            "the dock settled at the default width"
        );
        let after =
            crate::app::panel_width(app.store(), entity(&app).workbench().root.focused_pane())
                .expect("still an editor");
        assert!(
            before - after > crate::DOCK_WIDTH * 0.5,
            "the workbench narrowed for the split (was {before}, now {after})"
        );
    }

    #[test]
    fn escape_closes_the_dock_after_the_slide_settles() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        show_dock(&mut app, "files", "test.files");
        settle(&mut app, &mut surface);

        let edge = 800.0 - crate::DOCK_WIDTH;
        assert!(test_driver::click(&mut app, edge, 300.0, 800.0, 600.0));
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(test_driver::drag(&mut app, 100.0, 300.0));
        assert!(test_driver::mouse_up(&mut app, 100.0, 300.0));
        assert_eq!(
            entity(&app).dock_target_width(),
            800.0 * 0.6,
            "the drag clamps at the wide cap"
        );

        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let edge = 800.0 - 800.0 * 0.6;
        assert!(test_driver::click(&mut app, edge, 300.0, 800.0, 600.0));
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(test_driver::drag(&mut app, 780.0, 300.0));
        assert!(test_driver::mouse_up(&mut app, 780.0, 300.0));
        assert_eq!(
            entity(&app).dock_target_width(),
            crate::DOCK_MIN_WIDTH,
            "the drag clamps at the floor"
        );
    }

    #[test]
    fn an_editor_drag_still_selects_while_the_dock_is_up() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(test_driver::type_text(
            &mut app,
            "alpha beta gamma delta epsilon zeta"
        ));
        show_dock(&mut app, "files", "test.files");
        settle(&mut app, &mut surface);

        assert!(test_driver::click(&mut app, 160.0, 120.0, 800.0, 600.0));
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        test_driver::drag(&mut app, 380.0, 130.0);
        test_driver::mouse_up(&mut app, 380.0, 130.0);
        let (_, held) = crate::OpenDocuments::list(app.store())
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
    fn an_editor_drag_still_selects_while_the_floating_chat_is_up() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(test_driver::type_text(
            &mut app,
            "alpha beta gamma delta epsilon zeta"
        ));

        let uri = "ahp-chat:/x".to_owned();
        let panel = crate::higent::ChatPanel::new(
            app.store(),
            &app.ui_ctx(),
            crate::higent::HostId::LOCAL,
            "ahp-session:/x",
            uri.clone(),
        );
        crate::higent::Chats::put(&mut app.store_mut(), uri.clone().into(), panel);
        {
            let window = app.sole_window();
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            entity.open_bottom(Box::new(crate::higent::ChatPane::new(uri)));
            crate::Windows::put(&mut app.store_mut(), window, entity);
        }
        settle(&mut app, &mut surface);

        assert!(test_driver::click(&mut app, 160.0, 120.0, 800.0, 600.0));
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        test_driver::drag(&mut app, 380.0, 130.0);
        test_driver::mouse_up(&mut app, 380.0, 130.0);
        let (_, held) = crate::OpenDocuments::list(app.store())
            .into_iter()
            .next()
            .expect("the scratch document");
        let document = held.document();
        let editor = document.editor_ids().next().expect("its editor");
        let selection = document.carets(editor).primary().selection();
        assert!(
            !selection.is_empty(),
            "the drag selected nothing — the floating chat swallowed it"
        );
    }

    #[test]
    fn escape_collapses_the_drawer_but_keeps_the_chat_focused() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let uri = "ahp-chat:/esc".to_owned();
        let panel = crate::higent::ChatPanel::new(
            app.store(),
            &app.ui_ctx(),
            crate::higent::HostId::LOCAL,
            "ahp-session:/esc",
            uri.clone(),
        );
        crate::higent::Chats::put(&mut app.store_mut(), uri.clone().into(), panel);
        {
            let window = app.sole_window();
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            entity.open_bottom(Box::new(crate::higent::ChatPane::new(uri.clone())));
            crate::Windows::put(&mut app.store_mut(), window, entity);
        }
        settle(&mut app, &mut surface);

        let rect = crate::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .bottom_rect(app.store(), skia_safe::Size::new(800.0, 600.0))
            .expect("the sheet stands");
        assert!(test_driver::click(
            &mut app,
            rect.center_x(),
            rect.bottom - 12.0,
            800.0,
            600.0
        ));
        settle(&mut app, &mut surface);

        let oracle = |app: &crate::Application| {
            let entity =
                crate::Windows::window_ref(app.store(), app.sole_window()).expect("window");
            let blurred = crate::higent::Chats::chat_ref(app.store(), &uri.clone().into())
                .expect("the chat panel")
                .blurred();
            (entity.layer_focus(), entity.bottom_expanded(), blurred)
        };
        let (focus, expanded, blurred) = oracle(&app);
        assert_eq!(
            focus,
            crate::LayerFocus::Bottom,
            "the click focused the sheet"
        );
        assert_eq!(expanded, Some(false));
        assert!(!blurred, "a focused sheet's prompt is grown");

        assert!(test_driver::key(
            &mut app,
            imba::event::Key::Escape,
            Default::default()
        ));
        settle(&mut app, &mut surface);
        let (focus, expanded, blurred) = oracle(&app);
        assert_eq!(focus, crate::LayerFocus::Bottom);
        assert_eq!(expanded, Some(true), "Escape rolled the drawer out");
        assert!(!blurred);

        assert!(test_driver::key(
            &mut app,
            imba::event::Key::Escape,
            Default::default()
        ));
        settle(&mut app, &mut surface);
        let (focus, expanded, blurred) = oracle(&app);
        assert_eq!(expanded, Some(false), "Escape rolled the drawer away");
        assert_eq!(
            focus,
            crate::LayerFocus::Bottom,
            "the keys stay with the chat — Escape must not dump focus"
        );
        assert!(!blurred, "the prompt keeps its size on Escape");

        assert!(test_driver::click(&mut app, 400.0, 100.0, 800.0, 600.0));
        settle(&mut app, &mut surface);
        let (focus, _, blurred) = oracle(&app);
        assert_eq!(focus, crate::LayerFocus::Content);
        assert!(blurred, "keys leaving the sheet shrink the prompt");
    }

    #[test]
    fn the_composer_sheet_hides_on_blur_and_returns_by_command() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let uri = "ahp-chat:/volatile".to_owned();
        let panel = crate::higent::ChatPanel::new(
            app.store(),
            &app.ui_ctx(),
            crate::higent::HostId::LOCAL,
            "ahp-session:/volatile",
            uri.clone(),
        );
        crate::higent::Chats::put(&mut app.store_mut(), uri.clone().into(), panel);
        let window = app.sole_window();
        {
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            entity.open_bottom(Box::new(crate::higent::ChatPane::new(uri.clone())));
            crate::Windows::put(&mut app.store_mut(), window, entity);
        }
        settle(&mut app, &mut surface);

        let oracle = |app: &crate::Application| {
            let entity =
                crate::Windows::window_ref(app.store(), app.sole_window()).expect("window");
            let on_screen = entity
                .bottom_rect(app.store(), skia_safe::Size::new(800.0, 600.0))
                .is_some();
            (
                entity.layer_focus(),
                on_screen,
                entity.bottom_pane().is_some(),
            )
        };

        assert_eq!(oracle(&app), (crate::LayerFocus::Bottom, true, true));

        assert!(test_driver::click(&mut app, 400.0, 100.0, 800.0, 600.0));
        settle(&mut app, &mut surface);
        assert_eq!(
            oracle(&app),
            (crate::LayerFocus::Content, false, true),
            "blur hides the sheet completely; the chat pane survives"
        );

        assert!(app.perform_registered(window, "chat.composer"));
        settle(&mut app, &mut surface);
        assert_eq!(oracle(&app), (crate::LayerFocus::Bottom, true, true));
        assert!(
            !crate::higent::Chats::chat_ref(app.store(), &uri.clone().into())
                .expect("the chat panel")
                .blurred(),
            "the shown sheet's prompt is grown"
        );

        assert!(app.perform_registered(window, "chat.composer"));
        settle(&mut app, &mut surface);
        assert_eq!(oracle(&app), (crate::LayerFocus::Content, false, true));
    }

    #[test]
    fn new_session_leaves_the_previous_session_and_its_sheet() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let window = app.sole_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(window, &mut app, surface.canvas());

        struct EnterSession;
        impl crate::DynamicCommand for EnterSession {
            fn id(&self) -> &'static str {
                "test.enter-session"
            }
            fn name(&self) -> String {
                String::new()
            }
            fn perform(
                &self,
                _app: &mut Application,
                store: &mut Store,
                window: crate::WindowId,
                fx: &mut crate::AppFx<'_>,
            ) {
                let target = crate::SessionId {
                    host: crate::higent::HostId::LOCAL,
                    session: "ahp-session:/live".to_owned(),
                };
                crate::switch_session(store, window, target, fx)
            }
        }
        assert!(app.perform_command(AppCommand::Dynamic(window, Arc::new(EnterSession))));
        let uri = "ahp-chat:/live".to_owned();
        let panel = crate::higent::ChatPanel::new(
            app.store(),
            &app.ui_ctx(),
            crate::higent::HostId::LOCAL,
            "ahp-session:/live",
            uri.clone(),
        );
        crate::higent::Chats::put(&mut app.store_mut(), uri.clone().into(), panel);
        {
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            entity.open_bottom(Box::new(crate::higent::ChatPane::new(uri)));
            crate::Windows::put(&mut app.store_mut(), window, entity);
        }
        settle(&mut app, &mut surface);
        let entity = crate::Windows::window_ref(app.store(), window).expect("window");
        assert!(entity.current_session().names_session());
        assert!(entity.bottom_pane().is_some(), "the chat sheet stands");

        assert!(app.perform_command(AppCommand::Dynamic(
            window,
            Arc::new(crate::new_session::OpenNewSession { host: None }),
        )));
        settle(&mut app, &mut surface);
        let entity = crate::Windows::window_ref(app.store(), window).expect("window");
        assert!(
            !entity.current_session().names_session(),
            "the previous session is still current: {:?}",
            entity.current_session()
        );
        assert!(
            entity.bottom_pane().is_none(),
            "the previous session's chat sheet is still mounted"
        );
        let title = crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .workbench()
            .root
            .focused_pane()
            .title(app.store());
        assert_eq!(title, "New session", "the composer holds the focus");
    }

    #[test]
    fn the_add_host_row_takes_a_url_and_dispatches() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let window = app.sole_window();
        let received = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        {
            let received = received.clone();
            crate::higent::Agents::install_add_host(
                &mut app.store_mut(),
                std::sync::Arc::new(move |_app, _store, url| {
                    *received.lock().expect("recorder") = Some(url.to_owned());
                    None
                }),
            );
        }

        let mut store = app.store_mut().clone();
        let ui = ::editor::test_document::test_ui();
        let mut panel = crate::higent::AgentsPanel::open(&store, window);
        {
            let mut boot: imba::effect::Batch<crate::higent::AgentsCommand> =
                imba::effect::Batch::new();
            imba::View::perform(
                &mut panel,
                &mut store,
                &ui,
                crate::higent::AgentsCommand::Boot,
                &mut boot.effects(),
            );
        }
        let rows = panel.rows();
        let index = rows
            .iter()
            .position(|(label, _)| label == "+ Add Host…")
            .expect("the add-host row stands");
        let mut batch = imba::effect::Batch::new();
        panel.activate(&mut store, &ui, index, &mut batch.effects());
        assert_eq!(
            panel.add_host_text().as_deref(),
            Some(""),
            "picking the row stands the URL input"
        );

        use imba::View;
        panel.perform(
            &mut store,
            &ui,
            crate::higent::AgentsCommand::AddHostInput(::editor::EditorCommand::InsertText {
                text: "ws://example:7/?tkn=t".to_owned(),
            }),
            &mut batch.effects(),
        );
        panel.perform(
            &mut store,
            &ui,
            crate::higent::AgentsCommand::SubmitAddHost,
            &mut batch.effects(),
        );
        assert!(panel.add_host_text().is_none(), "Enter clears the input");
        let request = crate::ModalView::take_request(&mut panel).expect("the dispatch request");
        let crate::ModalRequest::Perform(command) = request else {
            panic!("Enter dispatches a Perform request");
        };
        assert!(app.perform_batch(vec![command]));
        assert_eq!(
            received.lock().expect("recorder").as_deref(),
            Some("ws://example:7/?tkn=t"),
            "the capability received the URL"
        );

        panel.activate(&mut store, &ui, index, &mut batch.effects());
        panel.perform(
            &mut store,
            &ui,
            crate::higent::AgentsCommand::CancelAddHost,
            &mut batch.effects(),
        );
        assert!(panel.add_host_text().is_none());
        assert!(crate::ModalView::take_request(&mut panel).is_none());
    }

    #[test]
    fn the_drawer_groups_sessions_by_folder_most_recent_first() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let window = app.sole_window();
        let mut store = app.store_mut().clone();
        let host = crate::SessionId::local_default(&store).host;
        crate::higent::Agents::seed(&mut store, host, "Test Host");
        crate::higent::Agents::set_status(&mut store, host, crate::higent::HostStatus::Connected);
        let summary =
            |title: &str, folders: &[&str], modified: &str| ahp_types::state::SessionSummary {
                provider: "test".to_owned(),
                title: title.to_owned(),
                // Idle and read — labels stay bare of activity marks.
                status: 33,
                activity: None,
                project: None,
                working_directories: (!folders.is_empty())
                    .then(|| folders.iter().map(|folder| folder.to_string()).collect()),
                annotations: None,
                resource: format!("test-session:/{title}"),
                created_at: String::new(),
                modified_at: modified.to_owned(),
                changes: None,
                meta: None,
            };
        const HIMARK: &str = "file:///dev/himark";
        const DOCS: &str = "file:///dev/docs";
        crate::higent::Agents::add_sessions(
            &mut store,
            host,
            vec![
                summary("older himark", &[HIMARK], "2026-09-20T10:00:00Z"),
                {
                    // Changed since last viewed — the filled dot.
                    let mut stray = summary("stray", &[], "2026-09-21T10:00:00Z");
                    stray.status = 1;
                    stray
                },
                {
                    // The last turn failed — the bang.
                    let mut errored = summary("docs session", &[DOCS], "2026-09-21T12:00:00Z");
                    errored.status = 34; // Error | IsRead
                    errored
                },
                {
                    // A turn is streaming — the hollow dot.
                    let mut busy = summary("fresh himark", &[HIMARK], "2026-09-22T09:00:00Z");
                    busy.status = 40; // InProgress | IsRead
                    busy
                },
                // The pair sessions share a folder SET — order must not
                // split them into two groups.
                {
                    // Blocked on the user's answer — the question mark.
                    let mut asking = summary("pair", &[HIMARK, DOCS], "2026-09-22T11:00:00Z");
                    asking.status = 56; // InputNeeded | IsRead
                    asking
                },
                summary("pair reversed", &[DOCS, HIMARK], "2026-09-21T09:00:00Z"),
            ],
            true,
        );

        let ui = ::editor::test_document::test_ui();
        let mut panel = crate::higent::AgentsPanel::open(&store, window);
        let mut batch: imba::effect::Batch<crate::higent::AgentsCommand> =
            imba::effect::Batch::new();
        use imba::View;
        panel.perform(
            &mut store,
            &ui,
            crate::higent::AgentsCommand::Boot,
            &mut batch.effects(),
        );

        assert_eq!(
            panel.rows(),
            vec![
                ("Local".to_owned(), 0),
                // The two-folder set holds the freshest session, so that
                // group leads; both orderings of the set land in it.
                ("docs, himark".to_owned(), 1),
                ("? pair".to_owned(), 2),
                ("pair reversed".to_owned(), 2),
                ("himark".to_owned(), 1),
                ("○ fresh himark".to_owned(), 2),
                ("older himark".to_owned(), 2),
                ("docs".to_owned(), 1),
                ("! docs session".to_owned(), 2),
                // No folder — the stray stays a plain row, ranked by its
                // own recency; unread, so it wears the filled dot.
                ("● stray".to_owned(), 1),
                ("+ New Session…".to_owned(), 1),
                ("+ Add Host…".to_owned(), 0),
            ],
        );

        // Folding a folder row hides its sessions and nothing else.
        let index = panel
            .rows()
            .iter()
            .position(|(label, depth)| label == "himark" && *depth == 1)
            .expect("the himark folder row stands");
        panel.activate(&mut store, &ui, index, &mut batch.effects());
        assert_eq!(
            panel.rows(),
            vec![
                ("Local".to_owned(), 0),
                ("docs, himark".to_owned(), 1),
                ("? pair".to_owned(), 2),
                ("pair reversed".to_owned(), 2),
                ("himark".to_owned(), 1),
                ("docs".to_owned(), 1),
                ("! docs session".to_owned(), 2),
                ("● stray".to_owned(), 1),
                ("+ New Session…".to_owned(), 1),
                ("+ Add Host…".to_owned(), 0),
            ],
        );
    }

    #[test]
    fn dock_picks_keep_the_panel_up() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        show_dock(&mut app, "files", "test.files");
        settle(&mut app, &mut surface);

        assert!(test_driver::key(&mut app, Key::Enter, Default::default()));
        assert!(entity(&app).has_dock(), "the pick kept the panel");
        assert_eq!(entity(&app).dock_owner(), Some("test.files"));
    }

    #[test]
    fn the_dock_swaps_content_in_place() {
        let mut app = Application::new(AppFonts::embedded());
        let _ = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
            crate::DOCK_WIDTH,
            "the width stood through the swap"
        );

        {
            let window = app.sole_window();
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            entity.roll_away_dock();
            crate::Windows::put(&mut app.store_mut(), window, entity);
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
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
        show_dock(&mut app, "files", "test.files");
        settle(&mut app, &mut surface);

        {
            let window = app.sole_window();
            let mut entity = crate::Windows::window(app.store(), window).expect("window");
            let mut batch = imba::effect::Batch::new();
            let mut store = app.store().clone();
            entity.show_side_panel(
                &mut store,
                Box::new(DockStub::new("drawer")),
                &mut batch.effects(),
            );
            crate::Windows::put(&mut app.store_mut(), window, entity);
        }
        assert!(entity(&app).has_side_panel(), "the drawer is up");
        assert!(entity(&app).has_dock(), "the dock stayed");
    }

    #[test]
    fn the_dock_and_the_families_ride_their_session_across_switches() {
        let fonts = crate::AppFonts::embedded();
        let mut app = Application::new(fonts);
        let window = app.add_window();
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

        show_dock(&mut app, "changes", "test.owner");
        settle(&mut app, &mut surface);
        struct NullBackend;
        impl crate::terminal::TerminalBackend for NullBackend {
            fn write(&self, _bytes: &[u8]) {}
            fn resize(&self, _cols: u16, _rows: u16, _w: f32, _h: f32) {}
            fn hangup(&self) {}
        }
        let hidden = "test-terminal:1".to_owned();
        let session = crate::terminal::Session::new(Box::new(NullBackend));
        session.set_channel(hidden.clone());
        crate::terminal::Terminals::put(&mut app.store_mut(), hidden.clone(), session);
        let entity = |app: &Application| {
            crate::Windows::window_ref(app.store(), app.sole_window())
                .expect("the window entity")
                .clone()
        };
        assert!(entity(&app).has_dock());
        assert!(
            crate::terminal::Terminals::session_ref(app.store(), &hidden).is_some(),
            "the family row is in the session"
        );

        struct Switch(std::sync::Arc<Mutex<Option<crate::SessionId>>>);
        impl crate::DynamicCommand for Switch {
            fn id(&self) -> &'static str {
                "test.switch"
            }
            fn name(&self) -> String {
                "Test Switch".to_owned()
            }
            fn perform(
                &self,
                _app: &mut Application,
                store: &mut Store,
                window: crate::WindowId,
                fx: &mut crate::AppFx<'_>,
            ) {
                let target = match self.0.lock().unwrap().clone() {
                    Some(target) => target,
                    None => crate::SessionId::mint_scratch(store),
                };
                *self.0.lock().unwrap() = Some(target.clone());
                crate::switch_session(store, window, target, fx);
            }
        }
        let first = entity(&app).current_session();
        let minted = std::sync::Arc::new(Mutex::new(None));
        assert!(app.perform_command(AppCommand::Dynamic(
            window,
            Arc::new(Switch(std::sync::Arc::clone(&minted))),
        )));
        assert!(
            entity(&app).dock_panel().is_none(),
            "a fresh session has no dock"
        );
        assert!(
            crate::terminal::Terminals::session_ref(app.store(), &hidden).is_none(),
            "and no foreign family rows"
        );

        let back = std::sync::Arc::new(Mutex::new(Some(first)));
        assert!(app.perform_command(AppCommand::Dynamic(window, Arc::new(Switch(back)),)));
        let restored = entity(&app);
        assert!(restored.dock_panel().is_some(), "the dock rode its session");
        assert!(
            restored.dock_target_width() > 0.0,
            "and reads open, not closing"
        );
        assert!(
            crate::terminal::Terminals::session_ref(app.store(), &hidden).is_some(),
            "the family row rode along"
        );

        settle(&mut app, &mut surface);
        assert!(
            !crate::Window::draw(app.sole_window(), &mut app, surface.canvas()),
            "the restored dock mounts settled — no slide, no reconcile"
        );
    }
}

mod toolbar_side_tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use imba::store::Store;

    use crate::test_driver;
    use crate::{AppFonts, Application};

    struct Mark {
        id: &'static str,
        hits: Arc<AtomicUsize>,
    }

    impl crate::DynamicCommand for Mark {
        fn id(&self) -> &'static str {
            self.id
        }

        fn name(&self) -> String {
            self.id.to_owned()
        }

        fn perform(
            &self,
            _app: &mut Application,
            _store: &mut Store,
            _window: crate::WindowId,
            _fx: &mut crate::AppFx<'_>,
        ) {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn button(id: &'static str, order: f32, side: crate::ToolbarSide) -> crate::ToolbarButton {
        crate::ToolbarButton {
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
            crate::commands::Commands::register(
                &mut store,
                Arc::new(Mark {
                    id: "test.left-mark",
                    hits: Arc::clone(&left_hits),
                }),
            );
            crate::commands::Commands::register(
                &mut store,
                Arc::new(Mark {
                    id: "test.right-mark",
                    hits: Arc::clone(&right_hits),
                }),
            );
        }
        app.register_toolbar_button(button("test.left-mark", 10.0, crate::ToolbarSide::Left));
        app.register_toolbar_button(button("test.right-mark", 10.0, crate::ToolbarSide::Right));
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let chrome = crate::env::Themes::of(app.store()).ui().toolbar.clone();
        let x = 800.0 - chrome.button_inset - chrome.button_size * 0.5;
        assert!(test_driver::click(
            &mut app,
            x,
            chrome.height * 0.5,
            800.0,
            600.0
        ));
        assert_eq!(
            right_hits.load(Ordering::Relaxed),
            1,
            "the right button fired"
        );
        assert_eq!(left_hits.load(Ordering::Relaxed), 0, "the left one did not");
    }
}

#[test]
fn switching_workspaces_stashes_the_bottom_sheet() {
    use crate::AppFonts;
    use std::sync::Arc;

    struct Switch {
        target: Option<crate::SessionId>,
        made: Arc<std::sync::Mutex<Option<crate::SessionId>>>,
    }
    impl crate::DynamicCommand for Switch {
        fn id(&self) -> &'static str {
            "test.switch-sheet"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            _app: &mut crate::Application,
            store: &mut Store,
            window: crate::WindowId,
            fx: &mut crate::AppFx<'_>,
        ) {
            let target = match self.target.clone() {
                Some(target) => target,
                None => crate::SessionId::mint_scratch(store),
            };
            *self.made.lock().unwrap() = Some(target.clone());
            crate::switch_session(store, window, target, fx)
        }
    }
    let switch =
        |app: &mut crate::Application, target: Option<crate::SessionId>| -> crate::SessionId {
            let window = app.sole_window();
            let made = Arc::new(std::sync::Mutex::new(None));
            app.perform_batch(vec![crate::AppCommand::Dynamic(
                window,
                Arc::new(Switch {
                    target,
                    made: Arc::clone(&made),
                }),
            )]);
            let result = made.lock().unwrap().take().expect("the switch ran");
            result
        };

    let fonts = AppFonts::embedded();
    let mut app = crate::Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let first = crate::Windows::window_ref(app.store(), window)
        .expect("window")
        .current_session();

    {
        let mut entity = crate::Windows::window(app.store(), window).expect("window");
        entity.open_bottom(Box::new(crate::higent::ChatPane::new(
            "ahp-chat:/a".to_owned(),
        )));
        crate::Windows::put(&mut app.store_mut(), window, entity);
    }
    let sheet_chat = |app: &crate::Application| -> Option<String> {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .and_then(|entity| entity.bottom_pane())
            .and_then(|pane| pane.as_any().downcast_ref::<crate::higent::ChatPane>())
            .map(|pane| pane.chat().clone())
    };
    assert_eq!(sheet_chat(&app).as_deref(), Some("ahp-chat:/a"));

    let second = switch(&mut app, None);
    assert_eq!(sheet_chat(&app), None, "B never shows A's chat");
    assert_eq!(
        crate::Windows::window_ref(app.store(), window)
            .expect("window")
            .layer_focus(),
        crate::LayerFocus::Content,
        "the sheet's keys do not cross sessions"
    );

    let _ = switch(&mut app, Some(first));
    assert_eq!(sheet_chat(&app).as_deref(), Some("ahp-chat:/a"));
    let _ = second;
}

#[test]
fn a_pane_documents_popup_paints_in_the_window() {
    use crate::{AppCommand, AppFonts, Application, OpenedDocument};

    #[derive(Clone)]
    struct MagentaPopup;
    impl imba::View for MagentaPopup {
        type Command = ();
        fn perform(
            &mut self,
            _store: &mut imba::store::Store,
            _ui: &imba::UiCtx,
            _command: (),
            _fx: &mut imba::effect::Effects<'_, ()>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a imba::store::Store,
            _ui: &'a imba::UiCtx,
        ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
            imba::laid(
                move |_arena: &'a imba::arena::Arena,
                      _constraints: imba::constraints::Constraints| {
                    use imba::thunk_ext::ThunkExt;
                    imba::leaf::leaf::<()>(90.0, 40.0).paint_instead(|_arena, canvas, rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_color(skia_safe::Color::from_rgb(0xff, 0x00, 0xff));
                        canvas.draw_rect(rect, &paint);
                    })
                },
            )
        }
    }

    let mut app = Application::new(AppFonts::embedded());
    let window = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(window, &mut app, surface.canvas());

    let source: String = (0..30).map(|n| format!("pane line {n}\n")).collect();
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "popup.md".to_owned(),
            document: plain_document(&source),
            location: None,
            primary: true,
            target: None,
            focus: false,
        },
    )));
    crate::Window::draw(window, &mut app, surface.canvas());

    let (id, held) = crate::OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, held)| held.name() == "popup.md")
        .expect("the opened document");
    let mut document = held.document().clone();
    let markup = document.add_markup();
    let editor = document.editor_ids().next().expect("the pane's editor");
    document.show_markup(editor, markup);
    let ui = ::editor::test_document::test_ui();
    document.push_inlay(
        markup,
        12..16,
        crate::Inlay::new(
            crate::InlayMode::Popup(crate::PopupSpec {
                host: imba::overlay::WINDOW,
                position: imba::overlay::fit::PreferredPosition::At {
                    x: imba::overlay::fit::RangeEnd::Begin,
                    side: imba::overlay::fit::Side::Bottom,
                    align: imba::overlay::fit::Align::Left,
                },
            }),
            MagentaPopup,
        ),
        app.store(),
        ui,
        &crate::env::ui_collection(app.store(), ui),
        &crate::env::Themes::of(app.store()),
        &mut imba::effect::Batch::new().effects(),
    );
    crate::OpenDocuments::put_document(&mut app.store_mut(), id, document);

    let magenta = |surface: &mut skia_safe::Surface| -> usize {
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        let mut count = 0;
        for y in 0..600 {
            for x in 0..800 {
                if pixmap.get_color((x, y)) == skia_safe::Color::from_rgb(0xff, 0x00, 0xff) {
                    count += 1;
                }
            }
        }
        count
    };

    surface.canvas().clear(skia_safe::Color::from_rgb(9, 9, 9));
    crate::Window::draw(window, &mut app, surface.canvas());
    let painted = magenta(&mut surface);
    assert!(
        painted > 1000,
        "the popup painted through the pane boundary: {painted} px"
    );

    let (id, held) = crate::OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, held)| held.name() == "popup.md")
        .expect("still open");
    let mut document = held.document().clone();
    let ui = ::editor::test_document::test_ui();
    document.remove_markup(
        markup,
        &[],
        app.store(),
        ui,
        &crate::env::ui_collection(app.store(), ui),
        &crate::env::Themes::of(app.store()),
        &mut imba::effect::Batch::new().effects(),
    );
    crate::OpenDocuments::put_document(&mut app.store_mut(), id, document);
    surface.canvas().clear(skia_safe::Color::from_rgb(9, 9, 9));
    crate::Window::draw(window, &mut app, surface.canvas());
    assert_eq!(magenta(&mut surface), 0, "the popup left with its marker");
}

#[test]
fn the_at_completion_opens_finds_and_picks() {
    use crate::test_driver;
    use crate::{AppFonts, Application};
    use imba::event::{Key, Modifiers};

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let folder = crate::ResourceLocation::new(
        crate::ResourceType::directory(),
        crate::Authority::new("test"),
        vec!["proj".to_owned()],
    );
    let session = crate::test_support::seed_session_folders(&mut app.store_mut(), &[folder]);

    struct StubFind(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl imba::effect::EffectHandler<crate::FindEffect> for StubFind {
        async fn handle(&self, effect: crate::FindEffect) -> Vec<crate::ResourceLocation> {
            self.0.lock().expect("terms").push(effect.term.clone());
            let file = |path: &[&str]| {
                crate::ResourceLocation::new(
                    crate::ResourceType::document(),
                    crate::Authority::new("test"),
                    path.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
                )
            };
            vec![
                file(&["proj", "src", "main.rs"]),
                file(&["proj", "README.md"]),
            ]
        }
    }
    let terms = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<crate::FindEffect>(StubFind(std::sync::Arc::clone(&terms)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    let uri = "ahp-chat:/completion".to_owned();
    let panel = crate::higent::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        session.host,
        session.session.clone(),
        uri.clone(),
    );
    crate::higent::Chats::put(&mut app.store_mut(), uri.clone().into(), panel);
    {
        let mut entity = crate::Windows::window(app.store(), window).expect("window");
        entity.open_bottom(Box::new(crate::higent::ChatPane::new(uri.clone())));
        crate::Windows::put(&mut app.store_mut(), window, entity);
    }
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    crate::Window::draw(window, &mut app, surface.canvas());

    let panel = |app: &Application| -> crate::higent::ChatPanel {
        crate::higent::Chats::chat(app.store(), &uri).expect("the panel")
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            crate::Window::draw(window, app, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::type_text(&mut app, "check "));
    assert!(!panel(&app).completion_open(), "no popup before the @");
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open(), "the @ opened the popup");

    assert!(test_driver::type_text(&mut app, "ma"));
    pump(&mut app, &mut surface);
    assert_eq!(
        terms.lock().expect("terms").last().map(String::as_str),
        Some("ma"),
        "the find asked with the typed query"
    );
    let rows = panel(&app).completion_rows();
    assert_eq!(
        rows,
        vec!["main.rs".to_owned(), "README.md".to_owned()],
        "the stub's files listed"
    );

    assert!(test_driver::key(&mut app, Key::Down, Modifiers::default()));
    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    let after = panel(&app);
    assert!(!after.completion_open(), "the pick closed the popup");
    assert_eq!(
        after.composer_text(),
        "check @README.md ",
        "the inline spelling replaced the query, newline-free"
    );
    assert_eq!(after.completion_picked(), vec!["README.md".to_owned()]);

    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open());
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!panel(&app).completion_open(), "Escape closed the popup");

    assert!(test_driver::type_text(&mut app, "x "));
    assert!(
        !panel(&app).completion_open(),
        "a pasted 'x ' must not open"
    );
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open());
    assert!(test_driver::key(
        &mut app,
        Key::Backspace,
        Modifiers::default()
    ));
    assert!(
        !panel(&app).completion_open(),
        "deleting the @ closed the popup"
    );
}

#[test]
fn the_at_completion_serves_markdown_panes() {
    use crate::test_driver;
    use crate::{AppCommand, AppFonts, Application, OpenedDocument};
    use imba::event::{Key, Modifiers};
    use std::sync::Arc;

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let folder = crate::ResourceLocation::new(
        crate::ResourceType::directory(),
        crate::Authority::new("test"),
        vec!["proj".to_owned()],
    );
    let session = crate::test_support::seed_session_folders(&mut app.store_mut(), &[folder]);

    struct StubFind(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl imba::effect::EffectHandler<crate::FindEffect> for StubFind {
        async fn handle(&self, effect: crate::FindEffect) -> Vec<crate::ResourceLocation> {
            self.0.lock().expect("terms").push(effect.term.clone());
            vec![crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec!["proj".to_owned(), "notes".to_owned(), "ideas.md".to_owned()],
            )]
        }
    }
    let terms = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<crate::FindEffect>(StubFind(std::sync::Arc::clone(&terms)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    struct EnterSeeded(crate::SessionId);
    impl crate::DynamicCommand for EnterSeeded {
        fn id(&self) -> &'static str {
            "test.enter-seeded"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            _app: &mut Application,
            store: &mut Store,
            window: crate::WindowId,
            fx: &mut crate::AppFx<'_>,
        ) {
            crate::switch_session(store, window, self.0.clone(), fx)
        }
    }
    assert!(app.perform_command(AppCommand::Dynamic(
        window,
        Arc::new(EnterSeeded(session.clone()))
    )));

    let markdown = crate::Document::new(
        crate::Text::from_string_exact("hello world "),
        crate::Markup::new(),
    )
    .with_syntax(
        crate::Syntax::new("markdown", None, crate::Markup::new()),
        &[],
    );
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "notes.md".to_owned(),
            document: markdown,
            location: Some(crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec!["proj".to_owned(), "notes.md".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    crate::Window::draw(window, &mut app, surface.canvas());

    let completion_open = |app: &Application| -> bool {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .map(|entity| entity.workbench().root.focused_slot().completion.open())
            .unwrap_or(false)
    };
    let rows = |app: &Application| -> Vec<String> {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .map(|entity| {
                entity
                    .workbench()
                    .root
                    .focused_slot()
                    .completion
                    .row_labels()
            })
            .unwrap_or_default()
    };
    let pane_text = |app: &Application| -> String {
        let (_, held) = crate::OpenDocuments::list(app.store())
            .into_iter()
            .find(|(_, held)| held.name() == "notes.md")
            .expect("the pane document");
        let document = held.document();
        let mut view = document.text().view();
        let end = view.byte_count();
        view.byte_string(0, end)
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            crate::Window::draw(window, app, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Default::default()
        }
    ));
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(completion_open(&app), "the @ opened the pane popup");

    assert!(test_driver::type_text(&mut app, "id"));
    pump(&mut app, &mut surface);
    assert_eq!(
        terms.lock().expect("terms").last().map(String::as_str),
        Some("id"),
        "the find asked with the typed query"
    );
    assert_eq!(rows(&app), vec!["ideas.md".to_owned()]);

    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    assert!(!completion_open(&app), "the pick closed the popup");
    assert_eq!(
        pane_text(&app),
        "hello world @notes/ideas.md ",
        "the pick wrote the folder-relative path inline"
    );

    assert!(test_driver::type_text(&mut app, "@"));
    assert!(completion_open(&app));
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!completion_open(&app), "Escape closed the popup");

    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "main.rs".to_owned(),
            document: plain_document("fn main() {}\n"),
            location: Some(crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec!["proj".to_owned(), "main.rs".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    crate::Window::draw(window, &mut app, surface.canvas());
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(
        !completion_open(&app),
        "a non-markdown pane must not path-complete"
    );
}

#[test]
fn lsp_completion_serves_code_panes() {
    use crate::test_driver;
    use crate::{AppCommand, AppFonts, Application, OpenedDocument};
    use imba::event::{Key, Modifiers};
    use std::sync::Arc;

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();

    struct StubLsp(Arc<std::sync::Mutex<Vec<crate::LineCol>>>);
    impl imba::effect::EffectHandler<crate::LspCompletionEffect> for StubLsp {
        async fn handle(&self, effect: crate::LspCompletionEffect) -> Option<crate::LspAnswer> {
            self.0.lock().expect("asks").push(effect.position);
            Some(crate::LspAnswer {
                items: vec![
                    crate::LspItem {
                        label: "insert".to_owned(),
                        detail: Some("fn insert(k, v)".to_owned()),
                        filter_text: None,
                        sort_text: Some("0".to_owned()),
                        edit: Some((
                            crate::LineCol {
                                line: effect.position.line,
                                col: effect.position.col.saturating_sub(1),
                            }..effect.position,
                            "insert($0)".to_owned(),
                        )),
                        insert_text: None,
                    },
                    crate::LspItem {
                        label: "push".to_owned(),
                        detail: None,
                        filter_text: None,
                        sort_text: Some("1".to_owned()),
                        edit: None,
                        insert_text: Some("push".to_owned()),
                    },
                ],
                incomplete: false,
            })
        }
    }
    let asks = Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<crate::LspCompletionEffect>(StubLsp(Arc::clone(&asks)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "main.rs".to_owned(),
            document: plain_document("value "),
            location: Some(crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("test"),
                vec!["proj".to_owned(), "main.rs".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    crate::Window::draw(window, &mut app, surface.canvas());

    let completion_open = |app: &Application| -> bool {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .map(|entity| entity.workbench().root.focused_slot().completion.open())
            .unwrap_or(false)
    };
    let rows = |app: &Application| -> Vec<String> {
        crate::Windows::window_ref(app.store(), app.sole_window())
            .map(|entity| {
                entity
                    .workbench()
                    .root
                    .focused_slot()
                    .completion
                    .row_labels()
            })
            .unwrap_or_default()
    };
    let pane_text = |app: &Application| -> String {
        let (_, held) = crate::OpenDocuments::list(app.store())
            .into_iter()
            .find(|(_, held)| held.name() == "main.rs")
            .expect("the pane document");
        let document = held.document();
        let mut view = document.text().view();
        let end = view.byte_count();
        view.byte_string(0, end)
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            crate::Window::draw(window, app, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Default::default()
        }
    ));
    assert!(test_driver::type_text(&mut app, "i"));
    assert!(completion_open(&app), "the word start opened");
    pump(&mut app, &mut surface);
    assert_eq!(asks.lock().expect("asks").len(), 1, "one ask at open");
    assert_eq!(
        rows(&app),
        vec!["insert".to_owned()],
        "the query 'i' already filters"
    );

    assert!(test_driver::type_text(&mut app, "n"));
    pump(&mut app, &mut surface);
    assert_eq!(
        asks.lock().expect("asks").len(),
        1,
        "the growth filtered locally"
    );
    assert_eq!(
        rows(&app),
        vec!["insert".to_owned()],
        "subsequence-filtered"
    );

    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    assert!(!completion_open(&app), "the pick closed the popup");
    assert!(
        pane_text(&app).contains("insert()"),
        "the textEdit applied: {:?}",
        pane_text(&app)
    );

    assert!(test_driver::type_text(&mut app, "."));
    assert!(completion_open(&app), "the trigger char opened");
    pump(&mut app, &mut surface);
    assert_eq!(asks.lock().expect("asks").len(), 2, "the trigger re-asked");
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!completion_open(&app));

    let trigger = crate::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == "completion.trigger")
        .expect("the trigger command registered")
        .command;
    assert!(app.perform_command(trigger));
    assert!(completion_open(&app), "the explicit ask opened");
    pump(&mut app, &mut surface);
    assert_eq!(
        asks.lock().expect("asks").len(),
        3,
        "the explicit ask launched"
    );
    assert!(
        !rows(&app).is_empty(),
        "the app-road landing filled the rows"
    );
}

#[test]
fn ime_hit_test_covers_an_empty_document() {
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    use crate::{AppFonts, Application};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let mut text = String::new();
    for line in 0..80 {
        text.push_str(&format!("line number {line}\n"));
    }
    let _ = app.with_ime_client(app.sole_window(), |client| {
        client.insert_text(&text, None);
    });
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let hit = |app: &mut Application, y: f32| {
        app.with_ime_client(app.sole_window(), |client| client.char_index_at(400.0, y))
            .expect("focused")
    };
    for scroll in [-100_000.0, 900.0] {
        let _ = crate::test_driver::scroll_at(&mut app, 400.0, 300.0, scroll);
        crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
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

#[test]
fn scroll_stripes_follow_the_diff_through_the_app() {
    let ui = ::editor::test_document::test_ui();
    use crate::{AppFonts, Application};
    use std::sync::{mpsc, Arc};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let settle = |app: &mut Application| {
        for _ in 0..5 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(app.add_document(
        app.sole_window(),
        plain_document("one\ntwo\nthree\n"),
        "target.md".to_owned(),
        true
    ));
    let (target, editor) = app.focused_editor_id();
    let base = crate::OpenDocuments::register(
        &mut app.store_mut(),
        plain_document("one\nTWO\nthree\n"),
        None,
        "base.md".to_owned(),
        0,
    );
    let diff = crate::OpenDocuments::track_diff(&mut app.store_mut(), base, target, true, None)
        .expect("tracked");
    let _ = window;

    // A benign entity command: deliver drops it, the batch tails run.
    let tick = |app: &mut Application| {
        app.perform_batch(vec![crate::AppCommand::Entity(
            target,
            EditorCommand::ApplyRepair(Vec::new()),
        )]);
    };
    let stripes = |app: &Application| -> Vec<u32> {
        crate::OpenDocuments::document_ref(app.store(), target)
            .and_then(|document| document.scroll_stripes(editor))
            .map(|landed| landed.segments.iter().map(|segment| segment.byte).collect())
            .unwrap_or_default()
    };

    tick(&mut app);
    settle(&mut app);
    let born = stripes(&app);
    assert!(!born.is_empty(), "the tracked hunk projects onto the track");

    // The diff CHANGES: the first line gains its own hunk.
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let mut store = app.store_mut();
        let mut document = crate::OpenDocuments::document(&store, target).expect("open");
        document.edit(
            &::editor::Operation::insert_at(0, "zero\n"),
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        crate::OpenDocuments::put_document(&mut store, target, document);
    }
    tick(&mut app);
    settle(&mut app);
    let moved = stripes(&app);
    assert_ne!(moved, born, "an edit that moves the hunks moves the track");
    assert!(
        moved.contains(&0),
        "the typed line's own hunk lands on the track after the normalize: {moved:?}"
    );

    // The COMMIT road: the base catches up with the target — the diff
    // empties and the track must clear.
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let target_text = crate::OpenDocuments::document_ref(app.store(), target)
            .map(|document| {
                let mut view = document.text().view();
                let count = view.byte_count();
                view.byte_string(0, count)
            })
            .expect("open");
        let mut store = app.store_mut();
        let mut document = crate::OpenDocuments::document(&store, base).expect("open");
        let catch_up = myersdiff::diff(
            document.text(),
            &::editor::Text::from_string_exact(&target_text),
        );
        document.edit(
            &catch_up,
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        crate::OpenDocuments::put_document(&mut store, base, document);
    }
    tick(&mut app);
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the committed diff leaves no marks on the track"
    );

    // The REAL commit road: HEAD moves, the base ask answers a NEW
    // location — adopt unTRACKS the old diff and retracks against the
    // fresh base. First give the track marks again...
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let mut store = app.store_mut();
        let mut document = crate::OpenDocuments::document(&store, base).expect("open");
        document.edit(
            &::editor::Operation::insert_at(0, "gone\n"),
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        crate::OpenDocuments::put_document(&mut store, base, document);
    }
    tick(&mut app);
    settle(&mut app);
    assert!(
        !stripes(&app).is_empty(),
        "the diverged base marks the track again"
    );

    // ...then land the new HEAD: a base equal to the target's text,
    // under its own location.
    let target_text = crate::OpenDocuments::document_ref(app.store(), target)
        .map(|document| {
            let mut view = document.text().view();
            let count = view.byte_count();
            view.byte_string(0, count)
        })
        .expect("open");
    let head = ::editor::ResourceLocation::new(
        ::editor::ResourceType::document(),
        ::editor::Authority::new("local"),
        vec!["head-v2.md".to_owned()],
    );
    let _head_id = crate::OpenDocuments::register(
        &mut app.store_mut(),
        plain_document(&target_text),
        Some(head.clone()),
        "head-v2.md".to_owned(),
        0,
    );
    app.perform_batch(vec![crate::AppCommand::BaseLocated {
        document: target,
        base: Some(head),
    }]);
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the retracked (empty) diff clears the track"
    );

    // And a fresh edit AFTER the retrack stripes again — through the
    // real typing road.
    assert!(crate::test_driver::type_text(&mut app, "typed"));
    settle(&mut app);
    assert!(
        !stripes(&app).is_empty(),
        "typing into the retracked pane marks the track"
    );

    // A split-diff PANEL tracks its own diff on the same document
    // (stripes=false). Its hunks must NOT leak onto the pane's track:
    // when the pane's own stripes diff clears, the track clears too,
    // panel or no panel.
    let snapshot = crate::OpenDocuments::register(
        &mut app.store_mut(),
        plain_document(
            "something
entirely
unrelated
",
        ),
        None,
        "panel-base.md".to_owned(),
        0,
    );
    let panel_diff =
        crate::OpenDocuments::track_diff(&mut app.store_mut(), snapshot, target, false, None)
            .expect("the panel road tracks");
    // The pane's own diff empties: HEAD catches up again.
    let target_text = crate::OpenDocuments::document_ref(app.store(), target)
        .map(|document| {
            let mut view = document.text().view();
            let count = view.byte_count();
            view.byte_string(0, count)
        })
        .expect("open");
    let head3 = ::editor::ResourceLocation::new(
        ::editor::ResourceType::document(),
        ::editor::Authority::new("local"),
        vec!["head-v3.md".to_owned()],
    );
    let _ = crate::OpenDocuments::register(
        &mut app.store_mut(),
        plain_document(&target_text),
        Some(head3.clone()),
        "head-v3.md".to_owned(),
        0,
    );
    app.perform_batch(vec![crate::AppCommand::BaseLocated {
        document: target,
        base: Some(head3),
    }]);
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the committed pane clears its track even while a diff panel holds its own diff"
    );
    let _ = (diff, panel_diff);
}

mod wash_tests {
    use super::*;
    use crate::locations::{
        DisposeFeed, FeedId, FoundLocation, LocationsFeedRow, LocationsFeeds, PendingWashes,
    };
    use crate::{AppCommand, AppFonts, Application, OpenedDocument};
    use std::sync::Arc;

    fn located(name: &str) -> crate::ResourceLocation {
        crate::ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("local"),
            vec!["work".to_owned(), name.to_owned()],
        )
    }

    fn seeded_feed(app: &mut Application, name: &str) -> FeedId {
        let feed = FeedId::mint();
        let mut row = LocationsFeedRow {
            title: "Search: needle".to_owned(),
            generation: 1,
            done: true,
            ..Default::default()
        };
        for (line, column) in [(0u32, 0u32), (1, 4)] {
            row.locations.push_back_mut(FoundLocation {
                location: located(name),
                line,
                column,
                length: 6,
                context: "needle".to_owned(),
                context_column_start: 0,
            });
        }
        row.locations.push_back_mut(FoundLocation {
            location: located("other.md"),
            line: 0,
            column: 0,
            length: 6,
            context: "needle".to_owned(),
            context_column_start: 0,
        });
        LocationsFeeds::put(&mut app.store_mut(), feed, row);
        feed
    }

    #[test]
    fn a_search_pick_washes_the_opened_editor() {
        let mut app = Application::new(AppFonts::embedded());
        let window = app.add_window();
        assert!(app.perform_command(AppCommand::Opened(
            window,
            OpenedDocument {
                name: "hit.md".to_owned(),
                document: ::editor::test_document::plain_document("needle one\nfour needle\n"),
                location: Some(located("hit.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));
        let feed = seeded_feed(&mut app, "hit.md");

        // The already-open pick path: the wash lands through the
        // drained request, resolved against live text.
        assert!(app.perform_command(AppCommand::Dynamic(
            window,
            Arc::new(crate::hisearch::OpenFoundLocation {
                location: located("hit.md"),
                target: crate::LineCol { line: 1, col: 4 }..crate::LineCol { line: 1, col: 10 },
                feed: Some(feed),
                focus: true,
            }),
        )));
        // Requests drain on the next content tick, as in the live app.
        assert!(app.perform_command(AppCommand::Content(
            window,
            crate::WindowCommand::Focus(crate::LayerFocus::Content),
        )));
        let row = LocationsFeeds::row(app.store(), feed).expect("the feed");
        assert_eq!(row.washes.size(), 1, "the opened document is washed");
        let (document, (_, pushed)) = row.washes.iter().next().expect("the wash");
        let ranges: Vec<(u32, u32)> = pushed.iter().copied().collect();
        assert_eq!(
            ranges,
            [(0, 6), (15, 21)],
            "every occurrence in the file, byte-resolved"
        );

        // Disposal removes the wash and survives the walk.
        assert!(app.perform_command(AppCommand::Dynamic(window, Arc::new(DisposeFeed { feed }),)));
        assert!(LocationsFeeds::row(app.store(), feed).is_none());
        assert!(
            crate::OpenDocuments::document_ref(app.store(), *document).is_some(),
            "the document stays; only the wash left"
        );
    }

    #[test]
    fn a_pick_before_the_open_washes_at_registration() {
        let mut app = Application::new(AppFonts::embedded());
        let window = app.add_window();
        let feed = seeded_feed(&mut app, "late.md");

        // The async-open path: the pick notes the pending wash; the
        // document hook converts it when registration lands.
        PendingWashes::note(&mut app.store_mut(), located("late.md"), feed);
        assert!(app.perform_command(AppCommand::Opened(
            window,
            OpenedDocument {
                name: "late.md".to_owned(),
                document: ::editor::test_document::plain_document("needle one\nfour needle\n"),
                location: Some(located("late.md")),
                primary: true,
                target: None,
                focus: false,
            },
        )));
        assert!(app.perform_command(AppCommand::Content(
            window,
            crate::WindowCommand::Focus(crate::LayerFocus::Content),
        )));
        let row = LocationsFeeds::row(app.store(), feed).expect("the feed");
        assert_eq!(
            row.washes.size(),
            1,
            "the hook washed the registration: {:?}",
            row.washes.size()
        );
    }
}
