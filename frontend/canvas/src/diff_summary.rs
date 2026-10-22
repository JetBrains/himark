// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The canvas SUMMARY PILL: the capsule floating top-center over the
//! scroll — the change glyph, "N files", the `+A −R` totals, and the
//! expansion state with its dropdown chevron. One press toggles every
//! file: collapse all while anything stands open, expand all after.
//! The canvas computes the numbers and the state; the pill only draws
//! and answers "was that press mine?".

use imba::{store::Store, ui::UiCtx};
use skia_safe::{Paint, Rect};

use editor::env;

/// What the canvas asks the pill to show.
pub struct SummarySpec {
    pub files: usize,
    pub added: i64,
    pub removed: i64,
    /// Of the files, how many stand collapsed.
    pub collapsed: usize,
}

pub struct SummaryPill {
    frame: Rect,
    baseline: f32,

    font: skia_safe::Font,
    shaper: std::rc::Rc<imba::layout::TextShaper>,

    background: skia_safe::Color,
    border: skia_safe::Color,
    text_color: skia_safe::Color,
    dim_color: skia_safe::Color,
    added_color: skia_safe::Color,
    removed_color: skia_safe::Color,

    files: String,
    added: Option<String>,
    removed: Option<String>,
    state: String,
}

/// The pill's top margin inside the pane.
const TOP: f32 = 10.0;

/// The whole strip the pill occupies above the first card — margin,
/// capsule, the gap to the cards. The canvas pads its list down by
/// this much; the frontend-host click tests mirror it.
pub fn strip_height(title_size: f32) -> f32 {
    TOP + (title_size * 0.9 * 2.1).ceil() + 12.0
}

impl SummaryPill {
    pub fn new(store: &Store, ui: &UiCtx, spec: SummarySpec, width: f32) -> Self {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let diff = theme.ui().diff.clone();
        let window = theme.ui().window.clone();
        let size = chrome.title_size * 0.9;
        let font = hikit::fonts::ui_text_font(ui, size);
        let shaper = imba::layout::TextShaper::of(ui);

        let files = match spec.files {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        let added = (spec.added > 0).then(|| format!("+{}", spec.added));
        let removed = (spec.removed > 0).then(|| format!("−{}", spec.removed));
        let state = match (spec.collapsed, spec.files - spec.collapsed.min(spec.files)) {
            (0, _) => "all expanded".to_owned(),
            (_, 0) => "all collapsed".to_owned(),
            (_, open) => format!("{open} expanded"),
        };

        // Measure the row: glyph, files, counts, divider, state and
        // its chevron — the capsule hugs the content.
        let gap = size * 0.55;
        let pad = size * 0.95;
        let glyph = size * 0.9;
        let chevron = size * 0.55;
        let mut content = glyph + gap + shaper.advance(&font, &files);
        if let Some(added) = &added {
            content += gap + shaper.advance(&font, added);
        }
        if let Some(removed) = &removed {
            content += gap * 0.6 + shaper.advance(&font, removed);
        }
        content += gap + 1.0 + gap + shaper.advance(&font, &state) + gap * 0.5 + chevron;

        let height = (size * 2.1).ceil();
        let pill_width = (content + pad * 2.0).ceil();
        let frame = Rect::from_xywh(((width - pill_width) * 0.5).max(0.0), TOP, pill_width, height);

        Self {
            frame,
            baseline: height * 0.5 + size * 0.36,
            font,
            shaper,
            background: diff.plaque_header.0,
            border: window.divider.0,
            text_color: chrome.text_color.0,
            dim_color: chrome.loader_color.0,
            added_color: chrome.added_color.0,
            removed_color: chrome.removed_color.0,
            files,
            added,
            removed,
            state,
        }
    }

    /// The strip the pill needs clear above the first card — the
    /// canvas pads the list down by this much.
    pub fn clearance(&self) -> f32 {
        self.frame.bottom + 12.0
    }

    pub fn hit(&self, point: skia_safe::Point) -> bool {
        point.x >= self.frame.left
            && point.x < self.frame.right
            && point.y >= self.frame.top
            && point.y < self.frame.bottom
    }

    pub fn paint(&self, canvas: &skia_safe::Canvas) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        let radius = self.frame.height() * 0.5;

        paint.set_color(self.background);
        canvas.draw_round_rect(self.frame, radius, radius, &paint);

        let size = self.font.size();
        let gap = size * 0.55;
        let baseline = self.frame.top + self.baseline;
        let mut x = self.frame.left + size * 0.95;

        // The change glyph: the COMPARE mark — two nodes joined by a
        // curve, the mock's leading icon.
        let glyph = size * 0.9;
        let mid = baseline - size * 0.36;
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_color(self.dim_color);
        paint.set_stroke_width((glyph * 0.14).max(1.0));
        paint.set_stroke_cap(skia_safe::paint::Cap::Round);
        let dot = glyph * 0.16;
        let (a, b) = (
            (x + dot, mid - glyph * 0.34),
            (x + glyph - dot, mid + glyph * 0.34),
        );
        canvas.draw_circle(a, dot, &paint);
        canvas.draw_circle(b, dot, &paint);
        let mut path = skia_safe::PathBuilder::new();
        path.move_to((a.0, a.1 + dot));
        path.cubic_to(
            (a.0, mid + glyph * 0.1),
            (b.0, mid - glyph * 0.1),
            (b.0, b.1 - dot),
        );
        canvas.draw_path(&path.detach(), &paint);
        paint.set_style(skia_safe::paint::Style::Fill);
        x += glyph + gap;

        x += self
            .shaper
            .draw(canvas, &self.font, &self.files, self.text_color, 0.0, x, baseline);
        if let Some(added) = &self.added {
            x += gap;
            x += self
                .shaper
                .draw(canvas, &self.font, added, self.added_color, 0.0, x, baseline);
        }
        if let Some(removed) = &self.removed {
            x += gap * 0.6;
            x += self
                .shaper
                .draw(canvas, &self.font, removed, self.removed_color, 0.0, x, baseline);
        }

        // The divider rule before the expansion state.
        x += gap;
        paint.set_color(self.border);
        canvas.draw_rect(
            Rect::from_xywh(x, self.frame.top + 6.0, 1.0, self.frame.height() - 12.0),
            &paint,
        );
        x += 1.0 + gap;

        x += self
            .shaper
            .draw(canvas, &self.font, &self.state, self.dim_color, 0.0, x, baseline);

        // The dropdown chevron.
        let chevron = size * 0.55;
        x += gap * 0.5;
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_color(self.dim_color);
        paint.set_stroke_width((chevron * 0.22).max(1.0));
        let mut path = skia_safe::PathBuilder::new();
        path.move_to((x, mid - chevron * 0.18));
        path.line_to((x + chevron * 0.5, mid + chevron * 0.3));
        path.line_to((x + chevron, mid - chevron * 0.18));
        canvas.draw_path(&path.detach(), &paint);
    }
}
