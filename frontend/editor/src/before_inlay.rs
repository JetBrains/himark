// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use imba::anim::{Animation, AnimationClock, Easing, Motion};
use imba::constraints::Constraints;
use operation::Bias;
use skia_safe::textlayout::FontCollection;
use skia_safe::Size;

use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};
use crate::editor_view::EditorView;
use crate::markup::{Inlay, InlayKey, InlayMode, MarkupId, MarkupLayer};

impl Document {
    pub fn toggle_before_inlay(
        &mut self,
        editor: EditorId,
        at: u32,
        base: &Document,
        diff: crate::diff::DiffId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(entry) = self.diff(diff) else {
            return;
        };
        let operation = entry.operation().clone();

        let mut view = self.text.view();
        if view.byte_count() == 0 {
            return;
        }
        let row = hard_lines(&mut view, at..at);

        const BLOCK_CAP_BYTES: u32 = 64 * 1024;

        let seed = operation
            .transform_offset_back(row.start, Bias::Left)
            .saturating_sub(BLOCK_CAP_BYTES + 1);
        let mut fragments = crate::diff::fragments_at(&operation, base.text(), seed);
        let mut block: Option<(Range<u32>, Range<u32>)> = None;
        let touches_row = |lines: &Range<u32>| lines.start < row.end && lines.end > row.start;
        let hit = loop {
            let Some(fragment) = fragments.next() else {
                break block.take().filter(|(lines, _)| touches_row(lines));
            };
            let lines = hard_lines(&mut view, fragment.right.clone());
            match &mut block {
                Some((standing, base_span))
                    if lines.start <= standing.end
                        && standing.end.saturating_sub(standing.start) <= BLOCK_CAP_BYTES =>
                {
                    standing.end = standing.end.max(lines.end);
                    if !fragment.left.is_empty() {
                        match base_span.start >= base_span.end {
                            true => *base_span = fragment.left.clone(),
                            false => base_span.end = base_span.end.max(fragment.left.end),
                        }
                    }
                }
                _ => {
                    if let Some(done) = block.take() {
                        if touches_row(&done.0) {
                            break Some(done);
                        }
                    }
                    if lines.start >= row.end {
                        break None;
                    }
                    block = Some((lines, fragment.left.clone()));
                }
            }
        };
        let Some((anchor, base_span)) = hit else {
            return;
        };
        if base_span.start >= base_span.end {
            return;
        }

        if let Some(markup_id) = self.editors.get(&editor).and_then(|state| state.before) {
            let standing: Vec<InlayKey> = self
                .feature_markup(markup_id)
                .map(|markup| {
                    markup
                        .all_inlays_in(anchor.clone())
                        .into_iter()
                        .map(|hit| InlayKey {
                            layer: MarkupLayer::Markup(markup_id),
                            key: hit.key.key,
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !standing.is_empty() {
                for key in standing {
                    self.remove_inlay(key, fonts, theme, fx);
                }
                return;
            }
        }

        let base_lines = hard_lines(&mut base.text().view(), base_span);

        let width = self
            .editors
            .get(&editor)
            .map(|state| state.layout.layout_width())
            .unwrap_or(400.0);
        let mut before = base.clone();

        let wash_id = before.add_markup();
        let mut wash = crate::markup::Markup::new();
        wash.push_styled(base_lines.clone(), crate::theme::StyleId::DiffDeleted);
        before.replace_markup(
            wash_id,
            wash,
            &[],
            fonts,
            theme,
            &mut imba::effect::Batch::new().effects(),
        );
        let fragment_set = before.add_fragment_set();
        let fragment_key = before.add_fragment(fragment_set, base_lines.clone());
        let before_editor = before.add_editor(
            width,
            Some(fragment_key),
            crate::document::EditorBuild::Complete,
            &[wash_id],
            fonts,
            theme,
            &mut imba::effect::Batch::new().effects(),
        );
        let mut card = EditorView {
            document: before,
            editor: before_editor,
            reports_geometry: false,
            location: None,
            // The fragment renders its own gutter — the BASE
            // document's line numbers, aligned with the host's
            // columns (cards only exist where the host shows one).
            gutter_width: theme.ui().editor_gutter.width,
            base: None,
        };
        card.blur();

        let Some(markup_id) = self.before_markup_of(editor) else {
            return;
        };
        self.push_inlay(
            markup_id,
            anchor,
            Inlay::new(InlayMode::Above, BeforeInlay::appearing(card, base_lines))
                .over_aligned(crate::markup::INLAY_HOST),
            fonts,
            theme,
            fx,
        );
    }

    pub fn expand_before_inlays(
        &mut self,
        editor: EditorId,
        base: &Document,
        diff: crate::diff::DiffId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let Some(entry) = self.diff(diff) else {
            return;
        };
        let operation = entry.operation().clone();
        let mut view = self.text.view();
        if view.byte_count() == 0 {
            return;
        }
        const BLOCK_CAP_BYTES: u32 = 64 * 1024;
        let mut fragments = crate::diff::fragments_at(&operation, base.text(), 0);
        let mut anchors: Vec<u32> = Vec::new();
        let mut block: Option<(Range<u32>, Range<u32>)> = None;
        let complete = |block: Option<(Range<u32>, Range<u32>)>, anchors: &mut Vec<u32>| {
            if let Some((lines, span)) = block {
                if span.start < span.end {
                    anchors.push(lines.start);
                }
            }
        };
        while let Some(fragment) = fragments.next() {
            let lines = hard_lines(&mut view, fragment.right.clone());
            match &mut block {
                Some((standing, base_span))
                    if lines.start <= standing.end
                        && standing.end.saturating_sub(standing.start) <= BLOCK_CAP_BYTES =>
                {
                    standing.end = standing.end.max(lines.end);
                    if !fragment.left.is_empty() {
                        match base_span.start >= base_span.end {
                            true => *base_span = fragment.left.clone(),
                            false => base_span.end = base_span.end.max(fragment.left.end),
                        }
                    }
                }
                _ => {
                    complete(block.take(), &mut anchors);
                    block = Some((lines, fragment.left.clone()));
                }
            }
        }
        complete(block.take(), &mut anchors);
        for at in anchors {
            self.toggle_before_inlay(editor, at, base, diff, fonts, theme, fx);
        }
    }

    fn before_markup_of(&mut self, editor: EditorId) -> Option<MarkupId> {
        if let Some(id) = self.editors.get(&editor)?.before {
            return Some(id);
        }
        let id = self.add_markup();
        let state = self.editors.get_mut(&editor)?;
        state.before = Some(id);
        state.markups.push(id);
        state.owned_markups.push(id);
        Some(id)
    }

    #[doc(hidden)]
    pub fn before_inlay_views(&self, editor: EditorId) -> Vec<(InlayKey, BeforeInlay)> {
        let Some(id) = self.editors.get(&editor).and_then(|state| state.before) else {
            return Vec::new();
        };
        self.feature_markup(id)
            .map(|markup| {
                markup
                    .all_inlays_in(0..u32::MAX)
                    .into_iter()
                    .filter_map(|hit| {
                        let card = hit.inlay.view_as::<BeforeInlay>()?;
                        Some((
                            InlayKey {
                                layer: MarkupLayer::Markup(id),
                                key: hit.key.key,
                            },
                            card.clone(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn before_inlays(&self, editor: EditorId) -> Vec<(Range<u32>, Range<u32>, String)> {
        let Some(id) = self.editors.get(&editor).and_then(|state| state.before) else {
            return Vec::new();
        };
        self.feature_markup(id)
            .map(|markup| {
                markup
                    .all_inlays_in(0..u32::MAX)
                    .into_iter()
                    .filter_map(|hit| {
                        let card = hit.inlay.view_as::<BeforeInlay>()?;
                        Some((
                            hit.range.clone(),
                            card.base_range.clone(),
                            card.shown_text(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn hard_lines(view: &mut text::TextView, range: Range<u32>) -> Range<u32> {
    let count = view.byte_count();
    if count == 0 {
        return 0..0;
    }
    let start_line = view.line_at((range.start as usize).min(count));
    let start = view.line_start_offset(start_line) as u32;
    let last = (range.end.max(range.start.saturating_add(1)) as usize - 1).min(count - 1);
    let last_line = view.line_at(last);
    let end = view.line_end_offset(last_line) as u32;
    start..end.max(start)
}

pub enum BeforeCommand {
    Editor(crate::editor_view::EditorCommand),

    Rewrap(f32),

    Tick(AnimationClock),
}

#[derive(Clone)]
pub struct BeforeInlay {
    view: EditorView,

    pub base_range: Range<u32>,

    grow: Animation<f32>,
}

impl BeforeInlay {
    fn appearing(view: EditorView, base_range: Range<u32>) -> Self {
        let motion = Motion::Ease {
            duration_ms: 160.0,
            easing: Easing::EaseOut,
        };
        let mut grow = Animation::done(0.0, motion);
        grow.set(1.0);
        Self {
            view,
            base_range,
            grow,
        }
    }

    #[doc(hidden)]
    pub fn shown_text(&self) -> String {
        self.view
            .document
            .text()
            .view()
            .substring(self.base_range.clone())
    }

    #[doc(hidden)]
    pub fn is_appearing(&self) -> bool {
        self.grow.running()
    }

    #[doc(hidden)]
    pub fn card_width(&self) -> f32 {
        self.view.layout_width()
    }

    #[doc(hidden)]
    pub fn card_focus(&self) -> crate::editor_view::EditorFocus {
        self.view.focus()
    }

    fn card_size(&self, constraints: Constraints) -> Size {
        let width = constraints.max.width.max(120.0);
        let natural = self.view.content_height();
        Size::new(
            width,
            (natural * self.grow.value().clamp(0.0, 1.0)).max(1.0),
        )
    }
}

impl imba::View for BeforeInlay {
    type Command = BeforeCommand;

    fn perform(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::UiCtx,
        command: BeforeCommand,
        fx: &mut imba::effect::Effects<'_, BeforeCommand>,
    ) {
        match command {
            BeforeCommand::Editor(command) => {
                use crate::editor_view::EditorCommand;

                if !matches!(
                    command,
                    EditorCommand::ApplyRepair(_)
                        | EditorCommand::ApplyReparse(_)
                        | EditorCommand::Retheme { .. }
                        | EditorCommand::Viewport { .. }
                ) {
                    self.view.focus_text();
                }
                fx.scope(BeforeCommand::Editor, |fx| {
                    imba::View::perform(&mut self.view, store, ui, command, fx)
                });
            }
            BeforeCommand::Rewrap(width) => {
                let fonts = crate::env::ui_collection(store, ui);
                let theme = crate::env::Themes::of(store);
                fx.scope(BeforeCommand::Editor, |fx| {
                    self.view
                        .document
                        .resize(self.view.editor, width, 0, &fonts, &theme, fx)
                });
            }
            BeforeCommand::Tick(now) => self.grow.advance(now),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, BeforeCommand> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                use imba::thunk_ext::ThunkExt;
                let size = self.card_size(constraints);
                let appearing = self.grow.running();
                // The fragment sits FLUSH — no frame, no pads: its
                // own gutter carries the base document's numbers and
                // its columns line up with the host editor's. The
                // animated height clips the reveal.
                let want = (size.width - self.view.gutter_width).max(120.0);
                let mut container = imba::container::container(arena, size);
                let editor = imba::Layout::layout(
                    self.view.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(size.width, 0.0),
                        max: Size::new(size.width, f32::MAX),
                    },
                )
                .map(BeforeCommand::Editor);
                container.place(0.0, 0.0, editor);

                let stale = !appearing && (self.view.layout_width() - want).abs() > 1.0;
                container.wrap(move |inner| CardWidget {
                    stale,
                    want,
                    animating: appearing,
                    inner,
                })
            },
        )
    }
}

struct CardWidget<Inner> {
    stale: bool,
    want: f32,
    animating: bool,
    inner: Inner,
}

impl<'a, Inner: imba::Widget<'a, BeforeCommand>> imba::Widget<'a, BeforeCommand>
    for CardWidget<Inner>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, BeforeCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &imba::arena::Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<BeforeCommand> {
        use imba::event::{Event, EventResult};
        let result = self.inner.handle_event(arena, event, viewport);
        if !matches!(result, EventResult::Ignored | EventResult::Handled) {
            return result;
        }
        if self.animating {
            if let Event::AnimationClock { now } = event {
                return EventResult::Command(BeforeCommand::Tick(*now));
            }
        }
        if self.stale && matches!(event, Event::Paint { .. }) {
            return EventResult::Command(BeforeCommand::Rewrap(self.want));
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, BeforeCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}
