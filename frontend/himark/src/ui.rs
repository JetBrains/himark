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
pub const RADIUS: f32 = 8.0;
pub const RADIUS_S: f32 = 4.0;
pub const RADIUS_XS: f32 = 3.0;

mod air_fonts;
pub mod air_tokens;

/// An Air typography role: font, CSS line height, color and letter spacing.
#[derive(Clone)]
pub struct TextStyle {
    pub font: skia_safe::Font,
    pub color: Color,
    pub tracking: f32,
    pub line_height: Option<f32>,
}

impl TextStyle {
    pub fn colored(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn sized(mut self, size: f32) -> Self {
        let scale = size / self.font.size();
        self.line_height = self.line_height.map(|height| height * scale);
        self.tracking *= scale;
        self.font.set_size(size);
        self
    }
}

fn role(
    store: &Store,
    size: f32,
    height: f32,
    weight: u16,
    tracking: f32,
    secondary: bool,
) -> TextStyle {
    let weight = weight + if air_tokens::light(store) { 20 } else { 0 };
    TextStyle {
        font: air_fonts::font(size, weight, false),
        color: if secondary {
            crate::env::Themes::of(store).ui().peeker.dim_text.0
        } else {
            crate::env::Themes::of(store).ui().peeker.text.0
        },
        tracking,
        line_height: Some(height),
    }
}

/// Air Text/default: Inter 13/16, optical weight 480 (500 in light).
pub fn label(store: &Store, _ui: &UiCtx) -> TextStyle {
    role(store, 13.0, 16.0, 480, 0.052, false)
}

/// Air Text/medium: Inter 12/16.
pub fn caption(store: &Store, _ui: &UiCtx) -> TextStyle {
    role(store, 12.0, 16.0, 500, 0.06, true)
}

/// Air Heading/h2-semibold: Inter 19/24.
pub fn heading(store: &Store, _ui: &UiCtx) -> TextStyle {
    role(store, 19.0, 24.0, 600, 0.0, false)
}

/// Air Heading/h5-semibold: uppercase Inter 10/14, 0.1em tracking.
pub fn caps(store: &Store, _ui: &UiCtx) -> TextStyle {
    role(store, 10.0, 14.0, 700, 1.0, true)
}

/// Air Text/small: Inter 10/14.
pub fn key_hint(store: &Store, _ui: &UiCtx) -> TextStyle {
    role(store, 10.0, 14.0, 500, 0.06, true)
}

/// Air Text/code: JetBrains Mono 13/22.
pub fn code(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle {
        font: air_fonts::font(13.0, if air_tokens::light(store) { 420 } else { 400 }, true),
        line_height: Some(22.0),
        tracking: 0.0,
        ..label(store, ui)
    }
}

pub fn text(style: &TextStyle, content: impl Into<String>) -> imba::Text {
    let text = imba::text(content, style.font.clone(), style.color).tracking(style.tracking);
    match style.line_height {
        Some(height) => text.line_height(height),
        None => text,
    }
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
                    (self.radius - 0.5).max(0.0),
                    (self.radius - 0.5).max(0.0),
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
            inset: space::S,
            trail_inset: space::XS,
            label: label(store, ui),
            trail: key_hint(store, ui),
            air: space::XS,
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
            trail_inset: space::XS,
            label: label(store, ui).sized(tree.font_size),
            trail: key_hint(store, ui),
            air: space::XS,
        }
    }

    /// Group/section headers (search result groups) — bold label.
    pub fn header(store: &Store, ui: &UiCtx) -> Self {
        let search = crate::env::Themes::of(store).ui().search.clone();
        Self {
            inset: search.group_text_x,
            trail_inset: search.group_text_x,
            label: role(store, 13.0, 16.0, 600, 0.0676, false).colored(search.group_text.0),
            trail: key_hint(store, ui),
            air: space::XS,
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
    background: Option<Color>,
    enabled: bool,
}

impl<'a, Command: 'a> ListRow<'a, Command> {
    pub fn new(arena: &'a Arena, style: RowStyle) -> Self {
        Self {
            arena,
            style,
            label: None,
            trails: Vec::new(),
            background: None,
            enabled: true,
        }
    }

    pub fn label(mut self, content: impl Into<String>) -> Self {
        self.label = Some(text(&self.style.label, content));
        self
    }

    /// Air List.Item interaction appearance. Apply before adding text slots.
    pub fn state(mut self, store: &Store, state: ControlState) -> Self {
        self.background = Some(air_tokens::color(
            store,
            match state {
                ControlState::Hovered | ControlState::Focused => "list-item-background-hovered",
                ControlState::Pressed => "list-item-background-focused",
                _ => "list-item-background-default",
            },
        ));
        if state == ControlState::Disabled {
            self.enabled = false;
            self.style.label.color = air_tokens::color(store, "text-disabled");
            self.style.trail.color = self.style.label.color;
        }
        self
    }

    pub fn selected(mut self, store: &Store) -> Self {
        self.background = Some(air_tokens::color(store, "list-item-background-selected"));
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
            background,
            enabled,
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
                RowEntry::Action(text, on_press) if enabled => {
                    row.child_by_baseline(text.on_click(move || on_press()).pad_insets(insets))
                }
                RowEntry::Action(text, _) => row.child_by_baseline(text.pad_insets(insets)),
            };
        }
        // The row's height is ITS OWN: the label's text block plus
        // the style's symmetric air — no shared row-height anywhere.
        let metrics = style.label.font.metrics().1;
        let height = style
            .label
            .line_height
            .unwrap_or_else(|| (-metrics.ascent + metrics.descent).ceil())
            + 2.0 * style.air;
        row.align(imba::Alignment::CenterStart)
            .height(height)
            .backdrop(
                Surface {
                    fill: background,
                    border: None,
                    radius: 4.0,
                }
                .painter(),
            )
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
        let ui = UiCtx::cold();
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
        let ui = UiCtx::cold();
        let arena = Arena::default();
        let style = RowStyle::drawer(&store, &ui);
        let metrics = style.label.font.metrics().1;
        let ascent = -metrics.ascent;
        // The row is its own text block plus `space::S` each side,
        // so the centered baseline sits at pad + ascent.
        let expected = space::XS
            + (style.label.line_height.unwrap() - (-metrics.ascent + metrics.descent)) * 0.5
            + ascent;

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

