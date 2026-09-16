// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Backend-free catalogue of the shared UI primitives, used by the desktop
//! gallery binary and its headless visual regression tests.
use imba::{
    arena::Arena, checkbox::CheckboxValue, thunk_ext::ThunkExt, Column, Layout, LayoutExt, Row,
    Store, UiCtx, View,
};

use himark::{ui::*, TreeItemView, TreeLabel, TreeTint};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GalleryMode {
    #[default]
    Interactive,
    AllStates,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryCommand {
    Mode(GalleryMode),
    ButtonState(usize, ControlState),
    Press,
    Check,
    Expand,
}

#[derive(Clone)]
pub struct GalleryView {
    mode: GalleryMode,
    pub(crate) presses: usize,
    pub(crate) checked: bool,
    pub(crate) expanded: bool,
    trees: [TreeItemView<TreeLabel>; 4],
    pub(crate) button_states: [ControlState; 3],
}

impl GalleryView {
    pub fn new(mode: GalleryMode) -> Self {
        Self {
            mode,
            presses: 0,
            button_states: [ControlState::Default; 3],
            checked: false,
            expanded: false,
            trees: [
                TreeItemView::branch(
                    TreeLabel::new("Collapsed directory".into(), true, false)
                        .tinted(TreeTint::Directory),
                    0,
                    false,
                )
                .toggling_on_body(),
                TreeItemView::branch(
                    TreeLabel::new("Expanded directory".into(), true, false)
                        .tinted(TreeTint::Directory),
                    0,
                    true,
                )
                .toggling_on_body(),
                TreeItemView::leaf(
                    TreeLabel::new("Nested file.rs".into(), true, false).tinted(TreeTint::File),
                    1,
                ),
                TreeItemView::leaf(
                    TreeLabel::new("Dimmed, non-selectable leaf".into(), false, true),
                    0,
                ),
            ],
        }
    }

    pub fn mode(&self) -> GalleryMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: GalleryMode) {
        self.mode = mode;
    }

    pub(crate) fn content<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, GalleryCommand> + imba::LayoutValue + 'a {
        let themes = himark::env::Themes::of(store);
        let theme = themes.ui();
        let static_states = self.mode == GalleryMode::AllStates;
        let title = heading(store, ui);
        let note = caption(store, ui);
        let mut page = Column::new(arena)
            .gap(space::XL)
            .child(text(&title, "Himark UI gallery"))
            .child(text(&note, "Air UI · shared components"))
            .child(
                Row::new(arena)
                    .gap(space::M)
                    .child(button(
                        arena,
                        store,
                        ui,
                        if !static_states {
                            ButtonRole::Primary
                        } else {
                            ButtonRole::Ghost
                        },
                        "Interactive",
                        || GalleryCommand::Mode(GalleryMode::Interactive),
                    ))
                    .child(button(
                        arena,
                        store,
                        ui,
                        if static_states {
                            ButtonRole::Primary
                        } else {
                            ButtonRole::Ghost
                        },
                        "Full list of states",
                        || GalleryCommand::Mode(GalleryMode::AllStates),
                    )),
            )
            .child(text(
                &note,
                if static_states {
                    "All supported states are shown together. Specimens are read-only."
                } else {
                    "Click buttons, toggle the checkbox, and expand the tree. Scroll to explore."
                },
            ));

        let section = |name: &str| text(&caps(store, ui), name);
        let mut typography = Column::new(arena)
            .gap(space::S)
            .child(section("01 / TYPOGRAPHY"));
        for (style, sample) in [
            (heading(store, ui), "Heading — a section title"),
            (label(store, ui), "Label — the quick brown fox"),
            (caption(store, ui), "Caption — secondary information"),
            (caps(store, ui), "CAPS — TRACKED SECTION LABEL"),
            (key_hint(store, ui), "Key hint — Ctrl+Shift+P"),
            (code(store, ui), "Code — fn main() {}"),
        ] {
            typography = typography.child(text(&style, sample));
        }
        page = page.child(typography);

        let mut surfaces = Row::new(arena).gap(space::L);
        for (name, surface) in [
            ("Fill", Surface::fill(theme.peeker.well.0)),
            (
                "Bordered",
                Surface::bordered(theme.peeker.well.0, theme.peeker.rule.0),
            ),
            ("Outline", Surface::outline(theme.peeker.rule.0)),
        ] {
            surfaces = surfaces.weighted(
                1.0,
                text(&label(store, ui), name)
                    .pad(space::M)
                    .backdrop(surface.painter()),
            );
        }
        page = page.child(
            Column::new(arena)
                .gap(space::M)
                .child(section("02 / SURFACES"))
                .child(surfaces),
        );

        let states = [
            ("Default", ControlState::Default),
            ("Hovered", ControlState::Hovered),
            ("Pressed", ControlState::Pressed),
            ("Focused", ControlState::Focused),
            ("Disabled", ControlState::Disabled),
        ];
        let mut buttons = Column::new(arena)
            .gap(space::M)
            .child(section("03 / BUTTONS"));
        for (index, (name, role)) in [
            ("Primary", ButtonRole::Primary),
            ("Secondary", ButtonRole::Secondary),
            ("Ghost", ButtonRole::Ghost),
        ]
        .into_iter()
        .enumerate()
        {
            let mut row = Row::new(arena).gap(space::L);
            if static_states {
                for (state_name, state) in states {
                    row = row.child(
                        Column::new(arena)
                            .gap(space::S)
                            .child(text(&note, state_name))
                            .child(
                                button(arena, store, ui, role, name, || GalleryCommand::Press)
                                    .state(state),
                            ),
                    );
                }
            } else {
                row = row.child(
                    button(arena, store, ui, role, name, || GalleryCommand::Press)
                        .state(self.button_states[index])
                        .on_state_change(move |state| GalleryCommand::ButtonState(index, state)),
                );
            }
            buttons = buttons.child(row);
        }
        buttons = buttons.child(text(
            &note,
            if static_states {
                "Primary, secondary and ghost buttons in every visual state.".to_owned()
            } else {
                format!("Button and row actions: {}", self.presses)
            },
        ));
        page = page.child(buttons);

        let mut checks = Column::new(arena)
            .gap(space::M)
            .child(section("04 / CHECKBOX"));
        let values: &[CheckboxValue] = if static_states {
            &[
                CheckboxValue::Unchecked,
                CheckboxValue::Checked,
                CheckboxValue::Indeterminate,
            ]
        } else if self.checked {
            &[CheckboxValue::Checked]
        } else {
            &[CheckboxValue::Unchecked]
        };
        for &value in values {
            let mut row = Row::new(arena).gap(space::XL);
            let check_states: &[(&str, ControlState)] =
                if static_states { &states } else { &states[..1] };
            for &(state_name, state) in check_states {
                let name = match value {
                    CheckboxValue::Unchecked => "Unchecked",
                    CheckboxValue::Checked => "Checked",
                    CheckboxValue::Indeterminate => "Indeterminate",
                };
                let label = label(store, ui).colored(air_tokens::color(
                    store,
                    if state == ControlState::Disabled {
                        "text-disabled"
                    } else {
                        "text-primary"
                    },
                ));
                let control = Row::new(arena)
                    .gap(6.0)
                    .align_items(imba::CrossAlign::Center)
                    .child(imba::fixed(
                        air_checkbox(store, value, state).map(|_| GalleryCommand::Check),
                    ))
                    .child(text(&label, name).on_click(|| GalleryCommand::Check));
                row = if static_states {
                    row.child(
                        Column::new(arena)
                            .gap(space::S)
                            .child(text(&note, state_name))
                            .child(control),
                    )
                } else {
                    row.child(control)
                };
            }
            checks = checks.child(row);
        }
        page = page.child(checks);

        let mut rows = Column::new(arena)
            .gap(space::XS)
            .child(section("05 / LIST ROWS"));
        for (name, style) in [
            ("Standard", RowStyle::standard(store, ui)),
            ("Drawer", RowStyle::drawer(store, ui)),
            ("Header", RowStyle::header(store, ui)),
        ] {
            rows = rows.child(
                ListRow::new(arena, style)
                    .label(name)
                    .trail("Metadata")
                    .action("Open", || GalleryCommand::Press),
            );
        }
        if static_states {
            rows = rows
                .child(ListRow::new(arena, RowStyle::standard(store, ui)).label("Label only"))
                .child(
                    ListRow::new(arena, RowStyle::standard(store, ui))
                        .label("Multiple trails")
                        .trail("Rust")
                        .trail("Ctrl+P"),
                );
        }
        if static_states {
            for (name, state) in [
                ("Hovered", ControlState::Hovered),
                ("Focused", ControlState::Focused),
                ("Disabled", ControlState::Disabled),
            ] {
                rows = rows.child(
                    ListRow::new(arena, RowStyle::standard(store, ui))
                        .state(store, state)
                        .label(name)
                        .trail("Metadata")
                        .action("Open", || GalleryCommand::Press),
                );
            }
            rows = rows.child(
                ListRow::new(arena, RowStyle::standard(store, ui))
                    .selected(store)
                    .label("Selected")
                    .trail("Metadata"),
            );
        }
        page = page.child(rows);

        let mut trees = Column::new(arena)
            .gap(space::XS)
            .child(section("06 / TREE ITEMS"));
        let branches: &[bool] = if static_states {
            &[false, true]
        } else {
            std::slice::from_ref(&self.expanded)
        };
        for &expanded in branches {
            trees = trees.child(
                self.trees[usize::from(expanded)]
                    .display(arena, store, ui)
                    .map_layout(|_| GalleryCommand::Expand),
            );
            if expanded {
                trees = trees.child(
                    self.trees[2]
                        .display(arena, store, ui)
                        .map_layout(|_| GalleryCommand::Press),
                );
            }
        }
        if static_states {
            trees = trees.child(
                self.trees[3]
                    .display(arena, store, ui)
                    .map_layout(|_| GalleryCommand::Press),
            );
        }
        page.child(trees).pad(space::XL)
    }

    pub(crate) fn apply(&mut self, command: GalleryCommand) {
        match command {
            GalleryCommand::Mode(mode) => self.set_mode(mode),
            _ if self.mode == GalleryMode::AllStates => {}
            GalleryCommand::Press => self.presses += 1,
            GalleryCommand::ButtonState(index, state) => self.button_states[index] = state,
            GalleryCommand::Check => self.checked = !self.checked,
            GalleryCommand::Expand => self.expanded = !self.expanded,
        }
    }
}

impl View for GalleryView {
    type Command = GalleryCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.apply(command);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        self.content(arena, store, ui)
    }
}
