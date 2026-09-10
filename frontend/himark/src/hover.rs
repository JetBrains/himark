use imba::effect::{CancellationToken, Effects};
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::UiCtx;

use crate::{DocumentId, LineCol, ResourceLocation};

#[derive(Clone, Debug)]
pub struct HoverInfo {
    pub markdown: String,
}

pub struct LspHoverEffect {
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl imba::effect::Effect for LspHoverEffect {
    type Result = Option<HoverInfo>;
}

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

#[derive(Clone, Default)]
pub struct Hover {
    lane: Option<CancellationToken>,
    serial: u64,

    anchor: Option<std::ops::Range<u32>>,

    arming: Option<Arming>,

    installed: Option<(DocumentId, ::editor::EditorId)>,
    markup: Option<::editor::MarkupId>,
    key: Option<::editor::InlayKey>,
}

impl Hover {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync<C, E>(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        _editor: ::editor::EditorId,
        byte: Option<u32>,
        location: &ResourceLocation,
        installed: Option<(DocumentId, ::editor::EditorId)>,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) where
        C: 'static,
        E: Fn(::editor::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        let word = byte.and_then(|byte| word_range(document, byte));

        if word == self.anchor {
            return;
        }

        self.retract(store, ui, document, fx, to_editor);
        self.anchor = word.clone();
        if word.is_none() || location.is_synthetic() {
            return;
        }
        self.installed = installed;
        self.arming = Some(Arming {
            location: location.clone(),
            rested: 0.0,
            last: None,
        });
    }

    pub fn armed(&self) -> bool {
        self.arming.is_some()
    }

    pub fn tick<C, W>(
        &mut self,
        document: &crate::Document,
        now: imba::anim::AnimationClock,
        fx: &mut Effects<'_, C>,
        wrap: W,
    ) where
        C: 'static,
        W: Fn(HoverFound) -> C + Send + Sync + Clone + 'static,
    {
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
        let mut view = document.text().view();
        let position = crate::line_col_at(&mut view, word.start as usize);
        let effect = imba::effect::AnyEffect::new(LspHoverEffect {
            location: arming.location,
            position,
        })
        .map(move |answer| wrap(HoverFound { serial, answer }));
        fx.relaunch_erased(&mut self.lane, effect);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn land<C, E>(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        found: HoverFound,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) where
        C: 'static,
        E: Fn(::editor::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
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
        let markup = document.add_markup();
        document.show_markup(editor, markup);
        self.markup = Some(markup);
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let view = HoverView::build(store, &info.markdown, &fonts, &theme);
        let mut key = None;
        fx.scope(to_editor, |fx| {
            key = Some(document.push_inlay(
                markup,
                word.clone(),
                crate::Inlay::new(
                    crate::InlayMode::Popup(crate::PopupSpec {
                        host: imba::overlay::WINDOW,
                        position: imba::overlay::fit::PreferredPosition::At {
                            x: imba::overlay::fit::RangeEnd::Begin,
                            side: imba::overlay::fit::Side::Top,
                            align: imba::overlay::fit::Align::Left,
                        },
                    }),
                    view,
                ),
                &fonts,
                &theme,
                fx,
            ));
        });
        self.key = key;
    }

    pub fn open(&self) -> bool {
        self.key.is_some()
    }

    pub fn inlay_key(&self) -> Option<::editor::InlayKey> {
        self.key
    }

    pub fn retract<C, E>(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) where
        C: 'static,
        E: Fn(::editor::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        if let Some(token) = self.lane.take() {
            fx.cancel(token);
        }
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let key = self.key.take();
        let markup = self.markup.take();
        if key.is_some() || markup.is_some() {
            fx.scope(to_editor, |fx| {
                if let Some(key) = key {
                    document.remove_inlay(key, &fonts, &theme, fx);
                }

                if let Some(markup) = markup {
                    document.remove_markup(markup, &[], &fonts, &theme, fx);
                }
            });
        }
        self.installed = None;
        self.anchor = None;
        self.arming = None;
    }

    pub fn clear(&mut self) {
        self.lane = None;
        self.anchor = None;
        self.arming = None;
        self.installed = None;
        self.markup = None;
        self.key = None;
    }

    pub fn installed(&self) -> Option<(DocumentId, ::editor::EditorId)> {
        self.installed
    }
}

fn word_range(document: &crate::Document, byte: u32) -> Option<std::ops::Range<u32>> {
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

#[derive(Clone)]
pub struct HoverView {
    view: crate::EditorView,
}

impl HoverView {
    fn build(
        store: &Store,
        markdown: &str,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::Theme,
    ) -> Self {
        let text = crate::Text::from_string_exact(markdown);
        let document = match crate::env::Parsers::of(store) {
            Some(parsers) => {
                crate::Document::from_language(text, "markdown", &parsers, fonts, theme)
            }
            None => crate::Document::new(text, crate::Markup::new()).with_syntax(
                crate::Syntax::new("markdown", None, crate::Markup::new()),
                &[],
            ),
        };
        Self {
            view: crate::EditorView::complete(document, CARD_WIDTH, fonts, theme),
        }
    }
}

impl imba::View for HoverView {
    type Command = ::editor::EditorCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn layout<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        _constraints: imba::constraints::Constraints,
    ) -> impl imba::Thunk<'a, Self::Command> + 'a {
        let theme = crate::env::Themes::of(store);
        let fill = theme.ui().combo.menu_fill.0;
        let document = &self.view.document;

        let inner_width = document
            .max_width(self.view.editor)
            .clamp(60.0, CARD_WIDTH);
        let inner_height = document.content_height(self.view.editor);
        let width = inner_width + CARD_PAD * 2.0;
        let height = inner_height + CARD_PAD * 2.0;
        let editor = self.view.layout(
            arena,
            store,
            ui,
            imba::constraints::Constraints {
                min: skia_safe::Size::new(inner_width, inner_height),
                max: skia_safe::Size::new(inner_width, f32::MAX),
            },
        );
        let mut card = imba::container::container(arena, skia_safe::Size::new(width, height));
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
    }
}

#[cfg(test)]
mod tests;
