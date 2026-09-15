// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Backend-free catalogue of the shared UI primitives, used by the desktop
//! gallery binary and its headless visual regression tests.
use imba::{
    arena::Arena,
    checkbox::{checkbox, CheckboxStyle},
    thunk_ext::ThunkExt,
    Column, Layout, LayoutExt, Row, Store, UiCtx, View,
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
}

impl GalleryView {
    pub fn new(mode: GalleryMode) -> Self {
        Self {
            mode,
            presses: 0,
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
        let title = heading(store, ui).sized(32.0);
        let note = caption(store, ui).sized(16.0);
        let mut page = Column::new(arena)
            .gap(space::XL)
            .child(text(&title, "Himark UI gallery"))
            .child(text(
                &note,
                "Shared components · imba + himark design system",
            ))
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
                        "INTERACTIVE",
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
                        "FULL LIST OF STATES",
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
                    .pad(space::XL)
                    .backdrop(surface.painter()),
            );
        }
        page = page.child(
            Column::new(arena)
                .gap(space::M)
                .child(section("02 / SURFACES"))
                .child(surfaces),
        );

        let mut buttons = Column::new(arena)
            .gap(space::M)
            .child(section("03 / BUTTONS"));
        for (name, role) in [
            ("PRIMARY", ButtonRole::Primary),
            ("GHOST", ButtonRole::Ghost),
        ] {
            let mut row = Row::new(arena).gap(space::L).child(button(
                arena,
                store,
                ui,
                role,
                format!("{name} / ENABLED"),
                || GalleryCommand::Press,
            ));
            if static_states {
                // Button::enabled controls input only; the production primitive
                // deliberately leaves disabled colors to its caller.
                row = row.child(
                    button(arena, store, ui, role, format!("{name} / DISABLED"), || {
                        GalleryCommand::Press
                    })
                    .enabled(false),
                );
            }
            buttons = buttons.child(row);
        }
        buttons = buttons.child(text(&note, if static_states {
            "Enabled / disabled. Buttons currently have no separate hover or pressed appearance.".to_owned()
        } else {
            format!("Button and row actions: {}", self.presses)
        }));
        page = page.child(buttons);

        let chrome = &theme.checkbox;
        let check_style = CheckboxStyle {
            size: chrome.size,
            radius: chrome.radius,
            stroke: chrome.stroke,
            border: chrome.border.0,
            fill: chrome.fill.0,
            check: chrome.check.0,
        };
        let mut checks = Row::new(arena).gap(space::XL);
        let states: &[bool] = if static_states {
            &[false, true]
        } else {
            std::slice::from_ref(&self.checked)
        };
        for &checked in states {
            checks = checks.child(
                Row::new(arena)
                    .gap(space::M)
                    .align_items(imba::CrossAlign::Center)
                    .child(imba::fixed(
                        checkbox(checked, check_style).map(|_| GalleryCommand::Check),
                    ))
                    .child(text(
                        &label(store, ui),
                        if checked { "Checked" } else { "Unchecked" },
                    )),
            );
        }
        page = page.child(
            Column::new(arena)
                .gap(space::M)
                .child(section("04 / CHECKBOX"))
                .child(checks),
        );

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
                    .action("OPEN", || GalleryCommand::Press),
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
