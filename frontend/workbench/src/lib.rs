// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench: windows and their pane tree, the toolbar band, the
//! dock and drawer layers, the find bar, the navigators machinery and
//! the pane-row minting — the shell's furniture, below the app. The
//! shell reaches in through Verb lanes and the ceremony-installed
//! registry roads; nothing here knows the app's command type.

pub mod dock;
pub mod drawer;
pub mod find;
pub mod navigation;
pub mod registry;
pub mod rows;
pub mod toolbar;
pub mod window;
pub mod workbench;
pub mod workbench_node;
