// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use himark::AppExt;
use himark::{test_document::plain_document, AppFonts, Application};

fn boot() -> Application {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePeeker));
    app
}

#[test]
fn the_peeker_toggles_filters_and_picks_through_the_registry() {
    let mut app = boot();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let located = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    };
    let open = |app: &mut Application, name: &str, text: &str| {
        assert!(app.perform_command(himark::AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: name.to_owned(),
                document: plain_document(text),
                location: Some(located(name)),
                primary: true,
                target: None,
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
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
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
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    let ui = imba::UiCtx::dont_use_too_slow();
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            himark::ResourceType::directory(),
            himark::Authority::new("test"),
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
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    let ui = imba::UiCtx::dont_use_too_slow();
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            himark::ResourceType::directory(),
            himark::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );

    let mut batch: Batch<PeekerCommand> = Batch::new();
    peeker.launch_find(&store, &ui, "file", &mut batch.effects());
    let locations: Vec<ResourceLocation> = (0..250)
        .map(|index| {
            ResourceLocation::new(
                himark::ResourceType::document(),
                himark::Authority::new("test"),
                vec!["project".to_owned(), format!("file-{index}.md")],
            )
        })
        .collect();
    let ui = imba::UiCtx::dont_use_too_slow();
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
