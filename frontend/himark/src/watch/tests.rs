use std::sync::{Arc, Mutex};

use imba::store::Store;

use super::*;
use crate::test_document::plain_document;
use crate::{AppFonts, Application, Document, OpenDocuments, OpenedDocument, ResourceLocation};

fn located(name: &str) -> ResourceLocation {
    ResourceLocation::new(
        crate::ResourceType::document(),
        crate::Authority::new("test"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

fn text_of(document: &Document) -> String {
    let mut view = document.text().view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

fn text_of_text(text: &crate::Text) -> String {
    let mut view = text.view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

fn landed_rebase(batch: imba::effect::Batch<crate::AppCommand>) -> RefetchRebase {
    use imba::effect::EffectHandler;
    let mut launches = crate::test_support::surviving_launches(batch);
    assert_eq!(launches.len(), 1, "one merge launch");
    let effect = launches
        .pop()
        .expect("launch")
        .into_payload()
        .split()
        .0
        .downcast::<RefetchDiffEffect>()
        .expect("the merge effect");
    imba::effect::block_on(Box::pin(async move {
        RefetchDiffHandler.handle(*effect).await
    }))
}

fn registered(store: &mut Store, source: &str) -> crate::DocumentId {
    let document = plain_document(source);
    let saved = document.revision();
    OpenDocuments::register(
        store,
        document,
        Some(located("a.md")),
        "a.md".to_owned(),
        saved,
    )
}

#[test]
fn a_clean_document_follows_the_disk() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\nbeta\n");
    let mut batch = imba::effect::Batch::new();
    let serial = OpenDocuments::stamp_refetch(&mut store, id);
    apply_refetched(
        &mut store,
        id,
        serial,
        Some("alpha\nCHANGED\n".to_owned()),
        &mut batch.effects(),
    );
    assert_eq!(
        text_of(&OpenDocuments::document_ref(&store, id).expect("the document")),
        "alpha\nbeta\n",
        "the landing itself edits nothing — the diff is the worker's"
    );

    let base_revision = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    let rebase = landed_rebase(batch);
    assert!(rebase.clean, "ours moved nothing — the plain reload");
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        base_revision,
        serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );
    let document = &OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(text_of(document), "alpha\nCHANGED\n");
    let entity = OpenDocuments::entity(&store, id).expect("registered");
    assert_eq!(
        entity.saved_revision(),
        document.revision(),
        "the disk and the document agree — no phantom dirty flag"
    );
    assert_eq!(
        text_of_text(entity.baseline()),
        "alpha\nCHANGED\n",
        "the baseline follows the disk"
    );
}

#[test]
fn a_stale_diff_landing_discards_itself() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    let stale_revision = document.revision();
    let operation = ::editor::diff::diff(
        document.text(),
        &crate::Text::from_string_exact("external\n"),
    );

    let mut document = OpenDocuments::document(&store, id).expect("the document");
    let editor = document.add_editor(
        600.0,
        None,
        ::editor::EditorBuild::Complete,
        &[],
        &::editor::embedded_fonts::source()(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_caret(editor, 0);
    document.insert(
        editor,
        "typed ",
        &::editor::embedded_fonts::source()(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    OpenDocuments::put_document(&mut store, id, document);

    OpenDocuments::edit_external(
        &mut store,
        id,
        stale_revision,
        &operation,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        text_of(&OpenDocuments::document_ref(&store, id).expect("the document")),
        "typed alpha\n",
        "the stale operation discards; ours wins"
    );
}

#[test]
fn an_absorbed_external_edit_kicks_the_reparse_lane() {
    let mut store = Store::new();
    let mut document = plain_document("alpha\nbeta\n");
    document.install_syntax(
        ::editor::Syntax {
            language: "test".to_owned(),
            tree: None,
            markup: crate::Markup::new(),
            folds: Default::default(),
            outline: Default::default(),
        },
        &[],
    );
    let saved = document.revision();
    let id = OpenDocuments::register(
        &mut store,
        document,
        Some(located("a.md")),
        "a.md".to_owned(),
        saved,
    );
    store.put(::editor::env::Parsers(std::sync::Arc::new(
        ::editor::SyntaxLanguages::new(),
    )));

    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    let base_revision = document.revision();
    let operation = ::editor::diff::diff(
        document.text(),
        &crate::Text::from_string_exact("alpha\nCHANGED\n"),
    );
    let mut batch = imba::effect::Batch::new();
    OpenDocuments::edit_external(
        &mut store,
        id,
        base_revision,
        &operation,
        &mut batch.effects(),
    );
    let launches = crate::test_support::surviving_launches(batch);
    assert!(
        launches
            .iter()
            .any(|effect| effect.is::<::editor::ReparseEffect>()),
        "the absorb launches the document's reparse"
    );
}

fn typed(store: &mut Store, id: crate::DocumentId, at: u32, text: &str) {
    let mut document = OpenDocuments::document(store, id).expect("the document");
    document.edit(
        &operation::Operation::insert_at(at, text),
        &::editor::embedded_fonts::source()(),
        &::editor::theme::Theme::embedded(),
        &mut imba::effect::Batch::new().effects(),
    );
    OpenDocuments::put_document(store, id, document);
}

#[test]
fn a_dirty_document_merges_the_external_change() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\nbeta\n");
    typed(&mut store, id, 0, "MINE ");

    let mut batch = imba::effect::Batch::new();
    let serial = OpenDocuments::stamp_refetch(&mut store, id);
    apply_refetched(
        &mut store,
        id,
        serial,
        Some("alpha\nEXTERNAL\n".to_owned()),
        &mut batch.effects(),
    );
    let base_revision = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    let rebase = landed_rebase(batch);
    assert!(!rebase.clean, "ours moved — this is a merge, not a reload");
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        base_revision,
        serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );

    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(
        text_of(document),
        "MINE alpha\nEXTERNAL\n",
        "the typing survived AND the disk's change landed"
    );
    let entity = OpenDocuments::entity(&store, id).expect("registered");
    assert_ne!(
        document.revision(),
        entity.saved_revision(),
        "memory holds more than disk — the merge stays dirty"
    );
    assert_eq!(
        text_of_text(entity.baseline()),
        "alpha\nEXTERNAL\n",
        "the baseline is the disk's text, not the merge"
    );
}

#[test]
fn a_dirty_save_echo_keeps_the_typing() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    typed(&mut store, id, 0, "typed ");
    let before = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();

    let mut batch = imba::effect::Batch::new();
    let serial = OpenDocuments::stamp_refetch(&mut store, id);
    apply_refetched(
        &mut store,
        id,
        serial,
        Some("alpha\n".to_owned()),
        &mut batch.effects(),
    );
    let rebase = landed_rebase(batch);
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        before,
        serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );

    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(text_of(document), "typed alpha\n", "the typing stands");
    assert_eq!(
        document.revision(),
        before,
        "an echo does not even mint a revision"
    );
}

#[test]
fn typing_racing_the_merge_discards_the_landing() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    let mut batch = imba::effect::Batch::new();
    let serial = OpenDocuments::stamp_refetch(&mut store, id);
    apply_refetched(
        &mut store,
        id,
        serial,
        Some("external\n".to_owned()),
        &mut batch.effects(),
    );
    let base_revision = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    let rebase = landed_rebase(batch);

    typed(&mut store, id, 0, "raced ");
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        base_revision,
        serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(text_of(document), "raced alpha\n", "the stale merge dropped");
    let entity = OpenDocuments::entity(&store, id).expect("registered");
    assert_eq!(
        text_of_text(entity.baseline()),
        "alpha\n",
        "a discarded landing stamps nothing"
    );
}

#[test]
fn the_saves_own_echo_is_a_no_op() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    let before = document.revision();
    let operation =
        ::editor::diff::diff(document.text(), &crate::Text::from_string_exact("alpha\n"));
    OpenDocuments::edit_external(
        &mut store,
        id,
        before,
        &operation,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        OpenDocuments::document_ref(&store, id)
            .expect("the document")
            .revision(),
        before,
        "an identical fetch does not even mint a revision"
    );
}

