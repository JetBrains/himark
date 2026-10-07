// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub(crate) use imba::{store::Store, View};
pub(crate) use std::str;

pub(crate) use editor::{
    document::Document,
    editor_view::EditorCommand,
    editor_view::EditorFocus,
    editor_view::EditorView,
    markup::inlay_anchors_line,
    markup::Inlay,
    markup::InlayMode,
    test_document::{fenced_code_document, list_document, plain_document},
};

pub(crate) use documents::entity_view::EditorIdView;
pub(crate) use himark::app_ext::AppExt;

fn test_docs() -> imba::store::Id<documents::OpenDocuments> {
    static DOCS: std::sync::OnceLock<imba::store::Id<documents::OpenDocuments>> =
        std::sync::OnceLock::new();
    *DOCS.get_or_init(imba::store::Id::mint)
}

struct TestPane {
    store: Store,
    view: EditorIdView,

    inlay_markup: editor::markup::MarkupId,
}

/// TEST SUPPORT: the landing's first-match jump rides the
/// `AnnounceSelect` round trip (docs/ui/list-keyboard.md §5) — drain
/// it out of a batch and hand back the command it would dispatch.
pub(crate) fn drain_announced<C: 'static>(batch: imba::effect::Batch<C>) -> Option<C> {
    use imba::effect::Message;
    let mut announced = None;
    for message in batch.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, lift) = effect.into_payload().split();
        if value
            .downcast::<hikit::list_keyboard::AnnounceSelect>()
            .is_ok()
        {
            announced = lift(Box::new(()));
        }
    }
    announced
}

impl TestPane {
    fn new(mut document: Document, width: f32) -> Self {
        let ui = ::editor::test_document::test_ui();
        let mut store = Store::new();
        let editor = document.add_editor(
            width,
            None,
            ::editor::document::EditorBuild::Complete,
            &[],
            &store,
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::theme::Theme::embedded(),
            &mut imba::effect::Batch::new().effects(),
        );
        let inlay_markup = document.add_markup();
        document.show_markup(editor, inlay_markup);
        let document_id = documents::OpenDocuments::register(
            &mut store,
            test_docs(),
            document,
            None,
            "test".to_owned(),
            0,
        );

        Self {
            view: EditorIdView::new(test_docs(), document_id, editor),
            store,
            inlay_markup,
        }
    }

    fn inlay_key(&self, key: u32) -> editor::markup::InlayKey {
        editor::markup::InlayKey::in_markup(self.inlay_markup, key)
    }

    fn resize(&mut self, width: f32, anchor: u32) -> imba::effect::Batch<EditorCommand> {
        let ui = ::editor::test_document::test_ui();
        let entity = self.view;
        let mut document =
            documents::OpenDocuments::document(&self.store, entity.documents(), entity.document())
                .expect("document");
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
        documents::OpenDocuments::put_document(
            &mut self.store,
            entity.documents(),
            entity.document(),
            document,
        );
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
        let mut pending = himark::test_support::surviving_launches(batch);
        while let Some(effect) = pending.pop() {
            let command = himark::test_support::handle_effect(
                effect,
                &himark::test_support::test_workshop(::editor::theme::Theme::embedded()),
            );
            let mut batch = imba::effect::Batch::new();
            self.view.perform(
                &mut self.store,
                ::editor::test_document::test_ui(),
                command,
                &mut batch.effects(),
            );
            pending.extend(himark::test_support::surviving_launches(batch));
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
            documents::OpenDocuments::document(&self.store, entity.documents(), entity.document())
                .expect("document");
        document.set_caret(entity.editor(), byte);
        documents::OpenDocuments::put_document(
            &mut self.store,
            entity.documents(),
            entity.document(),
            document,
        );
    }

    fn replace_inlay(
        &mut self,
        key: editor::markup::InlayKey,
        range: std::ops::Range<u32>,
        inlay: Inlay,
    ) {
        let ui = ::editor::test_document::test_ui();
        let entity = self.view;
        let mut document =
            documents::OpenDocuments::document(&self.store, entity.documents(), entity.document())
                .expect("document");
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
        documents::OpenDocuments::put_document(
            &mut self.store,
            entity.documents(),
            entity.document(),
            document,
        );
        self.finish(batch);
    }
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

#[derive(Clone, Debug)]
enum TestInlayCommand {
    Grow,
    GrowWide,
}

impl std::fmt::Display for TestInlayCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}

impl View for TestInlay {
    type Command = TestInlayCommand;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
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
        _ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
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

#[derive(Clone)]
struct FocusProbe;

#[derive(Clone, Debug)]
enum ProbeCommand {
    Poke,
}

impl std::fmt::Display for ProbeCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}

impl View for FocusProbe {
    type Command = ProbeCommand;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
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

mod chat;
mod completion;
mod diagnostics;
mod dock;
mod drawer;
mod editing;
mod find;
mod navigation;
mod palette;
mod panes;
mod toc;
mod toolbar;
mod washes;
mod workspaces;
