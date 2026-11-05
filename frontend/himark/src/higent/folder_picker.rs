// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The FOLDER PICKER: the session host's file tree on a peeker-style
//! sheet — the Files tab's view (`filetree::SessionTreeView`), whole,
//! rooted at the host's home (`RootInfo.default_directory`). Opened
//! by the add-folder road when the system chooser cannot browse the
//! session's machine: a remote host, and every iOS session. ⏎ walks
//! the tree; ⌘⏎ grants the cursor's directory and closes.

use std::sync::Arc;

use ::filetree::{SessionTree, SessionTreeView, TreeCommand};
use editor::location::ResourceLocation;
use gfx::Size;
use hikit::modal::{ModalRequest, ModalView, RequestSlot};
use hikit::sheet;
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    layout::{Insets, Layout as _, LayoutExt as _},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt as _,
    ui::UiCtx,
    View,
};

/// What the picker is opened OVER — the host's browse roots and the
/// grant to run on the choice.
pub(crate) struct FolderPick {
    /// The host's filesystem as locations: the home root (the
    /// connect's `defaultDirectory`), or the filesystem root for a
    /// host that offered none.
    pub roots: Vec<ResourceLocation>,
    /// The sheet's caps label, e.g. "ADD FOLDER · ZIGGY".
    pub caption: String,
    /// Builds the grant request for the chosen folder; the picker
    /// pushes it through `imba::command::Requests` and closes —
    /// uniform across the windowed and mounted roads.
    pub grant: Arc<dyn Fn(ResourceLocation) -> Arc<dyn imba::command::DynamicCommand> + Send + Sync>,
}

/// One picker tree per host-fs world, persisted like the dock's
/// (`SessionTree`): reopening finds the browse where it was left.
#[derive(Clone, Default)]
struct PickerTrees(std::collections::HashMap<String, imba::store::Id<SessionTree>>);

fn tree_slot(store: &mut Store, key: &str) -> imba::store::Id<SessionTree> {
    if let Some(id) = store
        .get::<PickerTrees>()
        .and_then(|trees| trees.0.get(key).copied())
    {
        return id;
    }
    let id = imba::store::Id::mint();
    store.put_entity(id, SessionTree::default());
    store.update::<PickerTrees>(|trees| {
        trees.0.insert(key.to_owned(), id);
    });
    id
}

#[derive(Clone)]
pub(crate) enum FolderPickerCommand {
    Tree(imba::scroll::ScrollCommand<TreeCommand>),
    /// ⌘⏎ — the cursor's directory becomes the session's.
    Choose,
    Close,
}

impl std::fmt::Display for FolderPickerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FolderPickerCommand::Tree(command) => command.fmt(out),
            FolderPickerCommand::Choose => out.write_str("folder picker choose"),
            FolderPickerCommand::Close => out.write_str("folder picker close"),
        }
    }
}

pub(crate) struct FolderPicker {
    /// The Files tab's tree, whole, in its own scroller (the view is
    /// scroller-free by design).
    tree: imba::scroll::ScrollView<SessionTreeView>,

    caption: String,

    grant: Arc<dyn Fn(ResourceLocation) -> Arc<dyn imba::command::DynamicCommand> + Send + Sync>,

    /// The windowed modal stands on its own SHEET, centered; the
    /// mounted road rides the host's glass (the peeker's recipe).
    floored: bool,

    request: RequestSlot<ModalRequest>,
}

impl Clone for FolderPicker {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
            caption: self.caption.clone(),
            grant: Arc::clone(&self.grant),
            floored: self.floored,
            request: RequestSlot::default(),
        }
    }
}

impl FolderPicker {
    pub(crate) fn open(
        store: &mut Store,
        ui: &UiCtx,
        pick: FolderPick,
        fx: &mut imba::effect::Effects<'_, FolderPickerCommand>,
    ) -> Self {
        let key = pick
            .roots
            .first()
            .map(|root| root.authority().as_str().to_owned())
            .unwrap_or_default();
        let trees = tree_slot(store, &key);
        let mirror: Arc<dyn Fn(&Store) -> Vec<ResourceLocation> + Send + Sync> = {
            let roots = pick.roots.clone();
            Arc::new(move |_| roots.clone())
        };
        // The cursor SEEDS on the home root: arrows move right away,
        // and ⌘⏎ with nothing else touched grants home itself.
        let reveal = pick.roots.first().cloned();
        let view = fx.scope(
            |command| FolderPickerCommand::Tree(imba::scroll::ScrollCommand::Content(command)),
            |fx| {
                SessionTreeView::open(
                    store,
                    ui,
                    trees,
                    &pick.roots,
                    mirror,
                    // These roots are not session folders: nothing to
                    // remove from anything.
                    Arc::new(|_, _| None),
                    reveal,
                    fx,
                )
            },
        );
        Self {
            tree: imba::scroll::ScrollView::new(view),
            caption: pick.caption,
            grant: pick.grant,
            floored: true,
            request: RequestSlot::default(),
        }
    }

