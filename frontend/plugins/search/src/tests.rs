use super::*;
use himark::AppExt;
use himark::{test_document::plain_document, AppFonts, Application};
use std::sync::{mpsc, Arc};

mod workspace {
    use super::*;
    use himark::{Authority, BuildDocumentEffect, FetchDocumentEffect, ResourceType};
    use imba::effect::EffectHandler;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn doc_location(name: &str) -> ResourceLocation {
        ResourceLocation::new(
            ResourceType::document(),
            Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
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
            Some("alpha\nworkspace needle body\nomega\n".to_owned())
        }
    }

    struct StubBuild;

    impl EffectHandler<BuildDocumentEffect> for StubBuild {
        async fn handle(&self, effect: BuildDocumentEffect) -> himark::BuiltDocument {
            let workshop = himark::test_support::test_workshop(himark::Theme::embedded());
            himark::prepare_built(
                &effect.location,
                plain_document(&effect.text),
                effect.prep.as_ref(),
                &workshop.fonts(),
                &workshop.theme(),
            )
        }
    }

    struct StubLsp;

    impl EffectHandler<himark::LspCompletionEffect> for StubLsp {
        async fn handle(&self, _effect: himark::LspCompletionEffect) -> Option<himark::LspAnswer> {
            None
        }
    }

    fn register_test_handlers(app: &mut Application) {
        register_handlers(app);
        app.register_handler::<himark::LspCompletionEffect>(StubLsp);
    }

    struct AddFolder;

