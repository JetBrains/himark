// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, constraints::Constraints, store::Store, UiCtx, View, Widget};

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
        let (commands, location, seat) = match self.gathered(store) {
            Some(view) => {
                let mut data = view.focus_data(store, ui);
                (
                    std::mem::take(&mut data.commands),
                    data.location.take(),
                    data.seat.take(),
                )
            }
            None => (Vec::new(), None, None),
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
            seat,
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
}

impl<'a> imba::Thunk<'a, EditorCommand> for GatheredPane<'a> {
    fn size(&self) -> skia_safe::Size {
        match &self.view {
            Some(view) => skia_safe::Size::new(
                view.gutter_width + view.layout_width() + self.content_pad * 2.0,
                view.content_height().max(self.constraints.min.height),
            ),
            None => skia_safe::Size::default(),
        }
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, EditorCommand> {
        let size = imba::Thunk::size(&self);
        let GatheredPane {
            ui,
            view,
            store,
            arena: frame,
            constraints,
            content_pad,
        } = self;
        // Realized ONCE, bounded by the frame's viewport: the
        // editor's own display derives its shared viewport and mints
        // its popup/projected/sticky emissions, which the container
        // collects (translated by the pad). The pane adds only the
        // scroll stripes — the one emission display leaves to hosts.
        // The old shape re-laid the editor per ask and re-built the
        // viewport for every overlay flavor.
        let mut pane = imba::container::container(frame, size);
        let mut stripes = Vec::new();
        if let Some(view) = view {
            let view: &'a EditorView = frame.alloc(view);
            pane.place(
                content_pad,
                0.0,
                imba::Layout::layout(view.display(frame, store, ui), frame, constraints),
            );
            stripes = view.scroll_stripe_overlays(frame, store, viewport);
            for overlay in &mut stripes {
                overlay.translate(content_pad, 0.0);
            }
        }
        let inner = pane.realize_into(viewport);
        imba::WidgetBox::new(arena, RealizedGatheredPane { inner, stripes })
    }
}

struct RealizedGatheredPane<'a> {
    inner: imba::container::RealizedContainer<'a, EditorCommand>,
    stripes: Vec<imba::overlay::Overlay<'a, EditorCommand>>,
}

impl<'a> Widget<'a, EditorCommand> for RealizedGatheredPane<'a> {
    fn size(&self) -> skia_safe::Size {
        Widget::size(&self.inner)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
        let mut overlays = self.inner.overlays();
        overlays.append(&mut self.stripes);
        overlays
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<EditorCommand> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, EditorCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}