    /// The GLASS face: the mounting host paints the island floor.
    pub(crate) fn unfloored(mut self) -> Self {
        self.floored = false;
        self
    }

    fn choose(&mut self, store: &mut Store) {
        let Some(location) = self.tree.content().selected_location() else {
            return;
        };
        if !location.kind().is_directory() {
            return; // folders only — a file row never grants
        }
        imba::command::Requests::push(store, (self.grant)(location));
        self.request.file(ModalRequest::Close);
    }
}

impl View for FolderPicker {
    type Command = FolderPickerCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, FolderPickerCommand> {
        use imba::focus::FocusData;
        // An inline rename/create or the type-ahead search owns its
        // keys; the picker keeps choose and close otherwise.
        let busy = self.tree.content().editing() || !self.tree.content().search_query().is_empty();
        let own = FocusData {
            commands: vec![imba::PresentableCommand::new(
                "folder-picker.close",
                "Close Folder Picker",
                FolderPickerCommand::Close,
            )],
            on_key: Some(Box::new(move |key, mods| match key {
                Key::Enter if mods.command && !busy => {
                    EventResult::command(FolderPickerCommand::Choose)
                }
                Key::Escape if !busy => EventResult::command(FolderPickerCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.tree.focus_data(store, ui).map(FolderPickerCommand::Tree))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(
            |command| FolderPickerCommand::Tree(imba::scroll::ScrollCommand::Content(command)),
            |fx| self.tree.content_mut().destroy(store, fx),
        );
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            FolderPickerCommand::Tree(command) => {
                fx.scope(FolderPickerCommand::Tree, |fx| {
                    self.tree.perform(store, ui, command, fx)
                });
                // The tree's own asks: its escape closes the picker;
                // a FILE activation would OPEN — the picker is not a
                // navigation surface, so that one is dropped.
                match self.tree.content_mut().take_request() {
                    Some(ModalRequest::Close) => self.request.file(ModalRequest::Close),
                    Some(_) | None => {}
                }
            }
            FolderPickerCommand::Choose => self.choose(store),
            FolderPickerCommand::Close => self.request.file(ModalRequest::Close),
        }
    }

    /// The dialog: the caption band, the host's tree between two
    /// hairlines, the key footer — one column on the jump sheet's
    /// chrome. Windowed, it stands on its own SHEET centered in the
    /// window and a press beside it closes; on the glass road the
    /// host's island is the sheet.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{fixed, Column};
            use imba::Thunk as _;
            let size = constraints.max;
            let themes = ::editor::env::Themes::of(store);
            let chrome = &themes.part(::hikit::chrome::PeekerChrome::KEY).jump;
            let ink = themes.part(::editor::theme::InkChrome::KEY);
            let sheet = sheet::Sheet(chrome.sheet.clone());
            let dialog = match self.floored {
                true => {
                    // A single column: the matches column's width is
                    // the dialog's — the preview never mounts here.
                    let width = list_width(sheet.width_in(size), chrome);
                    sheet.rect_in(size, Size::new(width, sheet.max_height_in(size)))
                }
                false => gfx::Rect::from_size(size),
            };
            let width = dialog.width();
            let loose = |height: f32| Constraints {
                min: Size::default(),
                max: Size::new(width, height),
            };

            let head = Column::new(arena)
                .child(hikit::ui::text(
                    &hikit::ui::caps(store, ui),
                    self.caption.clone(),
                ))
                .pad_insets(Insets {
                    left: chrome.pad_x,
                    right: chrome.pad_x,
                    top: chrome.pad_top,
                    bottom: chrome.pad_bottom,
                })
                .layout(arena, loose(dialog.height()));
            let foot = sheet::key_hints(
                arena,
                &[
                    ("↑↓", "move"),
                    ("⏎", "open folder"),
                    ("⌘⏎", "add folder"),
                    ("esc", "close"),
                ],
                &sheet::mono(ui, chrome.footer_size, ink.ink_2.0),
                &hikit::ui::brief(store, ui).sized(chrome.footer_size),
                chrome.footer_gap,
            )
            .pad_insets(Insets {
                left: chrome.pad_x,
                right: chrome.pad_x,
                top: chrome.footer_pad_y,
                bottom: chrome.footer_pad_y,
            })
            .layout(arena, loose(dialog.height()));
            let rows_h = (dialog.height()
                - head.size().height
                - foot.size().height
                - 2.0
                - 2.0 * chrome.list_pad_y)
                .max(chrome.row_height);
            let rows = imba::layout::Layout::layout(
                self.tree.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(width, rows_h)),
            )
            .map(FolderPickerCommand::Tree);
            let column = Column::new(arena)
                .child(fixed(head))
                .child(hikit::ui::rule_h(ink.rule.0, width))
                .child(fixed(rows).pad_insets(Insets {
                    top: chrome.list_pad_y,
                    bottom: chrome.list_pad_y,
                    ..Default::default()
                }))
                .child(hikit::ui::rule_h(ink.rule.0, width))
                .child(fixed(foot))
                .sized(width, dialog.height())
                .layout(arena, loose(dialog.height()));
            // A press on the sheet's own ground stays on the sheet.
            let column = imba::thunk_ext::ThunkExt::event(
                column,
                |_arena: &Arena, event: &Event<'_>, _size| match event {
                    Event::MouseDown { .. } => EventResult::handled(),
                    _ => EventResult::Ignored,
                },
            );

            let mut container = imba::container::container(arena, size);
            // The click-away catcher behind the dialog; windowed, it
            // also casts the sheet's shadow.
            let catcher = leaf::<FolderPickerCommand>(size.width, size.height).event(
                |_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::command(FolderPickerCommand::Close),
                    _ => EventResult::Ignored,
                },
            );
            match self.floored {
                true => {
                    container.place(0.0, 0.0, catcher.paint_below(sheet.shadow_painter(dialog)));
                    container.place(dialog.left, dialog.top, sheet.wrap(column));
                }
                false => {
                    container.place(0.0, 0.0, catcher);
                    container.place(dialog.left, dialog.top, column);
                }
            }
            container
        })
    }
}

