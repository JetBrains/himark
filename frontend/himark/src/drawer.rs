use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};

use crate::{ModalRequest, ModalView};

pub const DRAWER_WIDTH: f32 = 664.0;

const SLIDE_MS: f64 = 160.0;

pub enum DrawerCommand {
    Tick(AnimationClock),

    Content(imba::DynCommand),
}

pub(crate) struct Drawer {
    content: Box<dyn ModalView>,

    offset: Animation<f32>,

    closing: bool,

    request: Option<ModalRequest>,
}

impl Clone for Drawer {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            offset: self.offset,
            closing: self.closing,

            request: None,
        }
    }
}

impl Drawer {
    pub(crate) fn new(content: Box<dyn ModalView>) -> Self {
        let mut offset = Animation::done(
            -DRAWER_WIDTH,
            Motion::Ease {
                duration_ms: SLIDE_MS,
                easing: Easing::EaseOut,
            },
        );
        offset.set(0.0);
        Self {
            content,
            offset,
            closing: false,
            request: None,
        }
    }

    pub(crate) fn content(&self) -> &dyn ModalView {
        self.content.as_ref()
    }

    pub(crate) fn content_mut(&mut self) -> &mut Box<dyn ModalView> {
        &mut self.content
    }

    fn roll_away(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.offset.set(-DRAWER_WIDTH);
    }
}

impl View for Drawer {
    type Command = DrawerCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(DrawerCommand::Content, |fx| {
            imba::DynView::destroy_dyn(self.content_mut().as_mut(), store, fx)
        })
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DrawerCommand,
        fx: &mut imba::effect::Effects<'_, DrawerCommand>,
    ) {
        match command {
            DrawerCommand::Tick(now) => {
                self.offset.advance(now);
                if self.closing && !self.offset.running() {
                    self.request = Some(ModalRequest::Close);
                }
            }
            DrawerCommand::Content(command) => fx.scope(DrawerCommand::Content, |fx| {
                self.content.as_mut().perform_dyn(store, ui, command, fx)
            }),
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = constraints.max;
        let mut surface = container(arena, size);
        let content = self
            .content
            .as_ref()
            .layout_dyn(arena, store, ui, constraints)
            .map(DrawerCommand::Content);
        surface.place(self.offset.value(), 0.0, content);

        let animating = self.offset.running();
        let clock =
            leaf::<DrawerCommand>(size.width, size.height).event(move |_arena, event, _size| {
                match event {
                    Event::AnimationClock { now } if animating => {
                        EventResult::Command(DrawerCommand::Tick(*now))
                    }
                    _ => EventResult::Ignored,
                }
            });
        surface.place(0.0, 0.0, clock);
        surface
    }
}

impl ModalView for Drawer {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        if let Some(request) = self.request.take() {
            return Some(request);
        }
        match self.content.take_request() {
            Some(ModalRequest::Close) => {
                self.roll_away();
                None
            }

            other => other,
        }
    }

    fn release_widgets(&mut self) -> Vec<(crate::WidgetOrigin, Box<dyn crate::DynPanelView>)> {
        self.content.release_widgets()
    }

    fn focus_lost(&mut self) {
        self.roll_away();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
