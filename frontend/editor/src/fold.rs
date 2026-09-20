// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use imba::anim::{Animation, AnimationClock, Easing, Motion};

use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};
use crate::markup::{Inlay, InlayKey, InlayMode, InsteadKind, MarkupId, MarkupLayer};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FoldCommand {
    Unfold,
    Tick(AnimationClock),
}

impl Document {
    pub fn toggle_fold(
        &mut self,
        editor: EditorId,
        range: Range<u32>,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if let Some(key) = self.fold_matching(editor, &range) {
            let departing = self.fold_chip_at(key).is_some_and(FoldChip::is_departing);
            return self.set_fold_departure(key, !departing, store, ui, fonts, theme, fx);
        }
        let offered = self
            .foldables_in(range.start..range.start.saturating_add(1))
            .into_iter()
            .any(|foldable| foldable == range);
        if !offered {
            return;
        }

        let born = self
            .editors
            .get(&editor)
            .map(|state| folded_extent(&state.layout, &range))
            .unwrap_or(0.0);
        let Some(id) = self.fold_markup_of(editor) else {
            return;
        };
        let chip = Inlay::new(
            InlayMode::Instead(InsteadKind::Inline),
            FoldChip::appearing(born, theme.ui().fold_chip.height),
        );
        self.push_inlay(id, range, chip, store, ui, fonts, theme, fx);
    }

    pub(crate) fn fold_matching(&self, editor: EditorId, range: &Range<u32>) -> Option<InlayKey> {
        let id = self.editors.get(&editor)?.folds?;
        let markup = self.feature_markup(id)?;
        markup
            .inlays_in(
                range.start..range.start.saturating_add(1),
                MarkupLayer::Markup(id),
            )
            .find(|inlay| inlay.range == *range)
            .map(|inlay| inlay.key)
    }

    pub(crate) fn fold_chip_at(&self, key: InlayKey) -> Option<&FoldChip> {
        let MarkupLayer::Markup(id) = key.layer else {
            return None;
        };
        self.feature_markup(id)?
            .inlay_at(key.key)?
            .view_as::<FoldChip>()
    }

    pub(crate) fn set_fold_departure(
        &mut self,
        key: InlayKey,
        departing: bool,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some((range, mode)) = (match key.layer {
            MarkupLayer::Markup(id) => self
                .feature_markup(id)
                .and_then(|markup| markup.inlay_interval(key.key)),
            _ => None,
        }) else {
            return;
        };
        let Some(mut chip) = self.fold_chip_at(key).cloned() else {
            return;
        };
        match departing {
            true => chip.depart(),
            false => chip.revive(theme.ui().fold_chip.height),
        }
        if chip.departed() {
            return self.remove_inlay(key, store, ui, fonts, theme, fx);
        }
        self.replace_inlay(
            key,
            range,
            Inlay::new(mode, chip),
            store,
            ui,
            fonts,
            theme,
            fx,
        );
    }

    fn fold_markup_of(&mut self, editor: EditorId) -> Option<MarkupId> {
        if let Some(id) = self.editors.get(&editor)?.folds {
            return Some(id);
        }
        let id = self.add_markup();
        let state = self.editors.get_mut(&editor)?;
        state.folds = Some(id);
        state.markups.push(id);
        state.owned_markups.push(id);
        Some(id)
    }
}

fn folded_extent(layout: &crate::document_layout::DocumentLayout, range: &Range<u32>) -> f32 {
    let top = layout.height_before(range.start);
    let Some((cursor, bottom, _)) = layout.cursor_at_byte(range.end) else {
        return 0.0;
    };
    (bottom + cursor.element().height - top).max(0.0)
}

#[derive(Clone)]
pub struct FoldChip {
    height: Animation<f32>,

    born: f32,

    departing: bool,
}

impl FoldChip {
    fn appearing(born: f32, chrome_height: f32) -> Self {
        let motion = Motion::Ease {
            duration_ms: 160.0,
            easing: Easing::EaseOut,
        };

        let born = born.max(chrome_height);
        let mut height = Animation::done(born, motion);
        height.set(chrome_height);
        Self {
            height,
            born,
            departing: false,
        }
    }

    fn depart(&mut self) {
        self.departing = true;
        self.height.set(self.born);
    }

    fn revive(&mut self, chrome_height: f32) {
        self.departing = false;
        self.height.set(chrome_height);
    }

    pub(crate) fn is_departing(&self) -> bool {
        self.departing
    }

    pub(crate) fn departed(&self) -> bool {
        self.departing && !self.height.running()
    }

    pub(crate) fn spin(&self, chrome_height: f32) -> f32 {
        let span = self.born - chrome_height;
        if span <= f32::EPSILON {
            return match self.departing {
                true => 0.0,
                false => 1.0,
            };
        }
        (1.0 - (self.height.value() - chrome_height) / span).clamp(0.0, 1.0)
    }
}

impl imba::View for FoldChip {
    type Command = FoldCommand;

    fn perform(
        &mut self,
        _store: &mut imba::store::Store,
        _ui: &imba::UiCtx,
        command: FoldCommand,
        _fx: &mut imba::effect::Effects<'_, FoldCommand>,
    ) {
        match command {
            FoldCommand::Unfold => {}
            FoldCommand::Tick(now) => self.height.advance(now),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        _ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, FoldCommand> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                use imba::event::{Event, EventResult, MouseButton};
                use imba::thunk_ext::ThunkExt;

                let chrome = crate::env::Themes::of(store).ui().fold_chip.clone();
                let animating = self.height.running();
                let height = match animating || self.departing {
                    true => self.height.value(),
                    false => chrome.height,
                };
                let empty = animating || self.departing;
                let chip = imba::leaf::leaf(chrome.width, height)
                    .paint_instead(move |_arena, canvas, rect| {
                        if empty {
                            return;
                        }
                        use skia_safe::{Paint, Rect};
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        paint.set_color(chrome.fill.0);
                        canvas.draw_round_rect(
                            Rect::from_xywh(rect.left, rect.top, rect.width(), rect.height()),
                            chrome.radius,
                            chrome.radius,
                            &paint,
                        );
                        paint.set_color(chrome.dots.0);
                        let radius = (chrome.height * 0.08).max(1.0);
                        let cy = rect.top + rect.height() * 0.62;
                        let cx = rect.left + rect.width() * 0.5;
                        let step = radius * 4.0;
                        for dot in [-1.0f32, 0.0, 1.0] {
                            canvas.draw_circle((cx + dot * step, cy), radius, &paint);
                        }
                    })
                    .event(move |_arena, event, _size| match event {
                        Event::MouseDown {
                            button: MouseButton::Left,
                            ..
                        } => EventResult::Command(FoldCommand::Unfold),
                        Event::AnimationClock { now } if animating => {
                            EventResult::Command(FoldCommand::Tick(*now))
                        }
                        _ => EventResult::Ignored,
                    });
                let _ = arena;
                chip
            },
        )
    }
}