    impl himark::DynamicCommand for AddFolder {
        fn id(&self) -> &'static str {
            "test.add-folder"
        }
        fn name(&self) -> String {
            "Add Folder".to_owned()
        }
        fn perform(
            &self,
            _app: &mut Application,
            store: &mut imba::store::Store,
            _window: himark::WindowId,
            _fx: &mut himark::AppFx<'_>,
        ) {
            let id = himark::test_support::seed_session_folders(
                store,
                &[ResourceLocation::new(
                    himark::ResourceType::directory(),
                    Authority::new("test"),
                    vec!["project".to_owned()],
                )],
            );
            let mut discarded = imba::effect::Batch::new();
            himark::switch_session(store, _window, id, &mut discarded.effects());
        }
    }

    struct StubFindMany;
    impl EffectHandler<FindEffect> for StubFindMany {
        async fn handle(&self, _effect: FindEffect) -> Vec<ResourceLocation> {
            (0..MAX_WORKSPACE_DOCS + 2)
                .rev()
                .map(|index| doc_location(&format!("found-{index:03}.md")))
                .collect()
        }
    }

    #[test]
    fn the_contents_tree_is_the_result_set() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFindMany);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(&fetches)));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));

        assert!(app.perform_command(himark::AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: "open.md".to_owned(),
                document: plain_document("open needle here\n"),
                location: Some(doc_location("open.md")),
                primary: true,
                target: None,
            },
        )));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(himark::test_driver::type_text(&mut app, "x"));

        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        let contents_of = |app: &Application| -> Option<Vec<String>> {
            app.plugin_modal()
                .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
                .expect("the search modal is up")
                .contents_probe(app.store())
        };

        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        settle(&mut app);
        let files = contents_of(&app).expect("the contents tree stands");
        assert_eq!(
            files.len(),
            1 + MAX_WORKSPACE_DOCS + 2,
            "the open hit AND the WHOLE find — membership is not the fetch cap: {files:?}"
        );
        assert!(files.contains(&"open.md".to_owned()), "{files:?}");
        assert!(files.contains(&"found-000.md".to_owned()), "{files:?}");
        assert!(
            files.contains(&format!("found-{:03}.md", MAX_WORKSPACE_DOCS + 1)),
            "past the fetch cap the file still belongs: {files:?}"
        );
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            MAX_WORKSPACE_DOCS,
            "the cap bounds fetches, never membership"
        );

        let names = app
            .plugin_modal()
            .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
            .expect("the search modal is up")
            .group_names(app.store());
        let mut expected: Vec<String> = (0..MAX_WORKSPACE_DOCS)
            .map(|index| format!("found-{index:03}.md"))
            .collect();
        expected.push("open.md".to_owned());
        assert_eq!(names, expected, "groups follow the tree's order");

        assert!(himark::test_driver::type_text(&mut app, "zzz"));
        settle(&mut app);
        settle(&mut app);
        let files = contents_of(&app).expect("an open-docs miss keeps the found files");
        assert!(
            !files.contains(&"open.md".to_owned()),
            "the missed open document left the set: {files:?}"
        );
        assert_eq!(files.len(), MAX_WORKSPACE_DOCS + 2, "{files:?}");

        let view = app
            .plugin_modal()
            .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
            .expect("the search modal is up");
        let drawer = himark::PanelView::drawer_view(view, app.store(), app.sole_window());
        assert!(drawer.is_some(), "the drawer hands out the live tree");
    }

    #[test]
    fn rows_arrive_fully_built_under_interleaved_paints() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFindMany);

        struct StubFetchBig(Arc<AtomicUsize>);
        impl EffectHandler<FetchDocumentEffect> for StubFetchBig {
            async fn handle(&self, _effect: FetchDocumentEffect) -> Option<String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                let mut text = "filler line with words\n".repeat(8_000);
                text.push_str("workspace needle body\n");
                text.push_str(&"more filler below\n".repeat(1_000));
                Some(text)
            }
        }
        app.register_handler::<FetchDocumentEffect>(StubFetchBig(Arc::clone(&fetches)));

        struct MarkdownBuild;
        impl EffectHandler<BuildDocumentEffect> for MarkdownBuild {
            async fn handle(&self, effect: BuildDocumentEffect) -> himark::BuiltDocument {
                let workshop = himark::test_support::test_workshop(himark::Theme::embedded());
                let document = himarkdown::document_from_markdown(
                    &effect.text,
                    &workshop.fonts(),
                    &workshop.theme(),
                );
                himark::prepare_built(
                    &effect.location,
                    document,
                    effect.prep.as_ref(),
                    &workshop.fonts(),
                    &workshop.theme(),
                )
            }
        }
        app.register_handler::<BuildDocumentEffect>(MarkdownBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));
        assert!(app.add_document(
            app.sole_window(),
            plain_document("open needle here\n"),
            "open.md".to_owned(),
            true,
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(himark::test_driver::type_text(&mut app, "x"));
        let workspace = himark::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .current_session();
        assert!(app.open_panel(
            app.sole_window(),
            Box::new(SearchView::for_workspace(Some(workspace)))
        ));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle_draw = |app: &mut Application, surface: &mut skia_safe::Surface| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
                let _ = himark::Window::draw(app.sole_window(), app, surface.canvas());
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle_draw(&mut app, &mut surface);
        settle_draw(&mut app, &mut surface);
        settle_draw(&mut app, &mut surface);
        let mut heights = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    heights = search.group_heights(store);
                }
            });
        }
        let stuck = heights.iter().filter(|height| **height <= 30.0).count();
        assert_eq!(
            stuck,
            0,
            "every deferred row healed past the fallback: {} of {} stuck: {heights:?}",
            stuck,
            heights.len(),
        );
    }

    #[test]
    fn the_pin_fetches_the_beyond_cap_tail() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFindMany);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(&fetches)));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));
        assert!(app.perform_command(himark::AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: "open.md".to_owned(),
                document: plain_document("open needle here\n"),
                location: Some(doc_location("open.md")),
                primary: true,
                target: None,
            },
        )));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(himark::test_driver::type_text(&mut app, "x"));
        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        settle(&mut app);
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            MAX_WORKSPACE_DOCS,
            "the modal fetches to the cap"
        );

        assert!(app.perform_batch(vec![himark::AppCommand::Content(
            app.sole_window(),
            himark::WindowCommand::Modal(Box::new(SearchCommand::Pin) as imba::DynCommand),
        )]));
        assert!(app.plugin_modal().is_none(), "the pin dismissed the modal");

        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        settle(&mut app);
        settle(&mut app);
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            MAX_WORKSPACE_DOCS + 2,
            "the pin fetched the beyond-cap tail"
        );
        let mut names = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    names = search.group_names(store);
                }
            });
        }
        let mut expected: Vec<String> = (0..MAX_WORKSPACE_DOCS + 2)
            .map(|index| format!("found-{index:03}.md"))
            .collect();
        expected.push("open.md".to_owned());
        assert_eq!(
            names, expected,
            "every found file has its group, tree order"
        );

        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        settle(&mut app);
        settle(&mut app);
        let mut heights = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    heights = search.group_heights(store);
                }
            });
        }
        let stuck = heights.iter().filter(|height| **height <= 30.0).count();
        assert_eq!(stuck, 0, "pinned rows healed: {heights:?}");
    }

    #[test]
    fn groups_follow_the_trees_dirs_first_order() {
        struct StubFindNested;
        impl EffectHandler<FindEffect> for StubFindNested {
            async fn handle(&self, _effect: FindEffect) -> Vec<ResourceLocation> {
                vec![
                    doc_location("aaa.md"),
                    ResourceLocation::new(
                        ResourceType::document(),
                        Authority::new("test"),
                        vec![
                            "project".to_owned(),
                            "zz-dir".to_owned(),
                            "inner.md".to_owned(),
                        ],
                    ),
                    doc_location("zzz.md"),
                ]
            }
        }
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFindNested);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(&fetches)));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));
        assert!(app.perform_command(himark::AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: "open.md".to_owned(),
                document: plain_document("open needle here\n"),
                location: Some(doc_location("open.md")),
                primary: true,
                target: None,
            },
        )));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(himark::test_driver::type_text(&mut app, "x"));
        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        settle(&mut app);

        let view = app
            .plugin_modal()
            .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
            .expect("the search modal is up");
        let expected = vec![
            "inner.md".to_owned(),
            "aaa.md".to_owned(),
            "open.md".to_owned(),
            "zzz.md".to_owned(),
        ];
        assert_eq!(
            view.group_names(app.store()),
            expected,
            "groups follow the tree"
        );

        let tree_files: Vec<String> = view
            .contents_rows()
            .into_iter()
            .filter(|(_, _, pick)| *pick)
            .map(|(_, label, _)| label)
            .collect();
        assert_eq!(tree_files, expected, "one traversal, two faces");
    }

    #[test]
    fn a_find_that_outruns_the_scan_still_installs_its_groups() {
        struct StubFindNothing;
        impl EffectHandler<FindEffect> for StubFindNothing {
            async fn handle(&self, _effect: FindEffect) -> Vec<ResourceLocation> {
                Vec::new()
            }
        }
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFindNothing);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(&fetches)));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));
        assert!(app.add_document(
            app.sole_window(),
            plain_document("open needle here\n"),
            "open.md".to_owned(),
            true,
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(himark::test_driver::type_text(&mut app, "x"));
        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        assert!(himark::test_driver::type_text(&mut app, "needle"));

        assert!(app.perform_batch(vec![himark::AppCommand::Content(
            app.sole_window(),
            himark::WindowCommand::Modal(Box::new(SearchCommand::FoundLocations {
                serial: 1,
                locations: vec![doc_location("raced.md")],
            })),
        )]));
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            0,
            "no fetch before the scan installs — it would stamp a stale generation"
        );

        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        settle(&mut app);
        settle(&mut app);
        assert_eq!(fetches.load(Ordering::SeqCst), 1, "the raced find fetched");
        let names = app
            .plugin_modal()
            .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
            .expect("the search modal is up")
            .group_names(app.store());
        assert_eq!(
            names,
            vec!["raced.md".to_owned(), "open.md".to_owned()],
            "the raced find's group INSTALLED, in path order"
        );
    }

    #[test]
    fn workspace_hits_install_preview_groups_and_clean_up() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFind);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::clone(&fetches)));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );

        assert!(app.perform_batch(vec![himark::AppCommand::Dynamic(
            app.sole_window(),
            Arc::new(AddFolder)
        )]));

        assert!(app.add_document(
            app.sole_window(),
            plain_document("open needle here\n"),
            "open.md".to_owned(),
            true,
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        assert!(himark::test_driver::type_text(&mut app, "x"));

        let workspace = himark::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .current_session();
        assert!(app.open_panel(
            app.sole_window(),
            Box::new(SearchView::for_workspace(Some(workspace)))
        ));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let baseline_docs = himark::OpenDocuments::list(app.store()).len();

        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };

        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        settle(&mut app);

        let sizes = group_sizes_of(&app);
        assert_eq!(
            sizes.len(),
            2,
            "the open document's group AND the workspace group: {sizes:?}"
        );
        let mut heights = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    heights = search.group_heights(store);
                }
            });
        }
        assert!(
            heights.iter().all(|height| *height > 30.0),
            "deferred rows heal past the fallback height: {heights:?}"
        );
        assert_eq!(fetches.load(Ordering::SeqCst), 1, "one fetch per found doc");
        assert_eq!(
            himark::OpenDocuments::list(app.store()).len(),
            baseline_docs + 1,
            "the fetched document backs the workspace group"
        );
        assert!(
            himark::OpenDocuments::list(app.store())
                .iter()
                .any(|(_, entity)| entity.name() == "notes.md"),
            "found and displayed — it registered"
        );

        assert!(himark::test_driver::type_text(&mut app, "zzz"));
        settle(&mut app);
        settle(&mut app);
        assert_eq!(
            himark::OpenDocuments::list(app.store()).len(),
            baseline_docs,
            "the miss retracted the last editors — the document left whole"
        );
        assert!(
            himark::OpenDocuments::list(app.store())
                .iter()
                .all(|(_, entity)| entity.name() != "notes.md"),
            "and its registry entry left with it"
        );
        assert!(group_sizes_of(&app).is_empty(), "no groups for a miss");
    }

    fn group_sizes_of(app: &Application) -> Vec<usize> {
        let mut sizes = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    sizes = search.group_sizes(store);
                }
            });
        }
        sizes
    }

    #[test]
    fn cmd_enter_opens_the_focused_groups_document_in_full() {
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_test_handlers(&mut app);
        app.register_handler::<FindEffect>(StubFind);
        app.register_handler::<FetchDocumentEffect>(StubFetch(Arc::new(AtomicUsize::new(0))));
        app.register_handler::<BuildDocumentEffect>(StubBuild);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.add_document(
            app.sole_window(),
            plain_document("open needle here\n"),
            "open.md".to_owned(),
            true,
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        assert!(himark::test_driver::type_text(&mut app, "x"));
        let workspace = himark::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .current_session();
        assert!(app.open_panel(
            app.sole_window(),
            Box::new(SearchView::for_workspace(Some(workspace)))
        ));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        settle(&mut app);
        assert!(!group_sizes_of(&app).is_empty(), "results landed");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let _ = himark::test_driver::click(&mut app, 450.0, 200.0, 900.0, 700.0);
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert!(
            himark::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
                .iter()
                .any(|presentable| presentable.id == "workbench.open-in-full"),
            "the focused group offers the open"
        );
        let mut rows = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    rows = search.group_row_editors(store, 0);
                }
            });
        }
        let (row_document, row_editor) = *rows.first().expect("the first group's row");
        {
            let mut document = himark::OpenDocuments::document(app.store(), row_document)
                .expect("the row's document");
            document.set_caret(row_editor, 8);
            document.set_focus(row_editor, himark::EditorFocus::Text);
            let mut store = app.store_mut();
            himark::OpenDocuments::put_document(&mut store, row_document, document);
        }

        assert!(himark::test_driver::key(
            &mut app,
            imba::event::Key::Enter,
            imba::event::Modifiers {
                command: true,
                ..Default::default()
            },
        ));
        settle(&mut app);
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert_eq!(
            app.focused_document_text().as_deref(),
            Some("xopen needle here\n"),
            "the whole document opened in the pane"
        );
        let (document_id, editor_id) = app.focused_editor_id();
        assert_eq!(
            document_id, row_document,
            "the group's document took the pane"
        );
        let caret = himark::OpenDocuments::document_ref(app.store(), document_id)
            .expect("the shown document")
            .caret_byte(editor_id);
        assert_eq!(caret, 8, "the row's caret position carried into the pane");
    }
}

