// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Mutex;

use editor::location::ResourceType;

use super::*;

/// The session stand-in: the folder set the injected mirror answers,
/// and the trees id the reopen tests share.
#[derive(Clone)]
struct Workspace {
    trees: imba::store::Id<SessionTree>,
    folders: Arc<Mutex<Vec<ResourceLocation>>>,
}

fn open_view(
    store: &mut Store,
    workspace: Workspace,
    reveal: Option<ResourceLocation>,
    fx: &mut imba::effect::Effects<'_, TreeCommand>,
) -> SessionTreeView {
    let folders = workspace.folders.lock().unwrap().clone();
    let mirror = workspace.folders.clone();
    SessionTreeView::open(
        store,
        ::editor::test_document::test_ui(),
        workspace.trees,
        &folders,
        Arc::new(move |_store: &Store| mirror.lock().unwrap().clone()),
        Arc::new(|_store: &Store, _target| None),
        reveal,
        fx,
    )
}

fn location(kind: ResourceType, path: &[&str]) -> ResourceLocation {
    ResourceLocation::new(
        kind,
        editor::location::Authority::new("test"),
        path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

fn directory(path: &[&str]) -> ResourceLocation {
    location(ResourceType::directory(), path)
}

fn document(path: &[&str]) -> ResourceLocation {
    location(ResourceType::document(), path)
}

fn workspace_with(_store: &mut Store, folders: &[ResourceLocation]) -> Workspace {
    Workspace {
        trees: imba::store::Id::mint(),
        folders: Arc::new(Mutex::new(folders.to_vec())),
    }
}

#[test]
fn listings_grow_and_fold_the_tree() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.row_count(), 1, "the folder itself is the root row");

    let mut batch = imba::effect::Batch::new();
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
        &mut batch.effects(),
    );
    assert_eq!(batch.len(), 1, "expansion fetches");
    let mut again = imba::effect::Batch::new();
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
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
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 3);

    let mut batch = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        ::editor::test_document::test_ui(),
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
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 5, "children spliced under src");

    let mut collapse = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        ::editor::test_document::test_ui(),
        &mut collapse.effects(),
    );
    assert!(collapse.is_empty(), "collapse is local surgery");
    assert_eq!(view.row_count(), 3, "folded back; nothing cached");

    let mut reexpand = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        ::editor::test_document::test_ui(),
        &mut reexpand.effects(),
    );
    assert_eq!(reexpand.len(), 1, "re-expansion re-fetches");
}

