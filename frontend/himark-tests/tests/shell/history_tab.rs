// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The side column's History tab, rendered at the design's size
//! (1440×900 at 2x) over a seeded commit page shaped like the Main
//! artboard's right column — the newest commit open with its files,
//! the second one HERE.

use std::sync::Arc;

use imba::store::Store;
use imba::View as _;

use changesview::changes_view::{ChangesView, ChangesViewCommand, ViewSets};
use changesview::hichanges::{ChangeEntry, Changes, ChangesStatus, DigestedChangeset, Revision};
use changesview::hihistory::{CommitAuthor, CommitInfo, History, HistoryHead, HistorySnapshot};
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

/// The artboard's commit titles, newest first.
const TITLES: [&str; 17] = [
    "the scroll law stands measured — one tree per tick, and the suite holds it",
    "the document feed is an ordered lane — the servers hear edits in mirror order",
    "the quiet commit — a display-neutral batch keeps the standing tree",
    "heights land as data — the paint report is a belt, and it goes quiet",
    "the shell plays every absent host — overlays carry their attachment",
    "every file gets its plaque — the islands default lands",
    "the header lives with its surface, and the band leads",
    "one scrolling authority — the band leads the scroll",
    "the workbench chrome moves out, the shell keeps the seat",
    "the FFI goes on a diet — one minting door",
    "the features leave the host — terminal, find and files",
    "placement goes through the seated shell",
    "commands read the focus walk’s bindings",
    "the plugins register themselves — the host forgets them",
    "one engine tick per frame, and a starved retry",
    "minor",
    "a stale semantic-token landing is dropped",
];

/// A commit's files as the digest would land them: (path, +, −).
fn landed(
    folder: &editor::location::ResourceLocation,
    files: &[(&[&str], i64, i64)],
) -> DigestedChangeset {
    let entries = files
        .iter()
        .map(|(rel, added, removed)| {
            let mut path: Vec<&str> = folder.path().iter().map(String::as_str).collect();
            path.extend_from_slice(rel);
            let working = located(editor::location::ResourceType::document(), &path);
            ChangeEntry::assembled(
                format!("file:///{}", path.join("/")),
                rel.iter().map(|segment| segment.to_string()).collect(),
                working.clone(),
                Some(working.clone()),
                Some(working),
                Some(*added),
                Some(*removed),
            )
        })
        .collect();
    DigestedChangeset {
        status: ChangesStatus::Ready,
        entries,
    }
}

struct NoCanvas;

impl imba::command::DynamicCommand for NoCanvas {
    fn id(&self) -> &'static str {
        "test.no-canvas"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(&self, _store: &mut Store, _ui: &imba::ui::UiCtx, _fx: &mut imba::command::Fx<'_>) {}
}

/// Mounts the history tab over a seeded page: the newest commit's
/// files landed and expanded, the second commit picked (HERE).
struct ShowHistory;

impl himark::commands::HimarkCommand for ShowHistory {
    fn id(&self) -> &'static str {
        "test.show-history"
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
        let changes = himark::workspace::entity_state(&entity).changes();
        let history = Changes::of(store, changes).expect("the changes").history();
        let folder = located(editor::location::ResourceType::directory(), &["himark-jb"]);
        Changes::adopt_folders(store, changes, &[folder.clone()]);
        History::ensure_folder(store, history, &folder);
        let commits = TITLES
            .iter()
            .enumerate()
            .map(|(index, title)| CommitInfo {
                id: format!("c{index:02}"),
                summary: (*title).to_owned(),
                message: None,
                author: CommitAuthor {
                    name: "Andrey Zaytsev".to_owned(),
                    email: None,
                },
                refs: Vec::new(),
            })
            .collect();
        History::land_snapshot(
            store,
            history,
            &folder,
            HistorySnapshot {
                status: ChangesStatus::Ready,
                head: HistoryHead {
                    branch: Some("main".to_owned()),
                    ..HistoryHead::default()
                },
                commits,
                more: None,
            },
        );
        Changes::adopt_commit_state(
            store,
            changes,
            &folder,
            &Revision::new("c00"),
            &Ok(landed(
                &folder,
                &[
                    (
                        &["frontend", "frontend-host", "src", "chat_flow_tests.rs"],
                        98,
                        0,
                    ),
                    (&["frontend", "himark", "src", "app.rs"], 1, 0),
                    (&["frontend", "himark", "src", "himark.rs"], 13, 0),
                    (&["perf.csv"], 32, 0),
                ],
            )),
        );
        Changes::adopt_commit_state(
            store,
            changes,
            &folder,
            &Revision::new("c01"),
            &Ok(landed(
                &folder,
                &[(&["backend", "agent-host", "src", "server.rs"], 118, 35)],
            )),
        );

        let mut view = ChangesView::open(
            store,
            ui,
            changes,
            ViewSets::History,
            Arc::new(|_source, _reveal| imba::command::Verb::Dynamic(Arc::new(NoCanvas))),
        );
        let mut discarded = imba::effect::Batch::new();
        // The newest commit (the cursor's first stop) unfolds…
        view.perform(
            store,
            ui,
            ChangesViewCommand::Rows(hikit::list_keyboard::ListKeyCommand::Fold {
                index: 1,
                expand: true,
            }),
            &mut discarded.effects(),
        );
        // …and the second, under its seven tree rows, is picked.
        view.perform(
            store,
            ui,
            ChangesViewCommand::Rows(hikit::list_keyboard::ListKeyCommand::Inner(
                imba::list::ListCommand::Activate(9, imba::list::ActivateTrigger::Click),
            )),
            &mut discarded.effects(),
        );
        let view = Changes::mint_view(store, changes, view);
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(hikit::modal::Scrolled::new(
                        changesview::changes_view::ChangesPane::new(changes, view),
                    )),
                    "history.view",
                    fx,
                )
            },
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell history_tab::dump_history_tab_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_history_tab_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());
    register_tabs(&mut app);
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(ShowHistory))));
    super::dock::settle(&mut app, &mut surface);
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/history-tab.png"), data.as_bytes()).expect("write");
}