#[test]
fn ime_composition_reaches_the_search_field() {
    let fonts = { AppFonts::embedded() };
    let mut app = Application::new(fonts);
    register_handlers(&mut app);
    let _ = app.add_window();
    let (posted, _arriving) = mpsc::channel();
    let _runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    assert!(app.add_document(
        app.sole_window(),
        plain_document("alpha\nneedle\nomega\n"),
        "one".to_owned(),
        false,
    ));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(
        app.open_panel(app.sole_window(), Box::new(SearchView::new())),
        "search opens"
    );
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let set = app.with_ime_client(app.sole_window(), |client| {
        client.set_marked_text("\u{306B}", (1, 0), None);
    });
    assert!(set.is_some(), "the search field answers the IME event");
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let (marked, range) = app
        .with_ime_client(app.sole_window(), |client| {
            (client.has_marked_text(), client.marked_range())
        })
        .expect("the field is still focused");
    assert!(marked, "the composition is live in the query box");
    assert_eq!(range, Some((0, 1)));

    let rect = app
        .with_ime_client(app.sole_window(), |client| client.first_rect(0, 1))
        .expect("still focused")
        .expect("the candidate window has a caret rect");
    assert!(
        rect.1 >= 0.0 && rect.1 < 700.0,
        "the rect is in window coordinates: {rect:?}"
    );

    let _ = app.with_ime_client(app.sole_window(), |client| client.unmark_text());
    let query = app.with_ime_client(app.sole_window(), |client| client.substring_utf16(0, 8));
    assert_eq!(
        query.flatten().as_deref(),
        Some("\u{306B}"),
        "the committed composition is the query text"
    );
}

