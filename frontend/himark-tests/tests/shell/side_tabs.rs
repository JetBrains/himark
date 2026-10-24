// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The side column's Files and Find tabs, rendered at the design's
//! size (1440×900 at 2x) over seeded content shaped like the
//! artboards — a look at the tabs' own chrome.

use std::sync::Arc;

use imba::store::Store;

use himark::app::AppFonts;
use himark::app::Himark;
use himark::app_ext::AppExt as _;

fn located(
    kind: editor::location::ResourceType,
    path: &[&str],
) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        kind,
        editor::location::Authority::new("local"),
        path.iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<String>>(),
    )
}

fn dir(path: &[&str]) -> editor::location::ResourceLocation {
    located(editor::location::ResourceType::directory(), path)
}

fn doc(path: &[&str]) -> editor::location::ResourceLocation {
    located(editor::location::ResourceType::document(), path)
}

/// The side column's tabs, as the shell registers them.
fn register_tabs(app: &mut Himark) {
    for (order, (label, command)) in [
        ("History", "history.view"),
        ("Files", "files.tree"),
        ("Find", "search.view"),
        ("Changes", "changes.view"),
        ("Comments", "comments.view"),
    ]
    .into_iter()
    .enumerate()
    {
        app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
            command,
            label,
            order: order as f32,
            side: ::workbench::toolbar::ToolbarSide::Right,
            glyph: Arc::new(|_canvas, _rect, _color| {}),
        });
    }
}

fn write_png(surface: &mut skia_safe::Surface, path: String) {
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(path, data.as_bytes()).expect("write");
}

/// Mounts the files tab over a seeded tree: the listings land as
/// the host's answers would, the cursor stands on `find/Cargo.toml`;
/// a `filter` is typed into the tree's speed search.
struct ShowFiles {
    filter: Option<&'static str>,
}

impl himark::commands::HimarkCommand for ShowFiles {
    fn id(&self) -> &'static str {
        "test.show-files"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        use imba::View as _;
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let root = dir(&["himark-jb"]);
        let folders = vec![root.clone()];
        let mirror = folders.clone();
        let mut discarded = imba::effect::Batch::new();
        let mut panel = filetree::SessionTreeView::open(
            store,
            ui,
            imba::store::Id::mint(),
            &folders,
            Arc::new(move |_store: &Store| mirror.clone()),
            Arc::new(|_store: &Store, _target| None),
            None,
            &mut discarded.effects(),
        );
        let listings = [
            (
                root.clone(),
                [
                    "apps/",
                    "backend/",
                    "docs/",
                    "frontend/",
                    "protocol/",
                    "readme-screenshots/",
                    "scripts/",
                    "tools/",
                    "vendor/",
                    "target/",
                    "Cargo.toml",
                    "Cargo.lock",
                    "README.md",
                    "CONTRIBUTING.md",
                    "LICENSE",
                    "perf.csv",
                ]
                .as_slice(),
            ),
            (
                dir(&["himark-jb", "backend"]),
                ["agent-host/", "find/", "git/"].as_slice(),
            ),
            (
                dir(&["himark-jb", "backend", "find"]),
                ["src/", "Cargo.toml"].as_slice(),
            ),
            (
                dir(&["himark-jb", "backend", "find", "src"]),
                ["lib.rs"].as_slice(),
            ),
        ];
        for (parent, names) in listings {
            let entries = names
                .iter()
                .map(|name| {
                    let mut path: Vec<&str> = parent.path().iter().map(String::as_str).collect();
                    match name.strip_suffix('/') {
                        Some(name) => {
                            path.push(name);
                            dir(&path)
                        }
                        None => {
                            path.push(name);
                            doc(&path)
                        }
                    }
                })
                .collect();
            panel.perform(
                store,
                ui,
                filetree::TreeCommand::Listed {
                    parent,
                    entries: Some(entries),
                },
                &mut discarded.effects(),
            );
        }
        panel.perform(
            store,
            ui,
            filetree::TreeCommand::Follow {
                location: doc(&["himark-jb", "backend", "find", "Cargo.toml"]),
                generation: 1,
            },
            &mut discarded.effects(),
        );
        if let Some(filter) = self.filter {
            panel.perform(
                store,
                ui,
                filetree::TreeCommand::Rows(hikit::list_keyboard::ListKeyCommand::Input(
                    editor::editor_view::EditorCommand::InsertText {
                        text: filter.to_owned(),
                    },
                )),
                &mut discarded.effects(),
            );
        }
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(hikit::modal::Scrolled::new(panel)),
                    "files.tree",
                    fx,
                )
            },
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell side_tabs::dump_files_tab_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_files_tab_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());
    register_tabs(&mut app);
    assert!(app.perform_command(himark::app::windowed(
        window,
        Arc::new(ShowFiles { filter: None }),
    )));
    super::dock::settle(&mut app, &mut surface);
    write_png(&mut surface, format!("{dir}/files-tab.png"));
}

