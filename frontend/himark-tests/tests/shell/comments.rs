// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use himark::app::AppFonts;
use himark::app::Himark;
use himark::app_ext::AppExt;
use imba::store::Store;

fn at(path: &[&str]) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("local"),
        path.iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>(),
    )
}

fn seed(
    id: &str,
    path: &[&str],
    line: u32,
    texts: &[&str],
    resolved: bool,
) -> ::comments::CommentSeed {
    let at_line = documents::text_ext::LineCol { line, col: 0 };
    ::comments::CommentSeed {
        id: id.to_owned(),
        location: at(path),
        range: Some(at_line..at_line),
        turn_id: String::new(),
        resolved,
        entries: texts
            .iter()
            .enumerate()
            .map(|(index, text)| ::comments::EntryRecord {
                id: format!("{id}-{index}"),
                text: text::text::Text::from_string_exact(text),
                ours: index == 0,
            })
            .collect(),
    }
}

/// Seeds the window session's comments and docks their panel.
fn dock_seeded_comments(app: &mut Himark) -> imba::store::Id<::comments::Comments> {
    let window = app.sole_window();
    let comments = app.sole_family().comments();
    ::comments::Comments::adopt_folder(&mut app.store_mut(), comments, &at(&["himark-jb"]));
    ::comments::Comments::land_seeds(
        &mut app.store_mut(),
        comments,
        vec![
            seed(
                "a",
                &["himark-jb", "backend", "agent-host", "src", "server.rs"],
                611,
                &["The feed must drain inside the state lock — otherwise two edits race the servers.", "Agreed, moving it."],
                false,
            ),
            seed(
                "b",
                &["himark-jb", "backend", "agent-host", "src", "server.rs"],
                105,
                &["Default the thinking budget here instead of at every call site."],
                false,
            ),
            seed(
                "c",
                &["himark-jb", "frontend", "toc", "src", "lib.rs"],
                41,
                &["Why does the outline re-derive on every caret move?"],
                true,
            ),
            seed(
                "d",
                &["himark-jb", "README.md"],
                3,
                &["Mention the bundled fonts."],
                false,
            ),
        ],
    );

    struct ShowComments(imba::store::Id<::comments::Comments>);
    impl himark::commands::HimarkCommand for ShowComments {
        fn id(&self) -> &'static str {
            "comments.view"
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
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            let panel = ::comments::panel::CommentsView::open(store, ui, self.0);
            fx.scope(
                move |command| himark::shell::content(window, command),
                |fx| entity.show_dock(store, Box::new(panel), "comments.view", fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
    assert!(app.perform_command(himark::app::windowed(
        window,
        Arc::new(ShowComments(comments)),
    )));
    comments
}

#[test]
fn the_comments_dock_lists_files_and_lines() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1440, 900)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let _ = dock_seeded_comments(&mut app);
    super::dock::settle(&mut app, &mut surface);
    let panel = super::dock::entity(&app)
        .dock_panel()
        .expect("the dock is up")
        .as_any()
        .downcast_ref::<::comments::panel::CommentsView>()
        .expect("the dock holds the comments")
        .clone();
    let rows: Vec<(String, Option<u32>, usize)> = panel
        .comment_rows()
        .into_iter()
        .map(|facts| (facts.quote, facts.line, facts.replies))
        .collect();
    assert!(
        rows.contains(&(
            "The feed must drain inside the state lock — otherwise two edits race the servers."
                .to_owned(),
            Some(612),
            1
        )),
        "a comment row carries its quote, its 1-based line and its replies: {rows:?}"
    );
}

/// The Comments dock over seeded comments, 2880×1800 —
/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell dump_comments_dock_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_comments_dock_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());
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
    let _ = dock_seeded_comments(&mut app);
    super::dock::settle(&mut app, &mut surface);
    let data = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/comments-dock.png"), data.as_bytes()).expect("write");
}