#[test]
fn search_finds_occurrences_across_documents_as_bounded_rows() {
    let fonts = { AppFonts::embedded() };
    let mut app = Application::new(fonts);
    register_handlers(&mut app);
    let _ = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    assert!(app.add_document(
        app.sole_window(),
        plain_document("alpha\nthe needle here\nomega\n"),
        "one".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        plain_document("nothing\nneedle again\nneedle twice on lines\n"),
        "two".to_owned(),
        false,
    ));

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(
        app.open_panel(app.sole_window(), Box::new(SearchView::new())),
        "search opens"
    );
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(app.pane_count(), 1, "search replaces the focused panel");

    assert!(himark::test_driver::type_text(&mut app, "needle"));

    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let mut sizes = None;
    {
        let store = app.store();
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                sizes = Some(search.group_sizes(store));
            }
        });
    }
    assert_eq!(
        sizes,
        Some(vec![1, 1]),
        "adjacent matches merge into one contextual occurrence"
    );

    assert!(app.new_scratch(app.sole_window()));
    let mut still_there = false;
    app.for_each_plugin_panel(&mut |_| still_there = true);
    assert!(!still_there);
    assert_eq!(app.pane_count(), 1);
}

#[test]
fn a_new_query_cancels_the_in_flight_scan_and_find() {
    use imba::effect::{Batch, Message};
    let mut store = imba::store::Store::new();
    let workspace = himark::test_support::seed_session_folders(
        &mut store,
        &[ResourceLocation::new(
            himark::ResourceType::directory(),
            himark::Authority::new("test"),
            vec!["project".to_owned()],
        )],
    );
    let mut view = SearchView::for_workspace(Some(workspace));
    let ui = imba::UiCtx::new();

    let type_query = |view: &mut SearchView, store: &mut imba::store::Store, text: &str| {
        let mut batch: Batch<SearchCommand> = Batch::new();
        view.perform(
            store,
            &ui,
            SearchCommand::Input(EditorCommand::InsertText {
                text: text.to_owned(),
            }),
            &mut batch.effects(),
        );
        let mut cancels = Vec::new();
        let mut scans = Vec::new();
        let mut finds = Vec::new();
        for message in batch.drain() {
            match message {
                Message::Launch(token, effect) if effect.is::<SearchEffect>() => scans.push(token),
                Message::Launch(token, effect) if effect.is::<FindEffect>() => finds.push(token),
                Message::Relaunch(previous, token, effect) if effect.is::<SearchEffect>() => {
                    cancels.push(previous);
                    scans.push(token);
                }
                Message::Relaunch(previous, token, effect) if effect.is::<FindEffect>() => {
                    cancels.push(previous);
                    finds.push(token);
                }
                Message::Cancel(token) => cancels.push(token),
                _ => {}
            }
        }
        (cancels, scans, finds)
    };

    let (_, scans, finds) = type_query(&mut view, &mut store, "needle");
    assert_eq!(scans.len(), 1, "one scan holds its lane");
    assert_eq!(finds.len(), 1, "one workspace find holds its lane");

    let (cancels, next_scans, next_finds) = type_query(&mut view, &mut store, "s");
    assert!(
        cancels.contains(&scans[0]),
        "the keystroke cancels the in-flight scan"
    );
    assert!(
        cancels.contains(&finds[0]),
        "and the in-flight workspace find"
    );
    assert_eq!(next_scans.len(), 1);
    assert_eq!(next_finds.len(), 1);
}

