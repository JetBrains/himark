// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use editor::{EditorCommand, EditorId, EditorView, MarkupId};
use imba::effect::AnyEffect;
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult, Key},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use crate::DocumentId;

const MAX_MATCHES: usize = 20_000;

pub enum FindCommand {
    Input(EditorCommand),

    Next,
    Previous,

    Close,

    Scanned(Scan),
}

pub struct Scan {
    serial: u64,
    document: crate::DocumentId,
    revision: u64,
    query: String,
    matches: Vec<Range<u32>>,
}

pub struct FindScanEffect {
    text: text::Text,
    query: String,
    serial: u64,
    document: crate::DocumentId,
    revision: u64,
}

impl imba::effect::Effect for FindScanEffect {
    type Result = Scan;
}

pub struct FindScanHandler;

impl imba::effect::EffectHandler<FindScanEffect> for FindScanHandler {
    async fn handle(&self, effect: FindScanEffect) -> Scan {
        Scan {
            serial: effect.serial,
            document: effect.document,
            revision: effect.revision,
            query: effect.query.clone(),
            matches: scan(&effect.query, &effect.text),
        }
    }
}

#[derive(Clone)]
pub struct FindBar {
    input: EditorView,

    pub(crate) focused: bool,
    matches: Vec<Range<u32>>,
    current: usize,

    stepped: bool,

    installed: Option<(DocumentId, EditorId, MarkupId)>,

    scanned: Option<(u64, String)>,

    launched: Option<(u64, String)>,

    serial: u64,

    lane: Option<imba::effect::CancellationToken>,
}

impl FindBar {
    pub fn new() -> Self {
        let mut input = EditorView::input(600.0, crate::fonts::source());
        input.focus_text();
        Self {
            input,
            focused: true,
            matches: Vec::new(),
            current: 0,
            stepped: false,
            installed: None,
            scanned: None,
            launched: None,
            serial: 0,
            lane: None,
        }
    }

    pub fn seed(&mut self, query: &str) {
        let mut markup = editor::Markup::new();
        markup.push_styled_covering(0..query.len() as u32, editor::theme::StyleId::Input);
        let document = editor::Document::new(text::Text::from_string_exact(query), markup);
        let mut input = EditorView::of_document(
            document,
            600.0,
            &crate::fonts::source()(),
            &editor::theme::Theme::embedded(),
        );
        input.focus_text();
        self.input = input;
        self.refocus();
        self.scanned = None;
        self.launched = None;
        self.stepped = false;
    }

    pub fn matches(&self) -> &[Range<u32>] {
        &self.matches
    }

    pub fn query(&self) -> String {
        let end = self
            .input
            .document
            .text()
            .byte_count()
            .min(u32::MAX as usize) as u32;
        self.input.document.text().view().substring(0..end)
    }

    pub fn refocus(&mut self) {
        self.focused = true;
        self.input.focus_text();
        let end = self
            .input
            .document
            .text()
            .byte_count()
            .min(u32::MAX as usize) as u32;
        self.input.document.set_carets(
            self.input.editor,
            editor::MultiCaret::one(editor::Caret::selecting(0, end)),
        );
    }

    fn status(&self) -> String {
        match (self.matches.is_empty(), self.query().is_empty()) {
            (_, true) => String::new(),
            (true, false) => "no matches".to_owned(),
            (false, _) => format!("{}/{}", self.current + 1, self.matches.len()),
        }
    }

    pub fn sync(
        &mut self,
        store: &mut Store,
        target: Option<(DocumentId, EditorId)>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if let Some((old_document, _, _)) = self.installed {
            if target.map(|(document, _)| document) != Some(old_document) {
                self.uninstall(store, fonts, theme, fx);
                self.scanned = None;
                self.launched = None;
            }
        }
        if target.is_none() {
            return;
        }
        let query = self.query();
        if query.is_empty() {
            if self.installed.is_some() {
                self.uninstall(store, fonts, theme, fx);
            }
            self.matches.clear();
            self.scanned = None;
            self.launched = None;

            self.serial += 1;
            return;
        }
    }

