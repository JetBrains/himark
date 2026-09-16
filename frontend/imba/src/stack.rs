// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ui::UiCtx;
use skia_safe::Rect;

use crate::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    View, Widget,
};

#[derive(Clone)]
pub struct Stack<Base, Modal> {
    base: Base,
    modal: Option<Modal>,
}

pub enum StackCommand<BaseCommand, ModalCommand> {
    Base(BaseCommand),
    Modal(ModalCommand),
}

impl<Base, Modal> Stack<Base, Modal> {
    pub fn new(base: Base) -> Self {
        Self { base, modal: None }
    }

    pub fn base(&self) -> &Base {
        &self.base
    }

    pub fn base_mut(&mut self) -> &mut Base {
        &mut self.base
    }

    pub fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    pub fn modal_mut(&mut self) -> Option<&mut Modal> {
        self.modal.as_mut()
    }

    pub fn show(&mut self, modal: Modal) {
        self.modal = Some(modal);
    }

    pub fn dismiss(&mut self) -> Option<Modal> {
        self.modal.take()
    }
}

impl<Base, Modal> View for Stack<Base, Modal>
where
    Base: View,
    Modal: View,
    Base::Command: Send + 'static,
    Modal::Command: Send + 'static,
{
    type Command = StackCommand<Base::Command, Modal::Command>;

    fn destroy(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, Self::Command>) {
        fx.scope(StackCommand::Base, |fx| self.base.destroy(store, fx));
        if let Some(modal) = &mut self.modal {
            fx.scope(StackCommand::Modal, |fx| modal.destroy(store, fx));
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            StackCommand::Base(command) => fx.scope(StackCommand::Base, |fx| {
                self.base.perform(store, ui, command, fx)
            }),
            StackCommand::Modal(command) => {
                if let Some(modal) = &mut self.modal {
                    fx.scope(StackCommand::Modal, |fx| {
                        modal.perform(store, ui, command, fx)
                    });
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(
            move |_arena: &'a Arena, constraints: Constraints| StackWidget {
                base: crate::Layout::layout(
                    self.base.display(arena, store, ui),
                    arena,
                    constraints,
                ),
                modal: self.modal.as_ref().map(|modal| {
                    crate::Layout::layout(modal.display(arena, store, ui), arena, constraints)
                }),
            },
        )
    }
}

struct StackWidget<BaseThunk, ModalThunk> {
    base: BaseThunk,
    modal: Option<ModalThunk>,
}

impl<'a, BaseCommand: 'a, ModalCommand: 'a, BaseThunk, ModalThunk>
    crate::Thunk<'a, StackCommand<BaseCommand, ModalCommand>> for StackWidget<BaseThunk, ModalThunk>
where
    BaseThunk: crate::Thunk<'a, BaseCommand> + 'a,
    ModalThunk: crate::Thunk<'a, ModalCommand> + 'a,
{
    fn size(&self) -> skia_safe::Size {
        self.base.size()
    }

    fn realize(
        self,
        arena: &'a crate::arena::Arena,
        viewport: Rect,
    ) -> crate::WidgetBox<'a, StackCommand<BaseCommand, ModalCommand>> {
        let StackWidget { base, modal } = self;
        crate::WidgetBox::new(
            arena,
            RealizedStack {
                base: base.realize(arena, viewport),
                modal: modal.map(|modal| modal.realize(arena, viewport)),
            },
        )
    }
}

struct RealizedStack<'a, BaseCommand, ModalCommand> {
    base: crate::WidgetBox<'a, BaseCommand>,
    modal: Option<crate::WidgetBox<'a, ModalCommand>>,
}

impl<'a, BaseCommand: 'a, ModalCommand: 'a> Widget<'a, StackCommand<BaseCommand, ModalCommand>>
    for RealizedStack<'a, BaseCommand, ModalCommand>
{
    fn size(&self) -> skia_safe::Size {
        self.base.size()
    }

    fn overlays(
        &mut self,
    ) -> Vec<crate::overlay::Overlay<'a, StackCommand<BaseCommand, ModalCommand>>> {
        let mut overlays = crate::overlay::map_overlays(self.base.overlays(), &StackCommand::Base);
        if let Some(modal) = &mut self.modal {
            overlays.append(&mut crate::overlay::map_overlays(
                modal.overlays(),
                &StackCommand::Modal,
            ));
        }
        overlays
    }

    fn focus_data<'w>(
        &'w mut self,
    ) -> crate::focus::FocusData<'w, StackCommand<BaseCommand, ModalCommand>>
    where
        'a: 'w,
    {
        match &mut self.modal {
            Some(modal) => {
                let mut data = modal.focus_data().map(StackCommand::Modal);
                let mut inner_key = data.on_key.take();
                data.on_key = Some(Box::new(move |key, mods| {
                    match inner_key.as_deref_mut().map(|h| h(key, mods)) {
                        None | Some(EventResult::Ignored) => EventResult::Handled,
                        Some(result) => result,
                    }
                }));
                let mut inner_text = data.on_text.take();
                data.on_text = Some(Box::new(move |text| {
                    match inner_text.as_deref_mut().map(|h| h(text)) {
                        None | Some(EventResult::Ignored) => EventResult::Handled,
                        Some(result) => result,
                    }
                }));
                data
            }
            None => self.base.focus_data().map(StackCommand::Base),
        }
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<StackCommand<BaseCommand, ModalCommand>> {
        let Some(modal) = &self.modal else {
            return self
                .base
                .handle_event(arena, event, viewport)
                .map(StackCommand::Base);
        };

        if let Event::Paint { .. } | Event::AnimationClock { .. } | Event::ThemeChanged = event {
            let base_event = match event {
                Event::Paint { canvas, .. } => Event::Paint {
                    canvas,
                    focused: false,
                },
                other => *other,
            };
            return self
                .base
                .handle_event(arena, &base_event, viewport)
                .map(StackCommand::Base)
                .merge(
                    modal
                        .handle_event(arena, event, viewport)
                        .map(StackCommand::Modal),
                );
        }

        if let Event::HitTest { point, .. } = event {
            let missed = Event::HitTest {
                point: *point,
                miss: true,
            };
            return self
                .base
                .handle_event(arena, &missed, viewport)
                .map(StackCommand::Base)
                .merge(
                    modal
                        .handle_event(arena, event, viewport)
                        .map(StackCommand::Modal),
                );
        }

        match modal.handle_event(arena, event, viewport) {
            EventResult::Ignored => EventResult::Handled,
            result => result.map(StackCommand::Modal),
        }
    }
}