#[test]
fn a_stale_fetch_landing_never_reverts_the_fresh_reload() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");

    let stale_serial = OpenDocuments::stamp_refetch(&mut store, id);
    let fresh_serial = OpenDocuments::stamp_refetch(&mut store, id);

    let mut batch = imba::effect::Batch::new();
    apply_refetched(
        &mut store,
        id,
        fresh_serial,
        Some("NEW\n".to_owned()),
        &mut batch.effects(),
    );
    let base_revision = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    let rebase = landed_rebase(batch);
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        base_revision,
        fresh_serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        text_of(&OpenDocuments::document_ref(&store, id).expect("the document")),
        "NEW\n"
    );

    let mut batch = imba::effect::Batch::new();
    apply_refetched(
        &mut store,
        id,
        stale_serial,
        Some("alpha\n".to_owned()),
        &mut batch.effects(),
    );
    assert!(
        crate::test_support::surviving_launches(batch).is_empty(),
        "a superseded fetch launches nothing"
    );
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(text_of(document), "NEW\n", "the fresh reload stands");
    let entity = OpenDocuments::entity(&store, id).expect("registered");
    assert_eq!(
        text_of_text(entity.baseline()),
        "NEW\n",
        "the baseline stands too"
    );
}

#[test]
fn a_stale_diff_landing_drops_by_serial() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    let stale_serial = OpenDocuments::stamp_refetch(&mut store, id);
    let mut batch = imba::effect::Batch::new();
    apply_refetched(
        &mut store,
        id,
        stale_serial,
        Some("OLD\n".to_owned()),
        &mut batch.effects(),
    );
    let base_revision = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    let rebase = landed_rebase(batch);

    let _fresh_serial = OpenDocuments::stamp_refetch(&mut store, id);
    OpenDocuments::absorb_refetched(
        &mut store,
        id,
        base_revision,
        stale_serial,
        &rebase.operation,
        rebase.fetched,
        rebase.clean,
        &mut imba::effect::Batch::new().effects(),
    );
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(
        text_of(document),
        "alpha\n",
        "the stale diff dropped; the newer launch's landing decides"
    );
    let entity = OpenDocuments::entity(&store, id).expect("registered");
    assert_eq!(
        text_of_text(entity.baseline()),
        "alpha\n",
        "a dropped landing stamps nothing"
    );
}