/// The picker column's width in a sheet `width` wide — the jump
/// sheet's matches column, since no preview mounts beside it.
fn list_width(width: f32, chrome: &::hikit::chrome::JumpSheetChrome) -> f32 {
    chrome.list_width.min(width).max(1.0)
}

impl ModalView for FolderPicker {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::location::{Authority, ResourceType};

    fn root() -> ResourceLocation {
        ResourceLocation::new(
            ResourceType::directory(),
            Authority::new("test"),
            vec!["home".to_owned(), "user".to_owned()],
        )
    }

    struct Granted(ResourceLocation);

    impl imba::command::DynamicCommand for Granted {
        fn id(&self) -> &'static str {
            "test.granted"
        }
        fn name(&self) -> String {
            "Granted".to_owned()
        }
        fn perform(
            &self,
            _store: &mut Store,
            _ui: &UiCtx,
            _fx: &mut imba::command::Fx<'_>,
        ) {
        }
    }

    #[test]
    fn choosing_a_directory_grants_it_and_closes() {
        use imba::effect::Batch;
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let chosen = Arc::new(std::sync::Mutex::new(None));
        let recorder = Arc::clone(&chosen);
        let mut batch: Batch<FolderPickerCommand> = Batch::new();
        let mut picker = FolderPicker::open(
            &mut store,
            &ui,
            FolderPick {
                roots: vec![root()],
                caption: "ADD FOLDER · TEST".to_owned(),
                grant: Arc::new(move |folder| {
                    *recorder.lock().expect("the probe") = Some(folder.clone());
                    Arc::new(Granted(folder))
                }),
            },
            &mut batch.effects(),
        );

        // The home root stands as the tree's first row; the listing
        // lands like the Files tab's does.
        let sub = root().child(ResourceType::directory(), "projects");
        let listed = TreeCommand::Listed {
            parent: root(),
            entries: Some(vec![
                sub.clone(),
                root().child(ResourceType::document(), "notes.md"),
            ]),
        };
        let mut batch: Batch<FolderPickerCommand> = Batch::new();
        picker.perform(
            &mut store,
            &ui,
            FolderPickerCommand::Tree(imba::scroll::ScrollCommand::Content(listed)),
            &mut batch.effects(),
        );

        // The cursor starts on the root — a directory: ⌘⏎ grants it
        // and the sheet closes.
        let mut batch: Batch<FolderPickerCommand> = Batch::new();
        picker.perform(
            &mut store,
            &ui,
            FolderPickerCommand::Choose,
            &mut batch.effects(),
        );
        let drained = imba::command::Requests::drain(&mut store);
        assert_eq!(drained.len(), 1, "one grant request rides the queue");
        assert_eq!(drained[0].id(), "test.granted");
        assert_eq!(
            chosen.lock().expect("the probe").clone(),
            Some(root()),
            "the cursor's directory granted"
        );
        assert!(
            matches!(
                hikit::modal::ModalView::take_request(&mut picker),
                Some(ModalRequest::Close)
            ),
            "the pick closes the sheet"
        );
    }
}
