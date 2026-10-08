// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The hover card: a rest-armed ask under the pointer's word, shown
//! as the editor's own overlay above it. Who answers the ask is the
//! host's business (`HoverEffect`).

use imba::effect::{CancellationToken, Effects};
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::ui::UiCtx;

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::linecol::LineCol;
use crate::location::ResourceLocation;

#[derive(Clone, Debug)]
pub struct HoverInfo {
    pub markdown: String,
}

pub struct HoverEffect {
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl std::fmt::Display for HoverEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "hover /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for HoverEffect {
    type Result = Option<HoverInfo>;
}

#[derive(Clone)]
pub struct HoverFound {
    serial: u64,
    answer: Option<HoverInfo>,
}

const HOVER_REST_MS: f32 = 450.0;

#[derive(Clone)]
struct Arming {
    location: ResourceLocation,
    rested: f32,
    last: Option<imba::anim::AnimationClock>,
}

/// One editor's hover: the word under the pointer, the rest that
/// arms the ask, and the card once it lands.
#[derive(Clone, Default)]
pub struct Hover {
    lane: Option<CancellationToken>,
    serial: u64,

    anchor: Option<std::ops::Range<u32>>,

    arming: Option<Arming>,

    card: Option<HoverView>,
}

impl Hover {
    pub fn open(&self) -> bool {
        self.card.is_some()
    }

    pub fn armed(&self) -> bool {
        self.arming.is_some()
    }

    pub(crate) fn anchor(&self) -> Option<std::ops::Range<u32>> {
        self.anchor.clone()
    }

    pub(crate) fn card(&self) -> Option<&HoverView> {
        self.card.as_ref()
    }

    /// The pointer rests over `byte` (or nowhere): a new word arms
    /// the ask, the same word keeps what stands, no word retracts.
    pub(crate) fn sync(
        &mut self,
        document: &Document,
        byte: Option<u32>,
        location: &ResourceLocation,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let word = byte.and_then(|byte| word_range(document, byte));
        if word == self.anchor {
            return;
        }
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            eprintln!("[lsp] hover sync byte={byte:?} word={word:?} (was {:?})", self.anchor);
        }
        self.retract(fx);
        self.anchor = word.clone();
        if word.is_none() || location.is_synthetic() {
            return;
        }
        self.arming = Some(Arming {
            location: location.clone(),
            rested: 0.0,
            last: None,
        });
    }

    pub(crate) fn tick(
        &mut self,
        document: &Document,
        now: imba::anim::AnimationClock,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let (Some(arming), Some(word)) = (&mut self.arming, self.anchor.clone()) else {
            return;
        };
        arming.rested += arming
            .last
            .map(|last| now.millis_since(last) as f32)
            .unwrap_or(0.0);
        arming.last = Some(now);
        if arming.rested < HOVER_REST_MS {
            return;
        }
        let arming = self.arming.take().expect("matched above");
        self.serial += 1;
        let serial = self.serial;
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            eprintln!("[lsp] hover ask #{serial} for {word:?}");
        }
        let mut view = document.text().view();
        let position = crate::linecol::line_col_at(&mut view, word.start as usize);
        let effect = imba::effect::AnyEffect::new(HoverEffect {
            location: arming.location,
            position,
        })
        .map(move |answer| EditorCommand::HoverFound(HoverFound { serial, answer }));
        fx.relaunch_erased(&mut self.lane, effect);
    }

    pub(crate) fn land(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &Document,
        found: HoverFound,
    ) {
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            eprintln!(
                "[lsp] hover landed #{} (standing #{}, anchor {:?}, answer {})",
                found.serial,
                self.serial,
                self.anchor,
                found.answer.as_ref().map_or(0, |info| info.markdown.len())
            );
        }
        if found.serial != self.serial {
            return;
        }
        let (Some(word), Some(info)) = (self.anchor.clone(), found.answer) else {
            return;
        };
        if info.markdown.trim().is_empty() {
            return;
        }
        let len = document.text().byte_count().min(u32::MAX as usize) as u32;
        if word.end > len {
            return;
        }
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        self.card = Some(HoverView::build(store, &info.markdown, ui, &fonts, &theme));
    }

    pub(crate) fn retract(&mut self, fx: &mut Effects<'_, EditorCommand>) {
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() && (self.card.is_some() || self.arming.is_some() || self.lane.is_some()) {
            eprintln!("[lsp] hover retract (card {}, arming {}, lane {})", self.card.is_some(), self.arming.is_some(), self.lane.is_some());
        }
        if let Some(token) = self.lane.take() {
            fx.cancel(token);
        }
        self.card = None;
        self.anchor = None;
        self.arming = None;
    }
}

