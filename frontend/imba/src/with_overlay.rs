// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A view with an overlay standing on it — a context menu over a
//! row, a card beside a label. The overlay anchors at the host
//! view's own box and rides the ordinary overlay road to the window
//! (docs/ui/UI.md, Overlays), so a row deep inside a list needs no
//! geometry of its own: every container on the way translates the
//! anchor, the list included.

use skia_safe::{Point, Rect, Size};

use crate::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult, Key},
    focus::FocusData,
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    Thunk, ThunkBox, View,
};

const GAP: f32 = 8.0;

/// Where the overlay lands against the host's box, in host
/// coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Under the box, left-aligned; above it when the room below
    /// runs out (a menu).
    Below,

    /// To the right of the box, top-aligned; to its left when the
    /// room runs out (a tip).
    Beside,

    /// Over the box, left-aligned; under it when the room above runs
    /// out (a card over a word).
    Above,
}

impl Placement {
    pub fn origin(self, host: Size, anchor: Rect, size: Size) -> Point {
        match self {
            Placement::Below => {
                let x = anchor.left.min(host.width - size.width).max(0.0);
                let y = if anchor.bottom + size.height <= host.height {
                    anchor.bottom
                } else if anchor.top - size.height >= 0.0 {
                    anchor.top - size.height
                } else {
                    (host.height - size.height).max(0.0)
                };
                Point::new(x, y)
            }
            Placement::Above => {
                let x = anchor.left.min(host.width - size.width).max(0.0);
                let y = if anchor.top - size.height >= 0.0 {
                    anchor.top - size.height
                } else if anchor.bottom + size.height <= host.height {
                    anchor.bottom
                } else {
                    0.0
                };
                Point::new(x, y)
            }
            Placement::Beside => {
                let mut x = anchor.right + GAP;
                if x + size.width > host.width {
                    x = (anchor.left - GAP - size.width).max(0.0);
                }
                let y = anchor.top.min(host.height - size.height).max(0.0);
                Point::new(x, y)
            }
        }
    }
}

#[derive(Clone)]
pub enum WithOverlayCommand<C, D, O> {
    Host(C),
    Overlay(D),

    /// An overlay stands up on the view.
    Show(O),

    /// The overlay goes: Escape, or a press outside it.
    Dismiss,
}

impl<C: std::fmt::Display, D: std::fmt::Display, O> std::fmt::Display
    for WithOverlayCommand<C, D, O>
{
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WithOverlayCommand::Host(command) => command.fmt(out),
            WithOverlayCommand::Overlay(command) => command.fmt(out),
            WithOverlayCommand::Show(_) => out.write_str("show overlay"),
            WithOverlayCommand::Dismiss => out.write_str("dismiss overlay"),
        }
    }
}

#[derive(Clone)]
pub struct WithOverlay<V, O> {
    pub view: V,
    overlay: Option<O>,
    placement: Placement,

    /// A press anywhere outside the overlay dismisses it (a menu).
    backdrop: bool,
}

impl<V, O> WithOverlay<V, O> {
    pub fn new(view: V, placement: Placement) -> Self {
        Self {
            view,
            overlay: None,
            placement,
            backdrop: false,
        }
    }

    pub fn dismissing_on_outside_press(mut self) -> Self {
        self.backdrop = true;
        self
    }

    pub fn overlay(&self) -> Option<&O> {
        self.overlay.as_ref()
    }
}

