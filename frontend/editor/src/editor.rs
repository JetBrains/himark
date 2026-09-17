// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use imba::effect::Effects;
use skia_safe::textlayout::FontCollection;

use crate::{
    caret::MultiCaret,
    document::{Document, FragmentKey},
    document_layout::DocumentLayout,
    editor_view::{EditorCommand, EditorFocus},
    markup::MarkupId,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditorId(u64);

impl EditorId {
    pub(crate) fn surface_key(self) -> u64 {
        self.0
    }
}

impl EditorId {
    pub(crate) fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub(crate) struct EditorPlaceholder {
    pub(crate) text: Arc<str>,

    pub(crate) height: f32,
}

#[derive(Clone)]
pub struct Editor {
    pub(crate) layout: DocumentLayout,

    pub(crate) carets: MultiCaret,

    pub(crate) reveal: bool,

    pub(crate) markups: Vec<MarkupId>,

    pub(crate) owned_markups: Vec<MarkupId>,

    pub(crate) folds: Option<MarkupId>,

    pub(crate) before: Option<MarkupId>,

    pub(crate) bounds: Option<FragmentKey>,
    pub(crate) focus: EditorFocus,

    /// The seat's IDENTITY — the flat key the semantic walk hands to
    /// the layout fold; the widget born from this editor answers the
    /// IME ask by RECOGNIZING it (imba::focus::SeatKey). Clones share
    /// it: a persistent copy is the same logical editor.
    pub(crate) seat: imba::focus::SeatKey,

    pub(crate) softwrap: bool,

    pub(crate) scroll_x: f32,

    pub(crate) target_width: f32,

    pub(crate) viewport: Option<Range<f32>>,

    pub(crate) marked: Option<Range<u32>>,

    pub(crate) drag: Option<crate::caret::DragOrigin>,

    pub(crate) pair_managed: bool,

    pub(crate) placeholder: Option<EditorPlaceholder>,

    pub(crate) scroll_stripes: crate::scroll_stripe::StripeSlot,

    /// Where the viewport's anchored content sits AFTER a height
    /// mutation above it (docs/viewport-preservation.md §3): set by
    /// the mutation doors from the RETAINED `viewport` report, read
    /// by the settle pulse, cleared when the next Viewport report
    /// lands. Absolute, so repeated pulses converge.
    pub(crate) settle_to: Option<f32>,
}

impl Editor {
    pub(crate) fn overlay(&self) -> MarkupId {
        self.owned_markups[0]
    }

    pub(crate) fn sync_budget(&self) -> f32 {
        self.viewport
            .as_ref()
            .map(|viewport| ((viewport.end - viewport.start) * 1.5).max(500.0))
            .unwrap_or(crate::document_layout::SYNC_LAYOUT_HEIGHT)
    }

    pub(crate) fn new(
        content: &Document,
        markups: Vec<MarkupId>,
        owned_markups: Vec<MarkupId>,
        width: f32,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        bounds: Option<FragmentKey>,
        build: crate::document::EditorBuild,
    ) -> Self {
        debug_assert!(
            owned_markups.iter().all(|id| markups.contains(id)),
            "owned markups display on their own editor"
        );

        let globals: Vec<(crate::markup::MarkupId, &crate::markup::Markup)> = markups
            .iter()
            .filter_map(|id| content.feature_markup(*id).map(|markup| (*id, markup)))
            .chain(
                content
                    .document_scoped_markups()
                    .filter(|(id, _)| !markups.contains(id)),
            )
            .collect();

        let window = bounds.and_then(|key| content.fragment_range(key));
        let layout = match build {
            crate::document::EditorBuild::Complete => DocumentLayout::build_complete(
                content.text(),
                crate::markup::OverlaidMarkup::new(content.markup(), &globals),
                width,
                fonts,
                theme,
                window,
            ),
            crate::document::EditorBuild::Bounded => DocumentLayout::build(
                content.text(),
                crate::markup::OverlaidMarkup::new(content.markup(), &globals),
                width,
                fonts,
                theme,
                window,
            ),

            crate::document::EditorBuild::Prebuilt(prebuilt) => {
                let mut layout = prebuilt;
                layout.set_window(window);
                layout
            }
        };
        Self {
            layout,
            markups,
            owned_markups,
            carets: MultiCaret::single(
                bounds
                    .and_then(|key| content.fragment_range(key))
                    .map_or(0, |range| range.start),
            ),
            reveal: false,
            folds: None,
            before: None,
            bounds,
            focus: EditorFocus::Text,
            seat: imba::focus::SeatKey::mint(),
            softwrap: true,
            scroll_x: 0.0,
            target_width: width,
            viewport: None,
            marked: None,
            drag: None,
            pair_managed: false,
            placeholder: None,
            scroll_stripes: crate::scroll_stripe::StripeSlot::default(),
            settle_to: None,
        }
    }
}

pub type EditorEffects<'a> = Effects<'a, EditorCommand>;
