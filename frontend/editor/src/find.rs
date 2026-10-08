// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Find in the document: the bar over the editor's viewport, the
//! background scan, the match tints, the walk.

use std::ops::Range;

use imba::effect::{AnyEffect, Effects};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    Thunk, View,
};
use skia_safe::{Paint, Rect, Size};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::{EditorCommand, EditorView};
use crate::markup::MarkupId;

const MAX_MATCHES: usize = 20_000;

#[derive(Clone)]
pub enum FindCommand {
    /// ⌘F: open the bar, seeded from a short one-line selection, or
    /// refocus the standing one.
    Open,

    Input(Box<EditorCommand>),

    Next,
    Previous,

    Close,

    Scanned(Scan),
}

impl std::fmt::Display for FindCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FindCommand::Open => out.write_str("find open"),
            FindCommand::Input(command) => command.fmt(out),
            FindCommand::Next => out.write_str("find next"),
            FindCommand::Previous => out.write_str("find previous"),
            FindCommand::Close => out.write_str("find close"),
            FindCommand::Scanned(_) => out.write_str("find scanned"),
        }
    }
}

#[derive(Clone)]
pub struct Scan {
    serial: u64,
    revision: u64,
    query: String,
    matches: Vec<Range<u32>>,
}

pub struct FindScanEffect {
    text: text::text::Text,
    query: String,
    serial: u64,
    revision: u64,
}

impl std::fmt::Display for FindScanEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "scan find matches for {:?}", self.query)
    }
}

impl imba::effect::Effect for FindScanEffect {
    type Result = Scan;
}

pub struct FindScanHandler;

impl imba::effect::EffectHandler<FindScanEffect> for FindScanHandler {
    async fn handle(&self, effect: FindScanEffect) -> Scan {
        Scan {
            serial: effect.serial,
            revision: effect.revision,
            query: effect.query.clone(),
            matches: scan(&effect.query, &effect.text),
        }
    }
}

#[derive(Clone)]
pub struct FindBar {
    input: EditorView,

    pub focused: bool,
    matches: Vec<Range<u32>>,
    current: usize,

    stepped: bool,

    /// The match tints on the editor the bar serves.
    installed: Option<MarkupId>,

    scanned: Option<(u64, String)>,

    launched: Option<(u64, String)>,

    serial: u64,

    lane: Option<imba::effect::CancellationToken>,
}

