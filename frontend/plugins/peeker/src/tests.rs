// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use editor::test_document::plain_document;
use himark::app::AppFonts;
use himark::app::Himark;
use himark::app_ext::AppExt;

fn boot() -> Himark {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePeeker));
    app
}

#[test]
fn the_peeker_toggles_filters_and_picks_through_the_registry() {
    let mut app = boot();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let located = |name: &str| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    };
    let open = |app: &mut Himark, name: &str, text: &str| {
        assert!(app.perform_command(himark::app::opened(
            app.sole_window(),
            himark::app::OpenedDocument {
                documents: app.sole_documents(),
                name: name.to_owned(),
                document: plain_document(text),
                location: Some(located(name)),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    };

    open(&mut app, "beta", "beta body");
    assert!(himark::test_driver::type_text(&mut app, "x"));
    open(&mut app, "alpha", "alpha body");

    assert!(
        app.perform_registered(app.sole_window(), "peeker.toggle"),
        "the peeker opens"
    );
    app.draw_window(app.sole_window(), surface.canvas());
    let listed = labels(&app).expect("the peeker is the modal");
    assert!(
        listed.iter().any(|label| label == "alpha"),
        "open documents list under their names: {listed:?}"
    );
    assert!(
        preview_height(&app, app.store()).is_some(),
        "the selection previews"
    );

    assert!(himark::test_driver::type_text(&mut app, "beta"));
    let listed = labels(&app).expect("still open");
    assert_eq!(listed, vec!["beta"], "the filter narrowed the list");

    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(labels(&app).is_none(), "picking closes the peeker");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xbeta body"),
        "the picked document took the focused pane, unsaved edit intact"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(labels(&app).is_some());
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(labels(&app).is_none());
}

#[test]
fn a_query_keystroke_cancels_the_in_flight_find() {
    use imba::effect::{Batch, Message};
    let mut store = Store::new();
    let ui = ::editor::test_document::test_ui();
    let home = ahp_wire::SessionId::local_default(&store);
    let documents = ahp_session::session::state::Hosts::ensure_state(&mut store, &home).documents();
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        documents,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );

    let find_tokens = |peeker: &mut Peeker, query: &str| {
        let mut batch: Batch<PeekerCommand> = Batch::new();
        peeker.launch_find(&store, &ui, query, &mut batch.effects());
        let mut cancels = Vec::new();
        let mut finds = Vec::new();
        for message in batch.drain() {
            match message {
                Message::Launch(token, effect) if effect.is::<FindEffect>() => finds.push(token),
                Message::Relaunch(previous, token, effect) if effect.is::<FindEffect>() => {
                    cancels.push(previous);
                    finds.push(token);
                }
                Message::Cancel(token) => cancels.push(token),
                _ => {}
            }
        }
        (cancels, finds)
    };

    let (cancels, finds) = find_tokens(&mut peeker, "readme");
    assert!(cancels.is_empty(), "nothing in flight yet");
    assert_eq!(finds.len(), 1, "one find holds the lane");

    let (cancels, next) = find_tokens(&mut peeker, "readmes");
    assert_eq!(cancels, finds, "the keystroke cancels the in-flight find");
    assert_eq!(next.len(), 1, "the lane refills with one");
}

#[test]
fn the_list_caps_at_two_hundred_rows_and_counts_the_rest() {
    use imba::effect::Batch;
    let mut store = Store::new();
    let ui = ::editor::test_document::test_ui();
    let home = ahp_wire::SessionId::local_default(&store);
    let documents = ahp_session::session::state::Hosts::ensure_state(&mut store, &home).documents();
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        documents,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );

    let mut batch: Batch<PeekerCommand> = Batch::new();
    peeker.launch_find(&store, &ui, "file", &mut batch.effects());
    let locations: Vec<ResourceLocation> = (0..250)
        .map(|index| {
            ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["project".to_owned(), format!("file-{index}.md")],
            )
        })
        .collect();
    let ui = ::editor::test_document::test_ui();
    imba::View::perform(
        &mut peeker,
        &mut store,
        &ui,
        PeekerCommand::Found {
            serial: 1,
            locations,
        },
        &mut Batch::new().effects(),
    );

    assert_eq!(peeker.labels().len(), 200, "the list stops at the cap");
    assert_eq!(peeker.hidden_count(), 50, "the note counts the rest");
}

/// The jump sheet over a window, a query typed — the redesign's
/// dialog (Jump.dc.html) at the mock's 2x size.
/// `HIMARK_SHOT=<dir> cargo test -p peeker dump_jump_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_jump_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = boot();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let files: &[&[&str]] = &[
        &["backend", "agent-host", "src", "main.rs"],
        &["frontend", "ahp-chat", "src", "transcript.rs"],
        &["frontend", "hikit", "src", "panel_tests.rs"],
        &["frontend", "ahp-chat", "src", "composer.rs"],
        &["frontend", "himark", "src", "startup_profile.rs"],
        &["frontend", "ahp-chat", "src", "main.rs"],
        &["frontend", "himark", "src", "app_ext.rs"],
        &["tools", "grammar-pack", "src", "main.rs"],
        &["frontend", "himark", "src", "main.rs"],
        &["frontend", "himark", "src", "app.rs"],
    ];
    let body = "// Copyright © 2026 JetBrains s.r.o.\n// SPDX-License-Identifier: Apache-2.0\n\nfn main() {\n    let out = std::env::args()\n        .nth(1)\n        .expect(\"usage: grammar-pack <out-dir>\");\n    let out = std::path::PathBuf::from(out);\n    std::fs::create_dir_all(&out).expect(\"out dir\");\n}\n";
    for path in files {
        let location = editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            path.iter().map(|segment| segment.to_string()).collect::<Vec<_>>(),
        );
        assert!(app.perform_command(himark::app::opened(
            app.sole_window(),
            himark::app::OpenedDocument {
                documents: app.sole_documents(),
                name: path.last().unwrap().to_string(),
                document: plain_document(body),
                location: Some(location),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    }
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "ar"));
    for tick in 0..30 {
        app.draw_window(app.sole_window(), surface.canvas());
        let busy = himark::test_driver::animate(
            &mut app,
            imba::anim::AnimationClock::from_millis(tick as f64 * 32.0),
        );
        if !busy && tick > 1 {
            break;
        }
    }
    let data = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::create_dir_all(&dir).expect("the shot dir");
    std::fs::write(format!("{dir}/jump.png"), data.as_bytes()).expect("write");
}