mod modal {
    use super::*;

    fn spaced_matches(count: usize) -> String {
        let mut text = String::new();
        for index in 0..count {
            text.push_str(&format!("needle {index}\n"));
            for _ in 0..12 {
                text.push_str("filler line\n");
            }
        }
        text
    }

    fn modal_search_of(app: &Application) -> &SearchView {
        app.plugin_modal()
            .and_then(|modal| modal.as_any().downcast_ref::<SearchView>())
            .expect("the search modal is up")
    }

    fn panel_group_sizes(app: &Application) -> Vec<usize> {
        let mut sizes = Vec::new();
        {
            let store = app.store();
            app.for_each_plugin_panel(&mut |panel| {
                if let Some(search) = panel.as_any().downcast_ref::<SearchView>() {
                    sizes = search.group_sizes(store);
                }
            });
        }
        sizes
    }

    #[test]
    fn the_modal_budget_holds_back_and_pin_installs_the_rest() {
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_handlers(&mut app);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        for name in ["a.md", "b.md", "c.md"] {
            assert!(app.add_document(
                app.sole_window(),
                plain_document(&spaced_matches(60)),
                name.to_owned(),
                false,
            ));
        }
        let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 800)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);

        let sizes = modal_search_of(&app).group_sizes(app.store());
        assert_eq!(
            sizes,
            [50, 50, 0],
            "two shown groups plus the truncation note"
        );

        assert!(app.perform_batch(vec![himark::AppCommand::Content(
            app.sole_window(),
            himark::WindowCommand::Modal(Box::new(SearchCommand::Pin) as imba::DynCommand),
        )]));
        assert!(app.plugin_modal().is_none(), "the pin dismissed the modal");

        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        settle(&mut app);
        assert_eq!(
            panel_group_sizes(&app),
            [50, 50, 50],
            "the pinned panel carries every scanned occurrence, no note"
        );
    }

    #[test]
    fn cmd_enter_opens_the_focused_group_from_the_modal() {
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_handlers(&mut app);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );

        assert!(app.perform_command(himark::AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: "opened.md".to_owned(),
                document: plain_document("xopen needle here\n"),
                location: Some(himark::ResourceLocation::new(
                    himark::ResourceType::document(),
                    himark::Authority::new("test"),
                    vec!["project".to_owned(), "opened.md".to_owned()],
                )),
                primary: true,
                target: None,
            },
        )));
        let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 800)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert_eq!(
            modal_search_of(&app).group_sizes(app.store()),
            [1],
            "the one match grouped"
        );
        assert!(
            !modal_search_of(&app).contents_rows().is_empty(),
            "the contents tree stands — the index-shifting arrangement"
        );

        assert!(app.perform_batch(vec![himark::AppCommand::Content(
            app.sole_window(),
            himark::WindowCommand::Modal(Box::new(SearchCommand::List(
                imba::scroll::ScrollCommand::Content(himark::LocationListCommand::Results(
                    imba::list::ListCommand::Focus(0, None),
                )),
            )) as imba::DynCommand),
        )]));
        assert!(
            himark::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
                .iter()
                .any(|presentable| presentable.id == "workbench.open-in-full"),
            "the focused group offers the open"
        );

        assert!(himark::test_driver::key(
            &mut app,
            imba::event::Key::Enter,
            imba::event::Modifiers {
                command: true,
                ..Default::default()
            },
        ));
        settle(&mut app);
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        assert_eq!(
            app.focused_document_text().as_deref(),
            Some("xopen needle here\n"),
            "the group's whole document opened in the pane"
        );
    }

    #[test]
    fn closing_the_modal_uninstalls_its_editors() {
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        register_handlers(&mut app);
        let _ = app.add_window();
        let (posted, arriving) = mpsc::channel();
        let runner = app.attach_host(
            Arc::new(move |command| {
                let _ = posted.send(command);
            }),
            Arc::new(|| {}),
        );
        assert!(app.add_document(
            app.sole_window(),
            plain_document("one needle line\n"),
            "a.md".to_owned(),
            false,
        ));
        let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 800)).expect("surface");
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

        let editor_count = |app: &Application| -> usize {
            himark::OpenDocuments::list(app.store())
                .iter()
                .map(|(_, entity)| entity.document().editor_ids().count())
                .sum()
        };
        let baseline: usize = editor_count(&app);

        app.register_overlay_surface(overlay_surface());
        assert!(app.register_command(Arc::new(OpenSearch)));
        assert!(app.perform_registered(app.sole_window(), "search.open"));
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        let settle = |app: &mut Application| {
            for _ in 0..4 {
                runner.run();
                while let Ok(command) = arriving.try_recv() {
                    app.perform_batch(vec![command]);
                }
            }
        };
        assert!(himark::test_driver::type_text(&mut app, "needle"));
        settle(&mut app);
        assert!(
            editor_count(&app) > baseline,
            "the query installed occurrence editors"
        );

        assert!(app.perform_batch(vec![himark::AppCommand::Content(
            app.sole_window(),
            himark::WindowCommand::Modal(Box::new(SearchCommand::Close) as imba::DynCommand),
        )]));
        assert!(
            app.plugin_modal().is_none(),
            "the close dismissed the modal"
        );
        assert_eq!(
            editor_count(&app),
            baseline,
            "the close took every occurrence editor along"
        );
    }
}

