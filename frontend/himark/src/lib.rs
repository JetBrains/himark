// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod app;
mod app_ext;
pub mod changes_view;
mod commands;
pub mod completion;
mod diffs;
mod dock;
mod drawer;
mod effects;
mod find;
mod focus;
pub mod hiahp;
pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod higent;
pub mod hihistory;
pub mod hipeek;
pub mod hisearch;
pub mod hover;
mod keymap;
pub mod locations;
pub mod menu;
mod modal;
pub mod navigation;
pub mod new_session;
mod registry;
mod save;
mod startup_profile;
mod state;
mod stats;
pub mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod test_driver;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod toc;
// The UI kit lives in its own crate; the re-exports below are
// migration scaffolding — consumers move to `hikit` paths as they
// convert, and himark shrinks toward the protocol layer.
pub use hikit::{combo, fonts, forest, list_keyboard, rows, tree_item, ui};
pub use hikit::{rows::label_slice, rows::paint_panel_chrome, rows::panel_inset, tree_item::secondary_press, rows::selection_style, list_keyboard::subsequence_match, tree_item::tree_action, tree_item::tree_context, tree_item::tree_toggle, list_keyboard::ActivateTrigger, list_keyboard::AnnounceSelect, list_keyboard::AnnounceSelectHandler, forest::Forest, forest::ForestList, forest::ForestNode, forest::ForestSearcher, list_keyboard::ItemSource, rows::LabelRow, list_keyboard::ListKeyCommand, list_keyboard::ListKeyboardController, list_keyboard::NoSearcher, list_keyboard::Searcher, list_keyboard::SpeedSearchEffect, list_keyboard::SpeedSearchHandler, list_keyboard::SpeedSearchMatches, tree_item::TreeItemCommand, tree_item::TreeItemView, tree_item::TreeLabel, tree_item::TreeLabelCommand, tree_item::TreeListCommand, forest::TreeRow, tree_item::TreeTint};
pub mod diff_canvas;
pub mod diff_pane;
mod pane_rows;
mod toolbar;
mod watch;
mod window;
mod workbench;
mod workbench_node;
mod workspace;

pub use crate::diff_canvas::canvas::{CanvasNavigator, Canvases, DiffCanvasView};
pub use crate::diff_pane::{
    diff_panel, gathered_view, open_diff_documents, open_opened_diff_pane, pair_row_minter,
    DiffPanelView, DiffPlace, OpenDiff, PairPane,
};
pub use crate::diffs::{
    build_diff_view, gather_diff_view, install_opened_pair, rearm_base_asks, rewrap_pair,
    sync_stripe_bases, teardown_diff_view, DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler,
    DiffView, DiffViewId, StripeBaseResolver, StripeBases, OPEN_HALF_WIDTH,
};
pub use crate::pane_rows::{
    mint, mint_unfronted, CanvasRow, ChatRow, PaneRow, PairRow, RowMinters, TerminalRow,
};
pub use crate::workspace::{
    open_by_location_effect, open_locations, BuildDocumentEffect, BuiltDocument,
    CreateDocumentEffect, DeleteResourceEffect, DiffSide, DiffSideInput, FindEffect,
    ListDirectoryEffect, LocationsChannel, LspLocationsEffect, LspLocationsKind,
    MoveResourceEffect, OpenByLocationEffect, OpenDiffByLocationsEffect, OpenDiffPairEffect,
    OpenedDiffPair, PickSaveEffect, SearchLocationsEffect, SessionId, StoreDocumentEffect,
};
pub use app::*;
pub use app_ext::AppExt;
pub use commands::{palette_commands, AppRequests, Commands, DynamicCommand};
pub use ahp_lsp::{LspCompletionEffect, LspItem};
pub use dock::{DockCommand, DOCK_MIN_WIDTH, DOCK_WIDTH};
pub use documents::{lifecycle::close_editor, lifecycle::deliver, is_scratch, is_synthetic, text_ext::line_col_at, lifecycle::mount_editor, next_scratch_location, text_ext::offset_at, dynamic::DocumentCommand, dynamic::DocumentCommands, DocumentHook, DocumentId, entity_view::EditorIdView, FetchDocumentEffect, FetchResourceBytesEffect, text_ext::LineCol, OpenDocument, OpenDocuments};
pub use drawer::DRAWER_WIDTH;
pub use effects::*;
pub use find::{FindBar, FindCommand};
pub use imba::ime::ImeClient;
pub use imba::{clipboard::ClipboardClient, clipboard::ClipboardContent};
pub use keymap::{Keymap, Keymaps};
pub use modal::{dock_scope, modal_scope, side_scope, ModalRequest, ModalView, RequestSlot};
pub use navigation::{
    EditorPlace, NavigationLocation, Navigator, Navigators, NoPlace, Place, RecentLocations,
};
pub use save::{SaveAll, SaveDocument};
pub use state::AppState;
pub use stats::Stats;
pub use toc::{
    NavigateToPlace, OutlineCommand, OutlineEffect, OutlineHandler, OutlineRows, OutlineView,
    TocCommand, TocView, ToggleToc,
};
pub use toolbar::{
    composer_button, ToolbarButton, ToolbarButtons, ToolbarCommand, ToolbarRequest, ToolbarSide,
};
pub use watch::{
    refetch_document, sync_document_watches, FileChanged, ReloadDocument, SubscribeEffect,
    Subscription, UnsubscribeEffect, Watching,
};
pub use window::{LayerFocus, Window, WindowCommand, WindowId, Windows};
pub use workbench::{
    chat_column_engaged, chat_column_width, workbench_geometry, ChatColumn, Workbench,
    WorkbenchCommand, WorkbenchGeometry,
};
pub use workbench_node::{
    DynPanelView, EditorPane, NodeCommand, PaneCommand, Panel, PanelCommand, PanelRequest,
    PanelView, WidgetOrigin, WorkbenchNode,
};

#[cfg(test)]
#[path = "editor_tests.rs"]
mod editor_tests;
