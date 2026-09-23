// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The design system (docs/design-system.md): typography roles, the
//! spacing scale, `Surface`, and `ListRow` — chrome comes from this
//! role table, not from per-view constants. Views name structure
//! (`ListRow`, `Surface`) and pick roles; a bare f32 in a view is a
//! smell.

use imba::{arena::Arena, constraints::Constraints, store::Store, LayoutExt as _, UiCtx};
use skia_safe::{Color, Paint, Rect};

/// The spacing scale — every inset and gap is one of these.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

/// Corner radii: cards, wells, chips.
pub const RADIUS: f32 = 10.0;
pub const RADIUS_S: f32 = 6.0;
pub const RADIUS_XS: f32 = 4.0;

/// Type scale.
const LABEL_SIZE: f32 = 24.0;
const CAPTION_SIZE: f32 = 22.0;
const HEADING_SIZE: f32 = 24.0;
const CAPS_SIZE: f32 = 15.0;
const CAPS_TRACKING: f32 = 1.5;
const KEY_HINT_SIZE: f32 = 15.0;

/// A typography role resolved against the theme: font, color,
/// tracking. Placement is never part of the style — text sits on the
/// baseline its own metrics give it.
#[derive(Clone)]
pub struct TextStyle {
    pub font: skia_safe::Font,
    pub color: Color,
    pub tracking: f32,
    pub shaper: std::rc::Rc<imba::TextShaper>,
}

impl TextStyle {
    pub fn colored(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn sized(mut self, size: f32) -> Self {
        self.font.set_size(size);
        self
    }
}

/// Default row/body text.
pub fn label(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: crate::fonts::ui_text_font(ui, LABEL_SIZE),
        color: crate::env::Themes::of(store).ui().peeker.text.0,
        tracking: 0.0,
        shaper: imba::TextShaper::of(ui),
    }
}

/// Secondary, dimmed.
pub fn caption(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: crate::fonts::ui_text_font(ui, CAPTION_SIZE),
        color: crate::env::Themes::of(store).ui().peeker.dim_text.0,
        tracking: 0.0,
        shaper: imba::TextShaper::of(ui),
    }
}

/// Emphasized row/title text.
pub fn heading(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: crate::fonts::ui_font(ui, HEADING_SIZE),
        color: crate::env::Themes::of(store).ui().peeker.text.0,
        tracking: 0.0,
        shaper: imba::TextShaper::of(ui),
    }
}

/// The tracked small-caps chip/header look ("YOU", "REFRESH", group
/// headers). Callers pass UPPERCASED strings.
pub fn caps(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: crate::fonts::ui_font(ui, CAPS_SIZE),
        color: crate::env::Themes::of(store).ui().peeker.dim_text.0,
        tracking: CAPS_TRACKING,
        shaper: imba::TextShaper::of(ui),
    }
}

/// Shortcut hints.
pub fn key_hint(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: crate::fonts::ui_text_font(ui, KEY_HINT_SIZE),
        color: crate::env::Themes::of(store).ui().peeker.dim_text.0,
        tracking: 0.0,
        shaper: imba::TextShaper::of(ui),
    }
}

/// A styled `imba::Text`.
pub fn text(style: &TextStyle, content: impl Into<String>) -> imba::Text {
    imba::Text::with_shaper(
        content,
        style.font.clone(),
        style.color,
        style.shaper.clone(),
    )
    .tracking(style.tracking)
}

/// THE rounded fill-plus-hairline backdrop — every card, well and
/// chip paints through this one shape; only the colors are semantic.
#[derive(Clone, Copy)]
pub struct Surface {
    pub fill: Option<Color>,
    pub border: Option<Color>,
    pub radius: f32,
}

impl Surface {
    pub fn fill(fill: Color) -> Self {
        Self {
            fill: Some(fill),
            border: None,
            radius: RADIUS,
        }
    }

    pub fn bordered(fill: Color, border: Color) -> Self {
        Self {
            fill: Some(fill),
            border: Some(border),
            radius: RADIUS,
        }
    }

