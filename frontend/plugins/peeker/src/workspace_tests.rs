// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use ::editor::test_document::plain_document;
use editor::location::Authority;
use editor::location::ResourceType;
use himark::app::AppFonts;
use himark::app::Application;
use himark::app::OpenedDocument;
use himark::app_ext::AppExt;
use imba::effect::EffectHandler;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

fn doc_location(name: &str) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        Authority::new("test"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

fn folder_location() -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::directory(),
        Authority::new("test"),
        vec!["project".to_owned()],
    )
}

struct StubFind;

impl EffectHandler<FindEffect> for StubFind {
    async fn handle(&self, _effect: FindEffect) -> Vec<ResourceLocation> {
        vec![doc_location("notes.md")]
    }
}

struct StubFetch(Arc<AtomicUsize>);

impl EffectHandler<FetchDocumentEffect> for StubFetch {
    async fn handle(&self, _effect: FetchDocumentEffect) -> Option<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Some("workspace notes body".to_owned())
    }
}

struct StubBuild;

impl EffectHandler<BuildDocumentEffect> for StubBuild {
    async fn handle(&self, effect: BuildDocumentEffect) -> documents::BuiltDocument {
        documents::BuiltDocument {
            document: plain_document(&effect.text),
        }
    }
}

struct StubOpenByLocation;

impl EffectHandler<himark::workspace::OpenByLocationEffect> for StubOpenByLocation {
    async fn handle(
        &self,
        effect: himark::workspace::OpenByLocationEffect,
    ) -> himark::app::AppCommand {
        himark::app::AppCommand::Opened(
            effect.window,
            OpenedDocument {
                documents: effect.documents,
                name: effect.location.name().to_owned(),
                document: plain_document("fallback body"),
                location: Some(effect.location),
                primary: effect.primary,
                target: effect.target,
                focus: false,
            },
        )
    }
}

struct AddFolder;

impl himark::commands::WindowedCommand for AddFolder {
    fn id(&self) -> &'static str {
        "test.add-folder"
    }
    fn name(&self) -> String {
        "Add Folder".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let id = himark::test_support::seed_session_folders(store, &[folder_location()]);
        himark::app::switch_session(store, window, id, fx);
    }
}

fn boot(
    fetches: &Arc<AtomicUsize>,
) -> (
    Application,
    mpsc::Receiver<himark::app::AppCommand>,
    himark::effects::BackgroundRunner,
) {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(Arc::new(TogglePeeker));
    app.register_handler::<FindEffect>(StubFind);
    app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(fetches)));
    app.register_handler::<BuildDocumentEffect>(StubBuild);
    app.register_handler::<himark::workspace::OpenByLocationEffect>(StubOpenByLocation);

    struct StubWatch;
    impl imba::effect::EffectHandler<documents::watch::SubscribeEffect> for StubWatch {
        async fn handle(
            &self,
            _effect: documents::watch::SubscribeEffect,
        ) -> Option<documents::watch::Subscription> {
            Some(documents::watch::Subscription(7))
        }
    }
    app.register_handler::<documents::watch::SubscribeEffect>(StubWatch);
    app.observe_file_changes();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    (app, arriving, runner)
}

fn settle(
    app: &mut Application,
    arriving: &mpsc::Receiver<himark::app::AppCommand>,
    runner: &himark::effects::BackgroundRunner,
) {
    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }
}

#[test]
fn found_documents_preview_and_adopt_on_pick() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let (mut app, arriving, runner) = boot(&fetches);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(app.add_document(
        app.sole_window(),
        plain_document("alpha body"),
        "alpha".to_owned(),
        false
    ));
    assert!(app.perform_batch(vec![himark::app::AppCommand::Windowed(
        app.sole_window(),
        Arc::new(AddFolder)
    )]));
    let baseline_docs = documents::OpenDocuments::list(app.store(), app.sole_documents()).len();

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "no"));
    settle(&mut app, &arriving, &runner);

    let listed = labels(&app).expect("the peeker is the modal");
    assert_eq!(
        listed,
        vec!["notes.md"],
        "the found row lists like any other"
    );

    settle(&mut app, &arriving, &runner);
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "one fetch per location");
    assert!(
        preview_height(&app, app.store()).is_some(),
        "the found row previews like an open document"
    );
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs + 1,
        "the fetched preview registered at display"
    );

    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(labels(&app).is_none(), "picking closes the peeker");
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "adoption re-fetches nothing"
    );
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("workspace notes body"),
        "the adopted document took the pane"
    );
    let adopted = documents::OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .find(|(_, entity)| entity.name() == "notes.md")
        .expect("the adopted document is listed");
    assert_eq!(
        adopted.1.location().cloned(),
        Some(doc_location("notes.md"))
    );
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs + 1,
        "the temp became THE document; the displaced scratch is spared"
    );

    settle(&mut app, &arriving, &runner);
    let adopted = documents::OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .find(|(_, entity)| entity.name() == "notes.md")
        .expect("still listed");
    assert_eq!(
        adopted.1.watch(),
        Some(documents::watch::Subscription(7)),
        "the adopted document watches its file"
    );
}

#[test]
fn temps_clean_up_and_early_picks_fall_back() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let (mut app, arriving, runner) = boot(&fetches);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(app.perform_batch(vec![himark::app::AppCommand::Windowed(
        app.sole_window(),
        Arc::new(AddFolder)
    )]));
    let baseline_docs = documents::OpenDocuments::list(app.store(), app.sole_documents()).len();

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "no"));
    settle(&mut app, &arriving, &runner);
    settle(&mut app, &arriving, &runner);
    assert!(
        preview_height(&app, app.store()).is_some(),
        "the fetched temp previews while the peeker is up"
    );
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs + 1,
        "the fetched preview registered at display"
    );
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        Default::default()
    ));
    app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs,
        "the close retracted the preview editor — editorless, the document left whole"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "no"));

    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    settle(&mut app, &arriving, &runner);
    app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("fallback body"),
        "the early pick opened through the standard flow"
    );
}

#[test]
fn outside_dismissal_releases_glanced_documents() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let (mut app, arriving, runner) = boot(&fetches);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(app.perform_batch(vec![himark::app::AppCommand::Windowed(
        app.sole_window(),
        Arc::new(AddFolder)
    )]));
    let baseline_docs = documents::OpenDocuments::list(app.store(), app.sole_documents()).len();

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "no"));
    settle(&mut app, &arriving, &runner);
    settle(&mut app, &arriving, &runner);
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs + 1,
        "the glanced preview registered at display"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(labels(&app).is_none(), "the toggle dismissed the peeker");
    assert_eq!(
        documents::OpenDocuments::list(app.store(), app.sole_documents()).len(),
        baseline_docs,
        "the glanced document left the registry with the modal"
    );
}
