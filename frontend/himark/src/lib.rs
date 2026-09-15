// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod app;
mod app_ext;
pub mod combo;
mod commands;
pub mod completion;
mod diffs;
mod dock;
mod drawer;
mod effects;
mod find;
mod focus;
pub mod fonts;
mod forest;
pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod higent;
pub mod hihistory;
pub mod hover;
mod keymap;
mod location_list;
mod modal;
mod navigation;
pub mod new_session;
pub mod rows;
mod save;
mod sheet;
mod speedsearch;
mod startup_profile;
mod state;
mod stats;
pub mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod test_driver;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod toc;
mod tree_item;
pub mod ui;
pub use forest::{Forest, ForestList, ForestNode, ForestSearcher, TreeRow};
pub use speedsearch::{
    subsequence_match, ItemSource, Searcher, SpeedSearchCommand, SpeedSearchEffect,
    SpeedSearchHandler, SpeedSearchView,
};
pub use tree_item::{
    tree_interaction, TreeItemCommand, TreeItemView, TreeLabel, TreeLabelCommand, TreeListCommand,
    TreeTint,
};
mod family_rows;
mod toolbar;
mod watch;
mod window;
mod workbench;
mod workbench_node;
mod workspace;

pub use crate::diffs::{
    rearm_base_asks, sync_stripe_bases, DiffChanged, DiffHandle, DiffNormalizeEffect,
    DiffNormalizeHandler, DiffView, DiffViewId, FetchBaseEffect, StripeBases,
};
pub use crate::family_rows::{mint_unfronted, FamilyRow, RowMinter, RowMinters};
pub use crate::workspace::{
    open_by_location_effect, open_locations, prebuild_group, prepare_built, BuildDocumentEffect,
    BuiltDocument, FindEffect, FindTarget, ListDirectoryEffect, OpenByLocationEffect,
    OpenDiffByLocationsEffect, PickSaveEffect, RowPrep, ScratchSpaces, SessionId,
    StoreDocumentEffect,
};
pub use ::editor::*;
pub use app::*;
pub use app_ext::AppExt;
pub use commands::{palette_commands, AppRequests, Commands, DynamicCommand, LandingCommand};
pub use completion::{LspAnswer, LspCompletionEffect, LspItem};
pub use dock::{DockCommand, DOCK_MIN_WIDTH, DOCK_WIDTH};
pub use documents::{
    close_editor, deliver, is_scratch, is_synthetic, line_col_at, mount_editor,
    next_scratch_location, notify_change, offset_at, ChangeObserver, DocumentChangeEffect,
    DocumentHook, DocumentId, EditorIdView, FetchDocumentEffect, FetchResourceBytesEffect, LineCol,
    OpenDocument, OpenDocuments, TextChange,
};
pub use drawer::DRAWER_WIDTH;
pub use effects::*;
pub use find::{FindBar, FindCommand};
pub use imba::ImeClient;
pub use imba::{ClipboardClient, ClipboardContent};
pub use keymap::{Keymap, Keymaps};
pub use location_list::{
    snap_ranges, sort_locations, GroupCommand, GroupSpans, InstallGroup, ListEntry, ListId,
    ListPanel, ListPanelCommand, LocationList, LocationListCommand, LocationLists, PrebuiltRows,
    ResultGroup, ResultGroups, ResultRows, ResultsCommand, SpanSource,
};
pub use modal::{dock_scope, modal_scope, side_scope, ModalRequest, ModalView, RequestSlot};
pub use navigation::{
    EditorPlace, NavigationLocation, Navigator, Navigators, NoPlace, Place, RecentLocations,
};
pub use rows::{
    paint_panel_chrome, panel_inset, selection_style, LabelRow, RowList, RowListCommand,
};
pub use save::{SaveAll, SaveDocument};
pub use sheet::{composer_button, FloatingChat};
pub use state::{AppState, Gathered};
pub use stats::Stats;
pub use toc::{
    OutlineCommand, OutlineEffect, OutlineHandler, OutlineRows, OutlineView, TocCommand, TocView,
    ToggleToc,
};
pub use toolbar::{
    toggle_toolbar_session, OverlaySurface, OverlaySurfaces, ToolbarButton, ToolbarButtons,
    ToolbarCommand, ToolbarRequest, ToolbarSide,
};
pub use watch::{
    refetch_document, sync_document_watches, FileChanged, ReloadDocument, SubscribeEffect,
    Subscription, UnsubscribeEffect, Watching,
};
pub use window::{LayerFocus, Window, WindowCommand, WindowId, Windows};
pub use workbench::{workbench_geometry, Workbench, WorkbenchGeometry};
pub use workbench_node::{
    DynPanelView, EditorPane, NodeCommand, PaneCommand, Panel, PanelCommand, PanelRequest,
    PanelView, WidgetOrigin, WorkbenchNode,
};

#[cfg(test)]
#[path = "editor_tests.rs"]
mod editor_tests;