#[test]
fn a_document_click_requests_the_open() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![document(&["project", "README.md"])]),
        &store,
        ::editor::test_document::test_ui(),
    );
    let mut click = imba::effect::Batch::new();
    view.activate(
        1,
        &store,
        ::editor::test_document::test_ui(),
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
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
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

    workspace
        .folders
        .lock()
        .unwrap()
        .push(directory(&["other"]));
    let reopened = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        reopened.row_count(),
        4,
        "project stayed expanded; other joined as a root"
    );

    let second = workspace_with(&mut store, &[directory(&["elsewhere"])]);
    let other_tree = open_view(
        &mut store,
        second,
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(other_tree.row_count(), 1, "only elsewhere; nothing leaked");
    let first_again = open_view(
        &mut store,
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
fn folders_added_mid_session_join_on_paint() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.row_count(), 1);

    let paint = |view: &SessionTreeView, store: &Store| -> Vec<TreeCommand> {
        let arena = imba::arena::Arena::default();
        let ui = ::editor::test_document::test_ui();
        let size = skia_safe::Size::new(400.0, 600.0);
        let widget = imba::layout::Layout::layout(
            imba::View::display(view, &arena, store, &ui),
            &arena,
            imba::constraints::Constraints::tight(size),
        );
        let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(400.0, 600.0));
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 600)).expect("a surface");
        match imba::Widget::handle_event(
            &widget,
            &arena,
            &imba::event::Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
            skia_safe::Rect::from_wh(400.0, 600.0),
        ) {
            imba::event::EventResult::Command(command) => vec![command],
            imba::event::EventResult::Commands(commands) => commands,
            _ => Vec::new(),
        }
    };

    assert!(
        paint(&view, &store).is_empty(),
        "nothing to sync while the roots match"
    );

    workspace
        .folders
        .lock()
        .unwrap()
        .push(directory(&["other"]));
    let commands = paint(&view, &store);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, TreeCommand::SyncRoots { .. })),
        "the paint gate minted the sync"
    );
    for command in commands {
        view.perform(
            &mut store,
            ::editor::test_document::test_ui(),
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert_eq!(view.row_count(), 2, "the added folder joined as a root");

    assert!(
        paint(&view, &store).is_empty(),
        "the gate closed once the root stands"
    );
}

fn context_press(index: usize) -> TreeCommand {
    TreeCommand::Rows(hikit::ListKeyCommand::Inner(
        imba::scroll::ScrollCommand::Content(imba::list::ListCommand::Focus(
            index,
            Some(Box::new(imba::list::ListCommand::Child(
                index,
                hikit::TreeItemCommand::Inner(hikit::TreeLabelCommand::Context),
            ))),
        )),
    ))
}

fn menu_activate(index: usize) -> TreeCommand {
    TreeCommand::Menu(hikit::menu::MenuCommand::Rows(Box::new(
        hikit::ListKeyCommand::Inner(imba::scroll::ScrollCommand::Content(
            imba::list::ListCommand::Activate(index, imba::list::ActivateTrigger::Enter),
        )),
    )))
}

#[test]
fn a_context_press_menus_and_rename_commits_a_move() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![document(&["project", "README.md"])]),
        &store,
        ::editor::test_document::test_ui(),
    );

    view.perform(
        &mut store,
        &ui,
        context_press(1),
        &mut imba::effect::Batch::new().effects(),
    );
    let menu = view.menu.as_ref().expect("the press opened the menu");
    assert_eq!(menu.target, document(&["project", "README.md"]));

    // A file's items: Rename first, then Delete.
    view.perform(
        &mut store,
        &ui,
        menu_activate(0),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(view.menu.is_none(), "the pick closed the menu");
    assert!(view.edit.is_some(), "the pick started the rename");

    view.edit.as_mut().expect("editing").input =
        seeded_input(&store, ::editor::test_document::test_ui(), "CHANGED.md");
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        TreeCommand::CommitEdit,
        &mut batch.effects(),
    );
    assert!(view.edit.is_none(), "the commit ended the edit");
    let launches = batch.surviving_launches();
    let moved = launches
        .iter()
        .find_map(|effect| effect.get::<documents::MoveResourceEffect>())
        .expect("the commit launched the move");
    assert_eq!(moved.from, document(&["project", "README.md"]));
    assert_eq!(moved.to, document(&["project", "CHANGED.md"]));
}

#[test]
fn new_file_rides_a_placeholder_row_and_creates() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );

    view.perform(
        &mut store,
        &ui,
        context_press(0),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(view.menu.is_some());
    // A root directory's items: New File first, then Remove.
    view.perform(
        &mut store,
        &ui,
        menu_activate(0),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(view.edit.is_some(), "the pick started the create");
    assert_eq!(view.row_count(), 2, "the placeholder row joined");

    view.edit.as_mut().expect("editing").input =
        seeded_input(&store, ::editor::test_document::test_ui(), "new.txt");
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        TreeCommand::CommitEdit,
        &mut batch.effects(),
    );
    assert_eq!(view.row_count(), 1, "the placeholder left with the commit");
    let launches = batch.surviving_launches();
    let created = launches
        .iter()
        .find_map(|effect| effect.get::<documents::CreateDocumentEffect>())
        .expect("the commit launched the create");
    assert_eq!(created.location, document(&["project", "new.txt"]));
}

#[test]
fn an_empty_or_slashed_name_keeps_the_editor() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.start_create(
        directory(&["project"]),
        &store,
        ::editor::test_document::test_ui(),
    );
    for bad in ["", "  ", "a/b", ".."] {
        view.edit.as_mut().expect("editing").input =
            seeded_input(&store, ::editor::test_document::test_ui(), bad);
        let mut batch = imba::effect::Batch::new();
        view.perform(
            &mut store,
            &ui,
            TreeCommand::CommitEdit,
            &mut batch.effects(),
        );
        assert!(view.edit.is_some(), "{bad:?} does not commit");
        assert!(batch.surviving_launches().is_empty());
    }
}

