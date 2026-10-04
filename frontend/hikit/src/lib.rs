// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench UI KIT: the widget vocabulary the feature surfaces
//! are built from — tree forests, keyboard-driven lists, row and
//! panel chrome, combo boxes, the chrome fonts. Pure UI over
//! `imba` + `editor` (theme, env, fonts): no collections, no
//! protocol, no application.

pub mod combo;
pub mod family_row;
pub mod fonts;
pub mod forest;
pub mod list_keyboard;
pub mod menu;
pub mod modal;
pub mod navigation;
pub mod panel;
pub mod rows;
pub mod tree_item;
pub mod ui;

pub use family_row::{FamilyRow, Row};
pub use modal::{ModalRequest, ModalView, RequestSlot};
pub use navigation::{NavigationLocation, Navigator, NoPlace, Place};
pub use panel::{DynPanelView, PanelRequest, PanelView, RowMinter, WidgetOrigin};

pub use forest::{Forest, ForestList, ForestNode, ForestSearcher, TreeRow};
pub use list_keyboard::{
    subsequence_match, ActivateTrigger, AnnounceSelect, AnnounceSelectHandler, ItemSource,
    ListKeyCommand, ListKeyboardController, NoSearcher, Searcher, SpeedSearchEffect,
    SpeedSearchHandler, SpeedSearchMatches,
};
pub use rows::{label_slice, paint_panel_chrome, panel_inset, selection_style, LabelRow};
pub use tree_item::{
    secondary_press, tree_action, tree_context, tree_toggle, TreeItemCommand, TreeItemView,
    TreeLabel, TreeLabelCommand, TreeListCommand, TreeTint,
};
