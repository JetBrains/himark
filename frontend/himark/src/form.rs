// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's FORM — which pair of embedded themes the appearance
//! commands pick. An engine fact: the C door sets it once at boot
//! (`himark_set_form`) and every shell's theme commands read it.

use imba::store::Store;

/// The desktop pair (2x metrics, the workbench's insets) or the
/// phone pair (3x metrics, rows that are tap targets, the phone's
/// insets). A store resource the shell sets once at boot; absent,
/// the desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Form {
    #[default]
    Desktop,
    Phone,
}

impl Form {
    pub fn of(store: &Store) -> Form {
        store.get::<Form>().copied().unwrap_or_default()
    }

    pub fn set(store: &mut Store, form: Form) {
        store.put(form);
    }

    /// This form's embedded theme in the given appearance.
    pub fn theme(self, dark: bool) -> ::editor::theme::Theme {
        match (self, dark) {
            (Form::Desktop, true) => ::editor::theme::embedded(),
            (Form::Desktop, false) => ::editor::theme::light(),
            (Form::Phone, true) => ::editor::theme::ios(),
            (Form::Phone, false) => ::editor::theme::ios_light(),
        }
    }
}

/// Whether the standing theme is a dark one — every pair's light
/// half is named "…light"; the rest are dark.
pub fn theme_is_dark(store: &Store) -> bool {
    !::editor::env::Themes::of(store).name().ends_with("light")
}