    pub fn outline(border: Color) -> Self {
        Self {
            fill: None,
            border: Some(border),
            radius: RADIUS,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// The painter, for `.backdrop(...)`.
    pub fn painter(self) -> impl Fn(&Arena, &skia_safe::Canvas, Rect) {
        move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            if let Some(fill) = self.fill {
                paint.set_color(fill);
                canvas.draw_round_rect(rect, self.radius, self.radius, &paint);
            }
            if let Some(border) = self.border {
                paint.set_stroke(true);
                paint.set_stroke_width(1.0);
                paint.set_color(border);
                canvas.draw_round_rect(
                    rect.with_inset((0.5, 0.5)),
                    self.radius,
                    self.radius,
                    &paint,
                );
            }
        }
    }
}

/// A row's metrics plus its default text roles. Rows never place
/// text by formula: the label and trails share a baseline group and
/// the group centers vertically in the row.
#[derive(Clone)]
pub struct RowStyle {
    /// The label's left inset.
    pub inset: f32,
    /// The last trail's right inset — separate because a frame (the
    /// tree's indent) may own the left inset while the row still
    /// keeps its trails off the right edge.
    pub trail_inset: f32,
    pub label: TextStyle,
    pub trail: TextStyle,
    /// The symmetric breathing room above and below the text block —
    /// the row's height is the text plus twice this.
    pub air: f32,
}

impl RowStyle {
    /// List/menu rows (peeker rows, combo menu, pickers).
    pub fn standard(store: &Store, ui: &UiCtx) -> Self {
        Self {
            inset: space::L,
            trail_inset: space::L,
            label: label(store, ui),
            trail: caption(store, ui),
            air: space::S,
        }
    }

    /// Drawer/tree rows — sized by the text, per the tree chrome's
    /// FONT SIZE. The label's left inset is the TREE FRAME's business
    /// (the indent offset); only the trails keep an inset of their
    /// own.
    pub fn drawer(store: &Store, ui: &UiCtx) -> Self {
        let tree = crate::env::Themes::of(store).ui().tree.clone();
        Self {
            inset: 0.0,
            trail_inset: space::L,
            label: label(store, ui).sized(tree.font_size),
            trail: caption(store, ui).sized(tree.font_size),
            air: space::S,
        }
    }

    /// Group/section headers (search result groups) — bold label.
    pub fn header(store: &Store, ui: &UiCtx) -> Self {
        let search = crate::env::Themes::of(store).ui().search.clone();
        Self {
            inset: search.group_text_x,
            trail_inset: search.group_text_x,
            label: heading(store, ui)
                .sized(search.group_font_size)
                .colored(search.group_text.0),
            trail: caption(store, ui).sized(search.group_font_size),
            air: space::S,
        }
    }
}

enum RowEntry<'a, Command> {
    Text(imba::Text),
    Action(imba::Text, Box<dyn Fn() -> Command + 'a>),
}

/// The one leading-label-trail row: label runs from the left inset,
/// trails pin to the right one, everything baseline-aligned and
/// vertically centered. Presses on the row are the caller's business
/// (`.on_event` on the whole row); `action` gives one trail its own
/// press.
pub struct ListRow<'a, Command> {
    arena: &'a Arena,
    style: RowStyle,
    label: Option<imba::Text>,
    trails: Vec<RowEntry<'a, Command>>,
}

impl<'a, Command: 'a> ListRow<'a, Command> {
    pub fn new(arena: &'a Arena, style: RowStyle) -> Self {
        Self {
            arena,
            style,
            label: None,
            trails: Vec::new(),
        }
    }

    pub fn label(mut self, content: impl Into<String>) -> Self {
        self.label = Some(text(&self.style.label, content));
        self
    }

    pub fn label_styled(mut self, style: &TextStyle, content: impl Into<String>) -> Self {
        self.label = Some(text(style, content));
        self
    }

    pub fn trail(mut self, content: impl Into<String>) -> Self {
        let entry = RowEntry::Text(text(&self.style.trail, content));
        self.trails.push(entry);
        self
    }

    pub fn trail_styled(mut self, style: &TextStyle, content: impl Into<String>) -> Self {
        self.trails.push(RowEntry::Text(text(style, content)));
        self
    }

    /// A pressable trail (the row-level OPEN/actions).
    pub fn action(
        mut self,
        content: impl Into<String>,
        on_press: impl Fn() -> Command + 'a,
    ) -> Self {
        let entry = RowEntry::Action(text(&self.style.trail, content), Box::new(on_press));
        self.trails.push(entry);
        self
    }
}

impl<'a, Command> imba::LayoutValue for ListRow<'a, Command> {}

