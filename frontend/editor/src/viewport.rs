// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use skia_safe::{Canvas, Color, Paint, Rect};

use crate::markup::{BlockStyle, InlayMetrics, TextDecorationInterval};
use crate::shaped_line::{line_text_x, ShapedLine};

pub(crate) struct ViewportLine {
    pub(crate) byte_start: u32,
    pub(crate) byte_end: u32,

    pub(crate) top: f32,

    pub(crate) spacer: f32,
    pub(crate) height: f32,

    pub(crate) text_top: f32,

    pub(crate) x: f32,
    pub(crate) marks: BlockStyle,

    pub(crate) box_extent: Option<f32>,
    pub(crate) inlays: InlayMetrics,

    #[allow(dead_code)]
    pub(crate) inline: Range<usize>,
    #[allow(dead_code)]
    pub(crate) hidden: Range<usize>,

    pub(crate) shaped: Option<std::rc::Rc<ShapedLine>>,

    pub(crate) hard_line: Option<u32>,

    pub(crate) foldable: Option<LineFoldable>,

    pub(crate) baseline: Option<f32>,

    pub(crate) diff: Option<DiffLineKind>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffLineKind {
    Added,

    Modified,

    DeletedAbove,
}

/// Classifies visible rows against THE diff markup — the hunk entry
/// every tracked diff maintains (docs/scroll-stripe.md §7): one
/// `Diff*`-styled interval per hunk, target coordinates, shifted at
/// the edit door. The pane's washes and the scroll track read the
/// same entry; the three faces can never disagree.
struct StripeWalk<'a> {
    iter: crate::markup::RecursiveQuery<'a>,

    peeked: Option<(Range<u32>, crate::markup::StyleId)>,

    active: Vec<(Range<u32>, crate::markup::StyleId)>,
}

impl<'a> StripeWalk<'a> {
    fn new(markup: &'a crate::markup::Markup, from: u32) -> Self {
        use intervals::IntervalQuery;
        let mut iter = markup.query(from..u32::MAX, intervals::Order::Ascending);
        let peeked = Self::pull(&mut iter);
        Self {
            iter,
            peeked,
            active: Vec::new(),
        }
    }

    fn pull(
        iter: &mut crate::markup::RecursiveQuery<'a>,
    ) -> Option<(Range<u32>, crate::markup::StyleId)> {
        iter.find_map(|hit| match hit.value {
            crate::markup::Decoration::Styled(id) => Some((hit.range.clone(), *id)),
            _ => None,
        })
    }