/// Air Button/primary, Button/secondary and ButtonGhost/off.
#[derive(Clone, Copy)]
pub enum ButtonRole {
    Primary,
    Secondary,
    Ghost,
}

pub use imba::{ButtonVisual, ControlState};

pub fn button<'a, Command: 'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    role: ButtonRole,
    content: impl Into<String>,
    on_press: impl Fn() -> Command + 'a,
) -> imba::Button<'a, Command, impl Fn() -> Command + 'a> {
    let prefix = match role {
        ButtonRole::Primary => "button-primary",
        ButtonRole::Secondary => "button-secondary",
        ButtonRole::Ghost => "ghost-button-off",
    };
    let token = |suffix: &str| air_tokens::color(store, &format!("{prefix}-{suffix}"));
    let visual = |state: &str| ButtonVisual {
        fill: Some(token(&format!("background-{state}"))),
        stroke: Some(token(&format!("border-{state}"))),
        outline: None,
    };
    let focused = ButtonVisual {
        stroke: Some(token("focus-border")),
        outline: Some(token("focus-outline")),
        ..visual("default")
    };
    let mut style = label(store, ui).colored(token("text-default"));
    let ghost = matches!(role, ButtonRole::Ghost);
    if !ghost {
        style.tracking = 0.04;
    }
    let content = content.into();
    imba::Button::new(arena, text(&style, content.clone()), on_press)
        .disabled_content(arena, text(&style.colored(token("text-disabled")), content))
        .radius(if ghost { 3.0 } else { 4.0 })
        .min_width(if ghost { 0.0 } else { 60.0 })
        .max_width(if ghost { f32::MAX } else { 256.0 })
        // Includes the label wrapper's 4px (2px for Ghost) on each side.
        .pad_content(imba::Insets::xy(
            if ghost { 4.0 } else { 10.0 },
            if ghost { 2.0 } else { 4.0 },
        ))
        .visuals([
            visual("default"),
            visual("hovered"),
            visual("pressed"),
            focused,
            visual("disabled"),
        ])
}

/// Shared themed checkbox; the legacy imba checkbox remains available to custom themes.
pub fn air_checkbox<'a>(
    store: &Store,
    value: imba::checkbox::CheckboxValue,
    state: ControlState,
) -> impl imba::Thunk<'a, imba::checkbox::CheckboxCommand> + 'a {
    use imba::checkbox::{checkbox_with_visual, CheckboxStyle, CheckboxValue, CheckboxVisual};
    let on = value != CheckboxValue::Unchecked;
    let prefix = if on { "checkbox-on" } else { "checkbox-off" };
    let token = |suffix: &str| air_tokens::color(store, &format!("{prefix}-{suffix}"));
    let suffix = match state {
        ControlState::Disabled => "disabled",
        ControlState::Hovered => "hovered",
        _ => "default",
    };
    let focused = state == ControlState::Focused;
    let style = CheckboxStyle {
        size: 16.0,
        radius: 2.0,
        stroke: 1.0,
        ..Default::default()
    };
    let visual = CheckboxVisual {
        background: token(&format!("background-{suffix}")),
        border: token(&if focused {
            "focus-border".to_owned()
        } else {
            format!("border-{suffix}")
        }),
        icon: air_tokens::color(
            store,
            if state == ControlState::Disabled {
                "checkbox-icon-disabled"
            } else {
                "checkbox-icon-default"
            },
        ),
        outline: if focused {
            token("focus-outline")
        } else {
            Color::TRANSPARENT
        },
    };
    checkbox_with_visual(value, style, visual, state != ControlState::Disabled)
}
