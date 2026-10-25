// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The design system (docs/ui/design-system.md): typography roles, the
//! spacing scale, `Surface`, and `ListRow` — chrome comes from this
//! role table, not from per-view constants. Views name structure
//! (`ListRow`, `Surface`) and pick roles; a bare f32 in a view is a
//! smell.

use imba::{
    arena::Arena, constraints::Constraints, layout::LayoutExt as _, store::Store, ui::UiCtx,
};
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

/// A typography role resolved against the theme: font, color,
/// tracking. Placement is never part of the style — text sits on the
/// baseline its own metrics give it.
#[derive(Clone)]
pub struct TextStyle {
    pub font: skia_safe::Font,
    pub color: Color,
    pub tracking: f32,
    pub shaper: std::rc::Rc<imba::layout::TextShaper>,
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

    /// The role's line height, from the font's own metrics (ascent
    /// plus descent, rounded up) — what a row of this text is tall.
    pub fn line_height(&self) -> f32 {
        let metrics = self.font.metrics().1;
        (-metrics.ascent + metrics.descent).ceil()
    }

    /// The role at a ramp rung, in `color`.
    fn of(ui: &UiCtx, role: &editor::theme::TypeRole, color: Color) -> Self {
        Self {
            font: crate::fonts::role_font(ui, role),
            color,
            tracking: role.tracking,
            shaper: imba::layout::TextShaper::of(ui),
        }
    }
}

fn ramp(store: &Store) -> std::sync::Arc<editor::theme::TypeChrome> {
    editor::env::Themes::of(store).part(::editor::theme::TypeChrome::KEY)
}

fn ink(store: &Store) -> std::sync::Arc<editor::theme::InkChrome> {
    editor::env::Themes::of(store).part(::editor::theme::InkChrome::KEY)
}

/// READING — the agent's prose, list rows' body: ink 2.
pub fn reading(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).reading, ink(store).ink_2.0)
}

/// ROW — tree and list rows.
pub fn row(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).row, ink(store).ink_2.0)
}

/// ROW, strong — a directory's name, the expanded commit's title.
pub fn row_strong(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).row_strong, ink(store).ink_2.0)
}

/// BRIEF — paths, metadata, counts: ink 3.
pub fn brief(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).brief, ink(store).ink_3.0)
}

/// TITLE — the file, the session, the active tab: primary ink.
pub fn title(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).title, ink(store).ink.0)
}

/// SUBTITLE — the title's quiet companion ("in himark-jb").
pub fn subtitle(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).subtitle, ink(store).ink_3.0)
}

/// TAB, standing — the active tab: the subtitle's size in the
/// title's weight and ink, so a row of tabs never jumps on a switch.
pub fn tab_active(store: &Store, ui: &UiCtx) -> TextStyle {
    let ramp = ramp(store);
    let role = editor::theme::TypeRole {
        weight: ramp.title.weight,
        ..ramp.subtitle
    };
    TextStyle::of(ui, &role, ink(store).ink.0)
}

/// DISPLAY — the column's one big action (Sessions).
pub fn display(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).display, ink(store).ink.0)
}

/// PROMPT — your own messages.
pub fn prompt(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).prompt, ink(store).ink.0)
}

/// HEADLINE — the commit you're reading, condensed.
pub fn headline(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).headline, ink(store).ink_bright.0)
}

/// QUERY — the modal's light, large input (palette, jump).
pub fn query(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).query, ink(store).ink_bright.0)
}

/// QUERY, the side column's size (find).
pub fn query_small(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).query_small, ink(store).ink_bright.0)
}

/// LABEL — the tracked caps group heads and field labels. Callers
/// pass UPPERCASED strings.
pub fn caps(store: &Store, ui: &UiCtx) -> TextStyle {
    TextStyle::of(ui, &ramp(store).label, ink(store).ink_3.0)
}

