// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff FILE BAND, drawn once for every place a diff shows a file
//! header: the canvas rows and the chat's edit cells. The shape is the
//! canvas's own — the fold chevron (where folding exists), the `+N −M`
//! trail leading, the file name right-aligned before the hairline
//! buttons at the right edge, the band's body pressing the primary
//! action. Call sites pick the affordances and map the presses onto
//! their own command type; the geometry and the glyphs live here.

use imba::{
    arena::Arena,
    event::{Event, EventResult},
    store::Store,
    UiCtx, Widget,
};
use skia_safe::{Paint, Rect, Size};

use crate::env;

/// What a press on the band means — the canvas header's affordances.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffHeaderPress {
    /// The band's body (and the open-file button): the live file.
    OpenFile,
    /// The framed pair: the standalone side-by-side pane.
    OpenPane,
    /// The two columns: inline ⇄ split.
    ToggleFace,
    /// The chevron: fold the diff away / bring it back.
    ToggleCollapse,
}

/// What a call site asks the band to show.
pub(crate) struct DiffHeaderSpec {
    pub title: String,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    /// Some(collapsed): the fold chevron stands at the left edge.
    pub chevron: Option<bool>,
    /// The right-edge buttons, RIGHTMOST FIRST (the canvas order).
    pub buttons: Vec<DiffHeaderPress>,
    /// What the band's body presses. None: the body is inert.
    pub primary: Option<DiffHeaderPress>,
    /// Title typography — the canvas passes its H1, the chat its cell
    /// title. The trail and the glyphs size from the chat chrome in
    /// both, so the affordances match across the app.
    pub title_size: f32,
    pub bold: bool,
    pub title_color: Option<skia_safe::Color>,
    /// The text baseline, from the band's top.
    pub baseline: f32,
}

pub(crate) struct DiffHeaderFace {
    title: String,
    added: Option<i64>,
    removed: Option<i64>,

    band: f32,
    width: f32,
    inset: f32,
    baseline: f32,

    title_font: skia_safe::Font,
    title_color: skia_safe::Color,
    trail_font: skia_safe::Font,
    shaper: std::rc::Rc<imba::TextShaper>,
    added_color: skia_safe::Color,
    removed_color: skia_safe::Color,
    affordance_color: skia_safe::Color,

    chevron: Option<(Rect, bool)>,
    buttons: Vec<(DiffHeaderPress, Rect)>,
    primary: Option<DiffHeaderPress>,
}

impl DiffHeaderFace {
    pub(crate) fn new(
        store: &Store,
        ui: &UiCtx,
        spec: DiffHeaderSpec,
        band: f32,
        width: f32,
    ) -> Self {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let mut title_font = crate::fonts::ui_text_font(ui, spec.title_size);
        if spec.bold {
            title_font.set_embolden(true);
        }
        let inset = chrome.pad;
        let glyph = chrome.title_size * 1.2;
        let zone = glyph + chrome.title_size;

        // Button zones, right edge inward — first in the spec sits
        // rightmost, the canvas's own walk.
        let mut buttons = Vec::new();
        let mut right = width - inset;
        for press in spec.buttons {
            buttons.push((press, Rect::from_xywh(right - zone, 0.0, zone, band)));
            right -= zone;
        }

        Self {
            title: spec.title,
            added: spec.added.filter(|n| *n > 0),
            removed: spec.removed.filter(|n| *n > 0),
            band,
            width,
            inset,
            baseline: spec.baseline,
            title_font,
            title_color: spec.title_color.unwrap_or(chrome.text_color.0),
            trail_font: crate::fonts::ui_text_font(ui, chrome.title_size),
            shaper: imba::TextShaper::of(ui),
            added_color: chrome.added_color.0,
            removed_color: chrome.removed_color.0,
            affordance_color: chrome.loader_color.0,
            chevron: spec.chevron.map(|collapsed| {
                (
                    Rect::from_xywh(0.0, 0.0, inset + chrome.title_size, band),
                    collapsed,
                )
            }),
            buttons,
            primary: spec.primary,
        }
    }

    pub(crate) fn widget<Command, Map>(self, map: Map) -> DiffHeaderWidget<Map>
    where
        Map: Fn(DiffHeaderPress) -> Option<Command>,
    {
        DiffHeaderWidget { face: self, map }
    }

    fn press_at(&self, x: f32) -> Option<DiffHeaderPress> {
        if let Some((zone, _)) = &self.chevron {
            if zone.right > x {
                return Some(DiffHeaderPress::ToggleCollapse);
            }
        }
        for (press, zone) in &self.buttons {
            if x >= zone.left && x < zone.right {
                return Some(*press);
            }
        }
        self.primary
    }

    /// Where the trail (and the title's left clamp) starts: after the
    /// chevron when one stands, at the inset otherwise.
    fn lead(&self) -> f32 {
        match &self.chevron {
            Some((zone, _)) => zone.right,
            None => self.inset,
        }
    }