#[test]
fn a_missing_fetch_keeps_ours() {
    let mut store = Store::new();
    let id = registered(&mut store, "alpha\n");
    let mut batch = imba::effect::Batch::new();
    let serial = OpenDocuments::stamp_refetch(&mut store, id);
    apply_refetched(&mut store, id, serial, None, &mut batch.effects());
    assert_eq!(
        text_of(&OpenDocuments::document_ref(&store, id).expect("the document")),
        "alpha\n"
    );
}

#[test]
fn opens_watch_and_events_refetch() {
    struct StubWatch;
    impl imba::effect::EffectHandler<SubscribeEffect> for StubWatch {
        async fn handle(&self, _effect: SubscribeEffect) -> Option<Subscription> {
            Some(Subscription(7))
        }
    }
    struct StubFetch(Arc<Mutex<String>>);
    impl imba::effect::EffectHandler<crate::FetchDocumentEffect> for StubFetch {
        async fn handle(&self, _effect: crate::FetchDocumentEffect) -> Option<String> {
            Some(self.0.lock().expect("disk").clone())
        }
    }

    let disk = Arc::new(Mutex::new("alpha\n".to_owned()));
    let mut app = Application::new(AppFonts::embedded());
    app.register_handler::<SubscribeEffect>(StubWatch);
    app.register_handler::<crate::FetchDocumentEffect>(StubFetch(Arc::clone(&disk)));
    app.observe_file_changes();
    let window = app.add_window();
    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let settle = |app: &mut Application| {
        for _ in 0..4 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(crate::AppExt::perform_command(
        &mut app,
        crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "a.md".to_owned(),
                document: plain_document("alpha\n"),
                location: Some(located("a.md")),
                primary: true,
                target: None,
            },
        )
    ));
    settle(&mut app);

    let entity = OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, entity)| entity.name() == "a.md")
        .expect("the located open");
    assert_eq!(
        entity.1.watch(),
        Some(Subscription(7)),
        "the open subscribed"
    );

    *disk.lock().expect("disk") = "alpha\nexternal\n".to_owned();
    assert!(crate::AppExt::perform_command(
        &mut app,
        crate::AppCommand::FileChanged(Subscription(7))
    ));
    settle(&mut app);
    let document = OpenDocuments::document_ref(app.store(), entity.0).expect("the document");
    assert_eq!(text_of(document), "alpha\nexternal\n");
}