/// MARGIN — keys, tool invocations, line numbers, in the mono face.
pub fn margin(store: &Store, ui: &UiCtx) -> TextStyle {
    let role = ramp(store).margin;
    TextStyle {
        font: crate::fonts::mono_font(ui, role.size),
        color: ink(store).ink_3.0,
        tracking: role.tracking,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// Default row/body text — the READING role.
pub fn label(store: &Store, ui: &UiCtx) -> TextStyle {
    reading(store, ui)
}

/// Secondary, dimmed — the BRIEF role.
pub fn caption(store: &Store, ui: &UiCtx) -> TextStyle {
    brief(store, ui)
}

/// Emphasized row/title text — the TITLE role.
pub fn heading(store: &Store, ui: &UiCtx) -> TextStyle {
    title(store, ui)
}

/// A styled `imba::layout::Text`.
pub fn text(style: &TextStyle, content: impl Into<String>) -> imba::layout::Text {
    imba::layout::Text::with_shaper(
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
        move |_arena, canvas, rect| self.paint(canvas, rect)
    }

    /// The DECORATOR: the surface takes the widget it decorates
    /// and paints itself under it, at the inner's laid bounds.
    pub fn wrap<'a, Command: 'a>(
        self,
        inner: impl imba::thunk_ext::ThunkExt<'a, Command> + 'a,
    ) -> impl imba::Thunk<'a, Command> + 'a {
        inner.paint_below(move |_arena, canvas, rect| self.paint(canvas, rect))
    }

    fn paint(&self, canvas: &skia_safe::Canvas, rect: Rect) {
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

/// A FLAT GROUND painter, for `.backdrop(...)`: the page color under
/// a column, a sheet — the one fill the redesign keeps (no wells).
pub fn ground(color: Color) -> impl Fn(&Arena, &skia_safe::Canvas, Rect) {
    move |_arena, canvas, rect| {
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.draw_rect(rect, &paint);
    }
}

/// A vertical hairline RULE as a layout: one pixel wide, the given
/// height — the column divider.
pub fn rule_v<'a, Command: 'a>(
    color: Color,
    height: f32,
) -> impl imba::layout::Layout<'a, Command> + imba::layout::LayoutValue + 'a {
    imba::layout::Fill::<Command>::new()
        .sized(1.0, height)
        .backdrop(ground(color))
}

/// A horizontal hairline RULE as a layout: the given width, one
/// pixel tall — the rule between bands.
pub fn rule_h<'a, Command: 'a>(
    color: Color,
    width: f32,
) -> impl imba::layout::Layout<'a, Command> + imba::layout::LayoutValue + 'a {
    imba::layout::Fill::<Command>::new()
        .sized(width, 1.0)
        .backdrop(ground(color))
}

/// Per-edge 1px rules — the hairlines a sheet or panel wears where
/// a full `Surface::outline` would be too much chrome.
#[derive(Clone, Copy)]
pub struct Hairline {
    pub color: Color,
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Hairline {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            top: false,
            bottom: false,
            left: false,
            right: false,
        }
    }

    pub fn around(color: Color) -> Self {
        Self {
            color,
            top: true,
            bottom: true,
            left: true,
            right: true,
        }
    }

    pub fn top(mut self) -> Self {
        self.top = true;
        self
    }

    pub fn bottom(mut self) -> Self {
        self.bottom = true;
        self
    }

    pub fn left(mut self) -> Self {
        self.left = true;
        self
    }

    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }

    /// The DECORATOR: the rules take the widget they decorate and
    /// paint under it, along the inner's laid edges.
    pub fn wrap<'a, Command: 'a>(
        self,
        inner: impl imba::thunk_ext::ThunkExt<'a, Command> + 'a,
    ) -> impl imba::Thunk<'a, Command> + 'a {
        inner.paint_below(move |_arena, canvas, rect| self.paint(canvas, rect))
    }

    fn paint(&self, canvas: &skia_safe::Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_color(self.color);
        if self.top {
            canvas.draw_rect(
                Rect::from_xywh(rect.left, rect.top, rect.width(), 1.0),
                &paint,
            );
        }
        if self.bottom {
            canvas.draw_rect(
                Rect::from_xywh(rect.left, rect.bottom - 1.0, rect.width(), 1.0),
                &paint,
            );
        }
        if self.left {
            canvas.draw_rect(
                Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                &paint,
            );
        }
        if self.right {
            canvas.draw_rect(
                Rect::from_xywh(rect.right - 1.0, rect.top, 1.0, rect.height()),
                &paint,
            );
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
    /// FONT SIZE and AIR. The label's left inset is the TREE FRAME's
    /// business (the indent offset); only the trails keep an inset of
    /// their own.
    pub fn drawer(store: &Store, ui: &UiCtx) -> Self {
        let tree = editor::env::Themes::of(store).part(crate::chrome::TreeChrome::KEY);
        Self {
            inset: 0.0,
            trail_inset: space::L,
            label: label(store, ui).sized(tree.font_size),
            trail: caption(store, ui).sized(tree.font_size),
            air: tree.row_air,
        }
    }

    /// Group/section headers (search result groups) — bold label.
    pub fn header(store: &Store, ui: &UiCtx) -> Self {
        let search = editor::env::Themes::of(store).part(::editor::theme::SearchChrome::KEY);
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
    Text(imba::layout::Text),
    Action(imba::layout::Text, Box<dyn Fn() -> Command + 'a>),
}

/// The one leading-label-trail row: an optional badge (a short status
/// mark in its own color) leads, the label runs from it, trails pin to
/// the right inset, everything baseline-aligned and vertically
/// centered. Presses on the row are the caller's business (`.on_event`
/// on the whole row); `action` gives one trail its own press.
pub struct ListRow<'a, Command> {
    arena: &'a Arena,
    style: RowStyle,
    badge: Option<imba::layout::Text>,
    label: Option<imba::layout::Text>,
    /// A word GLUED to the label's end (a directory's "/").
    suffix: Option<imba::layout::Text>,
    trails: Vec<RowEntry<'a, Command>>,
}

impl<'a, Command: 'a> ListRow<'a, Command> {
    pub fn new(arena: &'a Arena, style: RowStyle) -> Self {
        Self {
            arena,
            style,
            badge: None,
            label: None,
            suffix: None,
            trails: Vec::new(),
        }
    }

    pub fn badge_styled(mut self, style: &TextStyle, content: impl Into<String>) -> Self {
        self.badge = Some(text(style, content));
        self
    }

    pub fn label(mut self, content: impl Into<String>) -> Self {
        self.label = Some(text(&self.style.label, content));
        self
    }

    pub fn label_styled(mut self, style: &TextStyle, content: impl Into<String>) -> Self {
        self.label = Some(text(style, content));
        self
    }

    /// A word right after the label, in its own style — no gap.
    pub fn suffix_styled(mut self, style: &TextStyle, content: impl Into<String>) -> Self {
        self.suffix = Some(text(style, content));
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

impl<'a, Command> imba::layout::LayoutValue for ListRow<'a, Command> {}

impl<'a, Command: 'a> imba::layout::Layout<'a, Command> for ListRow<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let ListRow {
            arena: row_arena,
            style,
            badge,
            label,
            suffix,
            trails,
        } = self;
        let mut row = imba::layout::Row::new(row_arena);
        let badged = badge.is_some();
        if let Some(badge) = badge {
            row = row.child_by_baseline(badge.pad_insets(imba::layout::Insets {
                left: style.inset,
                ..Default::default()
            }));
        }
        if let Some(label) = label {
            row = row.child_by_baseline(label.pad_insets(imba::layout::Insets {
                left: match badged {
                    true => space::S,
                    false => style.inset,
                },
                // A trail-less label keeps the trail inset off the
                // right edge — the ellipsis lands short of the frame,
                // not against it.
                right: match trails.is_empty() && suffix.is_none() {
                    true => style.trail_inset,
                    false => 0.0,
                },
                ..Default::default()
            }));
        }
        if let Some(suffix) = suffix {
            row = row.child_by_baseline(suffix.pad_insets(imba::layout::Insets {
                right: match trails.is_empty() {
                    true => style.trail_inset,
                    false => 0.0,
                },
                ..Default::default()
            }));
        }
        // The flexible gap is MAIN-AXIS only: a bare `Fill` would
        // stretch to the row's height and drag the baseline group's
        // extent with it — the texts would ride the row's top instead
        // of centering.
        row = row.weighted(1.0, imba::layout::Fill::new().height(0.0));
        let last = trails.len().saturating_sub(1);
        for (index, entry) in trails.into_iter().enumerate() {
            let insets = imba::layout::Insets {
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
        row.align(imba::layout::Alignment::CenterStart)
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
        let mut warmup: imba::list::ListSlice<crate::forest::TreeRow, u64> =
            imba::list::ListSlice::new();
        warmup.push_keyed(
            u64::MAX,
            crate::tree_item::TreeItemView::leaf(
                crate::tree_item::TreeLabel::new("warmup".to_owned(), true, false),
                1,
            ),
            &store,
            &ui,
        );
        let mut slice: imba::list::ListSlice<crate::forest::TreeRow, u64> =
            imba::list::ListSlice::new();
        let mut worst = 0.0f64;
        let mut first = 0.0f64;
        let started = std::time::Instant::now();
        for n in 0..200u64 {
            let one = std::time::Instant::now();
            slice.push_keyed(
                n,
                crate::tree_item::TreeItemView::leaf(
                    crate::tree_item::TreeLabel::new(format!("row {n}"), true, false),
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

        let thunk = imba::layout::Layout::layout(
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
) -> imba::layout::Button<'a, Command, impl Fn() -> Command + 'a> {
    let chat = editor::env::Themes::of(store).part(crate::chrome::ChatChrome::KEY);
    let style = match role {
        ButtonRole::Primary => caps(store, ui).colored(chat.on_accent.0),
        ButtonRole::Ghost => caps(store, ui),
    };
    let dim = style.color;
    let button = imba::layout::Button::new(arena, text(&style, content), on_press)
        .radius(RADIUS_S)
        .pad_content(imba::layout::Insets::xy(space::L, space::S));
    match role {
        ButtonRole::Primary => button.fill(chat.accent.0),
        ButtonRole::Ghost => button.stroke(dim),
    }
}