    fn classify(&mut self, range: Range<u32>) -> Option<DiffLineKind> {
        use crate::markup::StyleId;
        while let Some((peeked, _)) = &self.peeked {
            if peeked.start >= range.end {
                break;
            }
            let entered = self.peeked.take().expect("peeked");
            self.active.push(entered);
            self.peeked = Self::pull(&mut self.iter);
        }
        // A zero-length interval is a deletion MARKER at its point;
        // it belongs to the row whose start it sits at.
        self.active.retain(|(hit, _)| {
            hit.end > range.start || (hit.start == hit.end && hit.start >= range.start)
        });

        let mut added_overlap = 0u32;
        let mut modified = false;
        let mut deleted_at_start = false;
        let mut deleted_inside = false;
        for (hit, style) in &self.active {
            match style {
                StyleId::DiffAdded => {
                    let overlap_start = hit.start.max(range.start);
                    let overlap_end = hit.end.min(range.end);
                    added_overlap += overlap_end.saturating_sub(overlap_start);
                }
                StyleId::DiffModified => {
                    if hit.start < range.end && hit.end > range.start {
                        modified = true;
                    }
                }
                StyleId::DiffDeleted => {
                    if hit.start == range.start {
                        deleted_at_start = true;
                    } else if hit.start > range.start && hit.start < range.end {
                        deleted_inside = true;
                    }
                }
                _ => {}
            }
        }
        let row = range.end.saturating_sub(range.start);
        if row > 0 && added_overlap >= row {
            Some(match deleted_at_start {
                true => DiffLineKind::Modified,
                false => DiffLineKind::Added,
            })
        } else if added_overlap > 0 || modified || deleted_inside {
            Some(DiffLineKind::Modified)
        } else if deleted_at_start {
            Some(DiffLineKind::DeletedAbove)
        } else {
            None
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct LineFoldable {
    pub(crate) range: Range<u32>,
    pub(crate) folded: bool,

    pub(crate) spin: f32,
}

pub(crate) struct EditorViewport {
    pub(crate) lines: Vec<ViewportLine>,
    pub(crate) inline: Vec<TextDecorationInterval>,
    pub(crate) hidden: Vec<Range<u32>>,

    pub(crate) selections: Vec<Range<u32>>,
    pub(crate) layout_width: f32,

    pub(crate) tail_spacer: Option<(f32, f32)>,
}

impl EditorViewport {
    fn selections_in(&self, start: u32, end: u32) -> &[Range<u32>] {
        let from = self.selections.partition_point(|range| range.end <= start);
        let to = from + self.selections[from..].partition_point(|range| range.start < end);
        &self.selections[from..to]
    }

    pub(crate) fn build(
        document: &crate::document::Document,
        editor: crate::editor::EditorId,
        band: Range<f32>,
        focused: bool,
        gutter: bool,
        stripes: Option<crate::diff::DiffId>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Self {
        let state = document.editor(editor);
        let layout = &state.layout;
        let extras = document.extras_keyed(editor);
        let overlaid = crate::markup::OverlaidMarkup::new(document.markup(), &extras);
        let layout_width = layout.layout_width();

        let text_focused = focused && state.focus == crate::EditorFocus::Text;
        let selections: Vec<Range<u32>> = match text_focused {
            true => state
                .carets
                .carets()
                .iter()
                .filter(|caret| caret.has_selection())
                .map(|caret| caret.selection())
                .collect(),
            false => Vec::new(),
        };

        let mut viewport = Self {
            lines: Vec::with_capacity(64),
            inline: Vec::with_capacity(64),
            hidden: Vec::with_capacity(16),
            selections,
            layout_width,
            tail_spacer: None,
        };
        if layout.is_empty() {
            return viewport;
        }

        let probing = crate::env_flags::paint_probe();
        let probe = probing.then(std::time::Instant::now);

        crate::shape_cache::begin_build();
        let markup_changed_since =
            |since: u64, range: Range<u32>| document.markup_changed_in(since, range);
        let stamp = crate::shape_cache::ShapeStamp {
            token: document.token(),
            editor,
            revision: document.revision(),
            markup_generation: document.markup_generation(),
            markup_changed_since: &markup_changed_since,
            theme: theme.name_shared(),
            width_bits: layout_width.to_bits(),
        };

        let mut view = document.text().view();

        let mut box_cache: Option<(Range<u32>, f32)> = None;
        let (mut cursor, mut document_y, mut byte_start) = layout.cursor_at_y(band.start);

        let mut stripe_walk = (gutter)
            .then_some(stripes)
            .flatten()
            .and_then(|id| document.diff(id))
            .and_then(|entry| document.feature_markup(entry.markup()))
            .map(|markup| StripeWalk::new(markup, byte_start));
        let mut inline_scratch = Vec::new();
        let mut hidden_scratch = Vec::new();

        let mut carried_spacer = 0.0f32;
        let mut shaped_lines = 0usize;

        const FOLD_CHUNK: u32 = 4096;
        let mut foldable_chunk: Vec<Range<u32>> = Vec::new();
        let mut foldable_chunk_range = 0u32..0u32;
        let mut foldable_at = 0usize;

        // The band is swept in CONTIGUOUS VISIBLE SEGMENTS: the
        // sweep opens lazily at each segment's first line and is
        // DROPPED at every collapsed run (a fold, or a windowed
        // fragment's prefix — the zero-height items carrying bytes).
        // A fresh query then seeks past the gap in the interval
        // tree, so the gap's markup is never pulled: without the
        // split, one fold squashing a large file made the next
        // line's sweep swallow the whole gap — linear per frame
        // (the DiffCanvas trace, 2026-09-17).
        let mut marks_sweep: Option<crate::markup::LineMarksSweep<'_>> = None;

        let probe_byte_start = byte_start;
        let mut probe_iterations = 0usize;
        let mut probe_flat = 0usize;
        loop {
            probe_iterations += 1;
            let item = cursor.element();
            let byte_end = byte_start.saturating_add(item.byte_size);

            if item.height <= 0.0 {
                probe_flat += 1;
                if item.byte_size > 0 {
                    // A collapsed run ends the segment — the next
                    // visible line re-seeds the sweep past the gap.
                    marks_sweep = None;
                }
                document_y += item.height + item.spacer_above;
                carried_spacer += item.spacer_above;
                if !cursor.advance() {
                    break;
                }
                byte_start = byte_end;
                continue;
            }
            let line_range = byte_start..byte_end;
            let (marks, inlays) = marks_sweep
                .get_or_insert_with(|| overlaid.line_marks_sweep(byte_start, Some(layout_width)))
                .line(line_range.clone(), &mut inline_scratch, &mut hidden_scratch);

            if gutter
                && (line_range.end > foldable_chunk_range.end
                    || line_range.start < foldable_chunk_range.start)
            {
                foldable_chunk.clear();
                let from = line_range.start;
                let to = line_range.end.max(from.saturating_add(FOLD_CHUNK));
                document.foldables_into(from..to, &mut foldable_chunk);
                foldable_chunk_range = from..to;
                foldable_at = 0;
            }
            let item_top = document_y + item.spacer_above;
            document_y += item.height + item.spacer_above;

            if item_top > band.end {
                let spacer = item.spacer_above + carried_spacer;
                if spacer > 0.5 && item_top - spacer < band.end {
                    viewport.tail_spacer = Some((item_top - spacer, spacer));
                }
                break;
            }

            let inline_start = viewport.inline.len();
            viewport.inline.extend(inline_scratch.iter().cloned());

            if let Some(marked) = &state.marked {
                let lo = marked.start.max(byte_start);
                let hi = marked.end.min(byte_end);
                if lo < hi {
                    viewport.inline.push(TextDecorationInterval {
                        range: lo..hi,
                        id: crate::markup::StyleId::Composing,
                    });
                }
            }
            let inline_end = viewport.inline.len();
            let hidden_start = viewport.hidden.len();
            viewport.hidden.extend(hidden_scratch.iter().cloned());
            let hidden_end = viewport.hidden.len();

            let resolved = marks.resolved(theme);
            let text_top = item_top + inlays.above_height;

            let shaped = if resolved.rule.is_some() || inlays.has_instead() {
                None
            } else {
                let selected = !viewport.selections_in(byte_start, byte_end).is_empty();

                let marked = state.marked.as_ref().and_then(|marked| {
                    let lo = marked.start.max(byte_start);
                    let hi = marked.end.min(byte_end);
                    (lo < hi).then_some(lo..hi)
                });
                Some(crate::shape_cache::shaped(
                    &stamp,
                    line_range.clone(),
                    selected,
                    marked,
                    || {
                        shaped_lines += 1;
                        ShapedLine::new(
                            &mut view,
                            overlaid,
                            line_range.clone(),
                            &marks,
                            &viewport.inline[inline_start..inline_end],
                            &viewport.hidden[hidden_start..hidden_end],
                            fonts,
                            theme,
                            layout_width,
                            0.0,
                            selected,
                        )
                    },
                ))
            };

            let baseline = gutter
                .then(|| {
                    shaped
                        .as_ref()
                        .and_then(|shaped| shaped.first_baseline())
                        .map(|paragraph_baseline| text_top + paragraph_baseline)
                })
                .flatten();

            let hard_line = (gutter && !inlays.has_instead())
                .then(|| starts_hard_line(&mut view, byte_start))
                .flatten()
                .map(|_| view.line_at(byte_start as usize).0 as u32 + 1);

            while foldable_at < foldable_chunk.len()
                && foldable_chunk[foldable_at].start < line_range.start
            {
                foldable_at += 1;
            }
            let first_foldable = foldable_chunk
                .get(foldable_at)
                .filter(|range| gutter && range.start < line_range.end);
            let foldable = first_foldable.map(|range| {
                let standing = document.fold_matching(editor, range);
                let spin = standing
                    .and_then(|key| document.fold_chip_at(key))
                    .map_or(0.0, |chip| chip.spin(theme.ui().fold_chip.height));
                LineFoldable {
                    range: range.clone(),
                    folded: standing.is_some(),
                    spin,
                }
            });

            let diff = stripe_walk
                .as_mut()
                .and_then(|walk| walk.classify(byte_start..byte_end));

            let box_extent = resolved
                .background
                .filter(|background| background.kind == crate::theme::BackgroundKind::Box)
                .map(|_| match &box_cache {
                    Some((range, extent)) if range.contains(&byte_start) => *extent,
                    _ => {
                        let range = overlaid
                            .styled_range_at(byte_start, |id| {
                                theme.attributes(id).background.is_some_and(|background| {
                                    background.kind == crate::theme::BackgroundKind::Box
                                })
                            })
                            .unwrap_or(byte_start..byte_end);
                        let extent = layout.max_width_in(range.clone());
                        box_cache = Some((range, extent));
                        extent
                    }
                });
            viewport.lines.push(ViewportLine {
                byte_start,
                byte_end,
                top: item_top,
                spacer: item.spacer_above + std::mem::take(&mut carried_spacer),
                height: item.height,
                text_top,
                x: line_text_x(0.0, &resolved),
                marks,
                box_extent,
                inlays,
                inline: inline_start..inline_end,
                hidden: hidden_start..hidden_end,
                shaped,
                hard_line,
                foldable,
                baseline,
                diff,
            });

            if !cursor.advance() {
                break;
            }
            byte_start = byte_end;
        }

        if let Some(started) = probe {
            eprintln!(
                "[paint-probe] build={:.1}us lines={} shaped={} iters={} flat={} pulls={} active_peak={} y={} byte={} bounded={}",
                started.elapsed().as_secs_f64() * 1e6,
                viewport.lines.len(),
                shaped_lines,
                probe_iterations,
                probe_flat,
                marks_sweep.as_ref().map_or(0, |sweep| sweep.pulls),
                marks_sweep.as_ref().map_or(0, |sweep| sweep.active_peak),
                band.start,
                probe_byte_start,
                state.bounds.is_some(),
            );
        }
        viewport
    }

    pub(crate) fn paint(&self, canvas: &Canvas, theme: &crate::theme::Theme) {
        let probing = crate::env_flags::paint_probe();
        let probe = probing.then(std::time::Instant::now);
        let mut paint = Paint::default();
        paint.set_anti_alias(true);

        paint_rectangular_backgrounds(
            canvas,
            &mut paint,
            0.0,
            self.layout_width,
            &self.lines,
            theme,
        );

        for line in &self.lines {
            if line.spacer > 0.5 {
                paint.set_color(theme.ui().diff.spacer_fill.0);
                canvas.draw_rect(
                    Rect::from_xywh(
                        line.x,
                        line.top - line.spacer,
                        self.layout_width,
                        line.spacer,
                    ),
                    &paint,
                );
            }
        }

        if let Some((top, height)) = self.tail_spacer {
            paint.set_color(theme.ui().diff.spacer_fill.0);
            canvas.draw_rect(Rect::from_xywh(0.0, top, self.layout_width, height), &paint);
        }

        for line in &self.lines {
            let resolved = line.marks.resolved(theme);
            if let Some(gutter) = resolved.gutter {
                paint.set_color(gutter);
                canvas.draw_rect(
                    Rect::from_xywh(
                        line.x - theme.ui().gutter.inset,
                        line.top,
                        theme.ui().gutter.width,
                        line.height,
                    ),
                    &paint,
                );
            }

            if resolved.rule.is_some() && !line.inlays.has_instead() {
                paint.set_color(
                    theme
                        .attributes(crate::theme::StyleId::HorizontalLine)
                        .rule
                        .unwrap_or(Color::TRANSPARENT),
                );
                canvas.draw_rect(
                    Rect::from_xywh(
                        line.x,
                        line.top + theme.ui().rule.offset,
                        self.layout_width,
                        theme.ui().rule.thickness,
                    ),
                    &paint,
                );
            } else if let Some(shaped) = &line.shaped {
                for range in self.selections_in(line.byte_start, line.byte_end) {
                    let clamped = range.start.max(line.byte_start)..range.end.min(line.byte_end);
                    paint.set_color(theme.ui().caret.selection.0);
                    for rect in shaped.rects_for_range(clamped) {
                        canvas.draw_rect(
                            Rect::from_xywh(
                                rect.left,
                                line.text_top + rect.top,
                                (rect.right - rect.left).max(2.0),
                                rect.bottom - rect.top,
                            ),
                            &paint,
                        );
                    }
                }

                shaped.paint_in_slot(canvas, line.top, line.text_top, line.top + line.height);
            }
        }

        if let Some(started) = probe {
            eprintln!(
                "[paint-probe] draw={:.1}us lines={}",
                started.elapsed().as_secs_f64() * 1e6,
                self.lines.len(),
            );
        }
    }
}

fn starts_hard_line(view: &mut text::TextView, byte_start: u32) -> Option<()> {
    if byte_start == 0 {
        return Some(());
    }
    let mut previous: Vec<u8> = Vec::with_capacity(1);
    view.byte_range_into(byte_start as usize - 1, byte_start as usize, &mut previous);
    (previous.first() == Some(&b'\n')).then_some(())
}

struct RectangularBackground {
    x: f32,
    top: f32,
    bottom: f32,
    color: Color,
    inset: f32,

    extent: Option<f32>,
}

fn paint_rectangular_backgrounds(
    canvas: &Canvas,
    paint: &mut Paint,
    document_x: f32,
    layout_width: f32,
    lines: &[ViewportLine],
    theme: &crate::theme::Theme,
) {
    let mut background: Option<RectangularBackground> = None;

    for line in lines {
        let resolved = line.marks.resolved(theme);
        let rectangular = resolved.background.filter(|background| {
            matches!(
                background.kind,
                crate::theme::BackgroundKind::Block | crate::theme::BackgroundKind::Box
            )
        });
        if let Some(line_background) = rectangular {
            let top = line.top - theme.ui().code_panel.top_offset;
            let bottom = line.top + line.height - theme.ui().code_panel.top_offset;
            let inset = resolved.inset.unwrap_or(0.0);

            match &mut background {
                Some(background)
                    if (background.x - line.x).abs() < 0.1
                        && background.color == line_background.color
                        && background.extent.is_some() == line.box_extent.is_some() =>
                {
                    background.bottom = background.bottom.max(bottom);
                    background.extent = match (background.extent, line.box_extent) {
                        (Some(run), Some(line)) => Some(run.max(line)),
                        _ => background.extent,
                    };
                }
                _ => {
                    draw_rectangular_background(
                        canvas,
                        paint,
                        document_x,
                        layout_width,
                        background.take(),
                        theme,
                    );
                    background = Some(RectangularBackground {
                        x: line.x,
                        top,
                        bottom,
                        color: line_background.color,
                        inset,
                        extent: line.box_extent,
                    });
                }
            }
        } else {
            draw_rectangular_background(
                canvas,
                paint,
                document_x,
                layout_width,
                background.take(),
                theme,
            );
        }
    }

    draw_rectangular_background(canvas, paint, document_x, layout_width, background, theme);
}

fn draw_rectangular_background(
    canvas: &Canvas,
    paint: &mut Paint,
    document_x: f32,
    layout_width: f32,
    background: Option<RectangularBackground>,
    theme: &crate::theme::Theme,
) {
    let Some(background) = background else {
        return;
    };
    let height = (background.bottom - background.top).max(1.0);

    let left = (background.x - background.inset).max(document_x);

    let right = match background.extent {
        Some(extent) => (document_x + extent).max(left + 1.0),
        None => (document_x + layout_width).max(left + 1.0),
    };

    paint.set_color(background.color);
    canvas.draw_round_rect(
        Rect::from_xywh(left, background.top, right - left, height),
        theme.ui().code_panel.radius,
        theme.ui().code_panel.radius,
        paint,
    );
}
