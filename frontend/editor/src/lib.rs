// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod assist;
pub(crate) mod before_inlay;
pub mod caret;
pub mod completion;
pub(crate) mod caret_ops;
pub mod change_sink;
pub mod diff;
pub mod document;
pub mod document_layout;
pub(crate) mod document_render;
pub mod dynamic;
pub mod edit_log;
pub mod editor;
pub mod editor_view;
pub mod embedded_fonts;
pub mod enrich;
pub mod find;
pub mod hover;
pub mod env;
pub(crate) mod env_flags;
pub mod fold;
pub mod linecol;
pub mod location;
pub mod markup;
pub mod popup;
pub mod repair;
pub mod reparse;
pub mod scroll_stripe;
pub mod shape_cache;
pub(crate) mod shaped_line;
pub mod split_diff;
pub mod startup_profile;
pub mod sticky;
pub mod text_cursor;
pub mod theme;
pub mod undo;
pub mod unified_diff;
pub mod viewport;
pub(crate) mod width_tree;

pub type FontSource =
    std::sync::Arc<dyn Fn() -> skia_safe::textlayout::FontCollection + Send + Sync>;

#[doc(hidden)]
pub mod test_document;

#[cfg(test)]
pub(crate) mod metrics_probe;

#[cfg(test)]
mod tests;
