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

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
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
        GatheredPane {
            ui,
            view: self.gathered(store),
            store,
            arena,
            constraints,
            content_pad: ::editor::env::Themes::of(store).ui().window.content_pad,

            viewport: skia_safe::Rect::new_empty(),
        }
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

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, EditorCommand>
    where
        'a: 'w,
    {
        use imba::event::EventResult;
        use imba::focus::FocusData;

        let Some(view) = &self.view else {
            return FocusData::default();
        };
        let store = self.store;
        let ui = self.ui;
        let arena = self.arena;
        let constraints = self.constraints;
        let with_chain = move |f: &mut dyn FnMut(
            FocusData<'_, EditorCommand>,
        ) -> EventResult<EditorCommand>|
              -> EventResult<EditorCommand> {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let result = f(widget.focus_data());
            drop(widget);
            result
        };
        let commands = {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let commands = std::mem::take(&mut widget.focus_data().commands);
            drop(widget);
            commands
        };
        let location = {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let location = widget.focus_data().location.take();
            drop(widget);
            location
        };
        FocusData {
            commands,
            on_key: Some(Box::new(move |key, mods| {
                with_chain(&mut |mut data| data.key(key, mods))
            })),
            on_text: Some(Box::new(move |text| {
                with_chain(&mut |mut data| data.text(text))
            })),
            ime: Some(imba::focus::ImeSeat {
                origin: skia_safe::Point::new(self.content_pad, 0.0),
                clip: None,
                ask: Box::new(move |origin, clip, visit| {
                    with_chain(&mut |mut data| match data.ime.take() {
                        Some(mut seat) => {
                            let at = skia_safe::Point::new(
                                origin.x + seat.origin.x,
                                origin.y + seat.origin.y,
                            );
                            (seat.ask)(at, clip, visit)
                        }
                        None => EventResult::Ignored,
                    })
                }),
            }),
            clipboard: Some(Box::new(move |visit| {
                with_chain(&mut |mut data| match data.clipboard.as_mut() {
                    Some(seat) => seat(visit),
                    None => EventResult::Ignored,
                })
            })),
            location,
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
                    view.layout(arena, self.store, self.ui, self.constraints),
                );
                pane.realize(arena, viewport)
                    .handle_event(arena, event, viewport)
            }
            None => imba::event::EventResult::Ignored,
        }
    }
}
