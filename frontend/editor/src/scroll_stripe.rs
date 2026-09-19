// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Scroll-bar stripes (docs/editor/scroll-stripe.md): flagged feature markups
//! projected onto the pane's scroll track. The projection is one
//! linear background pass over (flagged markups × the editor's layout
//! rope), quantized onto a fixed grid; it runs as an effect on a
//! per-editor lane and lands as a command, strict last-wins.

use std::{ops::Range, sync::Arc};

use imba::arena::Arena;
use imba::event::{Event, EventResult};
use intervals::{IntervalQuery, Order};
use skia_safe::{Point, Rect, Size};

use crate::document::{Document, DocumentToken};
use crate::document_layout::DocumentLayout;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::markup::{Decoration, Markup};
use crate::theme::{StyleId, Theme};

pub const HOST: imba::overlay::OverlayHost = imba::overlay::OverlayHost("editor.scroll-stripe");

/// The projection grid: segment boundaries quantize onto this many
/// slots over the content height, so the landed value and the
/// per-frame paint are bounded by the GRID, never by the match count.
const GRID: u32 = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct StripeSegment {
    /// Layout px at derivation; the paint pass maps through
    /// `content_height` onto the track.
    pub y: Range<f32>,
    /// Resolved to a color AT PAINT, against the live theme.
    pub style: StyleId,
    /// The run's first byte — the click target.
    pub byte: u32,
}

#[derive(Debug, Default, PartialEq)]
pub struct ScrollStripes {
    /// Ascending by `y.start`, quantized and merged.
    pub segments: Vec<StripeSegment>,
    /// The layout height the segment ys address — the paint pass's
    /// denominator. Live height may have drifted (repairs landed);
    /// the next landing heals it, invisibly at track scale meanwhile.
    pub content_height: f32,
}

