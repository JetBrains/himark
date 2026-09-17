// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, constraints::Constraints, store::Store, Thunk, UiCtx, View, Widget};

use editor::{EditorCommand, EditorId, EditorView};

use crate::DocumentId;

impl EditorIdView {
    pub fn editor_width(pane_width: f32, window: &::editor::theme::WindowChrome) -> f32 {
        (pane_width - window.content_pad * 2.0).max(window.min_editor_width)
    }
}

#[derive(Clone, Copy)]
pub struct EditorIdView {
    document: DocumentId,
    editor: EditorId,

    blurred: bool,

    gutter: bool,
}

impl EditorIdView {
    pub fn new(document: DocumentId, editor: EditorId) -> Self {
        Self {
            document,
            editor,
            blurred: false,
            gutter: false,
        }
    }

    pub fn blurred(mut self) -> Self {
        self.blurred = true;
        self
    }

    pub fn with_gutter(mut self) -> Self {
        self.gutter = true;
        self
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    pub fn editor(&self) -> EditorId {
        self.editor
    }

    pub fn gathered(&self, store: &Store) -> Option<EditorView> {
        let document = crate::OpenDocuments::document(store, self.document)?;
        let mut view = EditorView {
            document,
            editor: self.editor,
            reports_geometry: true,

            location: crate::OpenDocuments::location(store, self.document),

            gutter_width: match self.gutter {
                true => ::editor::env::Themes::of(store).ui().editor_gutter.width,
                false => 0.0,
            },

            base: match self.gutter {
                true => {
                    crate::OpenDocuments::stripe_diff(store, self.document).and_then(|handle| {
                        let base = crate::OpenDocuments::document(store, handle.base)?;
                        Some((base, handle.id))
                    })
                }
                false => None,
            },
        };
        if self.blurred {
            view.blur();
        }
        Some(view)
    }
}

impl View for EditorIdView {
    type Command = EditorCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        crate::close_editor(store, self.document, self.editor);
        crate::OpenDocuments::remove_if_editorless(store, self.document, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut view) = self.gathered(store) else {
            return;
        };

        view.perform(store, ui, command, fx);
        crate::OpenDocuments::put_document(store, self.document, view.document);
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, EditorCommand> {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        // The editor view is MINTED from the store per ask (documents
        // are persistent, the clone is cheap); handlers re-mint per
        // call because the data may not outlive a temporary.
        let (commands, location) = match self.gathered(store) {
            Some(view) => {
                let mut data = view.focus_data(store, ui);
                (std::mem::take(&mut data.commands), data.location.take())
            }
            None => (Vec::new(), None),
        };
        let with_view =
            move |f: &mut dyn FnMut(FocusData<'_, EditorCommand>) -> EventResult<EditorCommand>| {
                match self.gathered(store) {
                    Some(view) => f(view.focus_data(store, ui)),
                    None => EventResult::Ignored,
                }
            };
        FocusData {
            commands,
            on_key: Some(Box::new(move |key, mods| {
                with_view(&mut |mut data| data.key(key, mods))
            })),
            on_text: Some(Box::new(move |text| {
                with_view(&mut |mut data| data.text(text))
            })),
            clipboard: Some(Box::new(move |visit| {
                with_view(&mut |mut data| match data.clipboard.as_mut() {
                    Some(seat) => seat(visit),
                    None => EventResult::Ignored,
                })
            })),
            location,
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let constraints = Constraints {
                min: constraints.min,
                max: skia_safe::Size::new(
                    Self::editor_width(
                        constraints.max.width,
                        &::editor::env::Themes::of(store).ui().window,
                    ),
                    constraints.max.height,
                ),
            };
            // The bare-editor host for projected inlays (deleted-code
            // cards from the gutter stripes): every list row and
            // workbench pane rides through here, and none of them is
            // a split pane, so the split's shared host stays in
            // charge there.
            imba::thunk_ext::ThunkExt::overlay_host(
                GatheredPane {
                    ui,
                    view: self.gathered(store),
                    store,
                    arena,
                    constraints,
                    content_pad: ::editor::env::Themes::of(store).ui().window.content_pad,

                    viewport: skia_safe::Rect::new_empty(),
                },
                ::editor::INLAY_HOST,
            )
        })
    }
}

struct GatheredPane<'a> {
    ui: &'a UiCtx,
    view: Option<EditorView>,
    store: &'a Store,

    arena: &'a Arena,
    constraints: Constraints,

    content_pad: f32,

    viewport: skia_safe::Rect,
}

impl<'a> imba::Thunk<'a, EditorCommand> for GatheredPane<'a> {
    fn size(&self) -> skia_safe::Size {
        Widget::size(self)
    }

    fn realize(
        mut self,
        arena: &'a Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, EditorCommand> {
        self.viewport = viewport;
        imba::WidgetBox::new(arena, self)
    }
}

impl<'a> Widget<'a, EditorCommand> for GatheredPane<'a> {
    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
        let Some(view) = &self.view else {
            return Vec::new();
        };

        let mut overlays = view.popup_overlays(
            self.arena,
            self.store,
            self.ui,
            self.constraints.max.width,
            self.viewport,
        );

        overlays.extend(view.sticky_overlays(
            self.arena,
            self.store,
            self.ui,
            self.constraints.max.width,
            self.viewport,
        ));

        overlays.extend(view.scroll_stripe_overlays(self.arena, self.store, self.viewport));
        for overlay in &mut overlays {
            overlay.translate(self.content_pad, 0.0);
        }
        overlays
    }

    fn size(&self) -> skia_safe::Size {
        match &self.view {
            Some(view) => skia_safe::Size::new(
                view.gutter_width + view.layout_width() + self.content_pad * 2.0,
                view.content_height().max(self.constraints.min.height),
            ),
            None => skia_safe::Size::default(),
        }
    }

    fn layout_data<'w>(&'w mut self) -> imba::focus::LayoutData<'w, EditorCommand>
    where
        'a: 'w,
    {
        use imba::event::EventResult;
        use imba::focus::LayoutData;

        let Some(view) = &self.view else {
            return LayoutData::default();
        };
        let store = self.store;
        let ui = self.ui;
        let arena = self.arena;
        let constraints = self.constraints;
        let viewport = self.viewport;
        LayoutData {
            // The rect is a layout question: the pane realizes its
            // editor ON THE ASK, bounded by the frame's viewport —
            // IME composition is rare enough that nothing else pays.
            ime: Some(imba::focus::ImeSeat {
                origin: skia_safe::Point::new(self.content_pad, 0.0),
                clip: None,
                ask: Box::new(move |origin, clip, visit| {
                    let mut widget =
                        imba::Layout::layout(view.display(arena, store, ui), arena, constraints)
                            .realize(arena, viewport);
                    let result = match widget.layout_data().ime.take() {
                        Some(mut seat) => {
                            let at = skia_safe::Point::new(
                                origin.x + seat.origin.x,
                                origin.y + seat.origin.y,
                            );
                            (seat.ask)(at, clip, visit)
                        }
                        None => EventResult::Ignored,
                    };
                    drop(widget);
                    result
                }),
            }),
        }
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<EditorCommand> {
        match &self.view {
            Some(view) => {
                let mut pane = imba::container::container(arena, Widget::size(self));
                pane.place(
                    self.content_pad,
                    0.0,
                    imba::Layout::layout(
                        view.display(arena, self.store, self.ui),
                        arena,
                        self.constraints,
                    ),
                );
                pane.realize(arena, viewport)
                    .handle_event(arena, event, viewport)
            }
            None => imba::event::EventResult::Ignored,
        }
    }
}
