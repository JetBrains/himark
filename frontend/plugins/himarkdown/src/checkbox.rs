// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use imba::checkbox::{checkbox, CheckboxCommand, CheckboxStyle};
use imba::{arena::Arena, store::Store, UiCtx};
use operation::{Op, Operation};

use himark::Theme;

pub(crate) fn checkbox_marker(raw: &str) -> Option<(Range<usize>, bool)> {
    let indent = raw.len() - raw.trim_start().len();
    let rest = &raw[indent..];
    let after_bullet = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))?;
    let marker_start = indent + (rest.len() - after_bullet.len());
    let inner = after_bullet.strip_prefix('[')?;
    let checked = match inner.chars().next()? {
        'x' | 'X' => true,
        ' ' => false,
        _ => return None,
    };
    let inner = &inner[1..];
    if !inner.starts_with(']') {
        return None;
    }
    let tail = &inner[1..];
    if !(tail.is_empty() || tail.starts_with(' ') || tail.starts_with('\n')) {
        return None;
    }
    Some((marker_start..marker_start + 3, checked))
}

#[derive(Clone)]
pub(crate) struct CheckboxView {
    checked: bool,

    range: Range<u32>,

    pending: Option<Operation>,
    chrome: himark::theme::CheckboxChrome,
}

impl CheckboxView {
    pub(crate) fn new(checked: bool, theme: &Theme) -> Self {
        Self {
            checked,
            range: 0..0,
            pending: None,
            chrome: theme.ui().checkbox.clone(),
        }
    }
}

impl imba::View for CheckboxView {
    type Command = CheckboxCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
            "checkbox.toggle",
            "Toggle Checkbox",
            CheckboxCommand::Toggle,
        )])
    }
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            CheckboxCommand::Toggle => {
                let (old, new) = match self.checked {
                    true => ("x", " "),
                    false => (" ", "x"),
                };
                self.pending = Some(Operation::from_ops([
                    Op::Retain(self.range.start + 1),
                    Op::Delete(old.to_owned()),
                    Op::Insert(new.to_owned()),
                ]));
                self.checked = !self.checked;
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        let chrome = &self.chrome;
        imba::fixed(checkbox(
            self.checked,
            CheckboxStyle {
                size: chrome.size,
                radius: chrome.radius,
                stroke: chrome.stroke,
                border: chrome.border.0,
                fill: chrome.fill.0,
                check: chrome.check.0,
            },
        ))
    }
}

impl himark::InlayEditing for CheckboxView {
    fn take_edit(&mut self) -> Option<Operation> {
        self.pending.take()
    }

    fn set_range(&mut self, range: Range<u32>) {
        self.range = range;
    }

    fn adopt_from(
        &mut self,
        previous: &Self,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &Theme,
    ) -> bool {
        self.checked == previous.checked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_task_markers() {
        assert_eq!(checkbox_marker("- [x] done"), Some((2..5, true)));
        assert_eq!(checkbox_marker("- [ ] open"), Some((2..5, false)));
        assert_eq!(checkbox_marker("  * [X] deep"), Some((4..7, true)));
        assert_eq!(checkbox_marker("- [x]"), Some((2..5, true)));
        assert_eq!(checkbox_marker("- [y] nope"), None);
        assert_eq!(checkbox_marker("- plain"), None);
        assert_eq!(checkbox_marker("plain [x]"), None);
    }

    #[test]
    fn the_checkbox_offers_its_toggle_for_presentation() {
        use himark::InlayEditing;
        use imba::View;
        let theme = Theme::embedded();
        let mut store = Store::new();
        let ui = UiCtx::cold();
        let mut checkbox = CheckboxView::new(false, &theme);
        checkbox.set_range(2..5);

        let mut commands = imba::focus::frame_commands(&checkbox, &store, &ui);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "checkbox.toggle");
        assert_eq!(commands[0].name, "Toggle Checkbox");

        let command = commands.pop().expect("the toggle").command;
        checkbox.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
        assert!(checkbox.take_edit().is_some(), "the toggle writes through");
        assert!(checkbox.checked);
    }

    #[test]
    fn toggling_writes_the_marker_through() {
        use himark::InlayEditing;
        use imba::View;
        let theme = Theme::embedded();
        let mut store = Store::new();
        let ui = UiCtx::cold();
        let mut checkbox = CheckboxView::new(false, &theme);
        checkbox.set_range(2..5);
        checkbox.perform(
            &mut store,
            &ui,
            CheckboxCommand::Toggle,
            &mut imba::effect::Batch::new().effects(),
        );
        let edit = checkbox.take_edit().expect("the toggle writes through");

        let ops: Vec<_> = edit.iter().collect();
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], Op::Retain(3)));
        assert!(matches!(&ops[1], Op::Delete(text) if text == " "));
        assert!(matches!(&ops[2], Op::Insert(text) if text == "x"));
    }
}
