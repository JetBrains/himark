use imba::arena::Arena;
use imba::constraints::Constraints;
use imba::event::{Event, EventResult};
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{Thunk, UiCtx};
use skia_safe::{Point, Rect, Size};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::markup::{Inlay, InlayCommand, InlayKey};

pub(crate) fn visible_popups<'a>(
    document: &Document,
    editor: EditorId,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::theme::Theme,
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    viewport: Rect,
    origin: Point,
) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
    if !document.has_popups(editor) || viewport.height() <= 0.0 {
        return Vec::new();
    }

    let band = document.visible_byte_band(editor, viewport.top, viewport.bottom);
    let mut overlays = Vec::new();
    for (key, range, inlay, spec) in document.popups_in(editor, band) {
        let Some((x, y, width, height)) =
            document.caret_content_rect(editor, range.start, fonts, theme)
        else {
            continue;
        };
        let width = document
            .caret_content_rect(editor, range.end, fonts, theme)
            .filter(|(_, end_y, _, _)| *end_y == y)
            .map(|(end_x, _, _, _)| (end_x - x).max(width))
            .unwrap_or(width);
        if y + height < viewport.top || y > viewport.bottom {
            continue;
        }
        let seed = PopupSeed {
            inlay,
            key,
            position: spec.position,
            store,
            ui,
            arena,
        };
        overlays.push(imba::overlay::Overlay {
            host: spec.host,
            anchor: Rect::from_xywh(origin.x + x, origin.y + y, width.max(1.0), height),
            content: Box::new(move |host_size: Size, anchor: Rect| seed.layout(host_size, anchor)),
        });
    }
    overlays
}

pub(crate) struct PopupSeed<'a> {
    pub inlay: Inlay,
    pub key: InlayKey,
    pub position: imba::overlay::fit::PreferredPosition,
    pub store: &'a Store,
    pub ui: &'a UiCtx,
    pub arena: &'a Arena,
}

impl<'a> PopupSeed<'a> {
    pub(crate) fn layout(
        self,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, EditorCommand>)> {
        let desired = self
            .inlay
            .layout(
                self.arena,
                self.store,
                self.ui,
                Constraints {
                    min: Size::default(),
                    max: Size::new(f32::INFINITY, f32::INFINITY),
                },
            )
            .size();
        let resolved = imba::overlay::fit::resolve(
            host_size,
            anchor,
            desired,

            Size::new(1.0, 1.0),
            self.position,
        );
        let key = self.key;
        let widget = PopupWidget {
            inlay: self.inlay,
            store: self.store,
            ui: self.ui,
            arena: self.arena,
            size: Size::new(resolved.width(), resolved.height()),
        };
        vec![(
            Point::new(resolved.left, resolved.top),
            imba::ThunkBox::new(
                self.arena,
                imba::eager(widget).map(move |command| EditorCommand::Inlay { key, command }),
            ),
        )]
    }
}

struct PopupWidget<'a> {
    inlay: Inlay,
    store: &'a Store,
    ui: &'a UiCtx,

    arena: &'a Arena,
    size: Size,
}

impl<'a> imba::Widget<'a, InlayCommand> for PopupWidget<'a> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<InlayCommand> {
        let thunk = self.inlay.layout(
            self.arena,
            self.store,
            self.ui,
            Constraints::tight(self.size),
        );
        let widget = thunk.realize(self.arena, viewport);
        widget.handle_event(arena, event, viewport)
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, InlayCommand>
    where
        'a: 'w,
    {
        use imba::focus::FocusData;
        let inlay = &self.inlay;
        let store = self.store;
        let ui = self.ui;
        let arena = self.arena;
        let size = self.size;
        let with_chain = move |f: &mut dyn FnMut(
            FocusData<'_, InlayCommand>,
        ) -> EventResult<InlayCommand>|
              -> EventResult<InlayCommand> {
            let mut widget = inlay
                .layout(arena, store, ui, Constraints::tight(size))
                .realize(arena, Rect::from_size(size));
            let result = f(widget.focus_data());
            drop(widget);
            result
        };
        let commands = {
            let mut widget = inlay
                .layout(arena, store, ui, Constraints::tight(size))
                .realize(arena, Rect::from_size(size));
            let commands = std::mem::take(&mut widget.focus_data().commands);
            drop(widget);
            commands
        };
        FocusData {
            commands,
            on_key: Some(Box::new(move |key, mods| {
                with_chain(&mut |mut data| data.key(key, mods))
            })),
            on_text: Some(Box::new(move |text| {
                with_chain(&mut |mut data| data.text(text))
            })),
            ..FocusData::default()
        }
    }
}
