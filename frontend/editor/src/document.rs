// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;
use operation::{Op, Operation};
use skia_safe::textlayout::FontCollection;
use text::Text;

use crate::edit_log::EditLog;
use crate::editor::{Editor, EditorEffects, EditorId, EditorPlaceholder};
use crate::editor_view::{EditorCommand, EditorFocus, Motion};
use crate::repair::{RepairEffect, RepairedLayout};
use crate::reparse::ReparseEffect;
use crate::reparse::{ReparseOutcome, ReparseWork, SyntaxLanguages, SyntaxSite};

use crate::markup::{
    Inlay, InlayCommand, InlayKey, InlayMode, Markup, MarkupId, MarkupLayer, Syntax, SyntaxId,
};

use std::{ops::Range, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FragmentSetId(u64);

impl FragmentSetId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FragmentKey {
    pub(crate) set: FragmentSetId,
    pub(crate) key: crate::markup::IntervalId,
}

pub enum EditorBuild {
    Complete,
    Bounded,
    Prebuilt(crate::document_layout::DocumentLayout),
}

pub const NOWRAP_LAYOUT_WIDTH: f32 = 1_000_000.0;

#[derive(Clone)]
pub struct Document {
    pub(crate) text: Text,

    parsed_revision: u64,

    pub(crate) syntax: Option<Syntax>,

    log: EditLog,

    undo: crate::undo::UndoHistory,

    markup_generation: u64,
    markup_changes: rpds::QueueSync<MarkupChange>,

    /// Bumped by every FLAGGED-entry lifecycle event (the flag set, a
    /// flagged entry's replace/remove, a pick joining) — the stripe
    /// sweep's whole input from the markup side (docs/scroll-stripe.md).
    scroll_stripe_generation: u64,

    markups: rpds::HashTrieMapSync<MarkupId, Markup>,

    fragments:
        rpds::HashTrieMapSync<FragmentSetId, intervals::Intervals<crate::markup::IntervalId, ()>>,

    diffs: rpds::HashTrieMapSync<crate::diff::DiffId, crate::diff::Diff>,

    pub(crate) editors: rpds::HashTrieMapSync<EditorId, Editor>,

    repair_token: Option<imba::effect::CancellationToken>,

    reparse_token: Option<imba::effect::CancellationToken>,

    enrich_base: Option<crate::ResourceLocation>,

    enrich: rpds::HashTrieMapSync<EnrichKey, EnrichSlot>,

    token: DocumentToken,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct EnrichKey {
    enricher: crate::enrich::EnricherId,
    editor: Option<EditorId>,
}

#[derive(Clone)]
struct EnrichSlot {
    markup: MarkupId,
    token: Option<imba::effect::CancellationToken>,

    launched: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct DocumentToken(u64);

#[derive(Debug)]
struct MarkupChange {
    generation: u64,
    ranges: Option<Vec<Range<u32>>>,
}

const MARKUP_CHANGE_LOG: usize = 64;
const MARKUP_CHANGE_RANGES: usize = 256;

impl DocumentToken {
    fn fresh() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl Document {
    pub fn new(text: Text, markup: Markup) -> Self {
        let mut markups = rpds::HashTrieMapSync::new_sync();
        if !markup.is_empty() {
            let mut markup = markup;
            markup.set_scope(crate::markup::MarkupScope::Document);
            markups.insert_mut(MarkupId::mint(), markup);
        }
        Self {
            text,
            parsed_revision: 0,
            syntax: None,
            log: EditLog::new(),
            undo: crate::undo::UndoHistory::default(),
            markup_generation: 0,
            markup_changes: rpds::QueueSync::new_sync(),
            scroll_stripe_generation: 0,
            markups,
            fragments: rpds::HashTrieMapSync::new_sync(),
            diffs: rpds::HashTrieMapSync::new_sync(),
            editors: rpds::HashTrieMapSync::new_sync(),
            repair_token: None,
            reparse_token: None,
            enrich_base: None,
            enrich: rpds::HashTrieMapSync::new_sync(),
            token: DocumentToken::fresh(),
        }
    }

    pub(crate) fn token(&self) -> DocumentToken {
        self.token
    }

    pub fn with_syntax(mut self, mut root: Syntax, sites: &[SyntaxSite]) -> Self {
        if !sites.is_empty() {
            let _ = root
                .markup
                .reconcile_syntaxes(&Markup::new(), &sites, &self.text);
        }
        self.syntax = Some(root);
        self
    }

    pub fn from_language(
        text: Text,
        language: &str,
        languages: &SyntaxLanguages,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) -> Self {
        let byte_count = text.byte_count().min(u32::MAX as usize) as u32;
        let Some((root, _, sites)) =
            languages.parse_syntax(language, &text, 0..byte_count, None, &[], fonts, theme)
        else {
            return Self::new(text, Markup::new());
        };
        let mut markup = Markup::new();
        if language != "markdown" {
            markup.push_styled_covering(0..byte_count, crate::theme::StyleId::SourceCode);
        }
        Self::new(text, markup).with_syntax(root, &sites)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn element_heights(&self, editor: EditorId) -> Vec<(u32, f32)> {
        self.editor(editor).layout.element_heights()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn element_spacers(&self, editor: EditorId) -> Vec<f32> {
        self.editor(editor).layout.element_spacers()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn element_byte_ranges(&self, editor: EditorId) -> Vec<std::ops::Range<u32>> {
        self.editor(editor).layout.element_byte_ranges()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fresh_layout_heights(
        &self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) -> Vec<(u32, f32)> {
        let state = self.editor(editor);
        let extras = Self::view_extras(&self.markups, state);
        let layout = crate::DocumentLayout::build_complete(
            &self.text,
            crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
            state.layout.layout_width(),
            fonts,
            theme,
            state.bounds.and_then(|key| {
                Some(
                    self.fragments
                        .get(&key.set)?
                        .find_by_id(&key.key)?
                        .range
                        .clone(),
                )
            }),
        );
        layout.element_heights()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn markup_styled_ranges(&self, id: MarkupId) -> Vec<std::ops::Range<u32>> {
        use intervals::{IntervalQuery, Order};
        let Some(markup) = self.markups.get(&id) else {
            return Vec::new();
        };
        markup
            .query(0..u32::MAX, Order::Ascending)
            .filter(|entry| matches!(entry.value, crate::markup::Decoration::Styled(_)))
            .map(|entry| entry.range.clone())
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn enrichment_markup(
        &self,
        enricher: crate::enrich::EnricherId,
        editor: Option<EditorId>,
    ) -> Option<MarkupId> {
        self.enrich
            .get(&EnrichKey { enricher, editor })
            .map(|slot| slot.markup)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn editor_shows_markup(&self, editor: EditorId, markup: MarkupId) -> bool {
        self.editors
            .get(&editor)
            .is_some_and(|state| state.markups.contains(&markup))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn probe_state(
        &self,
        editor: EditorId,
    ) -> (
        Option<u32>,
        Option<std::ops::Range<f32>>,
        u32,
        Option<std::ops::Range<u32>>,
    ) {
        let state = self.editor(editor);
        (
            state.layout.repair_pending(),
            state.viewport.clone(),
            state.carets.primary().offset(),
            self.markups
                .get(&state.overlay())
                .and_then(|overlay| overlay.unhide_range()),
        )
    }

    pub fn refresh_unhide(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) -> bool {
        let Some(state) = self.editors.get(&editor) else {
            return false;
        };
        let overlay_id = state.overlay();
        let line = crate::text_cursor::hard_line_range(&self.text, state.carets.primary().offset());
        if self
            .markups
            .get(&overlay_id)
            .and_then(|overlay| overlay.unhide_range())
            == Some(line.clone())
        {
            return false;
        }
        let Some(mut overlay) = self.markups.get(&overlay_id).cloned() else {
            return false;
        };
        let previous = overlay.set_unhide(line.clone());
        self.markups.insert_mut(overlay_id, overlay);
        let changed: Vec<Range<u32>> = std::iter::once(line.clone())
            .chain(previous.clone())
            .collect();
        self.note_markup_change(Some(&changed));
        let Some(state) = self.editors.get_mut(&editor) else {
            return false;
        };

        let mut from = line.start;
        state.layout.mark_modified_in(line);
        if let Some(previous) = previous {
            from = from.min(previous.start);
            state.layout.mark_modified_in(previous);
        }
        let extras = Self::view_extras(&self.markups, state);
        let width = state.layout.layout_width();
        state.layout.repair_layout_bounded(
            &self.text,
            crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
            width,
            fonts,
            theme,
            from,
            state.sync_budget(),
        );
        true
    }

    pub fn syntax(&self) -> Option<&Syntax> {
        self.syntax.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.log.revision()
    }

    pub fn log(&self) -> &EditLog {
        &self.log
    }

    pub fn markup_generation(&self) -> u64 {
        self.markup_generation
    }

    pub fn markup_changed_in(&self, since: u64, range: Range<u32>) -> bool {
        if since == self.markup_generation {
            return false;
        }
        let changes = &self.markup_changes;
        let Some(oldest) = changes.peek() else {
            return true;
        };
        if since + 1 < oldest.generation {
            return true;
        }
        changes
            .iter()
            .filter(|change| change.generation > since)
            .any(|change| match &change.ranges {
                None => true,
                Some(ranges) => ranges
                    .iter()
                    .any(|changed| changed.start < range.end && range.start < changed.end),
            })
    }

    fn note_markup_change(&mut self, ranges: Option<&[Range<u32>]>) {
        self.markup_generation += 1;
        let ranges = ranges
            .filter(|ranges| ranges.len() <= MARKUP_CHANGE_RANGES)
            .map(<[_]>::to_vec);
        while self.markup_changes.len() >= MARKUP_CHANGE_LOG {
            self.markup_changes.dequeue_mut();
        }
        self.markup_changes.enqueue_mut(MarkupChange {
            generation: self.markup_generation,
            ranges,
        });
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn markup(&self) -> &Markup {
        self.syntax
            .as_ref()
            .map(|syntax| &syntax.markup)
            .unwrap_or_else(|| Markup::empty())
    }

    pub fn add_syntax(&mut self, range: std::ops::Range<u32>, syntax: Syntax) -> SyntaxId {
        if self.syntax.is_none() {
            self.syntax = Some(Syntax::new(String::new(), None, Markup::new()));
        }
        let key = self.with_markup_mut(MarkupLayer::Syntax, |markup| {
            markup.add_syntax(range, syntax)
        });
        self.note_markup_change(None);
        key
    }

    fn edit_substance(
        &mut self,
        operation: &Operation,
        identity: crate::EditIdentity,
        provenance: Provenance,
    ) {
        let old_len = self.text.view().byte_count().min(u32::MAX as usize) as u32;
        self.text = self.text.edit(operation);

        if let Some(syntax) = &self.syntax {
            let mut entry = syntax.clone();
            let mut view = self.text.view();
            entry.markup.edit(operation, &mut view, 0);
            entry.folds.edit(crate::markup::interval_steps(operation));
            entry.outline.edit(crate::markup::interval_steps(operation));
            if let Some(tree) = &mut entry.tree {
                tree.edit(operation, &mut view, 0);
            }
            self.syntax = Some(entry);
        }
        let ids: Vec<MarkupId> = self.markups.keys().copied().collect();
        for id in ids {
            if let Some(markup) = self.markups.get(&id) {
                let mut markup = markup.clone();
                markup.edit(operation, &mut self.text.view(), 0);
                self.markups.insert_mut(id, markup);
            }
        }

        let sets: Vec<FragmentSetId> = self.fragments.keys().copied().collect();
        for set in sets {
            if let Some(tree) = self.fragments.get(&set) {
                let mut tree = tree.clone();
                tree.edit(crate::markup::interval_steps(operation));
                self.fragments.insert_mut(set, tree);
            }
        }

        if !self.diffs.is_empty() {
            let pad = old_len.saturating_sub(operation.old_len());
            let padded = match pad {
                0 => None,
                pad => Some(Operation::from_ops(
                    operation.iter().chain(std::iter::once(Op::Retain(pad))),
                )),
            };
            let padded = padded.as_ref().unwrap_or(operation);
            let ids: Vec<crate::diff::DiffId> = self.diffs.keys().copied().collect();
            for id in ids {
                if let Some(diff) = self.diffs.get(&id) {
                    let mut diff = diff.clone();

                    diff.operation = diff.operation.splice_compose(padded);
                    self.diffs.insert_mut(id, diff);
                }
            }
        }
        self.log.record_as(identity, operation, old_len);
        match provenance {
            Provenance::Ours => {
                let composing = self.editors.values().any(|editor| editor.marked.is_some());
                self.undo.note_edit(operation, old_len, composing);
            }

            Provenance::Shared => self.undo.carry_across(operation, old_len),
        }
    }

    pub fn edited_since_parse(&self) -> Vec<Range<u32>> {
        if self.syntax.is_none() {
            return Vec::new();
        }
        self.log.ranges_since(self.parsed_revision)
    }

    pub fn install_syntax(&mut self, mut root: Syntax, sites: &[SyntaxSite]) {
        self.parsed_revision = self.revision();
        let _ = root
            .markup
            .reconcile_syntaxes(self.markup(), sites, &self.text);
        self.syntax = Some(root);
        self.note_markup_change(None);
    }

    pub fn capture_reparse(
        &self,
        parsers: std::sync::Arc<SyntaxLanguages>,
    ) -> Option<ReparseEffect> {
        ReparseWork::capture(self, parsers).map(ReparseEffect::new)
    }

    pub fn launch_reparse(
        &mut self,
        parsers: std::sync::Arc<SyntaxLanguages>,
        fx: &mut EditorEffects<'_>,
    ) {
        if let Some(reparse) = self.capture_reparse(parsers) {
            fx.relaunch(&mut self.reparse_token, reparse);
        }
    }

    pub fn all_inlays_in(&self, range: Range<u32>) -> Vec<crate::markup::InlayInterval<'_>> {
        let extras: Vec<(MarkupId, &Markup)> = self.document_scoped_markups().collect();
        crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras)
            .all_inlays_in(range)
    }

    fn ensure_enrich_slot(&mut self, key: EnrichKey) -> EnrichSlot {
        if let Some(slot) = self.enrich.get(&key) {
            return slot.clone();
        }
        let markup = MarkupId::mint();
        let mut entry = Markup::new();
        if key.editor.is_none() {
            entry.set_scope(crate::markup::MarkupScope::Document);
        }
        self.markups.insert_mut(markup, entry);
        if let Some(editor) = key.editor {
            self.show_markup(editor, markup);
        }
        let slot = EnrichSlot {
            markup,
            token: None,
            launched: 0,
        };
        self.enrich.insert_mut(key, slot.clone());
        slot
    }

    pub fn launch_enrichment(
        &mut self,
        enrichers: &crate::enrich::Enrichers,
        languages: Option<std::sync::Arc<SyntaxLanguages>>,
        base: Option<crate::ResourceLocation>,
        changed: &[Range<u32>],
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(syntax) = self.syntax.clone() else {
            return;
        };
        let byte_count = self.text.byte_count().min(u32::MAX as usize) as u32;
        let anchor = self.editors.keys().next().copied();
        for enricher in enrichers.entries() {
            if !enricher.interest().syntax {
                continue;
            }
            let key = EnrichKey {
                enricher: enricher.id(),
                editor: None,
            };
            let mut slot = self.ensure_enrich_slot(key);
            let changed = match slot.launched {
                0 => vec![0..byte_count],
                _ if changed.is_empty() => continue,
                _ => changed.to_vec(),
            };
            let serial = slot.launched + 1;
            let previous = self.markups.get(&slot.markup).cloned().unwrap_or_default();
            let work = crate::enrich::EnrichWork {
                token: self.token,
                serial,
                markup: slot.markup,
                enricher: enricher.clone(),
                input: crate::enrich::EnrichInput {
                    text: self.text.clone(),
                    syntax: syntax.clone(),
                    revision: self.revision(),
                    changed,
                    previous,
                    base: base.clone(),
                    caret: None,
                },
                anchor,
                editor: None,
                languages: languages.clone(),
            };
            fx.relaunch(&mut slot.token, crate::enrich::EnrichEffect { work });
            slot.launched = serial;
            self.enrich.insert_mut(key, slot);
        }

        if enrichers.entries().iter().any(|e| e.interest().carets) {
            let editors: Vec<EditorId> = self.editors.keys().copied().collect();
            for editor in editors {
                self.launch_caret_enrichment(enrichers, languages.clone(), editor, fx);
            }
        }
    }

    pub fn launch_caret_enrichment(
        &mut self,
        enrichers: &crate::enrich::Enrichers,
        languages: Option<std::sync::Arc<SyntaxLanguages>>,
        editor: EditorId,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(syntax) = self.syntax.clone() else {
            return;
        };
        if !self.editors.contains_key(&editor) {
            return;
        }
        let caret = self.carets(editor).primary();
        for enricher in enrichers.entries() {
            if !enricher.interest().carets {
                continue;
            }
            let key = EnrichKey {
                enricher: enricher.id(),
                editor: Some(editor),
            };
            let mut slot = self.ensure_enrich_slot(key);
            let serial = slot.launched + 1;
            let previous = self.markups.get(&slot.markup).cloned().unwrap_or_default();
            let work = crate::enrich::EnrichWork {
                token: self.token,
                serial,
                markup: slot.markup,
                enricher: enricher.clone(),
                input: crate::enrich::EnrichInput {
                    text: self.text.clone(),
                    syntax: syntax.clone(),
                    revision: self.revision(),
                    changed: Vec::new(),
                    previous,
                    base: None,
                    caret: Some(crate::enrich::CaretContext {
                        selection: caret.selection(),
                        offset: caret.offset(),
                    }),
                },
                anchor: Some(editor),
                editor: Some(editor),
                languages: languages.clone(),
            };
            fx.relaunch(&mut slot.token, crate::enrich::EnrichEffect { work });
            slot.launched = serial;
            self.enrich.insert_mut(key, slot);
        }
    }

    pub fn land_reparse(
        &mut self,
        outcome: ReparseOutcome,
        base: Option<crate::ResourceLocation>,
        store: &Store,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let invalidated = self.apply_reparse_outcome(outcome, fonts, theme, fx);

        let fresh_base = base.is_some() && self.enrich_base != base;
        let ranges = match (invalidated, fresh_base) {
            (_, true) => vec![0..crate::text_cursor::byte_count(&self.text)],
            (Some(invalidated), false) => invalidated,
            (None, false) => return,
        };
        self.enrich_base = base.clone();
        if let Some(enrichers) = crate::env::Enrichers::of(store) {
            let languages = crate::env::Parsers::of(store);
            self.launch_enrichment(&enrichers, languages, base, &ranges, fx);
        }
    }

    pub fn apply_enrichment(
        &mut self,
        outcome: crate::enrich::EnrichOutcome,
        store: &mut Store,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if outcome.token != self.token {
            return;
        }
        let key = EnrichKey {
            enricher: outcome.enricher.id(),
            editor: outcome.editor,
        };
        let Some(slot) = self.enrich.get(&key).cloned() else {
            return;
        };
        if outcome.markup != slot.markup
            || outcome.serial != slot.launched
            || outcome.revision > self.revision()
        {
            return;
        }
        let mut replacement = outcome.replacement;
        let mut changed = outcome.changed;
        if let Some(since) = self.log.compose_since(outcome.revision) {
            for range in &mut changed {
                *range = EditLog::transform_range(range.clone(), &since);
            }
            let mut view = self.text.view();
            replacement.edit(&since, &mut view, 0);
            EditLog::coalesce(&mut changed);
        }

        if let Some(old) = self.markups.get(&slot.markup) {
            old.clone().destroy_inlays_in(&changed, store);
        }
        let Some(live) = self.markups.get(&slot.markup) else {
            return;
        };
        outcome
            .enricher
            .reconcile(&mut replacement, &changed, live, fonts, theme);

        outcome
            .enricher
            .install(store, &mut replacement, &changed, fonts, theme);
        self.replace_markup(slot.markup, replacement, &changed, fonts, theme, fx);
    }

    pub fn release_enrichment(&mut self, store: &mut Store) {
        let entries: Vec<MarkupId> = self.enrich.values().map(|slot| slot.markup).collect();
        for id in entries {
            if let Some(entry) = self.markups.get(&id) {
                entry.clone().destroy_inlays_in(&[0..u32::MAX], store);
            }
        }
    }

    pub fn enrich_now(
        &mut self,
        enrichers: &crate::enrich::Enrichers,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let byte_count = self.text.byte_count().min(u32::MAX as usize) as u32;
        self.enrich_sync(enrichers, &[0..byte_count], fonts, theme);
    }

    pub fn enrich_sync(
        &mut self,
        enrichers: &crate::enrich::Enrichers,
        changed: &[Range<u32>],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        if self.syntax.is_none() || changed.is_empty() {
            return;
        }

        let mut store = Store::new();
        for enricher in enrichers.entries() {
            if !enricher.interest().syntax {
                continue;
            }
            let key = EnrichKey {
                enricher: enricher.id(),
                editor: None,
            };
            let mut slot = self.ensure_enrich_slot(key);
            let serial = slot.launched + 1;
            slot.launched = serial;
            self.enrich.insert_mut(key, slot.clone());
            let work = crate::enrich::EnrichWork {
                token: self.token,
                serial,
                markup: slot.markup,
                enricher: enricher.clone(),
                input: crate::enrich::EnrichInput {
                    text: self.text.clone(),
                    syntax: self.syntax.clone().expect("guarded above"),
                    revision: self.revision(),
                    changed: changed.to_vec(),
                    previous: self.markups.get(&slot.markup).cloned().unwrap_or_default(),

                    base: None,
                    caret: None,
                },
                anchor: None,
                editor: None,

                languages: None,
            };

            let outcome = crate::enrich::now_or_never(Box::pin(crate::enrich::run_work(
                work,
                fonts,
                theme,
                imba::effect::EffectCaller::disconnected(),
            )));
            if let Some(outcome) = outcome {
                let mut batch = imba::effect::Batch::new();
                self.apply_enrichment(outcome, &mut store, fonts, theme, &mut batch.effects());
            }
        }
    }

    fn markup_of(&self, layer: MarkupLayer) -> Option<&Markup> {
        match layer {
            MarkupLayer::Markup(id) => self.markups.get(&id),
            MarkupLayer::Syntax => self.syntax.as_ref().map(|entry| &entry.markup),
        }
    }

    pub fn with_inlay_focus<R>(
        &self,
        arena: &imba::arena::Arena,
        store: &Store,
        ui: &imba::UiCtx,
        key: crate::markup::InlayKey,
        constraints: imba::constraints::Constraints,
        f: impl FnOnce(imba::focus::FocusData<'_, crate::markup::InlayCommand>) -> R,
    ) -> Option<R> {
        self.markup_of(key.layer)
            .and_then(|markup| markup.with_inlay_focus(arena, store, ui, key.key, constraints, f))
    }

    fn with_markup_mut<R>(&mut self, layer: MarkupLayer, f: impl FnOnce(&mut Markup) -> R) -> R {
        match layer {
            MarkupLayer::Markup(id) => {
                let mut markup = self
                    .markups
                    .get(&id)
                    .cloned()
                    .expect("a feature-layer key names a registered markup");
                let result = f(&mut markup);
                self.markups.insert_mut(id, markup);
                result
            }
            MarkupLayer::Syntax => {
                let mut entry = self
                    .syntax
                    .clone()
                    .expect("syntax-layer access needs a parse");
                let result = f(&mut entry.markup);
                self.syntax = Some(entry);
                result
            }
        }
    }

    fn splice_reparse(&mut self, outcome: ReparseOutcome) -> Option<Vec<Range<u32>>> {
        let (token, revision, mut invalidated, mut root, descended) = outcome.into_parts();

        if token != self.token {
            return None;
        }
        if revision < self.parsed_revision
            || revision > self.revision()
            || (invalidated.is_empty() && !descended)
        {
            return None;
        }

        if let Some(since) = self.log.compose_since(revision) {
            for range in &mut invalidated {
                *range = EditLog::transform_range(range.clone(), &since);
            }
            let mut view = self.text.view();
            root.markup.edit(&since, &mut view, 0);
            root.folds.edit(crate::markup::interval_steps(&since));
            root.outline.edit(crate::markup::interval_steps(&since));
            if let Some(tree) = &mut root.tree {
                tree.edit(&since, &mut view, 0);
            }
            EditLog::coalesce(&mut invalidated);
        }

        root.markup.carry_live_views_in(self.markup(), &invalidated);
        self.syntax = Some(root);
        self.parsed_revision = revision;
        self.note_markup_change(None);
        Some(invalidated)
    }

    pub fn foldables_in(&self, span: std::ops::Range<u32>) -> Vec<Range<u32>> {
        let mut out = Vec::new();
        self.foldables_into(span, &mut out);
        out
    }

    pub fn foldables_into(&self, span: std::ops::Range<u32>, out: &mut Vec<Range<u32>>) {
        use intervals::{IntervalQuery, Order};
        let Some(syntax) = &self.syntax else {
            return;
        };
        for entry in syntax.folds.query(span.clone(), Order::Ascending) {
            if span.start <= entry.range.start && entry.range.start < span.end {
                out.push(entry.range.clone());
            }
        }
        syntax.markup.collect_channel_folds_in(&span, out);
        out.sort_by_key(|range| (range.start, range.end));
    }

    pub fn has_outline(&self) -> bool {
        self.syntax
            .as_ref()
            .is_some_and(|syntax| !syntax.outline.is_empty() || syntax.markup.has_outline())
    }

    pub fn outline_items(
        &self,
    ) -> Vec<(
        SyntaxId,
        crate::markup::IntervalId,
        Range<u32>,
        crate::markup::OutlineItem,
    )> {
        use intervals::{IntervalQuery, Order};
        let Some(syntax) = &self.syntax else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in syntax.outline.query(0..u32::MAX, Order::Ascending) {
            out.push((
                SyntaxId::DOCUMENT,
                *entry.key,
                entry.range.clone(),
                entry.value.clone(),
            ));
        }
        syntax.markup.collect_outline_in(0, &mut out);
        out.sort_by_key(|(_, _, range, _)| (range.start, std::cmp::Reverse(range.end)));
        out
    }

    pub fn resolve_outline(
        &self,
        syntax: SyntaxId,
        key: crate::markup::IntervalId,
    ) -> Option<Range<u32>> {
        let root = self.syntax.as_ref()?;
        if syntax == SyntaxId::DOCUMENT {
            let found = root.outline.find_by_id(&key)?;
            return Some(found.range.clone());
        }
        root.markup.resolve_outline(syntax, key)
    }

    pub fn outline_enclosing(&self, offset: u32) -> Vec<Range<u32>> {
        use intervals::{IntervalQuery, Order};
        let Some(syntax) = &self.syntax else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in syntax
            .outline
            .query(offset..offset.saturating_add(1), Order::Ascending)
        {
            out.push(entry.range.clone());
        }
        syntax.markup.outline_enclosing_from(0, offset, &mut out);
        out.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        out
    }

    pub fn add_fragment_set(&mut self) -> FragmentSetId {
        let id = FragmentSetId::mint();
        self.fragments.insert_mut(id, intervals::Intervals::new());
        id
    }

    pub fn remove_fragment_set(&mut self, set: FragmentSetId) {
        self.fragments.remove_mut(&set);
    }

    pub fn add_fragment(&mut self, set: FragmentSetId, range: Range<u32>) -> FragmentKey {
        let Some(tree) = self.fragments.get(&set) else {
            debug_assert!(false, "add_fragment into a set that was never allocated");
            return FragmentKey {
                set,
                key: crate::markup::IntervalId(0),
            };
        };
        use intervals::IntervalQuery as _;
        let mut tree = tree.clone();
        let next = tree
            .query(0..u32::MAX, intervals::Order::Ascending)
            .map(|entry| entry.key.0.saturating_add(1))
            .max()
            .unwrap_or(0);
        let key = crate::markup::IntervalId(next);
        tree.insert([intervals::Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: (),
        }]);
        self.fragments.insert_mut(set, tree);
        FragmentKey { set, key }
    }

    pub fn fragment_range(&self, key: FragmentKey) -> Option<Range<u32>> {
        self.fragments
            .get(&key.set)?
            .find_by_id(&key.key)
            .map(|entry| entry.range.clone())
    }

    pub fn remove_fragments(&mut self, keys: impl IntoIterator<Item = FragmentKey>) {
        for fragment in keys {
            if let Some(tree) = self.fragments.get(&fragment.set) {
                let mut tree = tree.clone();
                tree.remove([&fragment.key]);
                self.fragments.insert_mut(fragment.set, tree);
            }
        }
    }

    fn reshape_markup_change(
        &mut self,
        id: MarkupId,
        changed: &[Range<u32>],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let byte_count = crate::text_cursor::byte_count(&self.text);

        let everywhere = self
            .markups
            .get(&id)
            .is_some_and(|markup| markup.scope() == crate::markup::MarkupScope::Document);
        for editor in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.editors.get_mut(&editor) else {
                continue;
            };
            if !everywhere && !state.markups.contains(&id) {
                continue;
            }
            for range in changed {
                let range = range.start.min(byte_count)..range.end.min(byte_count);
                if range.start < range.end {
                    state.layout.mark_modified_in(range);
                }
            }
        }
        self.note_markup_change(Some(changed));
        self.repair_damaged_viewports(fonts, theme, fx)
    }

    pub fn ensure_document_markup(&mut self, id: MarkupId) {
        let mut markup = self.markups.get(&id).cloned().unwrap_or_else(Markup::new);
        markup.set_scope(crate::markup::MarkupScope::Document);
        self.markups.insert_mut(id, markup);
    }

    pub fn push_inlay(
        &mut self,
        markup_id: MarkupId,
        range: Range<u32>,
        inlay: Inlay,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) -> InlayKey {
        self.note_markup_change(None);
        let span = crate::markup::inlay_repair_span(inlay.mode(), &range);
        let key = self.with_markup_mut(MarkupLayer::Markup(markup_id), |markup| InlayKey {
            layer: MarkupLayer::Markup(markup_id),
            key: markup.push_inlay(range, inlay),
        });
        self.repair_editors(span, fonts, theme, fx);
        key
    }

    pub fn remove_inlay(
        &mut self,
        key: InlayKey,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let MarkupLayer::Markup(markup_id) = key.layer else {
            return;
        };
        let Some((range, mode)) = self
            .markups
            .get(&markup_id)
            .and_then(|markup| markup.inlay_interval(key.key))
        else {
            return;
        };
        self.with_markup_mut(key.layer, |markup| markup.remove_keys([key.key]));
        self.note_markup_change(None);
        for editor in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.editors.get_mut(&editor) else {
                continue;
            };
            if state.focus == EditorFocus::Inlay(key) {
                state.focus = EditorFocus::Text;
            }
        }
        self.repair_editors(
            crate::markup::inlay_repair_span(mode, &range),
            fonts,
            theme,
            fx,
        );
    }

    fn perform_inlay_raw(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        key: InlayKey,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> (Option<(Range<u32>, InlayMode)>, Option<Operation>) {
        if let MarkupLayer::Markup(id) = key.layer {
            if !self.markups.contains_key(&id) {
                return (None, None);
            }
        }

        let (performed, edit) = self.with_markup_mut(key.layer, |markup| {
            markup.perform_inlay(store, ui, key.key, command, fx)
        });
        if performed.is_some() {
            self.note_markup_change(None);
        }
        (performed, edit)
    }

    fn view_extras<'a>(
        markups: &'a rpds::HashTrieMapSync<MarkupId, Markup>,
        editor: &Editor,
    ) -> Vec<(MarkupId, &'a Markup)> {
        editor
            .markups
            .iter()
            .filter_map(|id| markups.get(id).map(|markup| (*id, markup)))
            .chain(markups.iter().filter_map(|(id, markup)| {
                (markup.scope() == crate::markup::MarkupScope::Document
                    && !editor.markups.contains(id))
                .then_some((*id, markup))
            }))
            .collect()
    }

    pub fn add_editor(
        &mut self,
        width: f32,
        bounds: Option<FragmentKey>,
        build: EditorBuild,
        shown: &[MarkupId],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) -> EditorId {
        let complete = matches!(build, EditorBuild::Complete);
        let id = EditorId::fresh();

        let overlay = self.add_markup();
        let markups = std::iter::once(overlay)
            .chain(shown.iter().copied())
            .collect();
        let editor = Editor::new(
            self,
            markups,
            vec![overlay],
            width,
            fonts,
            theme,
            bounds,
            build,
        );
        self.editors.insert_mut(id, editor);
        self.refresh_unhide(id, fonts, theme);

        if !complete {
            self.pending_repairs(fx);
        }
        id
    }

    pub fn manage_repairs_in_pair(&mut self, editor: EditorId) {
        if let Some(state) = self.editors.get_mut(&editor) {
            state.pair_managed = true;
        }
    }

    pub fn remove_editor(&mut self, editor: EditorId) {
        if let Some(owned) = self
            .editors
            .get(&editor)
            .map(|state| state.owned_markups.clone())
        {
            self.editors.remove_mut(&editor);
            for id in owned {
                self.markups.remove_mut(&id);
            }

            let stale: Vec<EnrichKey> = self
                .enrich
                .keys()
                .filter(|key| key.editor == Some(editor))
                .copied()
                .collect();
            for key in stale {
                if let Some(slot) = self.enrich.get(&key) {
                    let markup = slot.markup;
                    self.markups.remove_mut(&markup);
                }
                self.enrich.remove_mut(&key);
            }
        }
    }

    pub fn has_editor(&self, editor: EditorId) -> bool {
        self.editors.contains_key(&editor)
    }

    pub fn editor_ids(&self) -> impl Iterator<Item = EditorId> + '_ {
        self.editors.keys().copied()
    }

    pub(crate) fn editor(&self, editor: EditorId) -> &Editor {
        self.editors
            .get(&editor)
            .expect("rendering binds an existing editor")
    }

    pub fn perform(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        editor: EditorId,
        command: EditorCommand,
        fx: &mut EditorEffects<'_>,
    ) {
        let base_revision = self.revision();

        let history = matches!(command, EditorCommand::Undo | EditorCommand::Redo);
        let carets_before = self.carets(editor);
        self.perform_command(store, ui, editor, command, fx);
        let carets_after = self.carets(editor);
        if !history && self.revision() != base_revision {
            self.undo.stamp(editor, &carets_before, &carets_after);
        }

        if carets_after != carets_before {
            if let Some(enrichers) = crate::env::Enrichers::of(store) {
                let languages = crate::env::Parsers::of(store);
                self.launch_caret_enrichment(&enrichers, languages, editor, fx);
            }
        }
    }

    fn perform_command(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        editor: EditorId,
        command: EditorCommand,
        fx: &mut EditorEffects<'_>,
    ) {
        let fonts = crate::env::ui_collection(store, ui);
        let theme = &crate::env::Themes::of(store);
        let revision_before = self.revision();

        let arms_reveal = matches!(
            command,
            EditorCommand::InsertText { .. }
                | EditorCommand::Enter { .. }
                | EditorCommand::Indent
                | EditorCommand::Outdent
                | EditorCommand::Backspace
                | EditorCommand::DeleteForward
                | EditorCommand::DeleteWordBack
                | EditorCommand::DeleteWordForward
                | EditorCommand::DeleteSelections
                | EditorCommand::Paste { .. }
                | EditorCommand::Undo
                | EditorCommand::Redo
                | EditorCommand::Move { .. }
                | EditorCommand::AddCaretAbove
                | EditorCommand::AddCaretBelow
                | EditorCommand::SelectNextOccurrence
                | EditorCommand::SelectAllOccurrences
        );
        match command {
            EditorCommand::InsertText { text } => {
                let typed = crate::assist::single_typed_char(&text)
                    .filter(|_| self.marked_of(editor).is_none());
                match typed {
                    Some(ch)
                        if self.assist_at_carets(
                            store,
                            editor,
                            crate::reparse::AssistKind::Typed(ch),
                            &fonts,
                            theme,
                            fx,
                        ) => {}
                    _ => self.insert(editor, &text, &fonts, theme, fx),
                }
            }
            EditorCommand::Enter { soft } => {
                let kind = crate::reparse::AssistKind::Enter { soft };
                if self.marked_of(editor).is_some()
                    || !self.assist_at_carets(store, editor, kind, &fonts, theme, fx)
                {
                    self.insert(editor, "\n", &fonts, theme, fx);
                }
            }
            EditorCommand::Indent => {
                let kind = crate::reparse::AssistKind::Indent;
                if self.marked_of(editor).is_none()
                    && !self.assist_at_carets(store, editor, kind, &fonts, theme, fx)
                {
                    self.insert(editor, crate::assist::INDENT_UNIT, &fonts, theme, fx);
                }
            }
            EditorCommand::Outdent => {
                let kind = crate::reparse::AssistKind::Outdent;
                if self.marked_of(editor).is_none() {
                    let _ = self.assist_at_carets(store, editor, kind, &fonts, theme, fx);
                }
            }
            EditorCommand::Backspace => {
                self.delete_at_carets(editor, Motion::Left, &fonts, theme, fx)
            }
            EditorCommand::DeleteForward => {
                self.delete_at_carets(editor, Motion::Right, &fonts, theme, fx)
            }
            EditorCommand::DeleteWordBack => {
                self.delete_at_carets(editor, Motion::WordLeft, &fonts, theme, fx)
            }
            EditorCommand::DeleteWordForward => {
                self.delete_at_carets(editor, Motion::WordRight, &fonts, theme, fx)
            }
            EditorCommand::DeleteSelections => self.delete_selections(editor, &fonts, theme, fx),
            EditorCommand::Undo => self.undo(editor, &fonts, theme, fx),
            EditorCommand::Redo => self.redo(editor, &fonts, theme, fx),
            EditorCommand::Paste { text } => self.insert(editor, &text, &fonts, theme, fx),
            EditorCommand::Move { motion, select } => {
                self.move_carets(editor, motion, select, &fonts, theme);
            }
            EditorCommand::SelectAll => self.select_all(editor),
            EditorCommand::CollapseCarets => self.collapse_carets(editor),
            EditorCommand::AddCaretAbove => self.add_caret_vertically(editor, true, &fonts, theme),
            EditorCommand::AddCaretBelow => self.add_caret_vertically(editor, false, &fonts, theme),
            EditorCommand::SelectNextOccurrence => self.select_next_occurrence(editor),
            EditorCommand::SelectAllOccurrences => self.select_all_occurrences(editor),
            EditorCommand::RevealSettled => {
                if let Some(state) = self.editors.get_mut(&editor) {
                    state.reveal = false;
                }
            }

            EditorCommand::RevealAt { byte } => self.reveal_at(editor, byte, &fonts, theme, fx),
            EditorCommand::Click { point, kind } => {
                self.click_carets(editor, point, kind, &fonts, theme);

                self.set_focus(editor, EditorFocus::Text);
            }
            EditorCommand::Drag { point } => self.drag_carets(editor, point, &fonts, theme),
            EditorCommand::DragEnd => {
                if let Some(state) = self.editors.get_mut(&editor) {
                    state.drag = None;
                }
            }
            EditorCommand::ToggleFold { range } => {
                self.toggle_fold(editor, range, &fonts, theme, fx)
            }
            EditorCommand::ToggleBeforeInlay { .. } => {}
            EditorCommand::Inlay { key, command } => {
                if matches!(
                    command.downcast_ref::<crate::fold::FoldCommand>(),
                    Some(crate::fold::FoldCommand::Unfold)
                ) {
                    self.set_fold_departure(key, true, &fonts, theme, fx);
                } else {
                    let fold_tick = matches!(
                        command.downcast_ref::<crate::fold::FoldCommand>(),
                        Some(crate::fold::FoldCommand::Tick(_))
                    );
                    let passive = fold_tick
                        || matches!(
                            command.downcast_ref::<crate::before_inlay::BeforeCommand>(),
                            Some(crate::before_inlay::BeforeCommand::Tick(_))
                        )
                        || self.markup_of(key.layer).is_some_and(|markup| {
                            matches!(
                                markup.inlay_interval(key.key),
                                Some((_, crate::markup::InlayMode::Popup(_)))
                            ) || markup.inlay_passive(key.key, &command)
                        });
                    self.perform_inlay(store, ui, editor, key, command, !passive, fx);
                    if fold_tick
                        && self
                            .fold_chip_at(key)
                            .is_some_and(crate::fold::FoldChip::departed)
                    {
                        self.remove_inlay(key, &fonts, theme, fx);
                    }
                }
            }

            EditorCommand::ApplyRepair(repaired) => {
                let mut moved = false;
                for item in repaired {
                    moved |= self.apply_repair_anchored(item);
                }
                if moved {
                    fx.settle();
                }

                self.pending_repairs(fx)
            }
            EditorCommand::ViewportTop(top) => {
                self.note_viewport_top(editor, top);
            }
            EditorCommand::Viewport {
                width,
                top,
                bottom,
                anchor,
            } => {
                if let Some(state) = self.editors.get_mut(&editor) {
                    state.viewport = Some(top..bottom);
                    // A fresh report means a frame painted at the
                    // settled position — the correction has landed.
                    state.settle_to = None;
                }
                if self.resize(editor, width, anchor, &fonts, theme, fx) {
                    return;
                }

                self.repair_visible_damage(editor, &fonts, theme, fx)
            }
            EditorCommand::ToggleSoftwrap => {
                let Some(state) = self.editors.get_mut(&editor) else {
                    return;
                };
                state.softwrap = !state.softwrap;
                state.scroll_x = 0.0;
                let anchor = state
                    .viewport
                    .clone()
                    .map_or(0, |viewport| state.layout.byte_at_y(viewport.start));
                let target = state.target_width;
                if target > 0.0 {
                    self.resize(editor, target, anchor, &fonts, theme, fx);
                }
            }
            EditorCommand::HorizontalScroll(delta) => {
                let Some(state) = self.editors.get_mut(&editor) else {
                    return;
                };
                let extent = (state.layout.max_width() - state.target_width).max(0.0);
                state.scroll_x = (state.scroll_x + delta).clamp(0.0, extent);
            }
            EditorCommand::Retheme {
                top,
                bottom,
                anchor,
            } => {
                if let Some(state) = self.editors.get_mut(&editor) {
                    state.viewport = Some(top..bottom);
                }
                self.retheme_editor(editor, anchor, &fonts, theme, fx)
            }
            EditorCommand::ApplyReparse(outcome) => {
                self.land_reparse(outcome, None, store, &fonts, theme, fx);
            }
            EditorCommand::ApplyEnrichment(outcome) => {
                self.apply_enrichment(outcome, store, &fonts, theme, fx)
            }
            EditorCommand::ApplyScrollStripes(outcome) => self.apply_scroll_stripes(outcome),
            EditorCommand::InsertTextReplacing { text, replacement } => {
                let mut view = self.text.view();
                let repl = view.utf16_to_byte(replacement.0)
                    ..view.utf16_to_byte(replacement.0.saturating_add(replacement.1));
                let end = text.len().min(u32::MAX as usize) as u32;
                self.set_marked_text(editor, &text, end..end, Some(repl), &fonts, theme, fx);
                self.unmark_text(editor);
            }
            EditorCommand::SetMarkedText {
                text,
                selected,
                replacement,
            } => {
                let sel = utf16_to_byte_in(&text, selected.0)
                    ..utf16_to_byte_in(&text, selected.0.saturating_add(selected.1));
                let repl = replacement.map(|(start, len)| {
                    let mut view = self.text.view();
                    view.utf16_to_byte(start)..view.utf16_to_byte(start.saturating_add(len))
                });
                self.set_marked_text(editor, &text, sel, repl, &fonts, theme, fx)
            }
            EditorCommand::UnmarkText => {
                self.unmark_text(editor);
            }

            EditorCommand::SetSelectionUtf16 { start, end } => {
                let range = {
                    let mut view = self.text.view();
                    view.utf16_to_byte(start)..view.utf16_to_byte(end)
                };
                self.set_carets(
                    editor,
                    crate::caret::MultiCaret::one(crate::caret::Caret::selecting(
                        range.start,
                        range.end,
                    )),
                );
                if self.refresh_unhide(editor, &fonts, theme) {
                    self.pending_repairs(fx);
                }
            }

            EditorCommand::Dynamic { .. } => {}

            EditorCommand::Hover(_) => {}
        };
        if arms_reveal {
            if let Some(state) = self.editors.get_mut(&editor) {
                state.reveal = true;
            }
        }

        if self.refresh_unhide(editor, &fonts, theme) {
            self.pending_repairs(fx);
        }

        if self.revision() != revision_before {
            if let Some(parsers) = crate::env::Parsers::of(store) {
                if let Some(work) = ReparseWork::capture(self, parsers) {
                    fx.relaunch(&mut self.reparse_token, ReparseEffect::new(work));
                }
            }
        }
    }

    pub fn insert(
        &mut self,
        editor: EditorId,
        text: &str,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if text.is_empty() {
            return;
        }

        if let Some(range) = self.marked_of(editor) {
            return self.replace_marked(editor, fonts, theme, range, text, None, fx);
        }

        self.insert_at_carets(editor, text, fonts, theme, fx)
    }

    pub fn set_marked_text(
        &mut self,
        editor: EditorId,
        text: &str,
        selected: Range<u32>,
        replacement: Option<Range<u32>>,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let range = replacement
            .or_else(|| self.marked_of(editor))
            .or_else(|| self.clamped_caret(editor).map(|c| c..c));
        let Some(range) = range else {
            return;
        };
        self.replace_marked(editor, fonts, theme, range, text, Some(selected), fx)
    }

    pub fn unmark_text(&mut self, editor: EditorId) {
        if let Some(editor) = self.editors.get_mut(&editor) {
            editor.marked = None;
        }
    }

    pub fn marked_range(&self, editor: EditorId) -> Option<Range<u32>> {
        self.marked_of(editor)
    }

    fn marked_of(&self, editor: EditorId) -> Option<Range<u32>> {
        self.editors.get(&editor)?.marked.clone()
    }

    fn replace_marked(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        range: Range<u32>,
        text: &str,
        selected: Option<Range<u32>>,
        fx: &mut EditorEffects<'_>,
    ) {
        let byte_count = crate::text_cursor::byte_count(&self.text);
        let start = range.start.min(byte_count);
        let end = range.end.min(byte_count).max(start);
        let deleted = self.text.view().byte_string(start as usize, end as usize);

        let mut ops = Vec::with_capacity(3);
        if start > 0 {
            ops.push(Op::Retain(start));
        }
        if !deleted.is_empty() {
            ops.push(Op::Delete(deleted));
        }
        if !text.is_empty() {
            ops.push(Op::Insert(text.to_owned()));
        }
        let text_len = text.len().min(u32::MAX as usize) as u32;
        let caret_after = match &selected {
            Some(selected) => start.saturating_add(selected.start.min(text_len)),
            None => start.saturating_add(text_len),
        };

        self.edit(&Operation::from_ops(ops), fonts, theme, fx);

        self.set_caret(editor, caret_after);

        if let Some(editor) = self.editors.get_mut(&editor) {
            editor.marked = selected.map(|_| start..start.saturating_add(text_len));
        }
    }

    pub fn undo(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if self.marked_of(editor).is_some() {
            return;
        }
        let Some(entry) = self.undo.take_undo() else {
            return;
        };
        let inverse = entry.operation.invert();
        self.undo.replaying = true;
        self.edit(&inverse, fonts, theme, fx);
        self.undo.replaying = false;
        self.restore_snapshot(editor, &entry.snapshot, true);
        self.undo.park_redo(entry);
    }

    pub fn redo(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if self.marked_of(editor).is_some() {
            return;
        }
        let Some(entry) = self.undo.take_redo() else {
            return;
        };
        self.undo.replaying = true;
        self.edit(&entry.operation.clone(), fonts, theme, fx);
        self.undo.replaying = false;
        self.restore_snapshot(editor, &entry.snapshot, false);
        self.undo.restore_undo(entry);
    }

    fn restore_snapshot(
        &mut self,
        invoking: EditorId,
        snapshot: &Option<crate::undo::CaretSnapshot>,
        undoing: bool,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        let carets = match undoing {
            true => snapshot.before.clone(),
            false => snapshot.after.clone(),
        };
        let target = match self.editors.contains_key(&snapshot.editor) {
            true => snapshot.editor,
            false => invoking,
        };
        self.set_carets(target, carets);
    }

    pub fn clear_undo_history(&mut self) {
        self.undo.reset();
    }

    pub fn edit(
        &mut self,
        operation: &Operation,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.edit_as(crate::EditIdentity::mint(), operation, fonts, theme, fx)
    }

    pub fn edit_as(
        &mut self,
        identity: crate::EditIdentity,
        operation: &Operation,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.edit_with(identity, Provenance::Ours, operation, fonts, theme, fx)
    }

    pub fn edit_shared(
        &mut self,
        identity: crate::EditIdentity,
        operation: &Operation,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.edit_with(identity, Provenance::Shared, operation, fonts, theme, fx)
    }

    fn edit_with(
        &mut self,
        identity: crate::EditIdentity,
        provenance: Provenance,
        operation: &Operation,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.edit_substance(operation, identity, provenance);
        let byte_count = crate::text_cursor::byte_count(&self.text);
        let collection = fonts.clone();

        for id in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(editor) = self.editors.get_mut(&id) else {
                continue;
            };
            // The viewport anchor, from RETAINED state (the widget's
            // Viewport report): captured against the pre-edit layout,
            // resolved after the bounded repair below.
            let door = editor
                .viewport
                .as_ref()
                .map(|viewport| viewport.start)
                .and_then(|top| {
                    (top > 0.5).then(|| {
                        let byte = editor.layout.byte_at_y(top);
                        (byte, top - editor.layout.height_before(byte), top)
                    })
                });
            let repair_start = editor.layout.edit(&operation);
            editor.carets = editor
                .carets
                .transformed(&operation)
                .clamped(&(0..byte_count));

            editor.marked = editor
                .marked
                .take()
                .map(|range| EditLog::transform_range(range, &operation));

            if let Some(key) = editor.bounds {
                let window = self
                    .fragments
                    .get(&key.set)
                    .and_then(|tree| tree.find_by_id(&key.key))
                    .map(|entry| entry.range.clone())
                    .unwrap_or(0..0);
                editor.layout.set_window(Some(window));
            }
            let extras = Self::view_extras(&self.markups, editor);
            let width = editor.layout.layout_width();
            editor.layout.repair_layout_bounded(
                &self.text,
                crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
                width,
                &collection,
                theme,
                repair_start,
                editor.sync_budget(),
            );
            if let Some((byte, dy, was)) = door {
                let byte = EditLog::transform_range(byte..byte, &operation)
                    .start
                    .min(byte_count);
                let fresh = editor.layout.height_before(byte) + dy;
                if (fresh - was).abs() > 0.5 {
                    editor.settle_to = Some(fresh);
                    fx.settle();
                }
            }
        }

        self.pending_repairs(fx)
    }

    fn clamped_carets(&self, editor: EditorId) -> Option<crate::caret::MultiCaret> {
        let state = self.editors.get(&editor)?;
        let window = self.editor_window(state);
        Some(state.carets.clamped(&window))
    }

    fn clamped_caret(&self, editor: EditorId) -> Option<u32> {
        Some(self.clamped_carets(editor)?.primary().offset())
    }

    fn editor_window(&self, state: &Editor) -> Range<u32> {
        let byte_count = crate::text_cursor::byte_count(&self.text);
        match state.bounds {
            Some(key) => self
                .fragment_range(key)
                .map(|range| range.start.min(byte_count)..range.end.min(byte_count))
                .unwrap_or(0..0),
            None => 0..byte_count,
        }
    }

    pub fn caret_byte(&self, editor: EditorId) -> u32 {
        self.editors
            .get(&editor)
            .map_or(0, |editor| editor.carets.primary().offset())
    }

    pub fn reveal_pending(&self, editor: EditorId) -> bool {
        self.editors.get(&editor).is_some_and(|state| state.reveal)
    }

    pub fn reveal_at(
        &mut self,
        editor: EditorId,
        byte: u32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.set_caret(editor, byte);
        if let Some(state) = self.editors.get_mut(&editor) {
            state.reveal = true;
        }
        if self.refresh_unhide(editor, fonts, theme) {
            self.pending_repairs(fx);
        }
    }

    pub fn reveal_selecting(
        &mut self,
        editor: EditorId,
        range: Range<u32>,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.set_carets(
            editor,
            crate::caret::MultiCaret::one(crate::caret::Caret::selecting(range.start, range.end)),
        );
        if let Some(state) = self.editors.get_mut(&editor) {
            state.reveal = true;
        }
        if self.refresh_unhide(editor, fonts, theme) {
            self.pending_repairs(fx);
        }
    }

    pub fn cancel_reveal(&mut self, editor: EditorId) {
        if let Some(state) = self.editors.get_mut(&editor) {
            state.reveal = false;
        }
    }

    pub fn viewport(&self, editor: EditorId) -> Option<std::ops::Range<f32>> {
        self.editor(editor).viewport.clone()
    }

    pub fn extras_vec(&self, editor: EditorId) -> Vec<&Markup> {
        Self::view_extras(&self.markups, self.editor(editor))
            .into_iter()
            .map(|(_, markup)| markup)
            .collect()
    }

    pub fn popups_in(
        &self,
        editor: EditorId,
        range: std::ops::Range<u32>,
    ) -> Vec<(
        crate::markup::InlayKey,
        std::ops::Range<u32>,
        crate::markup::Inlay,
        crate::markup::PopupSpec,
    )> {
        let extras = self.extras_keyed(editor);
        let markups = crate::markup::OverlaidMarkup::new(self.markup(), &extras);
        if !markups.has_popups() {
            return Vec::new();
        }
        markups
            .all_inlays_in(range)
            .into_iter()
            .filter_map(|interval| match interval.inlay.mode {
                crate::markup::InlayMode::Popup(spec) => Some((
                    interval.key,
                    interval.range.clone(),
                    interval.inlay.clone(),
                    spec,
                )),
                _ => None,
            })
            .collect()
    }

    pub fn has_popups(&self, editor: EditorId) -> bool {
        self.markup().has_popups()
            || self
                .extras_keyed(editor)
                .iter()
                .any(|(_, markup)| markup.has_popups())
    }

    pub fn extras_keyed(&self, editor: EditorId) -> Vec<(MarkupId, &Markup)> {
        Self::view_extras(&self.markups, self.editor(editor))
    }

    pub(crate) fn markup_refs(&self, ids: &[MarkupId]) -> Vec<(MarkupId, &Markup)> {
        ids.iter()
            .filter_map(|id| self.markups.get(id).map(|markup| (*id, markup)))
            .collect()
    }

    pub fn set_caret(&mut self, editor: EditorId, byte: u32) {
        self.set_carets(editor, crate::caret::MultiCaret::single(byte));
    }

    pub fn focus(&self, editor: EditorId) -> EditorFocus {
        self.editors
            .get(&editor)
            .map_or(EditorFocus::None, |editor| editor.focus)
    }

    pub fn set_focus(&mut self, editor: EditorId, focus: EditorFocus) {
        if let Some(editor) = self.editors.get_mut(&editor) {
            editor.focus = focus;
        }
    }

    pub fn set_placeholder(
        &mut self,
        editor: EditorId,
        placeholder: impl Into<Arc<str>>,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let text = placeholder.into();
        let Some(width) = self
            .editors
            .get(&editor)
            .map(|editor| editor.layout.layout_width())
        else {
            return;
        };
        let height = crate::shaped_line::ShapedLine::placeholder(
            &text,
            theme.ui().peeker.dim_text.0,
            fonts,
            theme,
            width,
            0.0,
        )
        .layout_height(theme);
        if let Some(editor) = self.editors.get_mut(&editor) {
            editor.placeholder = Some(EditorPlaceholder { text, height });
        }
    }

    pub fn layout_width(&self, editor: EditorId) -> f32 {
        self.editors
            .get(&editor)
            .map_or(0.0, |editor| editor.layout.layout_width())
    }

    pub fn softwrap(&self, editor: EditorId) -> bool {
        self.editors.get(&editor).is_none_or(|state| state.softwrap)
    }

    pub fn reported_width(&self, editor: EditorId) -> f32 {
        self.editors
            .get(&editor)
            .map_or(0.0, |state| state.target_width)
    }

    pub fn max_width(&self, editor: EditorId) -> f32 {
        self.editors
            .get(&editor)
            .map_or(0.0, |state| state.layout.max_width())
    }

    pub fn scroll_x(&self, editor: EditorId) -> f32 {
        self.editors
            .get(&editor)
            .map_or(0.0, |state| state.scroll_x)
    }

    pub fn repairs_pending(&self, editor: EditorId) -> bool {
        self.editors
            .get(&editor)
            .is_some_and(|state| state.layout.repair_pending().is_some())
    }

    pub fn content_height(&self, editor: EditorId) -> f32 {
        self.editors.get(&editor).map_or(0.0, |editor| {
            let laid = editor.layout.height();
            if self.text.byte_count() == 0 {
                laid.max(
                    editor
                        .placeholder
                        .as_ref()
                        .map_or(0.0, |placeholder| placeholder.height),
                )
            } else {
                laid
            }
        })
    }

    pub fn visible_damage(&self, editor: EditorId, top: f32, bottom: f32) -> bool {
        self.editors.get(&editor).is_some_and(|editor| {
            let visible =
                editor.layout.byte_at_y(top)..editor.layout.byte_at_y(bottom).saturating_add(1);
            editor.layout.damage_intersects(visible)
        })
    }

    pub fn visible_byte_band(
        &self,
        editor: EditorId,
        top: f32,
        bottom: f32,
    ) -> std::ops::Range<u32> {
        self.editors
            .get(&editor)
            .map_or(0..0, |editor| editor.layout.byte_band(top, bottom))
    }

    pub fn first_visible_byte(&self, editor: EditorId, y: f32) -> u32 {
        self.editors
            .get(&editor)
            .map_or(0, |editor| editor.layout.byte_at_y(y))
    }

    pub fn height_before(&self, editor: EditorId, byte: u32) -> f32 {
        self.editors
            .get(&editor)
            .map_or(0.0, |editor| editor.layout.height_before(byte))
    }

    /// Where the settle pulse should re-aim this editor's viewport,
    /// if a height mutation above it left a correction pending
    /// (docs/viewport-preservation.md §3).
    pub fn settle_target(&self, editor: EditorId) -> Option<f32> {
        self.editors.get(&editor)?.settle_to
    }

    /// A traversal re-observed the viewport top: keep the retained
    /// viewport honest (the full paint report stays throttled) and
    /// drop any pending correction — the observed move supersedes it
    /// (docs/viewport-preservation.md §3.1).
    pub fn note_viewport_top(&mut self, editor: EditorId, top: f32) {
        let Some(state) = self.editors.get_mut(&editor) else {
            return;
        };
        let height = state
            .viewport
            .as_ref()
            .map(|viewport| viewport.end - viewport.start)
            .unwrap_or(0.0);
        state.viewport = Some(top..top + height);
        state.settle_to = None;
    }

    pub fn document_layout(&self, editor: EditorId) -> Option<&crate::DocumentLayout> {
        self.editors.get(&editor).map(|editor| &editor.layout)
    }

    pub fn find_misaligned_boundary(&self, editor: EditorId) -> Option<u32> {
        self.editors
            .get(&editor)?
            .layout
            .find_misaligned_boundary(self.text())
    }

    pub fn apply_syntax(
        &mut self,
        root: Syntax,
        sites: &[SyntaxSite],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        self.install_syntax(root, sites);
        let byte_count = crate::text_cursor::byte_count(&self.text);
        for id in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(editor) = self.editors.get_mut(&id) else {
                continue;
            };
            editor.layout.mark_modified_in(0..byte_count);
        }
        self.repair_damaged_viewports(fonts, theme, fx)
    }

    pub fn apply_reparse_outcome(
        &mut self,
        outcome: ReparseOutcome,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) -> Option<Vec<Range<u32>>> {
        let probe = crate::env_flags::landing_probe();
        let started = std::time::Instant::now();
        let invalidated = self.splice_reparse(outcome)?;
        {
            let spliced = started.elapsed();
            for id in self.editors.keys().copied().collect::<Vec<_>>() {
                let Some(editor) = self.editors.get_mut(&id) else {
                    continue;
                };
                for range in &invalidated {
                    editor.layout.mark_modified_in(range.clone());
                }
            }
            let marked = started.elapsed();
            self.repair_damaged_viewports(fonts, theme, fx);
            if probe {
                eprintln!(
                    "[landing] splice={:.2}ms mark={:.2}ms repair={:.2}ms ranges={}",
                    spliced.as_secs_f64() * 1000.0,
                    (marked - spliced).as_secs_f64() * 1000.0,
                    started.elapsed().saturating_sub(marked).as_secs_f64() * 1000.0,
                    invalidated.len(),
                );
            }
        }
        Some(invalidated)
    }

    pub(crate) fn repair_landable(&self, repaired: &RepairedLayout) -> bool {
        let Some(editor) = self.editors.get(&repaired.editor) else {
            return false;
        };
        editor.layout.layout_width() == repaired.width
            && repaired.revision == self.revision()
            && repaired.markup_generation == self.markup_generation()
            && (repaired.layout.shaped_theme().is_empty()
                || editor.layout.shaped_theme().is_empty()
                || repaired.layout.shaped_theme() == editor.layout.shaped_theme())
    }

    /// `apply_repair` with the viewport door around it: capture the
    /// anchored byte against the OLD layout, land the repair, note
    /// where the anchor went. Returns whether a correction is now
    /// pending (docs/viewport-preservation.md §5).
    pub fn apply_repair_anchored(&mut self, repaired: RepairedLayout) -> bool {
        let id = repaired.editor;
        let door = self.editors.get(&id).and_then(|editor| {
            let top = editor.viewport.as_ref()?.start;
            (top > 0.5).then(|| {
                let byte = editor.layout.byte_at_y(top);
                (byte, top - editor.layout.height_before(byte), top)
            })
        });
        self.apply_repair(repaired);
        let (Some((byte, dy, was)), Some(editor)) = (door, self.editors.get_mut(&id)) else {
            return false;
        };
        let fresh = editor.layout.height_before(byte) + dy;
        if (fresh - was).abs() > 0.5 {
            editor.settle_to = Some(fresh);
            return true;
        }
        false
    }

    pub fn apply_repair(&mut self, repaired: RepairedLayout) {
        let revision = self.revision();
        let markup_generation = self.markup_generation();
        let Some(editor) = self.editors.get_mut(&repaired.editor) else {
            return;
        };
        if editor.layout.layout_width() != repaired.width
            || repaired.revision != revision
            || repaired.markup_generation != markup_generation
        {
            if crate::env_flags::trace_repair() {
                eprintln!(
                    "[repair] DISCARD width {} vs {}, rev {} vs {revision}, gen {} vs {markup_generation}",
                    repaired.width,
                    editor.layout.layout_width(),
                    repaired.revision,
                    repaired.markup_generation,
                );
            }
            return;
        }

        if !repaired.layout.shaped_theme().is_empty()
            && !editor.layout.shaped_theme().is_empty()
            && repaired.layout.shaped_theme() != editor.layout.shaped_theme()
        {
            return;
        }
        if crate::env_flags::trace_repair() {
            eprintln!(
                "[repair] APPLY editor={:?} height={}",
                repaired.editor,
                repaired.layout.height(),
            );
        }

        let mut incoming = repaired.layout;
        if let Some(stale) = editor.layout.take_stale_for_swap() {
            incoming.note_swap_fold(stale);
        }
        editor.layout = incoming;
    }

    pub(crate) fn take_healed(&mut self, editor: EditorId) -> Option<Range<u32>> {
        self.editors.get_mut(&editor)?.layout.take_healed()
    }

    pub(crate) fn take_swap_fold(&mut self, editor: EditorId) -> Option<Range<u32>> {
        self.editors.get_mut(&editor)?.layout.take_swap_fold()
    }

    pub fn resize(
        &mut self,
        editor: EditorId,
        width: f32,
        anchor_byte: u32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) -> bool {
        let byte_count = crate::text_cursor::byte_count(&self.text);
        let collection = fonts.clone();
        let Some(state) = self.editors.get_mut(&editor) else {
            return false;
        };
        state.target_width = width;

        let width = match state.softwrap {
            true => width,
            false => NOWRAP_LAYOUT_WIDTH,
        };
        if state.layout.layout_width() == width {
            return false;
        }

        if crate::env_flags::trace_resize() {
            eprintln!(
                "[resize] editor={editor:?} {} -> {width} anchor={anchor_byte}",
                state.layout.layout_width()
            );
        }
        // A rewrap moves EVERY height: capture the viewport anchor
        // against the old wrap, resolve against the new one
        // (docs/viewport-preservation.md §5).
        let door = state
            .viewport
            .as_ref()
            .map(|viewport| viewport.start)
            .and_then(|top| {
                (top > 0.5).then(|| {
                    let byte = state.layout.byte_at_y(top);
                    (byte, top - state.layout.height_before(byte), top)
                })
            });
        state.layout.mark_modified_in(0..byte_count);
        let extras = Self::view_extras(&self.markups, state);
        state.layout.repair_layout_bounded(
            &self.text,
            crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
            width,
            &collection,
            theme,
            anchor_byte.min(byte_count),
            state.sync_budget(),
        );
        if let Some((byte, dy, was)) = door {
            let fresh = state.layout.height_before(byte) + dy;
            if (fresh - was).abs() > 0.5 {
                state.settle_to = Some(fresh);
                fx.settle();
            }
        }
        self.pending_repairs(fx);
        true
    }

    pub fn retheme_editor(
        &mut self,
        editor: EditorId,
        anchor: u32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let byte_count = crate::text_cursor::byte_count(&self.text);
        let collection = fonts.clone();
        self.note_markup_change(None);
        if let Some(state) = self.editors.get_mut(&editor) {
            let door = state
                .viewport
                .as_ref()
                .map(|viewport| viewport.start)
                .and_then(|top| {
                    (top > 0.5).then(|| {
                        let byte = state.layout.byte_at_y(top);
                        (byte, top - state.layout.height_before(byte), top)
                    })
                });
            state.layout.mark_modified_in(0..byte_count);
            let extras = Self::view_extras(&self.markups, state);
            let width = state.layout.layout_width();
            state.layout.repair_layout_bounded(
                &self.text,
                crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
                width,
                &collection,
                theme,
                anchor.min(byte_count),
                state.sync_budget(),
            );
            if let Some((byte, dy, was)) = door {
                let fresh = state.layout.height_before(byte) + dy;
                if (fresh - was).abs() > 0.5 {
                    state.settle_to = Some(fresh);
                    fx.settle();
                }
            }
        }
        self.pending_repairs(fx)
    }

    pub fn perform_inlay(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        editor: EditorId,
        key: InlayKey,
        command: InlayCommand,
        take_focus: bool,
        fx: &mut EditorEffects<'_>,
    ) {
        let fonts = crate::env::ui_collection(store, ui);
        let theme = &crate::env::Themes::of(store);
        if take_focus {
            self.set_focus(editor, EditorFocus::Inlay(key));
        }
        let (performed, edit) = fx.scope(
            move |command| EditorCommand::Inlay { key, command },
            |fx| self.perform_inlay_raw(store, ui, key, command, fx),
        );

        if let Some(operation) = edit {
            self.edit(&operation, &fonts, theme, fx);
            if take_focus {
                self.set_focus(editor, EditorFocus::Inlay(key));
            }
        }
        if let Some((range, mode)) = performed {
            self.repair_editors(
                crate::markup::inlay_repair_span(mode, &range),
                &fonts,
                theme,
                fx,
            );
        }
    }

    pub fn swap_inlay(&mut self, key: InlayKey, range: Range<u32>, inlay: Inlay) {
        self.note_markup_change(None);
        self.with_markup_mut(key.layer, |markup| {
            markup.replace_inlay(key.key, range, inlay);
        });
    }

    #[doc(hidden)]
    pub fn replace_inlay(
        &mut self,
        key: InlayKey,
        range: Range<u32>,
        inlay: Inlay,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let mut span = crate::markup::inlay_repair_span(inlay.mode(), &range);
        if let MarkupLayer::Markup(markup_id) = key.layer {
            if let Some((old, mode)) = self
                .markups
                .get(&markup_id)
                .and_then(|markup| markup.inlay_interval(key.key))
            {
                let old = crate::markup::inlay_repair_span(mode, &old);
                span = span.start.min(old.start)..span.end.max(old.end);
            }
        }
        self.swap_inlay(key, range, inlay);
        self.repair_editors(span, fonts, theme, fx)
    }

    pub(crate) fn repair_visible_damage(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let collection = fonts.clone();
        let Some(state) = self.editors.get_mut(&editor) else {
            return;
        };
        let Some(viewport) = state.viewport.clone() else {
            return;
        };
        let visible = state.layout.byte_at_y(viewport.start)
            ..state.layout.byte_at_y(viewport.end).saturating_add(1);
        if !state.layout.damage_intersects(visible.clone()) {
            return;
        }
        let Some(start) = state.layout.repair_pending_at_or_after(visible.start) else {
            return;
        };
        let extras = Self::view_extras(&self.markups, state);
        let width = state.layout.layout_width();
        state.layout.repair_layout_bounded(
            &self.text,
            crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
            width,
            &collection,
            theme,
            start,
            state.sync_budget(),
        );
        self.pending_repairs(fx)
    }

    fn repair_damaged_viewports(
        &mut self,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let collection = fonts.clone();
        for id in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(editor) = self.editors.get_mut(&id) else {
                continue;
            };

            let anchor = editor
                .viewport
                .as_ref()
                .map(|viewport| editor.layout.byte_at_y(viewport.start))
                .unwrap_or(0);
            let Some(pending) = editor
                .layout
                .repair_pending_at_or_after(anchor)
                .or_else(|| editor.layout.repair_pending())
            else {
                continue;
            };
            let extras = Self::view_extras(&self.markups, editor);
            let width = editor.layout.layout_width();
            editor.layout.repair_layout_bounded(
                &self.text,
                crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
                width,
                &collection,
                theme,
                pending,
                editor.sync_budget(),
            );
        }
        self.pending_repairs(fx)
    }

    fn repair_editors(
        &mut self,
        span: Range<u32>,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let collection = fonts.clone();
        for id in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(editor) = self.editors.get_mut(&id) else {
                continue;
            };
            let repair_start = editor.layout.mark_modified_in(span.clone());
            let extras = Self::view_extras(&self.markups, editor);
            let width = editor.layout.layout_width();
            editor.layout.repair_layout_bounded(
                &self.text,
                crate::markup::OverlaidMarkup::new(syntax_markup(&self.syntax), &extras),
                width,
                &collection,
                theme,
                repair_start,
                editor.sync_budget(),
            );
        }
        self.pending_repairs(fx)
    }

    fn pending_repairs(&mut self, fx: &mut EditorEffects<'_>) {
        let plain = self
            .editors
            .iter()
            .filter(|(_, editor)| !editor.pair_managed);
        let effect = RepairEffect::collect(self, plain);
        if let Some(effect) = effect {
            if crate::env_flags::trace_repair() {
                eprintln!("[repair] LAUNCH {} editors", effect.editor_count());
            }

            fx.relaunch(&mut self.repair_token, effect);
        }
    }

    pub fn substance(&self) -> Document {
        let mut document = self.clone();
        document.editors = rpds::HashTrieMapSync::new_sync();
        document
    }

    pub fn carets(&self, editor: EditorId) -> crate::caret::MultiCaret {
        self.editors
            .get(&editor)
            .map_or_else(|| crate::caret::MultiCaret::single(0), |e| e.carets.clone())
    }

    pub fn set_carets(&mut self, editor: EditorId, carets: crate::caret::MultiCaret) {
        let Some(state) = self.editors.get(&editor) else {
            return;
        };
        let window = self.editor_window(state);
        if let Some(state) = self.editors.get_mut(&editor) {
            state.carets = carets.clamped(&window);
        }
    }

    pub fn add_diff(&mut self, operation: Operation, base_revision: u64) -> crate::diff::DiffId {
        debug_assert_eq!(
            operation.new_len(),
            crate::text_cursor::byte_count(&self.text),
            "a live diff's new side must cover this document's text"
        );
        let id = crate::diff::DiffId::mint();
        // THE diff markup, derived FROM the operation
        // (`diff::hunk_markup` — the presentation stage, where
        // whitespace/word preferences will parameterize): View-scoped
        // (pane halves pick it — an ordinary editor's text stays
        // untinted) and projected onto the scroll track; the
        // normalize lane refreshes it from then on.
        let markup = MarkupId::mint();
        self.markups
            .insert_mut(markup, crate::diff::hunk_markup(&operation, &self.text));
        self.scroll_stripe_generation += 1;
        self.diffs.insert_mut(
            id,
            crate::diff::Diff {
                operation,
                base_revision,
                markup,
                generation: 0,
            },
        );
        id
    }

    /// Lands a normalize run's freshly derived diff markup
    /// (docs/scroll-stripe.md §7): the worker derived it against
    /// `derived_at`; edits since then shift it home, and the swap
    /// damages exactly old ∪ new — the pane halves showing it repair,
    /// nobody else notices, and the flagged-entry bump wakes the
    /// scroll track.
    pub fn install_diff_markup(
        &mut self,
        id: crate::diff::DiffId,
        fresh: Markup,
        changed: Vec<Range<u32>>,
        derived_at: u64,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(diff) = self.diffs.get(&id) else {
            return;
        };
        let markup = diff.markup;
        let mut fresh = fresh;
        let mut changed = changed;
        if let Some(since) = self.log.compose_since(derived_at) {
            let mut view = self.text.view();
            fresh.edit(&since, &mut view, 0);
            for range in &mut changed {
                *range = EditLog::transform_range(range.clone(), &since);
            }
        }
        changed.retain(|range| range.start < range.end);
        EditLog::coalesce(&mut changed);
        self.replace_markup(markup, fresh, &changed, fonts, theme, fx);
    }

    pub fn remove_diff(
        &mut self,
        id: crate::diff::DiffId,
        changed: &[Range<u32>],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(diff) = self.diffs.get(&id) else {
            return;
        };
        let markup = diff.markup;
        // The markup leaves FIRST: `remove_markup`'s stripe-membership
        // check still sees the diff, so the generation bumps and the
        // track owes a clearing relaunch.
        self.remove_markup(markup, changed, fonts, theme, fx);
        self.diffs.remove_mut(&id);
    }

    pub fn diff(&self, id: crate::diff::DiffId) -> Option<&crate::diff::Diff> {
        self.diffs.get(&id)
    }

    pub fn apply_diff_base_edits(&mut self, id: crate::diff::DiffId, base_log: &EditLog) -> bool {
        let Some(diff) = self.diffs.get(&id) else {
            return true;
        };
        if diff.base_revision == base_log.revision() {
            return true;
        }
        let mut diff = diff.clone();
        let current = diff.apply_base_edits(base_log);
        if current {
            self.diffs.insert_mut(id, diff);
        }
        current
    }

    pub fn install_normalized_diff(
        &mut self,
        id: crate::diff::DiffId,
        operation: Operation,
        base_revision: u64,
    ) -> bool {
        if operation.new_len() != crate::text_cursor::byte_count(&self.text) {
            debug_assert!(
                false,
                "a normalized diff must land rebased to the current text"
            );
            return false;
        }
        let Some(diff) = self.diffs.get(&id) else {
            return false;
        };
        let mut diff = diff.clone();
        diff.operation = operation;
        diff.base_revision = base_revision;
        diff.generation += 1;
        self.diffs.insert_mut(id, diff);
        true
    }

    pub fn add_markup(&mut self) -> MarkupId {
        let id = MarkupId::mint();
        self.markups.insert_mut(id, Markup::new());
        id
    }

    /// Allocates a View-scoped entry OWNED by the editor: shown on it,
    /// removed with it (the fold-markup recipe) — no dismantle
    /// bookkeeping owed anywhere else.
    pub fn add_owned_markup(&mut self, editor: EditorId) -> MarkupId {
        let id = self.add_markup();
        self.show_markup(editor, id);
        if let Some(state) = self.editors.get_mut(&editor) {
            state.owned_markups.push(id);
        }
        id
    }

    pub fn replace_markup(
        &mut self,
        id: MarkupId,
        replacement: Markup,
        changed: &[Range<u32>],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(standing) = self.markups.get(&id) else {
            return;
        };

        let mut replacement = replacement;
        replacement.set_scope(standing.scope());
        if self.stripes_markup(id) {
            self.scroll_stripe_generation += 1;
        }
        self.markups.insert_mut(id, replacement);

        if changed.is_empty() {
            return;
        }
        self.reshape_markup_change(id, changed, fonts, theme, fx)
    }

    pub fn remove_markup(
        &mut self,
        id: MarkupId,
        changed: &[Range<u32>],
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if !self.markups.contains_key(&id) {
            return;
        }
        if !changed.is_empty() {
            self.reshape_markup_change(id, changed, fonts, theme, fx);
        }
        if self.stripes_markup(id) {
            self.scroll_stripe_generation += 1;
        }
        self.markups.remove_mut(&id);
        for editor in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.editors.get_mut(&editor) else {
                continue;
            };
            state.markups.retain(|markup| *markup != id);
            state.scroll_stripes.markups.retain(|markup| *markup != id);
        }

        self.note_markup_change(None);
    }

    pub fn show_markup(&mut self, editor: EditorId, id: MarkupId) {
        let shown = match self.editors.get_mut(&editor) {
            Some(state) if !state.markups.contains(&id) => {
                state.markups.push(id);
                true
            }
            _ => false,
        };
        if shown {
            self.note_markup_change(None);
        }
    }

    pub fn scroll_stripe_generation(&self) -> u64 {
        self.scroll_stripe_generation
    }

    /// Registers an entry as a scroll-stripe contributor for ONE
    /// editor's track (docs/scroll-stripe.md) — the find-bar shape:
    /// beside the feature's own `show_markup` pick. THE stripes
    /// diff's markup registers the same way (the documents layer does
    /// it at track/enable time); a diff some other view holds — a
    /// split-diff panel's — never leaks onto a pane's track.
    pub fn mark_scroll_stripes(&mut self, editor: EditorId, id: MarkupId) {
        if !self.markups.contains_key(&id) {
            return;
        }
        let Some(state) = self.editors.get_mut(&editor) else {
            return;
        };
        if !state.scroll_stripes.markups.contains(&id) {
            state.scroll_stripes.markups.push(id);
            self.scroll_stripe_generation += 1;
        }
    }

    /// Registers an entry on EVERY stripe-enabled editor's track —
    /// the documents layer's door for THE stripes diff arriving while
    /// panes already show tracks.
    pub fn mark_scroll_stripes_on_enabled(&mut self, id: MarkupId) {
        let enabled: Vec<EditorId> = self
            .editors
            .iter()
            .filter(|(_, state)| state.scroll_stripes.enabled)
            .map(|(editor, _)| *editor)
            .collect();
        for editor in enabled {
            self.mark_scroll_stripes(editor, id);
        }
    }

    /// Unregisters an entry from every track — the stripes diff
    /// stepping down (untracked, or its record losing the stripes
    /// role while a panel still holds the diff itself).
    pub fn unmark_scroll_stripes(&mut self, id: MarkupId) {
        let mut left = false;
        for editor in self.editors.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.editors.get_mut(&editor) else {
                continue;
            };
            let before = state.scroll_stripes.markups.len();
            state.scroll_stripes.markups.retain(|markup| *markup != id);
            left |= state.scroll_stripes.markups.len() != before;
        }
        if left {
            self.scroll_stripe_generation += 1;
        }
    }

    /// Whether an entry projects onto any scroll track — registered
    /// on some editor's slot. Bounded by the handful of editors,
    /// never by the document.
    fn stripes_markup(&self, id: MarkupId) -> bool {
        self.editors
            .values()
            .any(|state| state.scroll_stripes.markups.contains(&id))
    }

    /// Opts an editor into the stripe track — pane editors only; value
    /// inputs, rows and fragments never derive and never mint.
    pub fn enable_scroll_stripes(&mut self, editor: EditorId) {
        if let Some(state) = self.editors.get_mut(&editor) {
            state.scroll_stripes.enabled = true;
        }
    }

    /// The sweep's early-out: something to project — or something
    /// STALE still painted (the last contributor left; the track owes
    /// one clearing relaunch) — and someone showing a track.
    pub fn wants_scroll_stripes(&self) -> bool {
        let contributors = self
            .editors
            .values()
            .any(|state| !state.scroll_stripes.markups.is_empty());
        let painted = self.editors.values().any(|state| {
            state
                .scroll_stripes
                .landed
                .as_ref()
                .is_some_and(|stripes| !stripes.segments.is_empty())
        });
        (contributors || painted)
            && self
                .editors
                .values()
                .any(|state| state.scroll_stripes.enabled)
    }

    pub fn scroll_stripes(
        &self,
        editor: EditorId,
    ) -> Option<std::sync::Arc<crate::scroll_stripe::ScrollStripes>> {
        self.editors
            .get(&editor)
            .and_then(|state| state.scroll_stripes.landed.clone())
    }

    /// The batch-tail sweep's per-document half: compare each enabled
    /// editor's fingerprint against its last launch and answer the
    /// owed relaunches. The caller pushes them through `fx.relaunch`
    /// and hands the fresh tokens back (`note_scroll_stripe_token`).
    pub fn scroll_stripe_launches(
        &mut self,
        theme: &crate::theme::Theme,
    ) -> Vec<crate::scroll_stripe::StripeLaunch> {
        let editors: Vec<EditorId> = self
            .editors
            .iter()
            .filter(|(_, state)| state.scroll_stripes.enabled)
            .map(|(id, _)| *id)
            .collect();
        let mut launches = Vec::new();
        for editor in editors {
            let Some(state) = self.editors.get(&editor) else {
                continue;
            };
            let stamp = crate::scroll_stripe::StripeStamp {
                revision: self.log.revision(),
                generation: self.scroll_stripe_generation,
                height_bits: state.layout.height().to_bits(),
                theme: theme.name_shared(),
            };
            if state.scroll_stripes.stamp.as_ref() == Some(&stamp) {
                continue;
            }
            // Only REGISTERED entries project: the editor's slot set,
            // holding its feature entries and THE stripes diff.
            let markups: Vec<Markup> = state
                .scroll_stripes
                .markups
                .iter()
                .filter_map(|id| self.markups.get(id).cloned())
                .collect();
            let layout = state.layout.clone();
            let Some(state) = self.editors.get_mut(&editor) else {
                continue;
            };
            state.scroll_stripes.serial += 1;
            state.scroll_stripes.stamp = Some(stamp);
            let work = crate::scroll_stripe::StripeWork {
                token: self.token,
                editor,
                serial: state.scroll_stripes.serial,
                markups,
                layout,
            };
            launches.push(crate::scroll_stripe::StripeLaunch {
                editor,
                effect: crate::scroll_stripe::ScrollStripeEffect { work },
                supersedes: state.scroll_stripes.token.take(),
            });
        }
        launches
    }

    pub fn note_scroll_stripe_token(
        &mut self,
        editor: EditorId,
        token: Option<imba::effect::CancellationToken>,
    ) {
        if let Some(state) = self.editors.get_mut(&editor) {
            state.scroll_stripes.token = token;
        }
    }

    pub(crate) fn apply_scroll_stripes(&mut self, outcome: crate::scroll_stripe::StripeOutcome) {
        if outcome.token != self.token {
            return;
        }
        let Some(state) = self.editors.get_mut(&outcome.editor) else {
            return;
        };
        if outcome.serial != state.scroll_stripes.serial {
            return;
        }
        state.scroll_stripes.landed = Some(outcome.stripes);
    }

    pub fn prebuild_row_layouts(
        &self,
        shown: &[MarkupId],
        rows: &[Range<u32>],
        width: f32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) -> Vec<crate::document_layout::DocumentLayout> {
        let globals: Vec<(MarkupId, &Markup)> = shown
            .iter()
            .filter_map(|id| self.feature_markup(*id).map(|markup| (*id, markup)))
            .chain(
                self.document_scoped_markups()
                    .filter(|(id, _)| !shown.contains(id)),
            )
            .collect();
        rows.iter()
            .map(|range| {
                crate::document_layout::DocumentLayout::build(
                    &self.text,
                    crate::markup::OverlaidMarkup::new(self.markup(), &globals),
                    width,
                    fonts,
                    theme,
                    Some(range.clone()),
                )
            })
            .collect()
    }

    pub fn feature_markup(&self, id: MarkupId) -> Option<&Markup> {
        self.markups.get(&id)
    }

    pub fn document_scoped_markups(&self) -> impl Iterator<Item = (MarkupId, &Markup)> + '_ {
        self.markups
            .iter()
            .filter(|(_, markup)| markup.scope() == crate::markup::MarkupScope::Document)
            .map(|(id, markup)| (*id, markup))
    }

    pub fn window(&self, editor: EditorId) -> Range<u32> {
        match self.editors.get(&editor) {
            Some(state) => self.editor_window(state),
            None => 0..0,
        }
    }

    pub fn byte_at_point(
        &self,
        editor: EditorId,
        x: f32,
        y: f32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<u32> {
        if !self.editors.contains_key(&editor) {
            return None;
        }
        self.caret_at_point_in(editor, x, y, 0.0, 0.0, 0.0, fonts, theme)
    }
}

fn syntax_markup(syntax: &Option<Syntax>) -> &Markup {
    syntax
        .as_ref()
        .map(|entry| &entry.markup)
        .unwrap_or_else(|| Markup::empty())
}

fn utf16_to_byte_in(s: &str, utf16: u32) -> u32 {
    let mut units = 0u32;
    for (byte, ch) in s.char_indices() {
        if units >= utf16 {
            return byte as u32;
        }
        units += ch.len_utf16() as u32;
    }
    s.len().min(u32::MAX as usize) as u32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provenance {
    Ours,
    Shared,
}