fn word_range(document: &Document, byte: u32) -> Option<std::ops::Range<u32>> {
    let mut view = document.text().view();
    let len = view.byte_count().min(u32::MAX as usize) as u32;
    if byte > len {
        return None;
    }
    let line = view.line_at(byte as usize);
    let line_start = view.line_start_offset(line) as u32;
    let line_text = {
        let end = view.line_end_offset(line) as u32;
        view.substring(line_start..end)
    };
    let local = (byte - line_start) as usize;
    let bytes = line_text.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';

    if local >= bytes.len() || !is_ident(bytes[local]) {
        return None;
    }
    let mut start = local;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = local + 1;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }
    Some(line_start + start as u32..line_start + end as u32)
}

const CARD_WIDTH: f32 = 560.0;
const CARD_PAD: f32 = 10.0;

/// The card: the answer's markdown, rendered by an editor of its own.
#[derive(Clone)]
pub struct HoverView {
    view: crate::editor_view::EditorView,
}

impl HoverView {
    fn build(
        store: &Store,
        markdown: &str,
        ui: &UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Self {
        let text = text::text::Text::from_string_exact(markdown);
        let document = match crate::env::Parsers::of(store) {
            Some(parsers) => {
                Document::from_language(text, "markdown", &parsers, store, ui, fonts, theme)
            }
            None => Document::new(text, crate::markup::Markup::new()).with_syntax(
                crate::markup::Syntax::new("markdown", None, crate::markup::Markup::new()),
                &[],
            ),
        };
        Self {
            view: crate::editor_view::EditorView::complete(
                document, CARD_WIDTH, store, ui, fonts, theme,
            ),
        }
    }
}

impl imba::View for HoverView {
    type Command = EditorCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                let theme = crate::env::Themes::of(store);
                let fill = theme.ui().combo.menu_fill.0;
                let document = &self.view.document;

                let inner_width = document.max_width(self.view.editor).clamp(60.0, CARD_WIDTH);
                let inner_height = document.content_height(self.view.editor);
                let width = inner_width + CARD_PAD * 2.0;
                let height = inner_height + CARD_PAD * 2.0;
                let editor = imba::layout::Layout::layout(
                    self.view.display(arena, store, ui),
                    arena,
                    imba::constraints::Constraints {
                        min: skia_safe::Size::new(inner_width, inner_height),
                        max: skia_safe::Size::new(inner_width, f32::MAX),
                    },
                );
                let mut card =
                    imba::container::container(arena, skia_safe::Size::new(width, height));
                card.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<Self::Command>(width, height).paint_instead(
                        move |_arena, canvas, rect| {
                            let mut paint = skia_safe::Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(fill);
                            canvas.draw_round_rect(rect, 8.0, 8.0, &paint);
                        },
                    ),
                );
                card.place(CARD_PAD, CARD_PAD, editor);
                card
            },
        )
    }
}

impl Document {
    /// The pointer moved over this editor (or off it): the hover
    /// follows the word under it.
    pub(crate) fn hover_sync(
        &mut self,
        editor: EditorId,
        point: Option<skia_safe::Point>,
        location: Option<&ResourceLocation>,
        store: &Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let mut hover = std::mem::take(&mut self.editor_mut(editor).hover);
        match (point, location) {
            (Some(point), Some(location)) if !location.is_synthetic() => {
                let fonts = crate::env::ui_collection(store, ui);
                let theme = crate::env::Themes::of(store);
                let byte = self.byte_at_point(editor, point.x, point.y, store, ui, &fonts, &theme);
                hover.sync(self, byte, location, fx);
            }
            _ => {
                if hover.open() || hover.armed() {
                    hover.retract(fx);
                }
            }
        }
        self.editor_mut(editor).hover = hover;
    }

    pub(crate) fn hover_tick(
        &mut self,
        editor: EditorId,
        now: imba::anim::AnimationClock,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let mut hover = std::mem::take(&mut self.editor_mut(editor).hover);
        hover.tick(self, now, fx);
        self.editor_mut(editor).hover = hover;
    }

    pub(crate) fn hover_land(
        &mut self,
        editor: EditorId,
        found: HoverFound,
        store: &Store,
        ui: &UiCtx,
    ) {
        let mut hover = std::mem::take(&mut self.editor_mut(editor).hover);
        hover.land(store, ui, self, found);
        self.editor_mut(editor).hover = hover;
    }

    pub fn hover(&self, editor: EditorId) -> &Hover {
        &self.editor(editor).hover
    }
}
