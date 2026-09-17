// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Opt-in Air UI components, composed from Imba layouts and the editor theme.
//! Existing Himark controls can migrate to this crate independently.
//! Focus outlines use Imba's window overlay host; standalone renderers should
//! install `imba::overlay::WINDOW` on their root thunk.

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

mod outline;
mod typography;
pub use typography::{text, Text, TextStyle};
mod button;
pub use button::{Button, ButtonVisual, ControlState};
pub mod checkbox;
mod tree_item;
pub use tree_item::*;

#[cfg(test)]
mod tests;

fn role(
    store: &Store,
    ui: &UiCtx,
    role: &editor::theme::air::Typography,
    secondary: bool,
) -> TextStyle {
    let theme = editor::env::Themes::of(store);
    TextStyle::new(
        store,
        ui,
        role,
        if secondary {
            theme.ui().air.secondary_text.0
        } else {
            theme.ui().air.text.0
        },
    )
}

pub fn label(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.label,
        false,
    )
}

pub fn caption(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.caption,
        true,
    )
}

pub fn heading(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.heading,
        false,
    )
}

pub fn caps(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.caps,
        true,
    )
}

pub fn key_hint(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.key_hint,
        true,
    )
}

pub fn code(store: &Store, ui: &UiCtx) -> TextStyle {
    role(
        store,
        ui,
        &editor::env::Themes::of(store).ui().air.typography.code,
        false,
    )
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
        let tree = editor::env::Themes::of(store).ui().air.tree.clone();
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
        Self {
            inset: space::S,
            trail_inset: space::S,
            label: role(
                store,
                ui,
                &editor::env::Themes::of(store).ui().air.typography.header,
                true,
            ),
            trail: key_hint(store, ui),
            air: space::XS,
        }
    }
}

enum RowEntry<'a, Command> {
    Text(Text),
    Action(Text, Box<dyn Fn() -> Command + 'a>),
}

/// The one leading-label-trail row: label runs from the left inset,
/// trails pin to the right one, everything baseline-aligned and
/// vertically centered. Presses on the row are the caller's business
/// (`.on_event` on the whole row); `action` gives one trail its own
/// press.
pub struct ListRow<'a, Command> {
    arena: &'a Arena,
    style: RowStyle,
    label: Option<Text>,
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
        let theme = editor::env::Themes::of(store);
        let colors = &theme.ui().air;
        self.background = Some(match state {
            ControlState::Hovered | ControlState::Focused => colors.list.hovered.0,
            ControlState::Pressed => colors.list.pressed.0,
            _ => colors.list.default.0,
        });
        if state == ControlState::Disabled {
            self.enabled = false;
            self.style.label.color = colors.disabled_text.0;
            self.style.trail.color = self.style.label.color;
        }
        self
    }

    pub fn selected(mut self, store: &Store) -> Self {
        self.background = Some(editor::env::Themes::of(store).ui().air.list.selected.0);
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
                    row.child_by_baseline(text.on_click(on_press).pad_insets(insets))
                }
                RowEntry::Action(text, _) => row.child_by_baseline(text.pad_insets(insets)),
            };
        }
        // The row's height is ITS OWN: the label's text block plus
        // the style's symmetric air — no shared row-height anywhere.
        let height = style.label.line_height + 2.0 * style.air;
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

#[derive(Clone, Copy)]
pub enum ButtonRole {
    Primary,
    Secondary,
    Ghost,
}

pub fn button<'a, Command: 'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    role: ButtonRole,
    content: impl Into<String>,
    on_press: impl Fn() -> Command + 'a,
) -> Button<'a, Command, impl Fn() -> Command + 'a> {
    let theme = editor::env::Themes::of(store);
    let colors = match role {
        ButtonRole::Primary => &theme.ui().air.primary,
        ButtonRole::Secondary => &theme.ui().air.secondary,
        ButtonRole::Ghost => &theme.ui().air.ghost,
    };
    let mut style = label(store, ui).colored(colors.text.0);
    let ghost = matches!(role, ButtonRole::Ghost);
    if !ghost {
        style.tracking = 0.04;
    }
    let content = content.into();
    Button::new(arena, text(&style, content.clone()), on_press)
        .disabled_content(arena, text(&style.colored(colors.disabled_text.0), content))
        .radius(if ghost { 3.0 } else { 4.0 })
        .min_width(if ghost { 0.0 } else { 60.0 })
        .max_width(if ghost { f32::MAX } else { 256.0 })
        // Includes the label wrapper's 4px (2px for Ghost) on each side.
        .pad_content(imba::Insets::xy(
            if ghost { 4.0 } else { 10.0 },
            if ghost { 2.0 } else { 4.0 },
        ))
        .visuals([
            colors.states.default.into(),
            colors.states.hovered.into(),
            colors.states.pressed.into(),
            colors.states.focused.into(),
            colors.states.disabled.into(),
        ])
}

/// A themed Air checkbox with independently controlled value and appearance.
pub fn air_checkbox<'a>(
    store: &Store,
    value: checkbox::CheckboxValue,
    state: ControlState,
) -> impl imba::Thunk<'a, checkbox::CheckboxCommand> + 'a {
    use checkbox::{checkbox_with_visual, CheckboxStyle, CheckboxValue, CheckboxVisual};
    let theme = editor::env::Themes::of(store);
    let colors = &theme.ui().air.checkbox;
    let states = if value == CheckboxValue::Unchecked {
        &colors.unchecked
    } else {
        &colors.checked
    };
    let visual = match state {
        ControlState::Default => states.default,
        ControlState::Hovered => states.hovered,
        ControlState::Pressed => states.pressed,
        ControlState::Focused => states.focused,
        ControlState::Disabled => states.disabled,
    };
    checkbox_with_visual(
        value,
        CheckboxStyle {
            size: 16.0,
            radius: 2.0,
            stroke: 1.0,
        },
        CheckboxVisual {
            background: visual.fill.0,
            border: visual.border.0,
            icon: if state == ControlState::Disabled {
                colors.disabled_icon.0
            } else {
                colors.icon.0
            },
            outline: visual.outline.0,
        },
        state != ControlState::Disabled,
    )
}