#[test]
fn the_palette_reload_follows_the_disk() {
    struct StubFetch(Arc<Mutex<String>>);
    impl imba::effect::EffectHandler<crate::FetchDocumentEffect> for StubFetch {
        async fn handle(&self, _effect: crate::FetchDocumentEffect) -> Option<String> {
            Some(self.0.lock().expect("disk").clone())
        }
    }

    let disk = Arc::new(Mutex::new("alpha\n".to_owned()));
    let mut app = Application::new(AppFonts::embedded());
    app.register_handler::<crate::FetchDocumentEffect>(StubFetch(Arc::clone(&disk)));
    let window = app.add_window();
    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let settle = |app: &mut Application| {
        for _ in 0..4 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(crate::AppExt::perform_command(
        &mut app,
        crate::AppCommand::Opened(
            window,
            OpenedDocument {
                name: "a.md".to_owned(),
                document: plain_document("alpha\n"),
                location: Some(located("a.md")),
                primary: true,
                target: None,
            },
        )
    ));
    settle(&mut app);

    *disk.lock().expect("disk") = "alpha\nRELOADED\n".to_owned();
    assert!(crate::AppExt::perform_command(
        &mut app,
        crate::AppCommand::Dynamic(window, Arc::new(ReloadDocument)),
    ));
    settle(&mut app);

    let entity = OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, entity)| entity.name() == "a.md")
        .expect("the open");
    assert_eq!(
        text_of(entity.1.document()),
        "alpha\nRELOADED\n",
        "the manual reload followed the disk"
    );
    assert_eq!(
        entity.1.document().revision(),
        entity.1.saved_revision(),
        "a clean reload counts as saved"
    );
}

#[test]
fn a_shared_edit_is_not_a_reload() {
    let mut store = Store::new();
    let document = plain_document("alpha\n");
    let saved = document.revision();
    let id = OpenDocuments::register(
        &mut store,
        document,
        Some(located("a.md")),
        "a.md".to_owned(),
        saved,
    );

    let mut document = OpenDocuments::document(&store, id).expect("the document");
    let editor = document.add_editor(
        600.0,
        None,
        ::editor::EditorBuild::Complete,
        &[],
        &::editor::embedded_fonts::source()(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_caret(editor, 0);
    document.insert(
        editor,
        "typed ",
        &::editor::embedded_fonts::source()(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    OpenDocuments::put_document(&mut store, id, document);
    let dirty_at = OpenDocuments::document_ref(&store, id)
        .expect("the document")
        .revision();
    assert!(dirty_at > saved, "typing dirtied it");

    let identity = ::editor::EditIdentity::mint();
    let peer = ::editor::diff::diff(
        OpenDocuments::document_ref(&store, id).expect("the document").text(),
        &crate::Text::from_string_exact("typed alpha\npeer\n"),
    );
    let applied = OpenDocuments::edit_shared(
        &mut store,
        id,
        identity,
        dirty_at,
        &peer,
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(applied);
    let document = OpenDocuments::document_ref(&store, id).expect("the document");
    assert_eq!(text_of(&document), "typed alpha\npeer\n");
    assert_eq!(
        document.log().head(),
        Some(identity),
        "recorded under the peer's name for it"
    );
    assert_eq!(
        OpenDocuments::entity(&store, id).expect("the entity").saved_revision(),
        saved,
        "a peer's edit is not a save: the document is as dirty as it was"
    );

    assert!(!OpenDocuments::edit_shared(
        &mut store,
        id,
        ::editor::EditIdentity::mint(),
        dirty_at,
        &peer,
        &mut imba::effect::Batch::new().effects(),
    ));
}