/// THE per-editor lane: the landed value, the in-flight token, the
/// last-launched fingerprint and the last-wins serial. Plain values
/// riding the `Editor` like every other field.
#[derive(Clone, Default)]
pub(crate) struct StripeSlot {
    pub(crate) enabled: bool,
    /// The FEATURE entries this editor's track projects (a find
    /// bar's tints) — registered by the feature, beside its
    /// `show_markup` pick. Diff markups never register: the
    /// document's diffs map enumerates them.
    pub(crate) markups: Vec<crate::markup::MarkupId>,
    pub(crate) landed: Option<Arc<ScrollStripes>>,
    pub(crate) token: Option<imba::effect::CancellationToken>,
    pub(crate) stamp: Option<StripeStamp>,
    pub(crate) serial: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StripeStamp {
    pub(crate) revision: u64,
    pub(crate) generation: u64,
    pub(crate) height_bits: u32,
    pub(crate) theme: Arc<str>,
}

// ---- the derivation (worker-side) ---------------------------------

/// One linear pass: merge the flagged entries' interval streams,
/// co-walk the layout rope once accumulating (byte, y), quantize onto
/// the grid. O(elements + hits · log hits), never on the UI thread.
pub(crate) fn derive(markups: &[Markup], layout: &DocumentLayout, theme: &Theme) -> ScrollStripes {
    let content_height = layout.height();
    if content_height <= 0.0 {
        return ScrollStripes::default();
    }
    let mut hits: Vec<(Range<u32>, StyleId)> = Vec::new();
    for markup in markups {
        for interval in markup.query(0..u32::MAX, Order::Ascending) {
            if let Decoration::Styled(id) = interval.value {
                if theme.attributes(*id).stripe.is_some() {
                    hits.push((interval.range.clone(), *id));
                }
            }
        }
    }
    if hits.is_empty() {
        return ScrollStripes {
            segments: Vec::new(),
            content_height,
        };
    }
    hits.sort_by_key(|(range, _)| range.start);

    // Boundary events, resolved against one forward walk of the rope:
    // a hit's top is its start element's top, its bottom the element
    // holding its last byte (a zero-length marker takes its element's
    // whole extent).
    #[derive(Clone, Copy)]
    struct Boundary {
        byte: u32,
        hit: usize,
        end: bool,
    }
    let mut events: Vec<Boundary> = Vec::with_capacity(hits.len() * 2);
    for (index, (range, _)) in hits.iter().enumerate() {
        events.push(Boundary {
            byte: range.start,
            hit: index,
            end: false,
        });
        events.push(Boundary {
            byte: range.end.max(range.start.saturating_add(1)) - 1,
            hit: index,
            end: true,
        });
    }
    events.sort_by_key(|event| event.byte);

    // Boundaries past the covered bytes (a deletion marker at EOF)
    // resolve to the BOTTOM — the defaults are the walk's fallthrough.
    let mut tops = vec![content_height; hits.len()];
    let mut bottoms = vec![content_height; hits.len()];
    let mut next = 0usize;
    let mut y: u64 = 0;
    for (start, element) in layout.spans_from(0) {
        let extent = (element.height + element.spacer_above)
            .ceil()
            .min(u32::MAX as f32) as u32;
        let end = start.saturating_add(element.byte_size);
        while next < events.len() && events[next].byte < end {
            let event = events[next];
            if event.byte >= start {
                match event.end {
                    false => tops[event.hit] = y as f32,
                    true => bottoms[event.hit] = (y + extent as u64) as f32,
                }
            }
            next += 1;
        }
        y += extent as u64;
        if next >= events.len() {
            break;
        }
    }

    // Quantize onto the grid and merge touching same-style runs; a
    // merged run keeps its FIRST contributor's byte.
    let step = content_height / GRID as f32;
    let mut quantized: Vec<(StyleId, u32, u32, u32)> = hits
        .iter()
        .enumerate()
        .map(|(index, (range, style))| {
            let b0 = ((tops[index] / step) as u32).min(GRID - 1);
            let b1 = ((bottoms[index] / step).ceil() as u32).clamp(b0 + 1, GRID);
            (*style, b0, b1, range.start)
        })
        .collect();
    quantized.sort_by_key(|(style, b0, ..)| (*style, *b0));
    let mut segments: Vec<StripeSegment> = Vec::new();
    for (style, b0, b1, byte) in quantized {
        match segments.last_mut() {
            Some(last) if last.style == style && last.y.end >= b0 as f32 * step => {
                last.y.end = last.y.end.max(b1 as f32 * step);
            }
            _ => segments.push(StripeSegment {
                y: b0 as f32 * step..b1 as f32 * step,
                style,
                byte,
            }),
        }
    }
    segments.sort_by(|a, b| a.y.start.total_cmp(&b.y.start));
    ScrollStripes {
        segments,
        content_height,
    }
}

// ---- the effect and its lane ----------------------------------------

pub struct ScrollStripeEffect {
    pub(crate) work: StripeWork,
}

pub(crate) struct StripeWork {
    pub(crate) token: DocumentToken,
    pub(crate) editor: EditorId,
    pub(crate) serial: u64,
    pub(crate) markups: Vec<Markup>,
    pub(crate) layout: DocumentLayout,
}

pub struct StripeOutcome {
    pub(crate) token: DocumentToken,
    pub(crate) editor: EditorId,
    pub(crate) serial: u64,
    pub(crate) stripes: Arc<ScrollStripes>,
}

impl StripeOutcome {
    pub fn editor(&self) -> EditorId {
        self.editor
    }
}

impl imba::effect::Effect for ScrollStripeEffect {
    type Result = StripeOutcome;
}

pub struct ScrollStripeHandler(pub Arc<crate::env::Workshop>);

impl imba::effect::EffectHandler<ScrollStripeEffect> for ScrollStripeHandler {
    async fn handle(&self, effect: ScrollStripeEffect) -> StripeOutcome {
        run_work(effect.work, &self.0.theme())
    }
}

pub(crate) fn run_work(work: StripeWork, theme: &Theme) -> StripeOutcome {
    let stripes = derive(&work.markups, &work.layout, theme);
    StripeOutcome {
        token: work.token,
        editor: work.editor,
        serial: work.serial,
        stripes: Arc::new(stripes),
    }
}

/// The handler's synchronous core, as a door test harnesses can drive
/// without the runner (compute completes on its first poll anyway).
pub fn land(effect: ScrollStripeEffect, theme: &Theme) -> StripeOutcome {
    run_work(effect.work, theme)
}

/// One owed relaunch, answered by `Document::scroll_stripe_launches`:
/// the sweep pushes the effect through `fx.relaunch_erased` against
/// `supersedes` and hands the fresh token back to the document.
pub struct StripeLaunch {
    pub editor: EditorId,
    pub effect: ScrollStripeEffect,
    pub supersedes: Option<imba::effect::CancellationToken>,
}

// ---- rendering: the overlay road ------------------------------------

/// Minted from `EditorView`'s realize (the sticky recipe): one request
/// to the pane-level host wrapping the vertical scroll. The scroll's
/// realize offsets the anchor, so the panel lands in pane-viewport
/// coordinates at this frame's true scroll — `ScrollView` untouched.
pub(crate) fn scroll_stripe_overlays<'a>(
    document: &Document,
    editor: EditorId,
    theme: &Theme,
    arena: &'a Arena,
    viewport: Rect,
) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
    if viewport.height() <= 0.0 {
        return Vec::new();
    }
    let Some(stripes) = document
        .editors
        .get(&editor)
        .and_then(|state| state.scroll_stripes.landed.clone())
    else {
        return Vec::new();
    };
    if stripes.segments.is_empty() || stripes.content_height <= 0.0 {
        return Vec::new();
    }
    let theme = theme.clone();
    vec![imba::overlay::Overlay {
        host: HOST,
        anchor: Rect::from_xywh(0.0, viewport.top, 1.0, viewport.height()),
        content: Box::new(move |host_size: Size, _anchor: Rect| {
            let chrome = &theme.ui().scroll_stripe;
            let bar = &theme.ui().scrollbar;
            let x = (host_size.width - bar.margin - chrome.inset - chrome.width).max(0.0);
            let widget = StripeWidget {
                stripes,
                theme: theme.clone(),
                size: Size::new(chrome.width, host_size.height),
            };
            vec![(
                Point::new(x, 0.0),
                imba::ThunkBox::new(arena, imba::eager(widget)),
            )]
        }),
    }]
}