#[test]
fn a_displaced_search_survives_in_the_lists_family() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    register_handlers(&mut app);
    let _ = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    for name in ["one.md", "two.md"] {
        assert!(app.add_document(
            app.sole_window(),
            plain_document("a needle here\nand a needle there\n"),
            name.to_owned(),
            false
        ));
    }
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(app.open_panel(app.sole_window(), Box::new(SearchView::new())));
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "needle"));
    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }

    let rows = himark::LocationLists::titles(app.store());
    assert_eq!(rows.len(), 1, "one result set: {rows:?}");
    let (id, title) = rows[0].clone();
    assert!(title.contains("needle"), "titled by the query: {title}");
    let groups = |app: &Application| {
        himark::LocationLists::entry_ref(app.store(), id)
            .map(|entry| entry.list.content().group_sizes())
    };
    let installed = groups(&app).expect("the row stands");
    assert_eq!(installed.len(), 2, "both documents grouped: {installed:?}");

    assert!(app.open_panel(
        app.sole_window(),
        Box::new(himark::ListPanel::new(himark::ListId::mint()))
    ));
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(
        groups(&app).as_deref(),
        Some(installed.as_slice()),
        "the row survived displacement, installs intact"
    );

    assert!(app.open_panel(app.sole_window(), Box::new(himark::ListPanel::new(id))));
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(
        groups(&app).as_deref(),
        Some(installed.as_slice()),
        "the remount fronts the same installs"
    );
}
