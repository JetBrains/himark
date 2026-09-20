// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::{mpsc, Arc, Mutex};

use imba::event::{Key, Modifiers};

use super::*;
use crate::{test_document::plain_document, test_driver, AppExt, Application, OpenDocuments};

type Writes = Arc<Mutex<Vec<(ResourceLocation, String)>>>;

struct StoreHandler {
    writes: Writes,
    succeeds: bool,
}

impl crate::EffectHandler<StoreDocumentEffect> for StoreHandler {
    async fn handle(&self, effect: StoreDocumentEffect) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push((effect.location, effect.text));
        self.succeeds
    }
}

fn setup(
    succeeds: bool,
) -> (
    Application,
    crate::BackgroundRunner,
    mpsc::Receiver<crate::AppCommand>,
    Writes,
) {
    let mut app = Application::new(crate::AppFonts::embedded());
    app.add_window();
    app.register_editor_command(Arc::new(SaveDocument::existing_files()));
    let writes = Writes::default();
    app.register_handler::<StoreDocumentEffect>(StoreHandler {
        writes: writes.clone(),
        succeeds,
    });
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            posted.send(command).unwrap();
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).unwrap();
    crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    (app, runner, arriving, writes)
}

fn location() -> ResourceLocation {
    ResourceLocation::new(
        crate::ResourceType::document(),
        crate::Authority::new("local"),
        vec!["notes.md".to_owned()],
    )
}

fn open_file(app: &mut Application) -> crate::DocumentId {
    app.perform_command(crate::AppCommand::Opened(
        app.sole_window(),
        crate::OpenedDocument {
            name: "notes.md".to_owned(),
            document: plain_document("original"),
            location: Some(location()),
            primary: true,
            target: None,
            focus: false,
        },
    ));
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).unwrap();
    crate::Window::draw(app.sole_window(), app, surface.canvas());
    OpenDocuments::by_location(app.store(), &location()).unwrap()
}

fn land(app: &mut Application, arriving: &mpsc::Receiver<crate::AppCommand>) {
    for command in arriving.try_iter() {
        app.perform_batch(vec![command]);
    }
}

#[test]
fn existing_file_save_works_without_a_picker_and_preserves_later_edits() {
    for mods in [
        Modifiers {
            command: true,
            ..Default::default()
        },
        Modifiers {
            control: true,
            ..Default::default()
        },
    ] {
        let (mut app, runner, arriving, writes) = setup(true);
        let id = open_file(&mut app);
        assert!(test_driver::type_text(&mut app, "saved "));
        let revision = OpenDocuments::document_ref(app.store(), id)
            .unwrap()
            .revision();
        assert!(test_driver::key(&mut app, Key::Char('s'), mods));
        runner.run();
        assert_eq!(
            *writes.lock().unwrap(),
            [(location(), "saved original".to_owned())]
        );

        assert!(test_driver::type_text(&mut app, "later "));
        land(&mut app, &arriving);
        let entity = OpenDocuments::entity(app.store(), id).unwrap();
        assert_eq!(entity.saved_revision(), revision);
        assert_ne!(entity.saved_revision(), entity.document().revision());
        assert!(app.perform_registered(app.sole_window(), "file.save"));
        runner.run();
        land(&mut app, &arriving);
        assert_eq!(
            writes.lock().unwrap().last().unwrap().1,
            "saved later original"
        );
        let entity = OpenDocuments::entity(app.store(), id).unwrap();
        assert_eq!(entity.saved_revision(), entity.document().revision());
    }
}

#[test]
fn failed_save_keeps_the_document_modified() {
    let (mut app, runner, arriving, writes) = setup(false);
    let id = open_file(&mut app);
    let saved = OpenDocuments::entity(app.store(), id)
        .unwrap()
        .saved_revision();
    assert!(test_driver::type_text(&mut app, "edited "));
    assert!(app.perform_registered(app.sole_window(), "file.save"));
    runner.run();
    land(&mut app, &arriving);
    assert_eq!(writes.lock().unwrap().len(), 1);
    let entity = OpenDocuments::entity(app.store(), id).unwrap();
    assert_eq!(entity.saved_revision(), saved);
    assert_ne!(entity.saved_revision(), entity.document().revision());
}

#[test]
fn scratch_save_is_unavailable_without_a_picker() {
    let (mut app, runner, arriving, writes) = setup(true);
    assert!(test_driver::type_text(&mut app, "scratch text"));
    assert!(!app.perform_registered(app.sole_window(), "file.save"));
    for mods in [
        Modifiers {
            command: true,
            ..Default::default()
        },
        Modifiers {
            control: true,
            ..Default::default()
        },
    ] {
        assert!(!test_driver::key(&mut app, Key::Char('s'), mods));
    }
    runner.run();
    land(&mut app, &arriving);
    assert!(writes.lock().unwrap().is_empty());
}

#[test]
fn save_all_stores_every_modified_file_and_the_title_drops_its_mark() {
    let (mut app, runner, arriving, writes) = setup(true);
    app.register_command(Arc::new(SaveAll));
    let window = app.sole_window();

    let named = |name: &str| {
        ResourceLocation::new(
            crate::ResourceType::document(),
            crate::Authority::new("local"),
            vec![name.to_owned()],
        )
    };
    let open = |app: &mut Application, name: &str| -> crate::DocumentId {
        app.perform_command(crate::AppCommand::Opened(
            app.sole_window(),
            crate::OpenedDocument {
                name: name.to_owned(),
                document: plain_document("original"),
                location: Some(named(name)),
                primary: true,
                target: None,
                focus: false,
            },
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).unwrap();
        crate::Window::draw(app.sole_window(), app, surface.canvas());
        OpenDocuments::by_location(app.store(), &named(name)).unwrap()
    };
    let title = |app: &Application| {
        crate::Windows::window_ref(app.store(), window)
            .expect("the window")
            .workbench()
            .root
            .focused_pane()
            .title(app.store())
    };

    let first = open(&mut app, "notes.md");
    assert!(test_driver::type_text(&mut app, "one "));
    let second = open(&mut app, "other.md");
    assert_eq!(title(&app), "other.md", "clean files carry no mark");
    assert!(test_driver::type_text(&mut app, "two "));
    assert_eq!(
        title(&app),
        "other.md*",
        "the omnibox marks the unsaved file"
    );

    assert!(app.perform_registered(window, "file.save-all"));
    runner.run();
    land(&mut app, &arriving);

    let mut stored: Vec<(String, String)> = writes
        .lock()
        .unwrap()
        .iter()
        .map(|(location, text)| (location.name().to_owned(), text.clone()))
        .collect();
    stored.sort();
    assert_eq!(
        stored,
        [
            ("notes.md".to_owned(), "one original".to_owned()),
            ("other.md".to_owned(), "two original".to_owned()),
        ],
        "every modified file stored; the scratch stayed home"
    );
    for id in [first, second] {
        let entity = OpenDocuments::entity(app.store(), id).unwrap();
        assert!(!entity.modified(), "the landing marked the save");
    }
    assert_eq!(title(&app), "other.md", "the mark leaves with the save");
}
