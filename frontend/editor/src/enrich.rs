// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{future::Future, ops::Range, pin::Pin, sync::Arc};

use text::Text;

use crate::{
    document::DocumentToken,
    markup::{Markup, MarkupBuilder, MarkupId, Syntax},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EnricherId(pub &'static str);

#[derive(Clone, Copy, Debug)]
pub struct Interest {
    pub syntax: bool,
    pub carets: bool,
}

impl Default for Interest {
    fn default() -> Self {
        Self {
            syntax: true,
            carets: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CaretContext {
    pub selection: Range<u32>,

    pub offset: u32,
}

pub struct EnrichInput {
    pub text: Text,

    pub syntax: Syntax,

    pub revision: u64,

    pub changed: Vec<Range<u32>>,

    pub previous: Markup,

    pub base: Option<crate::ResourceLocation>,

    pub caret: Option<CaretContext>,
}

pub struct Enrichment {
    pub replacement: MarkupBuilder,
    pub changed: Vec<Range<u32>>,
}

impl Enrichment {
    pub fn none() -> Self {
        Self {
            replacement: Markup::builder(),
            changed: Vec::new(),
        }
    }
}

pub type EnrichFuture<'a> = Pin<Box<dyn Future<Output = Enrichment> + 'a>>;

pub fn ready(enrichment: Enrichment) -> EnrichFuture<'static> {
    Box::pin(std::future::ready(enrichment))
}

pub struct EnrichCx<'a> {
    pub fonts: &'a skia_safe::textlayout::FontCollection,
    pub theme: &'a crate::theme::Theme,
    pub caller: imba::effect::EffectCaller,

    pub languages: Option<Arc<crate::reparse::SyntaxLanguages>>,

    pub measure: MeasureCtx<'a>,
}

/// Where a derive gets its inlay-measure ctx: the UI thread hands
/// its own pair in; the effect handler lends its kept Workshop pair.
/// Never minted per call — see `UiCtx::dont_use_too_slow`.
pub enum MeasureCtx<'a> {
    Handed {
        store: &'a imba::store::Store,
        ui: &'a imba::UiCtx,
    },
    Kept(Arc<crate::env::Workshop>),
}

impl MeasureCtx<'_> {
    pub fn measure<R>(
        &self,
        width: f32,
        f: impl FnOnce(crate::markup::InlayMeasure<'_>) -> R,
    ) -> R {
        match self {
            MeasureCtx::Handed { store, ui } => f(crate::markup::InlayMeasure { width, store, ui }),
            MeasureCtx::Kept(workshop) => workshop.measure(width, f),
        }
    }

    pub fn with_ctx<R>(&self, f: impl FnOnce(&imba::store::Store, &imba::UiCtx) -> R) -> R {
        match self {
            MeasureCtx::Handed { store, ui } => f(store, ui),
            MeasureCtx::Kept(workshop) => workshop.with_ctx(f),
        }
    }
}

pub trait Enricher: Send + Sync {
    fn id(&self) -> EnricherId;

    fn interest(&self) -> Interest {
        Interest::default()
    }

    fn derive<'a>(&'a self, input: &'a EnrichInput, cx: &'a EnrichCx<'a>) -> EnrichFuture<'a>;

    fn reconcile(
        &self,
        replacement: &mut Markup,
        changed: &[Range<u32>],
        live: &Markup,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let _ = (fonts, theme);
        replacement.carry_live_views_in(live, changed);
    }

    fn install(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::UiCtx,
        _replacement: &mut Markup,
        _changed: &[Range<u32>],
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &crate::theme::Theme,
    ) {
    }
}

#[derive(Default, Clone)]
pub struct Enrichers {
    entries: Vec<Arc<dyn Enricher>>,
}

impl Enrichers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, enricher: Arc<dyn Enricher>) {
        debug_assert!(
            !self.entries.iter().any(|entry| entry.id() == enricher.id()),
            "enricher ids key entries and lanes — {:?} registered twice",
            enricher.id()
        );
        self.entries.push(enricher);
    }

    pub fn entries(&self) -> &[Arc<dyn Enricher>] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub struct EnrichWork {
    pub(crate) token: DocumentToken,
    pub(crate) serial: u64,
    pub(crate) markup: MarkupId,
    pub(crate) enricher: Arc<dyn Enricher>,
    pub(crate) input: EnrichInput,

    pub(crate) anchor: Option<crate::editor::EditorId>,

    pub(crate) editor: Option<crate::editor::EditorId>,

    pub(crate) languages: Option<Arc<crate::reparse::SyntaxLanguages>>,
}

pub struct EnrichOutcome {
    pub(crate) token: DocumentToken,
    pub(crate) revision: u64,
    pub(crate) serial: u64,
    pub(crate) markup: MarkupId,
    pub(crate) enricher: Arc<dyn Enricher>,

    pub(crate) replacement: Markup,
    pub(crate) changed: Vec<Range<u32>>,
    anchor: Option<crate::editor::EditorId>,

    pub(crate) editor: Option<crate::editor::EditorId>,
}

impl EnrichOutcome {
    pub fn anchor(&self) -> Option<crate::editor::EditorId> {
        self.anchor
    }

    pub fn describes(&self) -> (EnricherId, &[Range<u32>]) {
        (self.enricher.id(), &self.changed)
    }
}

pub struct EnrichEffect {
    pub(crate) work: EnrichWork,
}

impl EnrichEffect {
    pub fn enricher(&self) -> EnricherId {
        self.work.enricher.id()
    }
}

impl imba::effect::Effect for EnrichEffect {
    type Result = crate::editor_view::EditorCommand;
}

pub struct EnrichHandler {
    pub workshop: Arc<crate::env::Workshop>,
    pub caller: imba::effect::EffectCaller,
}

impl imba::effect::EffectHandler<EnrichEffect> for EnrichHandler {
    async fn handle(&self, effect: EnrichEffect) -> crate::editor_view::EditorCommand {
        crate::editor_view::EditorCommand::ApplyEnrichment(self.run(effect.work).await)
    }
}

impl EnrichHandler {
    pub async fn run(&self, work: EnrichWork) -> EnrichOutcome {
        let fonts = self.workshop.fonts();
        let theme = self.workshop.theme();
        run_work(
            work,
            &fonts,
            &theme,
            MeasureCtx::Kept(self.workshop.clone()),
            self.caller.clone(),
        )
        .await
    }
}

pub(crate) async fn run_work(
    work: EnrichWork,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::theme::Theme,
    measure: MeasureCtx<'_>,
    caller: imba::effect::EffectCaller,
) -> EnrichOutcome {
    let cx = EnrichCx {
        fonts,
        theme,
        caller,
        languages: work.languages.clone(),
        measure,
    };
    let fresh = work.enricher.derive(&work.input, &cx).await;
    let mut replacement = work.input.previous.clone();
    if !fresh.changed.is_empty() {
        cx.measure.with_ctx(|store, ui| {
            replacement.splice(&fresh.changed, fresh.replacement, store, ui, fonts, theme)
        });
    }
    EnrichOutcome {
        token: work.token,
        revision: work.input.revision,
        serial: work.serial,
        markup: work.markup,
        enricher: work.enricher,
        replacement,
        changed: fresh.changed,
        anchor: work.anchor,
        editor: work.editor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::InlayMode;
    use imba::effect::EffectHandler;

    fn fonts() -> skia_safe::textlayout::FontCollection {
        crate::test_document::test_fonts_collection().clone()
    }

    fn theme() -> crate::theme::Theme {
        crate::theme::Theme::embedded()
    }

    fn workshop() -> Arc<crate::env::Workshop> {
        Arc::new(crate::env::Workshop::new(
            crate::embedded_fonts::source(),
            theme(),
        ))
    }

    #[derive(Clone)]
    struct Badge;

    impl imba::View for Badge {
        type Command = std::convert::Infallible;
        fn perform(
            &mut self,
            _store: &mut imba::store::Store,
            _ui: &imba::UiCtx,
            command: Self::Command,
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
            match command {}
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a imba::store::Store,
            _ui: &'a imba::UiCtx,
        ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
            imba::laid(
                move |_arena: &'a imba::arena::Arena,
                      _constraints: imba::constraints::Constraints| {
                    imba::leaf::leaf(10.0, 10.0)
                },
            )
        }
    }

    struct BadgePass {
        derived: std::sync::Mutex<Vec<Vec<Range<u32>>>>,
    }

    impl BadgePass {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                derived: std::sync::Mutex::new(Vec::new()),
            })
        }
    }

    impl Enricher for BadgePass {
        fn id(&self) -> EnricherId {
            EnricherId("test-badges")
        }

        fn derive<'a>(&'a self, input: &'a EnrichInput, _cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
            self.derived
                .lock()
                .expect("derive log")
                .push(input.changed.clone());
            let mut builder = Markup::builder();
            let mut changed = Vec::new();
            let text = input.text.byte_string(0, input.text.byte_count());
            for (at, _) in text.match_indices("@@") {
                let subject = at as u32..at as u32 + 2;
                let touched = input
                    .changed
                    .iter()
                    .any(|range| subject.start <= range.end && range.start <= subject.end);
                if touched {
                    builder.push_inlay(
                        subject.clone(),
                        crate::markup::Inlay::new(InlayMode::Under, Badge),
                    );
                    changed.push(subject);
                }
            }
            ready(Enrichment {
                replacement: builder,
                changed,
            })
        }
    }

    fn document(source: &str) -> crate::Document {
        let mut document =
            crate::Document::new(text::Text::from_string_exact(source), Markup::new());
        let _ = document.add_syntax(
            0..0,
            crate::markup::Syntax::new("fake", None, Markup::new()),
        );
        document
    }

    fn registry(pass: &Arc<BadgePass>) -> Enrichers {
        let mut registry = Enrichers::new();
        registry.register(pass.clone() as Arc<dyn Enricher>);
        registry
    }

    fn badge_count(document: &crate::Document) -> usize {
        let len = document.text().byte_count() as u32;
        document.all_inlays_in(0..len).len()
    }

    fn launched(
        document: &mut crate::Document,
        registry: &Enrichers,
        changed: &[Range<u32>],
    ) -> EnrichEffect {
        let mut batch = imba::effect::Batch::new();
        document.launch_enrichment(registry, None, None, changed, &mut batch.effects());
        let mut launches = batch.surviving_launches();
        assert_eq!(launches.len(), 1, "one pass, one lane, one launch");
        *launches
            .pop()
            .expect("launch")
            .into_payload()
            .split()
            .0
            .downcast::<EnrichEffect>()
            .expect("the enrich effect")
    }

    fn land(effect: EnrichEffect) -> crate::editor_view::EditorCommand {
        let handler = EnrichHandler {
            workshop: workshop(),
            caller: imba::effect::EffectCaller::disconnected(),
        };
        imba::effect::block_on(Box::pin(async move { handler.handle(effect).await }))
    }

    fn apply(document: &mut crate::Document, command: crate::editor_view::EditorCommand) {
        let crate::editor_view::EditorCommand::ApplyEnrichment(outcome) = command else {
            panic!("the enrich handler lands ApplyEnrichment");
        };
        let mut batch = imba::effect::Batch::new();
        {
            let mut store = imba::store::Store::new();
            let ui = crate::test_document::test_ui();
            document.apply_enrichment(
                outcome,
                &mut store,
                ui,
                &fonts(),
                &theme(),
                &mut batch.effects(),
            );
        };
    }

    #[test]
    fn the_first_run_is_full_range_and_lands_in_the_pass_entry() {
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut document = document("plain @@ text with another @@ subject\n");
        let effect = launched(&mut document, &registry, &[0..1]);
        apply(&mut document, land(effect));
        assert_eq!(badge_count(&document), 2, "both subjects landed");
        assert!(
            document.markup().all_inlays_in(0..u32::MAX).is_empty(),
            "the parse product carries no widgets — the pass's entry does"
        );
        let derived = pass.derived.lock().expect("log");
        let len = document.text().byte_count() as u32;
        assert_eq!(
            derived.as_slice(),
            &[vec![0..len]],
            "the cold slot derived the full document"
        );
    }

    #[test]
    fn later_runs_are_bounded_by_the_changed_ranges() {
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut document = document("head @@ middle @@ tail\n");
        let effect = launched(&mut document, &registry, &[0..1]);
        apply(&mut document, land(effect));
        let effect = launched(&mut document, &registry, &[0..7]);
        apply(&mut document, land(effect));
        let derived = pass.derived.lock().expect("log");
        assert_eq!(
            derived[1],
            vec![0..7],
            "the second run saw the trigger's ranges"
        );
        assert_eq!(
            badge_count(&document),
            2,
            "the untouched subject survived the splice"
        );
    }

    #[test]
    fn a_superseded_landing_discards_itself() {
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut document = document("one @@\n");
        let stale = launched(&mut document, &registry, &[0..1]);
        let stale = land(stale);

        let fresh = launched(&mut document, &registry, &[0..7]);
        apply(&mut document, stale);
        assert_eq!(badge_count(&document), 0, "the superseded landing dropped");
        apply(&mut document, land(fresh));
        assert_eq!(badge_count(&document), 1, "the latest landing applied");
    }

    #[test]
    fn a_landing_rebases_over_edits_since_capture() {
        let store = &imba::store::Store::new();
        let ui = crate::test_document::test_ui();
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut document = document("abc @@ def\n");
        let effect = launched(&mut document, &registry, &[0..1]);
        let command = land(effect);

        document.edit(
            &operation::Operation::insert_at(0, "XXXX"),
            store,
            ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        );
        apply(&mut document, command);
        let len = document.text().byte_count() as u32;
        let inlays = document.all_inlays_in(0..len);
        assert_eq!(inlays.len(), 1);
        assert_eq!(
            inlays[0].range,
            8..10,
            "the badge shifted with the insert (4 + 'abc ')"
        );
    }

    fn apply_collect(
        document: &mut crate::Document,
        command: crate::editor_view::EditorCommand,
    ) -> imba::effect::Batch<crate::editor_view::EditorCommand> {
        let crate::editor_view::EditorCommand::ApplyEnrichment(outcome) = command else {
            panic!("the enrich handler lands ApplyEnrichment");
        };
        let mut batch = imba::effect::Batch::new();
        {
            let mut store = imba::store::Store::new();
            let ui = crate::test_document::test_ui();
            document.apply_enrichment(
                outcome,
                &mut store,
                ui,
                &fonts(),
                &theme(),
                &mut batch.effects(),
            );
        };
        batch
    }

    fn drain_repairs(
        document: &mut crate::Document,
        editor: crate::editor::EditorId,
        batch: imba::effect::Batch<crate::editor_view::EditorCommand>,
    ) {
        use imba::effect::EffectHandler;
        let workshop = workshop();
        let mut store = imba::store::Store::new();
        let ui = crate::test_document::test_ui();
        let mut pending = batch.surviving_launches();
        let mut rounds = 0;
        while let Some(effect) = pending.pop() {
            rounds += 1;
            assert!(rounds < 100_000, "the repair chain must converge");
            let (value, _lift) = effect.into_payload().split();
            let effect = value
                .downcast::<crate::repair::RepairEffect>()
                .expect("a landing's tail is repair work");
            let handler = crate::repair::RepairHandler(Arc::clone(&workshop));
            let command =
                imba::effect::block_on(Box::pin(async move { handler.handle(*effect).await }));
            let mut batch = imba::effect::Batch::new();
            document.perform(&mut store, &ui, editor, command, &mut batch.effects());
            pending.extend(batch.surviving_launches());
        }
    }

    fn element_at(
        document: &crate::Document,
        editor: crate::editor::EditorId,
        byte: u32,
    ) -> (usize, f32) {
        let ranges = document.element_byte_ranges(editor);
        let heights = document.element_heights(editor);
        let index = ranges
            .iter()
            .position(|range| range.start <= byte && byte < range.end)
            .expect("an element covers the subject");
        (index, heights[index].1)
    }

    #[test]
    fn a_landing_damages_and_repairs_the_shown_layout() {
        let store = &imba::store::Store::new();
        let ui = crate::test_document::test_ui();
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut document = document("alpha beta\ngamma @@ delta\ntail line\n");
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            store,
            ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        );
        let subject = document
            .text()
            .byte_string(0, document.text().byte_count())
            .find("@@")
            .expect("subject") as u32;
        let (index, before) = element_at(&document, editor, subject);

        let effect = launched(&mut document, &registry, &[0..1]);
        let batch = apply_collect(&mut document, land(effect));
        drop(batch);

        let (_, after) = element_at(&document, editor, subject);
        assert!(
            (after - before - 10.0).abs() < 0.5,
            "the subject's element grew by the badge height: {before} -> {after}"
        );
        let live = document.element_heights(editor);
        let reference =
            crate::EditorView::complete(document.clone(), 400.0, store, ui, &fonts(), &theme())
                .element_heights();
        assert_eq!(
            live, reference,
            "the landed layout equals a from-scratch layout over the enriched document"
        );
        let _ = index;
    }

    #[test]
    fn a_landing_beyond_the_sync_budget_repairs_through_effects() {
        let store = &imba::store::Store::new();
        let ui = crate::test_document::test_ui();
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut source = String::new();
        for line in 0..2000 {
            source.push_str(&format!("line {line} carries a subject @@ of its own\n"));
        }
        let mut document = document(&source);
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            store,
            ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        );

        {
            let mut store = imba::store::Store::new();
            let ui = crate::test_document::test_ui();
            document.perform(
                &mut store,
                &ui,
                editor,
                crate::editor_view::EditorCommand::Viewport {
                    width: 400.0,
                    top: 0.0,
                    bottom: 300.0,
                    anchor: 0,
                },
                &mut imba::effect::Batch::new().effects(),
            );
        }
        let deep = source.rfind("@@").expect("subject") as u32;
        let (_, before) = element_at(&document, editor, deep);

        let effect = launched(&mut document, &registry, &[0..1]);
        let batch = apply_collect(&mut document, land(effect));
        assert!(
            batch.len() > 0,
            "budget-exceeding damage leaves as repair effects"
        );
        drain_repairs(&mut document, editor, batch);

        let (_, after) = element_at(&document, editor, deep);
        assert!(
            (after - before - 10.0).abs() < 0.5,
            "the drained repairs laid the deep badge in: {before} -> {after}"
        );
        let live = document.element_heights(editor);
        let reference =
            crate::EditorView::complete(document.clone(), 400.0, store, ui, &fonts(), &theme())
                .element_heights();
        assert_eq!(
            live, reference,
            "the repaired layout equals the from-scratch reference"
        );
    }

    struct CaretTint {
        derived: std::sync::Mutex<Vec<Option<u32>>>,
    }

    impl CaretTint {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                derived: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn id() -> EnricherId {
            EnricherId("test-caret-tint")
        }
    }

    impl Enricher for CaretTint {
        fn id(&self) -> EnricherId {
            Self::id()
        }

        fn interest(&self) -> Interest {
            Interest {
                syntax: false,
                carets: true,
            }
        }

        fn derive<'a>(&'a self, input: &'a EnrichInput, _cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
            let caret = input.caret.as_ref().map(|caret| caret.offset);
            self.derived.lock().expect("derive log").push(caret);
            let len = input.text.byte_count() as u32;
            let mut fresh = Vec::new();
            if let Some(at) = caret {
                if at < len {
                    fresh.push(at..at + 1);
                }
            }
            let mut changed = input.previous.styled_ranges_in(0..u32::MAX);
            changed.extend(fresh.iter().cloned());
            changed.sort_by_key(|range| range.start);
            changed.dedup();
            let mut builder = Markup::builder();
            for range in fresh {
                builder.push_styled(range, crate::theme::StyleId::Match);
            }
            ready(Enrichment {
                replacement: builder,
                changed,
            })
        }
    }

    fn caret_registry(pass: &Arc<CaretTint>) -> Enrichers {
        let mut registry = Enrichers::new();
        registry.register(pass.clone() as Arc<dyn Enricher>);
        registry
    }

    fn editor_for(document: &mut crate::Document) -> crate::editor::EditorId {
        let store = &imba::store::Store::new();
        let ui = crate::test_document::test_ui();
        document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            store,
            ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        )
    }

    fn land_all(
        document: &mut crate::Document,
        batch: imba::effect::Batch<crate::editor_view::EditorCommand>,
    ) -> usize {
        let mut landed = 0;
        for launch in batch.surviving_launches() {
            let Ok(effect) = launch.into_payload().split().0.downcast::<EnrichEffect>() else {
                continue;
            };
            apply(document, land(*effect));
            landed += 1;
        }
        landed
    }

    #[test]
    fn a_caret_move_lands_a_per_editor_entry() {
        let pass = CaretTint::new();
        let registry = caret_registry(&pass);
        let mut document = document("abc def\n");
        let first = editor_for(&mut document);
        let second = editor_for(&mut document);
        let mut store = imba::store::Store::new();
        store.put(crate::env::Enrichers(Arc::new(registry)));
        let ui = crate::test_document::test_ui();
        let mut batch = imba::effect::Batch::new();
        document.perform(
            &mut store,
            &ui,
            first,
            crate::editor_view::EditorCommand::Move {
                motion: crate::editor_view::Motion::Right,
                select: false,
            },
            &mut batch.effects(),
        );
        assert_eq!(land_all(&mut document, batch), 1, "one caret run landed");
        assert_eq!(
            pass.derived.lock().expect("log").as_slice(),
            &[Some(1)],
            "the run saw the moved caret"
        );
        let markup = document
            .enrichment_markup(CaretTint::id(), Some(first))
            .expect("the per-editor slot stands");
        assert_eq!(
            document.markup_styled_ranges(markup),
            vec![1..2],
            "the tint sits under the caret"
        );
        assert!(
            document.editor_shows_markup(first, markup),
            "the acting editor picks its entry"
        );
        assert!(
            !document.editor_shows_markup(second, markup),
            "the other editor never sees this caret's highlight"
        );
        assert!(
            document.enrichment_markup(CaretTint::id(), None).is_none(),
            "a caret pass owns no document slot"
        );
    }

    #[test]
    fn a_parse_landing_relaunches_caret_passes_per_editor() {
        let pass = CaretTint::new();
        let registry = caret_registry(&pass);
        let mut document = document("abc def\n");
        let first = editor_for(&mut document);
        let second = editor_for(&mut document);
        document.set_carets(
            second,
            crate::caret::MultiCaret::normalized(vec![crate::caret::Caret::at(4)], 0),
        );
        let mut batch = imba::effect::Batch::new();
        document.launch_enrichment(&registry, None, None, &[0..1], &mut batch.effects());
        assert_eq!(
            land_all(&mut document, batch),
            2,
            "one caret run per editor"
        );
        let mut seen = pass.derived.lock().expect("log").clone();
        seen.sort();
        assert_eq!(seen, vec![Some(0), Some(4)], "each run saw its own caret");
        let first_markup = document
            .enrichment_markup(CaretTint::id(), Some(first))
            .expect("first slot");
        let second_markup = document
            .enrichment_markup(CaretTint::id(), Some(second))
            .expect("second slot");
        assert_eq!(document.markup_styled_ranges(first_markup), vec![0..1]);
        assert_eq!(document.markup_styled_ranges(second_markup), vec![4..5]);
    }

    #[test]
    fn a_removed_editors_caret_landing_drops() {
        let pass = CaretTint::new();
        let registry = caret_registry(&pass);
        let mut document = document("abc def\n");
        let editor = editor_for(&mut document);
        let mut batch = imba::effect::Batch::new();
        document.launch_caret_enrichment(&registry, None, editor, &mut batch.effects());
        document.remove_editor(editor);
        assert_eq!(
            land_all(&mut document, batch),
            1,
            "the run still lands — into the guards"
        );
        assert!(
            document
                .enrichment_markup(CaretTint::id(), Some(editor))
                .is_none(),
            "the slot left with its editor"
        );
    }

    #[test]
    fn a_second_move_washes_the_stale_tint() {
        let pass = CaretTint::new();
        let registry = caret_registry(&pass);
        let mut document = document("abc def\n");
        let editor = editor_for(&mut document);
        let mut store = imba::store::Store::new();
        store.put(crate::env::Enrichers(Arc::new(registry)));
        let ui = crate::test_document::test_ui();
        for _ in 0..2 {
            let mut batch = imba::effect::Batch::new();
            document.perform(
                &mut store,
                &ui,
                editor,
                crate::editor_view::EditorCommand::Move {
                    motion: crate::editor_view::Motion::Right,
                    select: false,
                },
                &mut batch.effects(),
            );
            land_all(&mut document, batch);
        }
        let markup = document
            .enrichment_markup(CaretTint::id(), Some(editor))
            .expect("slot");
        assert_eq!(
            document.markup_styled_ranges(markup),
            vec![2..3],
            "only the latest caret's tint remains"
        );
    }

    #[test]
    fn a_foreign_documents_outcome_never_lands() {
        let pass = BadgePass::new();
        let registry = registry(&pass);
        let mut source = document("subject @@\n");
        let mut other = document("subject @@\n");
        let effect = launched(&mut source, &registry, &[0..1]);
        let command = land(effect);

        let primer = launched(&mut other, &registry, &[0..1]);
        drop(primer);
        apply(&mut other, command);
        assert_eq!(badge_count(&other), 0, "the foreign outcome dropped");
    }
}

pub(crate) fn now_or_never<T>(future: Pin<Box<dyn Future<Output = T> + '_>>) -> Option<T> {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop_raw() -> RawWaker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| noop_raw(), |_| {}, |_| {}, |_| {});
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut future = future;
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}