#[test]
fn an_unfocused_paint_cancels_the_edit() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.start_create(
        directory(&["project"]),
        &store,
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 2);

    let commands = {
        let arena = imba::arena::Arena::default();
        let test_ui = ::editor::test_document::test_ui();
        let widget = imba::layout::Layout::layout(
            imba::View::display(&view, &arena, &store, test_ui),
            &arena,
            imba::constraints::Constraints::tight(skia_safe::Size::new(400.0, 600.0)),
        );
        let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(400.0, 600.0));
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 600)).expect("a surface");
        match imba::Widget::handle_event(
            &widget,
            &arena,
            &imba::event::Event::Paint {
                canvas: surface.canvas(),
                focused: false,
            },
            skia_safe::Rect::from_wh(400.0, 600.0),
        ) {
            imba::event::EventResult::Command(command) => vec![command],
            imba::event::EventResult::Commands(commands) => commands,
            _ => Vec::new(),
        }
    };
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, TreeCommand::CancelEdit)),
        "losing focus cancels"
    );
    for command in commands {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert!(view.edit.is_none());
    assert_eq!(view.row_count(), 1, "the placeholder left with the cancel");
}

#[test]
fn a_departed_folder_leaves_the_tree_on_paint() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(
        &mut store,
        &[directory(&["project"]), directory(&["other"])],
    );
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["other"]),
        Some(vec![document(&["other", "a.md"])]),
        &store,
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 3);

    // The session drops `other` (the removal echo landed in the
    // mirror the injected closure reads).
    workspace
        .folders
        .lock()
        .unwrap()
        .retain(|held| !held.path().contains(&"other".to_owned()));

    let commands = {
        let arena = imba::arena::Arena::default();
        let test_ui = ::editor::test_document::test_ui();
        let widget = imba::layout::Layout::layout(
            imba::View::display(&view, &arena, &store, test_ui),
            &arena,
            imba::constraints::Constraints::tight(skia_safe::Size::new(400.0, 600.0)),
        );
        let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(400.0, 600.0));
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 600)).expect("a surface");
        match imba::Widget::handle_event(
            &widget,
            &arena,
            &imba::event::Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
            skia_safe::Rect::from_wh(400.0, 600.0),
        ) {
            imba::event::EventResult::Command(command) => vec![command],
            imba::event::EventResult::Commands(commands) => commands,
            _ => Vec::new(),
        }
    };
    assert!(
        commands.iter().any(
            |command| matches!(command, TreeCommand::SyncRoots { stale, .. } if !stale.is_empty())
        ),
        "the gate flagged the departed root"
    );
    for command in commands {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert_eq!(view.row_count(), 1, "the root and its subtree left");
    assert!(view.tree.is_visible(&directory(&["project"])));
}

#[test]
fn dismissal_files_the_close() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
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
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![directory(&["project", "src"])]),
        &store,
        ::editor::test_document::test_ui(),
    );
    view.activate(
        1,
        &store,
        ::editor::test_document::test_ui(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project", "src"]),
        Some(vec![directory(&["project", "src", "deep"])]),
        &store,
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 3);

    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project", "src", "deep"]),
        Some(vec![document(&["project", "src", "deep", "a.md"])]),
        &store,
        ::editor::test_document::test_ui(),
    );
    assert_eq!(view.row_count(), 1, "the stale landing changed nothing");
}

