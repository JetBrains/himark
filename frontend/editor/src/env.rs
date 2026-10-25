// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

#[derive(Clone)]
pub struct Parsers(pub std::sync::Arc<crate::reparse::SyntaxLanguages>);

impl Parsers {
    pub fn of(store: &Store) -> Option<std::sync::Arc<crate::reparse::SyntaxLanguages>> {
        store.get::<Parsers>().map(|parsers| parsers.0.clone())
    }
}

/// The diff policy the edge installed (docs/editor/structural-diff.md): every
/// place that computes an `Operation` from two texts reads it from
/// here. Absent only in bare-store unit tests — then the exact but
/// content-blind [`crate::diff::ReplaceAll`] stands in.
#[derive(Clone)]
pub struct Differ(pub std::sync::Arc<dyn crate::diff::DiffPolicy>);

impl Differ {
    pub fn of(store: &Store) -> std::sync::Arc<dyn crate::diff::DiffPolicy> {
        store
            .get::<Differ>()
            .map(|differ| differ.0.clone())
            .unwrap_or_else(|| std::sync::Arc::new(crate::diff::ReplaceAll))
    }
}

#[derive(Clone)]
pub struct Enrichers(pub std::sync::Arc<crate::enrich::Enrichers>);

impl Enrichers {
    pub fn of(store: &Store) -> Option<std::sync::Arc<crate::enrich::Enrichers>> {
        store
            .get::<Enrichers>()
            .map(|enrichers| enrichers.0.clone())
    }
}

pub fn ui_typeface(
    _ui: &imba::ui::UiCtx,
    families: &[impl AsRef<str>],
    style: skia_safe::FontStyle,
) -> Option<skia_safe::Typeface> {
    let manager = skia_safe::FontMgr::new();
    for family in families {
        if let Some(face) = manager.match_family_style(family.as_ref(), style) {
            return Some(face);
        }
    }
    manager
        .legacy_make_typeface(None, style)
        .or_else(|| Some(crate::embedded_fonts::typeface()))
}

pub struct Workshop {
    theme: std::sync::Mutex<crate::theme::Theme>,
    /// The effect handler's OWN measure ctx — a store seeded with
    /// the current theme plus a warm `UiCtx`, kept for the
    /// handler's lifetime (ui.rs's doctrine) and handed into every
    /// background layout pass that can meet an inlay. The UI thread
    /// passes its real store/ui instead; nothing is static.
    measure: std::sync::Mutex<(imba::store::Store, imba::ui::UiCtx)>,
    /// The installed enrichment passes, mirrored into the measure
    /// store so background document builds enrich like the app's.
    enrichers: std::sync::Mutex<Option<std::sync::Arc<crate::enrich::Enrichers>>>,
}

unsafe impl Send for Workshop {}
unsafe impl Sync for Workshop {}

impl Workshop {
    /// The workshop lays text out on ITS thread, so its kept ctx
    /// carries the same font world the UI thread shapes with —
    /// handed in, never fetched from a global. A world of its own
    /// would mean a second system font scan and a second typeface
    /// bridge, which is what made a resize freeze.
    pub fn new(world: imba::shape::FontWorld, theme: crate::theme::Theme) -> Self {
        let mut seeded = imba::store::Store::new();
        Themes::set(&mut seeded, theme.clone());
        let ui = imba::ui::UiCtx::dont_use_too_slow();
        ui.set(world);
        Self {
            theme: std::sync::Mutex::new(theme),
            measure: std::sync::Mutex::new((seeded, ui)),
            enrichers: std::sync::Mutex::new(None),
        }
    }

    /// Mirror the app's enrichment passes into the workshop's measure
    /// store — background builds (`with_ctx`) pick them up through
    /// `env::Enrichers::of` like any store reader.
    pub fn install_enrichers(&self, enrichers: std::sync::Arc<crate::enrich::Enrichers>) {
        self.measure
            .lock()
            .expect("workshop measure")
            .0
            .put(Enrichers(enrichers.clone()));
        *self.enrichers.lock().expect("workshop enrichers") = Some(enrichers);
    }

    /// Run `f` with this handler's measure ctx.
    pub fn measure<R>(
        &self,
        width: f32,
        f: impl FnOnce(crate::markup::InlayMeasure<'_>) -> R,
    ) -> R {
        let kept = self.measure.lock().expect("workshop measure");
        let (store, ui) = &*kept;
        f(crate::markup::InlayMeasure { width, store, ui })
    }

    /// Run `f` with this handler's kept (store, ui) pair directly —
    /// for callees that take the pair rather than a ready measure.
    pub fn with_ctx<R>(&self, f: impl FnOnce(&imba::store::Store, &imba::ui::UiCtx) -> R) -> R {
        let kept = self.measure.lock().expect("workshop measure");
        f(&kept.0, &kept.1)
    }

    pub fn theme(&self) -> crate::theme::Theme {
        self.theme.lock().expect("workshop theme").clone()
    }

    pub fn set_theme(&self, theme: crate::theme::Theme) {
        let mut seeded = imba::store::Store::new();
        Themes::set(&mut seeded, theme.clone());
        if let Some(enrichers) = self.enrichers.lock().expect("workshop enrichers").clone() {
            seeded.put(Enrichers(enrichers));
        }
        self.measure.lock().expect("workshop measure").0 = seeded;
        *self.theme.lock().expect("workshop theme") = theme;
    }
}

pub use imba::theme::Themes;

/// The frame's focused SEAT — the semantic walk's answer, stashed
/// into the frame store by whoever builds a tree. The editor derives
/// its selections-visible bit from it AT BUILD TIME, so the viewport
/// snapshot never needs a focused "upgrade" at paint. Absent (bare
/// harnesses, cells): fall back to the editor's own focus state.
#[derive(Clone, Copy)]
pub struct FrameFocus(pub imba::focus::SeatKey);

impl FrameFocus {
    pub fn set(store: &mut imba::store::Store, seat: imba::focus::SeatKey) {
        store.put(FrameFocus(seat));
    }

    pub fn of(store: &imba::store::Store) -> Option<imba::focus::SeatKey> {
        store.get::<FrameFocus>().map(|frame| frame.0)
    }
}
