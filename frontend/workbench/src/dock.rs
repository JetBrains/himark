// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use skia_safe::Size;

use hikit::modal::ModalRequest;
use hikit::modal::ModalView;

/// TEST SUPPORT: the side width the tests lay against — the theme's
/// `window.side_width`; production reads the theme.
#[doc(hidden)]
pub const DOCK_WIDTH: f32 = 720.0;

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub const DOCK_MIN_WIDTH: f32 = 240.0;

pub(crate) const DOCK_MAX_RATIO: f32 = 0.6;

pub(crate) const SLIDE_MS: f64 = 160.0;

pub(crate) const HANDLE_REACH: f32 = 4.0;

#[derive(Clone)]
pub enum DockCommand {
    Tick(AnimationClock),

    BeginResize,

    Resize(f32),

    EndResize,

    Content(imba::dyn_view::DynCommand),
}

impl std::fmt::Display for DockCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockCommand::Content(command) => command.fmt(out),
            DockCommand::Tick(_) => out.write_str("dock tick"),
            DockCommand::BeginResize => out.write_str("dock begin resize"),
            DockCommand::Resize(_) => out.write_str("dock resize"),
            DockCommand::EndResize => out.write_str("dock end resize"),
        }
    }
}

pub(crate) struct Dock {
    content: Box<dyn ModalView>,

    owner: &'static str,

    width: f32,

    reveal: Animation<f32>,

    closing: bool,

    resizing: bool,

    request: Option<ModalRequest>,
}

impl Clone for Dock {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            owner: self.owner,
            width: self.width,
            reveal: self.reveal,
            closing: self.closing,
            resizing: self.resizing,

            request: None,
        }
    }
}

impl Dock {
    pub fn new(content: Box<dyn ModalView>, owner: &'static str, width: f32) -> Self {
        let mut reveal = Animation::done(
            0.0,
            Motion::Ease {
                duration_ms: SLIDE_MS,
                easing: Easing::EaseOut,
            },
        );
        reveal.set(width);
        Self {
            content,
            owner,
            width,
            reveal,
            closing: false,
            resizing: false,
            request: None,
        }
    }

    pub(crate) fn content(&self) -> &dyn ModalView {
        self.content.as_ref()
    }

    pub(crate) fn content_mut(&mut self) -> &mut Box<dyn ModalView> {
        &mut self.content
    }

    pub(crate) fn owner(&self) -> &'static str {
        self.owner
    }

    pub(crate) fn swap(
        &mut self,
        content: Box<dyn ModalView>,
        owner: &'static str,
    ) -> Box<dyn ModalView> {
        if self.closing {
            self.closing = false;
            self.reveal.set(self.width);
        }
        self.owner = owner;
        std::mem::replace(&mut self.content, content)
    }

    /// How much of the column stands revealed, in WHOLE pixels —
    /// the band's rule, the body's rule and the focus edge all land
    /// on the same one.
    pub(crate) fn revealed(&self) -> f32 {
        self.reveal.value().round()
    }

    pub(crate) fn target_width(&self) -> f32 {
        match self.closing {
            true => 0.0,
            false => self.width,
        }
    }

    pub(crate) fn width(&self) -> f32 {
        self.width
    }

    pub(crate) fn roll_away(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.resizing = false;
        self.reveal.set(0.0);
    }

    pub(crate) fn settle(&mut self) {
        self.resizing = false;
        self.reveal.jump(self.width);
    }
}

impl View for Dock {
    type Command = DockCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, DockCommand> {
        self.content
            .as_ref()
            .focus_data_dyn(store, ui)
            .map(DockCommand::Content)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(DockCommand::Content, |fx| {
            imba::dyn_view::DynView::destroy_dyn(self.content_mut().as_mut(), store, fx)
        })
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DockCommand,
        fx: &mut imba::effect::Effects<'_, DockCommand>,
    ) {
        match command {
            DockCommand::Tick(now) => {
                self.reveal.advance(now);
                if self.closing && !self.reveal.running() {
                    self.request = Some(ModalRequest::Close);
                }
            }
            DockCommand::BeginResize => {
                self.resizing = true;
            }
            DockCommand::Resize(width) => {
                self.width = width;
                self.reveal.jump(width);
            }
            DockCommand::EndResize => {
                self.resizing = false;
            }
            DockCommand::Content(command) => fx.scope(DockCommand::Content, |fx| {
                self.content.as_mut().perform_dyn(store, ui, command, fx)
            }),
        }
    }

    /// The side column's BODY: the content over the ground, one
    /// hairline on its left edge (the toolbar's section rule
    /// continues it — the tabs are the band's). The column slides in
    /// from the right by its reveal; a handle on that edge resizes
    /// it.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            use imba::layout::Layout as _;
            use imba::layout::LayoutExt as _;
            let size = constraints.max;
            let revealed = self.revealed();
            let edge = size.width - revealed;
            let mut surface = container(arena, size);

            let theme = ::editor::env::Themes::of(store);
            let ink = theme.ui().ink.clone();
            let body_width = self.width.max(1.0);

            let content = imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
                self.content
                    .as_ref()
                    .layout_dyn(arena, store, ui, constraints)
                    .map(DockCommand::Content)
            });
            let body = imba::layout::Column::new(arena)
                .weighted(1.0, content)
                .sized(body_width, size.height)
                .backdrop(hikit::ui::ground(ink.ground.0))
                .shield();
            surface.place_boxed(
                edge,
                0.0,
                body.layout(arena, Constraints::tight(Size::new(body_width, size.height))),
            );
            // The column's edge rule stands OVER its ground — the
            // split tree's mirror of the chat divider.
            surface.place_boxed(
                edge,
                0.0,
                hikit::ui::rule_v::<DockCommand>(ink.rule.0, size.height)
                    .layout(arena, Constraints::tight(Size::new(1.0, size.height))),
            );

            let animating = self.reveal.running();
            let resizing = self.resizing;
            let surface_width = size.width;
            let handle = leaf::<DockCommand>(HANDLE_REACH * 2.0, size.height).event(
                move |_arena, event, _leaf_size| match event {
                    Event::AnimationClock { now } if animating => {
                        EventResult::command(DockCommand::Tick(*now))
                    }
                    Event::MouseDown { .. } if !animating => {
                        EventResult::command(DockCommand::BeginResize)
                    }
                    Event::MouseDrag { point, .. } if resizing => {
                        let width = (surface_width - (edge - HANDLE_REACH) - point.x)
                            .clamp(DOCK_MIN_WIDTH, surface_width * DOCK_MAX_RATIO);
                        EventResult::command(DockCommand::Resize(width))
                    }
                    Event::MouseUp { .. } if resizing => {
                        EventResult::command(DockCommand::EndResize)
                    }
                    _ => EventResult::Ignored,
                },
            );
            surface.place(edge - HANDLE_REACH, 0.0, handle);

            if animating {
                let clock =
                    leaf::<DockCommand>(1.0, 1.0).event(move |_arena, event, _size| match event {
                        Event::AnimationClock { now } => {
                            EventResult::command(DockCommand::Tick(*now))
                        }
                        _ => EventResult::Ignored,
                    });
                surface.place(0.0, 0.0, clock);
            }
            surface
        })
    }
}

impl Dock {

    pub(crate) fn take_request(&mut self) -> Option<ModalRequest> {
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

    pub(crate) fn release_widgets(
        &mut self,
    ) -> Vec<(
        hikit::panel::WidgetOrigin,
        Box<dyn hikit::panel::DynPanelView>,
    )> {
        self.content.release_widgets()
    }
}
