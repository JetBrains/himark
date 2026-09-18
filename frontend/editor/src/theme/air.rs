// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Theme data for the opt-in Air components. Colors and typography are resolved
//! from the same JSON as the rest of the application, including custom themes.

use super::{Rgba, TreeChrome};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct AirTheme {
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub secondary_text: Rgba,
    pub disabled_text: Rgba,
    pub icon: Rgba,
    pub typography: TypographyRoles,
    pub primary: ButtonTheme,
    pub secondary: ButtonTheme,
    pub ghost: ButtonTheme,
    pub checkbox: CheckboxTheme,
    pub list: ListTheme,
    pub tree: TreeChrome,
}

// Older theme files predate Air components. Their existing chrome remains
// unchanged, and Air uses the embedded defaults until the theme opts in.
pub(super) fn defaults() -> AirTheme {
    super::Theme::embedded().ui().air.clone()
}

#[derive(Clone, Deserialize)]
pub struct TypographyRoles {
    pub label: Typography,
    pub caption: Typography,
    pub heading: Typography,
    pub caps: Typography,
    pub key_hint: Typography,
    pub code: Typography,
    pub header: Typography,
}

#[derive(Clone, Deserialize)]
pub struct Typography {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub weight: u16,
    pub tracking: f32,
}

#[derive(Clone, Copy, Deserialize)]
pub struct ControlColors {
    pub fill: Rgba,
    pub border: Rgba,
    pub outline: Rgba,
}

#[derive(Clone, Deserialize)]
pub struct ControlStates {
    pub default: ControlColors,
    pub hovered: ControlColors,
    pub pressed: ControlColors,
    pub focused: ControlColors,
    pub disabled: ControlColors,
}

#[derive(Clone, Deserialize)]
pub struct ButtonTheme {
    pub states: ControlStates,
    pub text: Rgba,
    pub disabled_text: Rgba,
}

#[derive(Clone, Deserialize)]
pub struct CheckboxTheme {
    pub unchecked: ControlStates,
    pub checked: ControlStates,
    pub icon: Rgba,
    pub disabled_icon: Rgba,
}

#[derive(Clone, Deserialize)]
pub struct ListTheme {
    pub default: Rgba,
    pub hovered: Rgba,
    pub pressed: Rgba,
    pub selected: Rgba,
}