impl FindBar {
    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, FindCommand> {
        use imba::focus::FocusData;
        if !self.focused {
            return FocusData::default();
        }
        let own = FocusData {
            on_key: Some(Box::new(|key, mods| match key {
                Key::Enter if mods.shift => EventResult::Command(FindCommand::Previous),
                Key::Enter => EventResult::Command(FindCommand::Next),
                Key::Escape => EventResult::Command(FindCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            self.input
                .focus_data(store, ui)
                .map(|command| FindCommand::Input(Box::new(command))),
        )
    }

    fn new(store: &Store, ui: &UiCtx) -> Self {
        let mut input = EditorView::input(600.0, store, ui, crate::env::Fonts::of(store));
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

    fn seed(&mut self, store: &Store, ui: &UiCtx, query: &str) {
        let mut markup = crate::markup::Markup::new();
        markup.push_styled_covering(0..query.len() as u32, crate::theme::StyleId::Input);
        let document = Document::new(text::text::Text::from_string_exact(query), markup);
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let mut input = EditorView::of_document(document, 600.0, store, ui, &fonts, &theme);
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

    fn refocus(&mut self) {
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
            crate::caret::MultiCaret::one(crate::caret::Caret::selecting(0, end)),
        );
    }

    fn status(&self) -> String {
        match (self.matches.is_empty(), self.query().is_empty()) {
            (_, true) => String::new(),
            (true, false) => "no matches".to_owned(),
            (false, _) => format!("{}/{}", self.current + 1, self.matches.len()),
        }
    }

    /// Keep up with the editor: an emptied query takes its tints
    /// away; a changed text or query asks for a fresh scan, unless
    /// that very pair was scanned or is in flight.
    fn sync(
        &mut self,
        store: &Store,
        document: &mut Document,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let query = self.query();
        if query.is_empty() {
            if self.installed.is_some() {
                self.uninstall(store, document, ui, fx);
            }
            self.matches.clear();
            self.scanned = None;
            self.launched = None;
            self.serial += 1;
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
            revision: document.revision(),
        };
        fx.relaunch_erased(
            &mut self.lane,
            AnyEffect::new(effect).map(|scan| EditorCommand::Find(FindCommand::Scanned(scan))),
        );
    }

    fn adopt(
        &mut self,
        store: &Store,
        document: &mut Document,
        editor: EditorId,
        landed: &Scan,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if landed.serial != self.serial || self.query() != landed.query {
            return;
        }
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let matches = landed.matches.clone();

        let mut changed: Vec<Range<u32>> = self.matches.clone();
        changed.extend(matches.iter().cloned());
        let mut tints = crate::markup::Markup::new();
        for range in &matches {
            tints.push_styled(range.clone(), crate::theme::StyleId::Match);
        }
        let markup = match self.installed {
            Some(markup) => markup,
            None => {
                let markup = document.add_markup();
                document.show_markup(editor, markup);
                document.mark_scroll_stripes(editor, markup);
                self.installed = Some(markup);
                markup
            }
        };
        document.replace_markup(markup, tints, &changed, store, ui, &fonts, &theme, fx);
        self.scanned = Some((landed.revision, landed.query.clone()));

        self.current = self.current.min(matches.len().saturating_sub(1));
        self.matches = matches;
    }

    fn step(
        &mut self,
        store: &Store,
        document: &mut Document,
        editor: EditorId,
        forward: bool,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
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
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        document.reveal_selecting(editor, found, store, ui, &fonts, &theme, fx);
    }

    fn uninstall(
        &mut self,
        store: &Store,
        document: &mut Document,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let Some(markup) = self.installed.take() else {
            return;
        };
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        document.remove_markup(markup, &self.matches, store, ui, &fonts, &theme, fx);
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> impl Thunk<'a, FindCommand> + 'a {
        let chrome = crate::env::Themes::of(store).ui().search.clone();
        let height = chrome.input_height + chrome.pad;
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
            imba::layout::Layout::layout(
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
            .map(|command| FindCommand::Input(Box::new(command)))
            .focus_scope(self.focused),
        );

        let status = self.status();
        let font = status_font(ui);
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
        bar
    }
}

fn status_font(ui: &UiCtx) -> skia_safe::Font {
    struct StatusTypeface(skia_safe::Typeface);
    let typeface = ui.env(|| {
        StatusTypeface(
            crate::env::ui_typeface(ui, &[] as &[&str], skia_safe::FontStyle::normal())
                .expect("a ui typeface"),
        )
    });
    let mut font = skia_safe::Font::from_typeface(typeface.0.clone(), 22.0);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}

impl Document {
    pub fn find(&self, editor: EditorId) -> Option<&FindBar> {
        self.editor(editor).find.as_ref()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn find_mut(&mut self, editor: EditorId) -> Option<&mut FindBar> {
        self.editor_mut(editor).find.as_mut()
    }

    pub(crate) fn find_perform(
        &mut self,
        editor: EditorId,
        command: FindCommand,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let mut find = self.editor_mut(editor).find.take();
        match command {
            FindCommand::Open => {
                let seed = {
                    let caret = self.carets(editor).primary();
                    caret
                        .has_selection()
                        .then(|| caret.selection())
                        .and_then(|selection| {
                            if selection.end - selection.start > 200 {
                                return None;
                            }
                            let text = self.text().view().substring(selection);
                            (!text.contains('\n')).then_some(text)
                        })
                };
                match (&mut find, seed) {
                    (Some(find), Some(seed)) => find.seed(store, ui, &seed),
                    (Some(find), None) => find.refocus(),
                    (None, seed) => {
                        let mut fresh = FindBar::new(store, ui);
                        if let Some(seed) = &seed {
                            fresh.seed(store, ui, seed);
                        }
                        find = Some(fresh);
                    }
                }
            }
            FindCommand::Input(command) => {
                if let Some(find) = &mut find {
                    find.focused = true;
                    fx.scope(
                        |command| EditorCommand::Find(FindCommand::Input(Box::new(command))),
                        |fx| imba::View::perform(&mut find.input, store, ui, *command, fx),
                    );
                    find.stepped = false;
                }
            }
            FindCommand::Next | FindCommand::Previous => {
                let forward = matches!(command, FindCommand::Next);
                if let Some(find) = &mut find {
                    find.sync(store, self, ui, fx);
                    find.step(store, self, editor, forward, ui, fx);
                }
            }
            FindCommand::Close => {
                if let Some(mut closing) = find.take() {
                    closing.uninstall(store, self, ui, fx);
                }
            }
            FindCommand::Scanned(landed) => {
                if let Some(find) = &mut find {
                    find.adopt(store, self, editor, &landed, ui, fx);
                }
            }
        }
        if let Some(find) = &mut find {
            find.sync(store, self, ui, fx);
        }
        self.editor_mut(editor).find = find;
    }

    /// After a command on the text: the tints follow it, and a press
    /// in the text takes the keys back from the bar.
    pub(crate) fn find_sync(
        &mut self,
        editor: EditorId,
        clicked: bool,
        store: &Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let Some(mut find) = self.editor_mut(editor).find.take() else {
            return;
        };
        if clicked {
            find.focused = false;
        }
        find.sync(store, self, ui, fx);
        self.editor_mut(editor).find = Some(find);
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

fn scan(query: &str, text: &text::text::Text) -> Vec<Range<u32>> {
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
