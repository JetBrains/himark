// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ResourceType;

use super::*;

fn location(kind: ResourceType, path: &[&str]) -> ResourceLocation {
    ResourceLocation::new(
        kind,
        crate::Authority::new("test"),
        path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

fn directory(path: &[&str]) -> ResourceLocation {
    location(ResourceType::directory(), path)
}

fn document(path: &[&str]) -> ResourceLocation {
    location(ResourceType::document(), path)
}

fn workspace_with(store: &mut Store, folders: &[ResourceLocation]) -> crate::SessionId {
    crate::test_support::seed_session_folders(store, folders)
}

#[test]
fn listings_grow_and_fold_the_tree() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.row_count(), 1, "the folder itself is the root row");

    let mut batch = imba::effect::Batch::new();
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut batch.effects(),
    );
    assert_eq!(batch.len(), 1, "expansion fetches");
    let mut again = imba::effect::Batch::new();
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut again.effects(),
    );
    assert!(again.is_empty(), "a mashed triangle asks once");
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(view.row_count(), 3);

    let mut batch = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut batch.effects(),
    );
    assert_eq!(batch.len(), 1, "src expansion fetches");
    view.tree.splice_listing(
        directory(&["project", "src"]),
        Some(vec![
            document(&["project", "src", "lib.rs"]),
            document(&["project", "src", "main.rs"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(view.row_count(), 5, "children spliced under src");

    let mut collapse = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut collapse.effects(),
    );
    assert!(collapse.is_empty(), "collapse is local surgery");
    assert_eq!(view.row_count(), 3, "folded back; nothing cached");

    let mut reexpand = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut reexpand.effects(),
    );
    assert_eq!(reexpand.len(), 1, "re-expansion re-fetches");
}

#[test]
fn a_document_click_requests_the_open() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![document(&["project", "README.md"])]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    let mut click = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut click.effects(),
    );
    assert!(click.is_empty());
    let Some(ModalRequest::OpenLocations(locations)) = view.take_request() else {
        panic!("the click filed an open request");
    };
    assert_eq!(locations, vec![document(&["project", "README.md"])]);
    assert!(view.take_request().is_none(), "drained once");
}

#[test]
fn expansion_survives_reopen_and_new_folders_join() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.perform(
        &mut store,
        &ui,
        TreeCommand::Listed {
            parent: directory(&["project"]),
            entries: Some(vec![
                directory(&["project", "src"]),
                document(&["project", "README.md"]),
            ]),
        },
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.row_count(), 3);
    drop(view);

    crate::test_support::add_session_folders(&mut store, &workspace, &[directory(&["other"])]);
    let reopened = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        reopened.row_count(),
        4,
        "project stayed expanded; other joined as a root"
    );

    let stashed = store.take::<SessionTree>().unwrap_or_default();
    let second =
        crate::test_support::seed_session_folders(&mut store, &[directory(&["elsewhere"])]);
    let other_tree = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        second,
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(other_tree.row_count(), 1, "only elsewhere; nothing leaked");
    store.put(stashed);
    let first_again = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        first_again.row_count(),
        4,
        "the first workspace's tree intact"
    );
}

#[test]
fn dismissal_files_the_close() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(view.take_request().is_none());
    view.perform(
        &mut store,
        &ui,
        TreeCommand::Dismiss,
        &mut imba::effect::Batch::new().effects(),
    );
    let Some(ModalRequest::Close) = view.take_request() else {
        panic!("escape asks the app to close");
    };
}

#[test]
fn a_stale_listing_drops() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![directory(&["project", "src"])]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    view.activate(
        1,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project", "src"]),
        Some(vec![directory(&["project", "src", "deep"])]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(view.row_count(), 3);

    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project", "src", "deep"]),
        Some(vec![document(&["project", "src", "deep", "a.md"])]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(view.row_count(), 1, "the stale landing changed nothing");
}

#[test]
fn opened_roots_join_the_workspace_once() {
    let mut store = Store::new();
    let root = directory(&["project"]);
    let session = crate::test_support::seed_session_folders(
        &mut store,
        &[root.clone(), root.clone(), directory(&["other"])],
    );
    let folders = crate::higent::session_folders(&store, &session);
    let unique: std::collections::HashSet<_> = folders
        .iter()
        .map(|folder| folder.path().to_vec())
        .collect();
    assert_eq!(unique.len(), 2);
    assert_eq!(folders[0].path(), root.path());
}

#[test]
fn expanded_folders_watch_and_events_relist() {
    let mut store = Store::new();
    crate::Watching::install(&mut store);
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    let ui = imba::UiCtx::dont_use_too_slow();

    let mut batch = imba::effect::Batch::new();
    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Listed {
            parent: directory(&["project"]),
            entries: Some(vec![
                directory(&["project", "src"]),
                document(&["project", "README.md"]),
            ]),
        },
        &mut batch.effects(),
    );
    let launches = crate::test_support::surviving_launches(batch);
    assert!(
        launches
            .iter()
            .any(|effect| effect.get::<crate::SubscribeEffect>().is_some()),
        "an expanded folder asks for its watch"
    );

    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Watched {
            parent: directory(&["project"]),
            subscription: Some(crate::Subscription(9)),
        },
        &mut imba::effect::Batch::new().effects(),
    );

    let mut batch = imba::effect::Batch::new();
    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Changed(vec![crate::Subscription(9)]),
        &mut batch.effects(),
    );
    let launches = crate::test_support::surviving_launches(batch);
    assert!(
        launches
            .iter()
            .any(|effect| effect.get::<crate::ListDirectoryEffect>().is_some()),
        "a change re-lists the watched folder"
    );

    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Listed {
            parent: directory(&["project"]),
            entries: Some(vec![document(&["project", "README.md"])]),
        },
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.row_count(), 2, "the re-list replaced the children");

    let mut batch = imba::effect::Batch::new();
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut batch.effects(),
    );
    let launches = crate::test_support::surviving_launches(batch);
    assert!(
        launches
            .iter()
            .any(|effect| effect.get::<crate::UnsubscribeEffect>().is_some()),
        "the fold unsubscribes"
    );
    assert!(view.tree.watches.is_empty());
}