    fn paint(&self, canvas: &skia_safe::Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        let baseline = rect.top + self.baseline;

        // The chevron: right-pointing when collapsed, down when open.
        if let Some((zone, collapsed)) = &self.chevron {
            let glyph = self.trail_font.size() * 0.5;
            let center = (
                rect.left + zone.left + self.inset * 0.5 + glyph * 0.5,
                baseline - glyph * 0.6,
            );
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((glyph * 0.22).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            paint.set_color(self.affordance_color);
            let mut path = skia_safe::PathBuilder::new();
            if *collapsed {
                path.move_to((center.0 - glyph * 0.25, center.1 - glyph * 0.5));
                path.line_to((center.0 + glyph * 0.35, center.1));
                path.line_to((center.0 - glyph * 0.25, center.1 + glyph * 0.5));
            } else {
                path.move_to((center.0 - glyph * 0.5, center.1 - glyph * 0.25));
                path.line_to((center.0, center.1 + glyph * 0.35));
                path.line_to((center.0 + glyph * 0.5, center.1 - glyph * 0.25));
            }
            canvas.draw_path(&path.detach(), &paint);
            paint.set_style(skia_safe::paint::Style::Fill);
        }

        // The +N −M trail after the chevron.
        let mut x = rect.left + self.lead();
        if let Some(added) = self.added {
            let label = format!("+{added}");
            x += self.shaper.draw(
                canvas,
                &self.trail_font,
                &label,
                self.added_color,
                0.0,
                x,
                baseline,
            ) + 8.0;
        }
        if let Some(removed) = self.removed {
            self.shaper.draw(
                canvas,
                &self.trail_font,
                &format!("−{removed}"),
                self.removed_color,
                0.0,
                x,
                baseline,
            );
        }

        // The buttons, hairline glyphs on the affordance color.
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_color(self.affordance_color);
        let side = self.trail_font.size() * 1.05;
        paint.set_stroke_width((side * 0.11).max(1.0));
        for (press, zone) in &self.buttons {
            let center = (
                rect.left + zone.left + zone.width() * 0.5,
                baseline - side * 0.42,
            );
            let half = side * 0.5;
            let frame = Rect::from_xywh(center.0 - half, center.1 - half, side, side);
            match press {
                DiffHeaderPress::ToggleFace => {
                    // Two columns — switch the diff's face.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    canvas.draw_line((center.0, frame.top), (center.0, frame.bottom), &paint);
                }
                DiffHeaderPress::OpenFile => {
                    // A corner arrow leaving the box.
                    let inset = side * 0.22;
                    let mut path = skia_safe::PathBuilder::new();
                    path.move_to((frame.left + side * 0.5, frame.top + inset));
                    path.line_to((frame.left + inset, frame.top + inset));
                    path.line_to((frame.left + inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.top + side * 0.5));
                    canvas.draw_path(&path.detach(), &paint);
                    let mut arrow = skia_safe::PathBuilder::new();
                    arrow.move_to((center.0 + side * 0.05, center.1 - side * 0.05));
                    arrow.line_to((frame.right, frame.top));
                    arrow.move_to((frame.right - side * 0.32, frame.top));
                    arrow.line_to((frame.right, frame.top));
                    arrow.line_to((frame.right, frame.top + side * 0.32));
                    canvas.draw_path(&arrow.detach(), &paint);
                }
                DiffHeaderPress::OpenPane => {
                    // A framed pair of panes — the standalone
                    // side-by-side diff.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    let third = frame.left + frame.width() * 0.5;
                    canvas.draw_line((third, frame.top), (third, frame.bottom), &paint);
                    let mid = frame.top + frame.height() * 0.5;
                    canvas.draw_line((frame.left, mid), (third, mid), &paint);
                }
                _ => {}
            }
        }
        paint.set_style(skia_safe::paint::Style::Fill);

        // The file name, right-aligned before the buttons.
        let buttons_left = self
            .buttons
            .last()
            .map(|(_, zone)| zone.left)
            .unwrap_or(self.width - self.inset);
        let title_width = self.shaper.advance(&self.title_font, &self.title);
        self.shaper.draw(
            canvas,
            &self.title_font,
            &self.title,
            self.title_color,
            0.0,
            (rect.left + buttons_left - self.inset - title_width).max(rect.left + self.lead()),
            baseline,
        );
    }
}

pub(crate) struct DiffHeaderWidget<Map> {
    face: DiffHeaderFace,
    map: Map,
}

impl<'a, Command, Map> Widget<'a, Command> for DiffHeaderWidget<Map>
where
    Map: Fn(DiffHeaderPress) -> Option<Command>,
{
    fn size(&self) -> Size {
        Size::new(self.face.width, self.face.band)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { canvas, .. } => {
                self.face.paint(canvas, Rect::from_size(self.size()));
                EventResult::Handled
            }
            Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point,
                ..
            } => match self
                .face
                .press_at(point.x)
                .and_then(|press| (self.map)(press))
            {
                Some(command) => EventResult::Command(command),
                None => EventResult::Ignored,
            },
            _ => EventResult::Ignored,
        }
    }
}