/// The same tab while a filter stands: the live query reads in the
/// FILTER field.
/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell side_tabs::dump_files_tab_filtering_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_files_tab_filtering_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());
    register_tabs(&mut app);
    assert!(app.perform_command(himark::app::windowed(
        window,
        Arc::new(ShowFiles {
            filter: Some("carg"),
        }),
    )));
    super::dock::settle(&mut app, &mut surface);
    write_png(&mut surface, format!("{dir}/files-tab-filtering.png"));
}

/// Mounts the find tab over a seeded feed shaped like the artboard's
/// "view for" results.
struct ShowFind;

impl himark::commands::HimarkCommand for ShowFind {
    fn id(&self) -> &'static str {
        "test.show-find"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        let lists = himark::workspace::entity_state(&entity).lists();
        locations::LocationLists::adopt_folders(store, lists, &[dir(&["himark-jb"])]);
        let feed = locations::FeedId::mint();
        let mut row = locations::LocationsFeedRow {
            title: "Search: view for".to_owned(),
            query: "view for".to_owned(),
            generation: 1,
            done: true,
            ..Default::default()
        };
        let hits: [(&[&str], u32, &str); 16] = [
            (
                &["frontend", "ahp-chat", "src", "transcript.rs"],
                110,
                "impl View for TranscriptView {",
            ),
            (
                &["frontend", "ahp-chat", "src", "transcript.rs"],
                187,
                "impl View for ChatRow {",
            ),
            (
                &["frontend", "ahp-chat", "src", "transcript.rs"],
                430,
                "impl View for TranscriptPane {",
            ),
            (
                &["frontend", "ahp-chat", "src", "transcript.rs"],
                501,
                "impl hikit::panel::PanelView for TranscriptPane",
            ),
            (
                &["frontend", "ahp-chat", "src", "chats.rs"],
                36,
                "impl imba::View for ChatPane {",
            ),
            (
                &["frontend", "ahp-chat", "src", "chats.rs"],
                95,
                "impl hikit::panel::PanelView for ChatPane",
            ),
            (
                &["frontend", "ahp-chat", "src", "composer.rs"],
                57,
                "impl imba::View for ComposerPane {",
            ),
            (
                &["frontend", "ahp-chat", "src", "composer.rs"],
                139,
                "impl hikit::panel::PanelView for ComposerPane",
            ),
            (
                &["frontend", "ahp-chat", "src", "cell.rs"],
                641,
                "impl View for Cell {",
            ),
            (
                &["frontend", "ahp-chat", "src", "chat.rs"],
                211,
                "    /// Build the view for `id` from the session",
            ),
            (
                &["frontend", "ahp-chat", "src", "tool_group.rs"],
                79,
                "impl View for ToolRowView {",
            ),
            (
                &["frontend", "ahp-chat", "src", "turn.rs"],
                144,
                "impl View for TurnView {",
            ),
            (
                &["frontend", "ahp-session", "src", "session", "drawer.rs"],
                532,
                "impl View for AgentsPanel {",
            ),
            (
                &["frontend", "ahp-session", "src", "session", "drawer.rs"],
                587,
                "impl ModalView for AgentsPanel {",
            ),
            (
                &["docs", "navigation.md"],
                63,
                "impl PanelView for ClosedPanel {",
            ),
            (
                &["docs", "model-view.md"],
                8,
                "POSTPONED — per-view for now. The",
            ),
        ];
        for (path, line, context) in hits {
            let column = context.to_ascii_lowercase().find("view for").unwrap_or(0) as u32;
            row.locations.push_back_mut(locations::FoundLocation {
                location: doc(path),
                line,
                column,
                length: 8,
                context: context.to_owned(),
                context_column_start: 0,
            });
        }
        locations::LocationLists::put(store, lists, feed, row);
        locations::LocationLists::set_search(store, lists, feed);
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = locations::search::SearchView::open(store, ui, lists);
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), "search.view", fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell side_tabs::dump_find_tab_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_find_tab_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());
    register_tabs(&mut app);
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(ShowFind))));
    super::dock::settle(&mut app, &mut surface);
    write_png(&mut surface, format!("{dir}/find-tab.png"));
}