#[test]
fn cursor_walks_and_enter_opens() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    let mut drive = |view: &mut SessionTreeView, command| {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };

    drive(&mut view, TreeCommand::Select(1));
    drive(&mut view, TreeCommand::Select(1));
    assert_eq!(view.selected_name().as_deref(), Some("README.md"));
    drive(&mut view, TreeCommand::Pick);
    let Some(ModalRequest::OpenLocations(locations)) = view.take_request() else {
        panic!("Enter opens the selected document");
    };
    assert_eq!(locations, vec![document(&["project", "README.md"])]);

    drive(&mut view, TreeCommand::Fold(false));
    assert_eq!(view.selected_name().as_deref(), Some("project"));
    drive(&mut view, TreeCommand::Fold(false));
    assert_eq!(view.row_count(), 1, "the fold took the subtree");
}

#[test]
fn a_relist_keeps_expanded_subtrees() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace,
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    view.tree.splice_listing(
        directory(&["project", "src"]),
        Some(vec![document(&["project", "src", "lib.rs"])]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(view.row_count(), 4, "root, src/, lib.rs, README");
    view.tree
        .list
        .inner_mut()
        .content_mut()
        .select_only(document(&["project", "src", "lib.rs"]));

    let removed = view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
            document(&["project", "fresh.txt"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert!(removed.is_empty(), "nothing left");
    assert_eq!(view.row_count(), 5, "the fresh file joined");
    assert!(
        view.tree.is_listed(&directory(&["project", "src"])),
        "src stayed expanded"
    );
    assert!(
        view.tree
            .is_visible(&document(&["project", "src", "lib.rs"])),
        "the subtree rows stand"
    );
    assert_eq!(
        view.tree.list.inner().content().cursor(),
        Some(&document(&["project", "src", "lib.rs"])),
        "selection rode the merge"
    );

    let generation = view.tree.list.inner().content().generation();
    let removed = view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
            document(&["project", "fresh.txt"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert!(removed.is_empty());
    assert_eq!(
        view.tree.list.inner().content().generation(),
        generation,
        "an echo touches nothing"
    );

    let removed = view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            document(&["project", "README.md"]),
            document(&["project", "fresh.txt"]),
        ]),
        &store,
        &imba::UiCtx::dont_use_too_slow(),
    );
    assert_eq!(removed, vec![directory(&["project", "src"])]);
    assert_eq!(view.row_count(), 3, "root, README, fresh");
    assert!(!view
        .tree
        .is_visible(&document(&["project", "src", "lib.rs"])));
}

#[test]
fn a_theme_switch_re_resolves_the_selection_style() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = SessionTreeView::open(
        &mut store,
        &imba::UiCtx::dont_use_too_slow(),
        workspace,
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    let dark = crate::env::Themes::of(&store).ui().tree.highlight.0;
    assert_eq!(
        view.tree
            .list
            .inner()
            .content()
            .selection_style()
            .map(|style| style.fill),
        Some(dark),
        "born with the current theme's wash"
    );

    crate::env::Themes::set(&mut store, ::editor::theme::Theme::light());
    let light = crate::env::Themes::of(&store).ui().tree.highlight.0;
    assert_ne!(dark, light, "the themes disagree, or this test is vacuous");

    let commands = {
        let arena = imba::arena::Arena::default();
        let ui = imba::UiCtx::dont_use_too_slow();
        let size = skia_safe::Size::new(400.0, 600.0);
        let widget = imba::Layout::layout(
            imba::View::display(&view, &arena, &store, &ui),
            &arena,
            imba::constraints::Constraints::tight(size),
        );
        let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(400.0, 600.0));
        match imba::Widget::handle_event(
            &widget,
            &arena,
            &imba::event::Event::ThemeChanged,
            skia_safe::Rect::from_wh(400.0, 600.0),
        ) {
            imba::event::EventResult::Command(command) => vec![command],
            imba::event::EventResult::Commands(commands) => commands,
            _ => panic!("the broadcast answers the retheme"),
        }
    };
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, TreeCommand::Retheme)),
        "the panel minted the retheme command"
    );
    for command in commands {
        view.perform(
            &mut store,
            &imba::UiCtx::dont_use_too_slow(),
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert_eq!(
        view.tree
            .list
            .inner()
            .content()
            .selection_style()
            .map(|style| style.fill),
        Some(light),
        "the wash follows the switched theme"
    );
}