struct StripeWidget {
    stripes: Arc<ScrollStripes>,
    theme: Theme,
    size: Size,
}

impl StripeWidget {
    /// Document y ↔ track y: the whole content maps onto the knob's
    /// track band (the scrollbar's own insets).
    fn track(&self) -> (f32, f32) {
        let inset = self.theme.ui().scrollbar.track_inset;
        let height = (self.size.height - inset * 2.0).max(1.0);
        (inset, height / self.stripes.content_height.max(1.0))
    }

    fn mark(&self, segment: &StripeSegment, top: f32, scale: f32) -> Rect {
        let min_height = self.theme.ui().scroll_stripe.min_height;
        let bottom = self.size.height - top;
        let y1 = (top + segment.y.end * scale)
            .max(top + segment.y.start * scale + min_height)
            .min(bottom);
        let y0 = (y1 - min_height)
            .min(top + segment.y.start * scale)
            .max(top);
        Rect::from_ltrb(0.0, y0, self.size.width, y1)
    }
}

impl<'a> imba::Widget<'a, EditorCommand> for StripeWidget {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<EditorCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                let (top, scale) = self.track();
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(false);
                for segment in &self.stripes.segments {
                    let Some(color) = self.theme.attributes(segment.style).stripe else {
                        continue;
                    };
                    paint.set_color(color);
                    canvas.draw_rect(self.mark(segment, top, scale), &paint);
                }
                EventResult::Handled
            }

            Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point,
                ..
            } => {
                // Nearest mark within a slop — bounded by the GRID.
                let (top, scale) = self.track();
                let slop = self.theme.ui().scroll_stripe.min_height * 2.0;
                let mut nearest: Option<(f32, u32)> = None;
                for segment in &self.stripes.segments {
                    let mark = self.mark(segment, top, scale);
                    let distance = match point.y {
                        y if y < mark.top => mark.top - y,
                        y if y > mark.bottom => y - mark.bottom,
                        _ => 0.0,
                    };
                    if nearest.is_none_or(|(best, _)| distance < best) {
                        nearest = Some((distance, segment.byte));
                    }
                }
                match nearest {
                    Some((distance, byte)) if distance <= slop => {
                        EventResult::Command(EditorCommand::RevealAt { byte })
                    }
                    _ => EventResult::Handled,
                }
            }

            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests;