    pub(crate) fn launch<R: 'static>(
        &mut self,
        store: &Store,
        target: Option<(DocumentId, EditorId)>,
        fx: &mut Effects<'_, R>,
        wrap: impl Fn(Scan) -> R + Send + Sync + 'static,
    ) {
        let Some((document_id, _)) = target else {
            return;
        };
        let Some(document) = crate::OpenDocuments::document_ref(store, document_id) else {
            return;
        };
        let query = self.query();
        if query.is_empty() {
            return;
        }
        let stamp = (document.revision(), query.clone());
        if self.scanned.as_ref() == Some(&stamp) || self.launched.as_ref() == Some(&stamp) {
            return;
        }
        self.launched = Some(stamp);
        self.serial += 1;
        let effect = FindScanEffect {
            text: document.text().clone(),
            query,
            serial: self.serial,
            document: document_id,
            revision: document.revision(),
        };
        fx.relaunch_erased(&mut self.lane, AnyEffect::new(effect).map(wrap));
    }

    pub(crate) fn adopt(
        &mut self,
        store: &mut Store,
        target: Option<(DocumentId, EditorId)>,
        landed: &Scan,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if landed.serial != self.serial {
            return;
        }
        let Some((document_id, editor)) = target else {
            return;
        };
        if document_id != landed.document || self.query() != landed.query {
            return;
        }
        let Some(mut document) = crate::OpenDocuments::document(store, document_id) else {
            return;
        };
        let matches = landed.matches.clone();

        let mut changed: Vec<Range<u32>> = self.matches.clone();
        changed.extend(matches.iter().cloned());
        let mut tints = editor::Markup::new();
        for range in &matches {
            tints.push_styled(range.clone(), editor::theme::StyleId::Match);
        }
        let markup = match self.installed {
            Some((_, _, markup)) => markup,
            None => {
                let markup = document.add_markup();
                document.show_markup(editor, markup);
                document.mark_scroll_stripes(editor, markup);
                self.installed = Some((document_id, editor, markup));
                markup
            }
        };
        document.replace_markup(markup, tints, &changed, fonts, theme, fx);
        self.scanned = Some((landed.revision, landed.query.clone()));

        self.current = self.current.min(matches.len().saturating_sub(1));
        self.matches = matches;
        crate::OpenDocuments::put_document(store, document_id, document);
    }

    pub fn step(
        &mut self,
        store: &mut Store,
        forward: bool,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let Some((document_id, editor, _)) = self.installed else {
            return;
        };
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len();

        if self.stepped {
            self.current = match forward {
                true => (self.current + 1) % count,
                false => (self.current + count - 1) % count,
            };
        } else {
            self.current = self.current.min(count - 1);
            self.stepped = true;
        }
        let found = self.matches[self.current].clone();
        let Some(mut document) = crate::OpenDocuments::document(store, document_id) else {
            return;
        };
        document.reveal_selecting(editor, found, fonts, theme, fx);
        crate::OpenDocuments::put_document(store, document_id, document);
    }

    pub fn uninstall(
        &mut self,
        store: &mut Store,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let Some((document_id, _, markup)) = self.installed.take() else {
            return;
        };
        let Some(mut document) = crate::OpenDocuments::document(store, document_id) else {
            return;
        };
        document.remove_markup(markup, &self.matches, fonts, theme, fx);
        crate::OpenDocuments::put_document(store, document_id, document);
    }

    pub(crate) fn perform_input(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: EditorCommand,
        fx: &mut Effects<'_, FindCommand>,
    ) {
        self.focused = true;
        fx.scope(FindCommand::Input, |fx| {
            imba::View::perform(&mut self.input, store, ui, command, fx)
        });

        self.stepped = false;
    }

    pub fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> impl Thunk<'a, FindCommand> + 'a {
        let chrome = ::editor::env::Themes::of(store).ui().search.clone();
        let height = Self::height(&chrome);
        let pad = chrome.pad;
        let inner_height = (chrome.input_height - chrome.input_pad_y * 2.0).max(1.0);

        let status_width = 150.0;
        let mut bar = imba::container::container(arena, Size::new(width, height));

        let input_fill = chrome.input_fill;
        let well = Rect::from_xywh(
            pad,
            pad * 0.5,
            (width - pad * 2.0 - status_width).max(1.0),
            chrome.input_height,
        );
        let backdrop = imba::leaf::leaf::<FindCommand>(width, height)
            .paint_instead(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(input_fill.0);
                canvas.draw_round_rect(
                    Rect::from_xywh(
                        rect.left + well.left,
                        rect.top + well.top,
                        well.width(),
                        well.height(),
                    ),
                    8.0,
                    8.0,
                    &paint,
                );
            })
            .event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::Handled,
                _ => EventResult::Ignored,
            });
        bar.place(0.0, 0.0, backdrop);

        bar.place(
            pad + chrome.input_pad_x,
            pad * 0.5 + chrome.input_pad_y,
            imba::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(0.0, inner_height),
                    max: Size::new(
                        (well.width() - chrome.input_pad_x * 2.0).max(1.0),
                        inner_height,
                    ),
                },
            )
            .map(FindCommand::Input)
            .focus_scope(self.focused),
        );

        let status = self.status();
        let font = crate::fonts::ui_text_font(ui, 22.0);
        let label_x = width - pad - status_width;
        let label = imba::leaf::leaf::<FindCommand>(status_width, height).paint_instead(
            move |_arena, canvas, rect| {
                if status.is_empty() {
                    return;
                }
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(skia_safe::Color::from_argb(0xaa, 0x80, 0x86, 0x90));
                canvas.draw_str(
                    status.as_str(),
                    (rect.left, rect.top + rect.height() * 0.62),
                    &font,
                    &paint,
                );
            },
        );
        bar.place(label_x, 0.0, label);

        let focused = self.focused;
        let keymap =
            imba::leaf::leaf::<FindCommand>(width, height).event(move |_arena, event, _size| {
                match event {
                    Event::KeyDown { key, mods } if focused => match key {
                        Key::Enter if mods.shift => EventResult::Command(FindCommand::Previous),
                        Key::Enter => EventResult::Command(FindCommand::Next),
                        Key::Escape => EventResult::Command(FindCommand::Close),
                        _ => EventResult::Ignored,
                    },
                    _ => EventResult::Ignored,
                }
            });
        bar.place(0.0, 0.0, keymap);
        bar
    }

    pub fn height(chrome: &editor::theme::SearchChrome) -> f32 {
        chrome.input_height + chrome.pad
    }
}

fn matcher(query: &str) -> Option<regex::Regex> {
    let build = |pattern: &str| {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
    };
    build(query).or_else(|_| build(&regex::escape(query))).ok()
}

fn scan(query: &str, text: &text::Text) -> Vec<Range<u32>> {
    let Some(matcher) = matcher(query) else {
        return Vec::new();
    };
    let count = text.byte_count();
    let mut view = text.view();
    let mut matches = Vec::new();
    let mut start = 0usize;

    const WINDOW: usize = 64 * 1024;
    while start < count && matches.len() < MAX_MATCHES {
        let mut end = (start + WINDOW).min(count);
        while end < count {
            let probe_end = (end + 4096).min(count);
            let mut probe = Vec::new();
            view.byte_range_into(end, probe_end, &mut probe);
            if let Some(at) = probe.iter().position(|byte| *byte == b'\n') {
                end += at + 1;
                break;
            }
            end = probe_end;
        }
        let window = view.byte_string(start, end);
        for found in matcher.find_iter(&window).filter(|found| !found.is_empty()) {
            matches.push((start + found.start()) as u32..(start + found.end()) as u32);
            if matches.len() >= MAX_MATCHES {
                break;
            }
        }
        start = end;
    }
    matches
}