#[test]
fn expanded_folders_watch_and_events_relist() {
    let mut store = Store::new();
    documents::watch::Watching::install(&mut store);
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    let ui = ::editor::test_document::test_ui();

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
    let launches = batch.surviving_launches();
    assert!(
        launches
            .iter()
            .any(|effect| effect.get::<documents::watch::SubscribeEffect>().is_some()),
        "an expanded folder asks for its watch"
    );

    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Watched {
            parent: directory(&["project"]),
            subscription: Some(documents::watch::Subscription(9)),
        },
        &mut imba::effect::Batch::new().effects(),
    );

    let mut batch = imba::effect::Batch::new();
    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        TreeCommand::Changed(vec![documents::watch::Subscription(9)]),
        &mut batch.effects(),
    );
    let launches = batch.surviving_launches();
    assert!(
        launches
            .iter()
            .any(|effect| effect.get::<documents::ListDirectoryEffect>().is_some()),
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
        ::editor::test_document::test_ui(),
        &mut batch.effects(),
    );
    let launches = batch.surviving_launches();
    assert!(
        launches.iter().any(|effect| effect
            .get::<documents::watch::UnsubscribeEffect>()
            .is_some()),
        "the fold unsubscribes"
    );
    assert!(view.tree.watches.is_empty());
}

#[test]
fn cursor_walks_and_enter_opens() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
        workspace.clone(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    view.activate(
        0,
        &store,
        ::editor::test_document::test_ui(),
        &mut imba::effect::Batch::new().effects(),
    );
    view.tree.splice_listing(
        directory(&["project"]),
        Some(vec![
            directory(&["project", "src"]),
            document(&["project", "README.md"]),
        ]),
        &store,
        ::editor::test_document::test_ui(),
    );
    // The commands the controller's key table emits, verbatim
    // (docs/ui/list-keyboard.md §3).
    use imba::list::{ActivateTrigger, ListOps};
    let mut drive = |view: &mut SessionTreeView, command| {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };
    let step = |view: &SessionTreeView, delta| {
        let index = view.tree.list.step_index(delta).expect("a stepped row");
        TreeCommand::Rows(view.tree.list.select_command(index))
    };
    let enter = |view: &SessionTreeView| {
        let index = view.tree.list.cursor_index().expect("a cursor row");
        TreeCommand::Rows(
            view.tree
                .list
                .activate_command(index, ActivateTrigger::Enter),
        )
    };
    let fold = |view: &SessionTreeView, expand| {
        let index = view.tree.list.cursor_index().expect("a cursor row");
        TreeCommand::Rows(hikit::ListKeyCommand::Fold { index, expand })
    };

    let command = step(&view, 1);
    drive(&mut view, command);
    let command = step(&view, 1);
    drive(&mut view, command);
    assert_eq!(view.selected_name().as_deref(), Some("README.md"));
    let command = enter(&view);
    drive(&mut view, command);
    let Some(ModalRequest::OpenLocations(locations)) = view.take_request() else {
        panic!("Enter opens the selected document");
    };
    assert_eq!(locations, vec![document(&["project", "README.md"])]);

    let command = fold(&view, false);
    drive(&mut view, command);
    assert_eq!(view.selected_name().as_deref(), Some("project"));
    let command = fold(&view, false);
    drive(&mut view, command);
    assert_eq!(view.row_count(), 1, "the fold took the subtree");
}

#[test]
fn a_relist_keeps_expanded_subtrees() {
    let mut store = Store::new();
    let workspace = workspace_with(&mut store, &[directory(&["project"])]);
    let mut view = open_view(
        &mut store,
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
        ::editor::test_document::test_ui(),
    );
    view.tree.splice_listing(
        directory(&["project", "src"]),
        Some(vec![document(&["project", "src", "lib.rs"])]),
        &store,
        ::editor::test_document::test_ui(),
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
        ::editor::test_document::test_ui(),
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
        ::editor::test_document::test_ui(),
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
        ::editor::test_document::test_ui(),
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
    let mut view = open_view(
        &mut store,
        workspace,
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    let dark = editor::env::Themes::of(&store).ui().tree.highlight.0;
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

    editor::env::Themes::set(&mut store, ::editor::theme::Theme::light());
    let light = editor::env::Themes::of(&store).ui().tree.highlight.0;
    assert_ne!(dark, light, "the themes disagree, or this test is vacuous");

    let commands = {
        let arena = imba::arena::Arena::default();
        let ui = ::editor::test_document::test_ui();
        let size = skia_safe::Size::new(400.0, 600.0);
        let widget = imba::layout::Layout::layout(
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
            ::editor::test_document::test_ui(),
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
