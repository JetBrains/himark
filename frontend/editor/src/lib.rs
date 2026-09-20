// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod assist;
mod before_inlay;
mod caret;
mod caret_ops;
mod change_sink;
pub mod diff;
mod document;
mod document_layout;
mod document_render;
mod dynamic;
mod edit_log;
mod editor;
mod editor_view;
pub mod embedded_fonts;
mod enrich;
pub mod env;
mod env_flags;
mod fold;
mod location;
mod markup;
mod popup;
mod repair;
mod reparse;
pub mod scroll_stripe;
mod shape_cache;
mod shaped_line;
mod split_diff;
mod startup_profile;
pub mod sticky;
pub mod text_cursor;
pub mod theme;
mod undo;
mod unified_diff;
mod viewport;
pub(crate) mod width_tree;

pub use before_inlay::{BeforeCommand, BeforeInlay};
pub use caret::{Caret, MultiCaret};
pub use change_sink::{ChangeSink, InstalledChangeSink};
pub use document::{Document, EditorBuild};
pub use dynamic::{DynamicEditorCommand, EditorCommands};
pub use edit_log::{bridge, common_base, EditIdentity, EditLog};
pub use location::{Authority, ResourceLocation, ResourceType};
pub use unified_diff::{DiffLayout, UnifiedDiffCommand, UnifiedDiffEffects, UnifiedDiffView};

pub type FontSource =
    std::sync::Arc<dyn Fn() -> skia_safe::textlayout::FontCollection + Send + Sync>;
pub use document_layout::{DocumentLayout, SYNC_LAYOUT_HEIGHT};
pub use editor::{Editor, EditorEffects, EditorId};

pub use editor_view::{ClickKind, EditorCommand, EditorFocus, EditorView, Motion};
pub use enrich::{
    ready as enrich_ready, CaretContext, EnrichCx, EnrichEffect, EnrichFuture, EnrichHandler,
    EnrichInput, EnrichOutcome, Enricher, EnricherId, Enrichers, Enrichment, Interest, MeasureCtx,
};
pub use env::Workshop;
pub use markup::{
    inlay_anchors_line, inlay_repair_span, set_diff, BlockStyle, Inlay, InlayCommand, InlayEditing,
    InlayInterval, InlayKey, InlayMode, InsteadKind, IntervalId, Markup, MarkupBuilder, MarkupId,
    MarkupLayer, OutlineItem, OverlaidMarkup, PopupSpec, StyleId, Syntax, SyntaxId, TextAlignment,
    TextAttributes, TextDecorationInterval, INLAY_HOST,
};
pub use operation::Operation;
pub use repair::{RepairEffect, RepairHandler, RepairedLayout};
pub use reparse::{
    Assist, AssistKind, AssistRequest, LanguageEntry, ReparseOutcome, ReparseWork, SideGrammar,
    SyntaxLanguage, SyntaxLanguages, SyntaxSite, SyntaxTree,
};
pub use reparse::{ReparseEffect, ReparseHandler};
pub use split_diff::{
    prepare_marks, DiffState, PreparedMarks, RepairDiffEffect, RepairDiffHandler, SplitDiffCommand,
    SplitDiffEffects, SplitDiffView,
};
pub use text::Text;
pub use theme::{StyleId as ThemeStyleId, Theme};

#[doc(hidden)]
pub mod test_document;

#[cfg(test)]
mod metrics_probe;

#[cfg(test)]
mod tests;

pub use crate::document::{FragmentKey, FragmentSetId};