impl<'a, Command: 'a> imba::Layout<'a, Command> for ListRow<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let ListRow {
            arena: row_arena,
            style,
            label,
            trails,
        } = self;
        let mut row = imba::Row::new(row_arena);
        if let Some(label) = label {
            row = row.child_by_baseline(label.pad_insets(imba::Insets {
                left: style.inset,
                ..Default::default()
            }));
        }
        // The flexible gap is MAIN-AXIS only: a bare `Fill` would
        // stretch to the row's height and drag the baseline group's
        // extent with it — the texts would ride the row's top instead
        // of centering.
        row = row.weighted(1.0, imba::Fill::new().height(0.0));
        let last = trails.len().saturating_sub(1);
        for (index, entry) in trails.into_iter().enumerate() {
            let insets = imba::Insets {
                left: space::S,
                right: match index == last {
                    true => style.trail_inset,
                    false => 0.0,
                },
                ..Default::default()
            };
            row = match entry {
                RowEntry::Text(text) => row.child_by_baseline(text.pad_insets(insets)),
                RowEntry::Action(text, on_press) => {
                    row.child_by_baseline(text.on_click(move || on_press()).pad_insets(insets))
                }
            };
        }
        // The row's height is ITS OWN: the label's text block plus
        // the style's symmetric air — no shared row-height anywhere.
        let metrics = style.label.font.metrics().1;
        let height = (-metrics.ascent + metrics.descent).ceil() + 2.0 * style.air;
        row.align(imba::Alignment::CenterStart)
            .height(height)
            .layout(arena, constraints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "a timing probe, run with --nocapture to read it"]
    fn timing_probe_pushes() {
        let store = Store::new();
        let ui = UiCtx::dont_use_too_slow();
        // Warm the ctx the way the app's long-lived one is warm: the
        // first typeface resolution is a boot cost, not a push cost.
        let mut warmup: imba::list::ListSlice<crate::TreeRow, u64> = imba::list::ListSlice::new();
        warmup.push_keyed(
            u64::MAX,
            crate::TreeItemView::leaf(crate::TreeLabel::new("warmup".to_owned(), true, false), 1),
            &store,
            &ui,
        );
        let mut slice: imba::list::ListSlice<crate::TreeRow, u64> = imba::list::ListSlice::new();
        let mut worst = 0.0f64;
        let mut first = 0.0f64;
        let started = std::time::Instant::now();
        for n in 0..200u64 {
            let one = std::time::Instant::now();
            slice.push_keyed(
                n,
                crate::TreeItemView::leaf(
                    crate::TreeLabel::new(format!("row {n}"), true, false),
                    1,
                ),
                &store,
                &ui,
            );
            let took = one.elapsed().as_secs_f64() * 1e3;
            if n == 0 {
                first = took;
            }
            worst = worst.max(took);
        }
        eprintln!(
            "[timing] 200 pushes: total {:.1}ms, first {first:.1}ms, worst {worst:.1}ms",
            started.elapsed().as_secs_f64() * 1e3
        );
    }

    #[test]
    fn a_list_rows_text_centers_in_the_row() {
        // The regression: the flexible gap stretched to the row's
        // height, so the baseline group hugged the row's TOP — tree
        // labels floated above their disclosure glyphs.
        let store = Store::new();
        let ui = UiCtx::dont_use_too_slow();
        let arena = Arena::default();
        let style = RowStyle::drawer(&store, &ui);
        let metrics = style.label.font.metrics().1;
        let ascent = -metrics.ascent;
        // The row is its own text block plus `space::S` each side,
        // so the centered baseline sits at pad + ascent.
        let expected = space::S + ascent;

        let thunk = imba::Layout::layout(
            ListRow::<()>::new(&arena, style.clone())
                .label("himark-jb")
                .trail("+3 −1"),
            &arena,
            Constraints {
                min: skia_safe::Size::default(),
                max: skia_safe::Size::new(400.0, f32::MAX),
            },
        );
        let baseline = imba::Thunk::first_baseline(&thunk).expect("the label answers a baseline");
        assert!(
            (baseline - expected).abs() < 1.5,
            "the label's baseline centers: {baseline} vs expected {expected}"
        );
    }
}

/// Button roles: `Primary` is the accent call-to-action, `Ghost` the
/// quiet outlined chip. Labels are caps-tracked; callers pass
/// UPPERCASE.
#[derive(Clone, Copy)]
pub enum ButtonRole {
    Primary,
    Ghost,
}

pub fn button<'a, Command: 'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    role: ButtonRole,
    content: impl Into<String>,
    on_press: impl Fn() -> Command + 'a,
) -> imba::Button<'a, Command, impl Fn() -> Command + 'a> {
    let chat = crate::env::Themes::of(store).ui().chat.clone();
    let style = match role {
        ButtonRole::Primary => caps(store, ui).colored(chat.on_accent.0),
        ButtonRole::Ghost => caps(store, ui),
    };
    let dim = style.color;
    let button = imba::Button::new(arena, text(&style, content), on_press)
        .radius(RADIUS_S)
        .pad_content(imba::Insets::xy(space::L, space::S));
    match role {
        ButtonRole::Primary => button.fill(chat.accent.0),
        ButtonRole::Ghost => button.stroke(dim),
    }
}