impl<V, O> View for WithOverlay<V, O>
where
    V: View,
    O: View + Clone + Send + Sync + 'static,
{
    type Command = WithOverlayCommand<V::Command, O::Command, O>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            WithOverlayCommand::Host(command) => fx.scope(WithOverlayCommand::Host, |fx| {
                self.view.perform(store, ui, command, fx)
            }),
            WithOverlayCommand::Overlay(command) => {
                if let Some(overlay) = &mut self.overlay {
                    fx.scope(WithOverlayCommand::Overlay, |fx| {
                        overlay.perform(store, ui, command, fx)
                    });
                }
            }
            WithOverlayCommand::Show(overlay) => self.overlay = Some(overlay),
            WithOverlayCommand::Dismiss => self.overlay = None,
        }
    }

    /// A standing overlay answers the keys ahead of its host; Escape
    /// dismisses it.
    fn focus_data<'w>(&'w self, store: &'w Store, ui: &'w UiCtx) -> FocusData<'w, Self::Command> {
        let host = self
            .view
            .focus_data(store, ui)
            .map(WithOverlayCommand::Host);
        let Some(overlay) = &self.overlay else {
            return host;
        };
        let own = FocusData {
            on_key: Some(Box::new(|key, _mods| match key {
                Key::Escape => EventResult::Command(WithOverlayCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            overlay
                .focus_data(store, ui)
                .map(WithOverlayCommand::Overlay),
        )
        .merge_under(host)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::layout::Layout<'a, Self::Command> + crate::layout::LayoutValue + 'a {
        crate::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let inner = crate::layout::Layout::layout(
                self.view.display(arena, store, ui),
                arena,
                constraints,
            )
            .map(WithOverlayCommand::Host);
            let Some(overlay) = &self.overlay else {
                return ThunkBox::new(arena, inner);
            };
            let placement = self.placement;
            let backdrop = self.backdrop;
            ThunkBox::new(
                arena,
                inner.overlay(crate::overlay::WINDOW, move |host: Size, anchor: Rect| {
                    let content = ThunkBox::new(
                        arena,
                        crate::layout::Layout::layout(
                            overlay.display(arena, store, ui),
                            arena,
                            Constraints::tight(host).loosen(),
                        )
                        .map(WithOverlayCommand::Overlay),
                    );
                    let origin = placement.origin(host, anchor, content.size());
                    let mut placed = Vec::with_capacity(2);
                    if backdrop {
                        let veil = leaf::<Self::Command>(host.width, host.height).event(
                            |_arena, event, _size| match event {
                                Event::MouseDown { .. } => {
                                    EventResult::Command(WithOverlayCommand::Dismiss)
                                }
                                _ => EventResult::Ignored,
                            },
                        );
                        placed.push((Point::new(0.0, 0.0), ThunkBox::new(arena, veil)));
                    }
                    placed.push((origin, content));
                    placed
                }),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Widget;

    #[test]
    fn below_drops_under_the_anchor_and_flips_above_at_the_edge() {
        let host = Size::new(800.0, 600.0);
        let size = Size::new(100.0, 50.0);
        let under = Placement::Below.origin(host, Rect::from_xywh(100.0, 100.0, 40.0, 20.0), size);
        assert_eq!(under, Point::new(100.0, 120.0));
        let flipped =
            Placement::Below.origin(host, Rect::from_xywh(100.0, 580.0, 40.0, 20.0), size);
        assert_eq!(flipped, Point::new(100.0, 530.0));
        let clamped =
            Placement::Below.origin(host, Rect::from_xywh(780.0, 100.0, 40.0, 20.0), size);
        assert_eq!(clamped.x, 700.0, "never past the host's right edge");
    }

    #[test]
    fn beside_sits_right_of_the_anchor_and_flips_left_at_the_edge() {
        let host = Size::new(800.0, 600.0);
        let size = Size::new(100.0, 50.0);
        let right = Placement::Beside.origin(host, Rect::from_xywh(100.0, 100.0, 40.0, 20.0), size);
        assert_eq!(right, Point::new(148.0, 100.0));
        let flipped =
            Placement::Beside.origin(host, Rect::from_xywh(700.0, 100.0, 40.0, 20.0), size);
        assert_eq!(flipped, Point::new(592.0, 100.0));
    }

    #[derive(Clone)]
    struct Box_(f32, f32);
    impl View for Box_ {
        type Command = u32;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &UiCtx,
            _command: u32,
            _fx: &mut Effects<'_, u32>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a UiCtx,
        ) -> impl crate::layout::Layout<'a, u32> + crate::layout::LayoutValue + 'a {
            let (width, height) = (self.0, self.1);
            crate::layout::laid(move |_arena: &'a Arena, _constraints: Constraints| {
                leaf(width, height)
            })
        }
    }

    fn overlays(view: &WithOverlay<Box_, Box_>) -> usize {
        let arena = Arena::default();
        let store = Store::new();
        let ui = UiCtx::dont_use_too_slow();
        let mut widget = crate::layout::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 40.0)),
        )
        .realize(&arena, Rect::from_wh(200.0, 40.0));
        let count = widget.overlays().len();
        drop(widget);
        count
    }

    #[test]
    fn a_shown_overlay_rides_the_host_and_dismisses() {
        let mut view = WithOverlay::new(Box_(200.0, 40.0), Placement::Below);
        assert_eq!(overlays(&view), 0);
        let mut store = Store::new();
        let ui = UiCtx::dont_use_too_slow();
        let mut batch = crate::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            WithOverlayCommand::Show(Box_(60.0, 24.0)),
            &mut batch.effects(),
        );
        assert_eq!(overlays(&view), 1, "the overlay is one request on the host");
        view.perform(
            &mut store,
            &ui,
            WithOverlayCommand::Dismiss,
            &mut batch.effects(),
        );
        assert_eq!(overlays(&view), 0);
    }
}
