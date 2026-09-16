// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::{env, EditorCommand, EditorView};
use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View, Widget,
};
use skia_safe::{Rect, Size};

use crate::higent::cell::document_text;

type ChatChrome = crate::theme::ChatChrome;

pub enum ComposerCommand {
    Editor(ScrollCommand<EditorCommand>),

    Submit,

    Stop,

    ToggleExpand,

    Tick(AnimationClock),

    Rewrap(f32),
}

pub(crate) struct ComposerProps {
    pub focused: bool,
}

pub(crate) struct Composer {
    input: ScrollView<EditorView>,

    expanded: bool,
    band: Animation<f32>,

    blurred: bool,
}

impl Clone for Composer {
    fn clone(&self) -> Self {
        Self {
            input: self.input.clone(),
            expanded: self.expanded,
            blurred: self.blurred,
            band: self.band,
        }
    }
}

const EDITOR_CHILD: usize = 1;

fn fresh_input() -> ScrollView<EditorView> {
    let document = crate::Document::new(crate::Text::from_string_exact(""), crate::Markup::new())
        .with_syntax(
            crate::Syntax::new("markdown", None, crate::Markup::new()),
            &[],
        );
    let fonts = crate::fonts::source()();
    let theme = crate::Theme::embedded();
    let mut view = EditorView::of_document(document, 600.0, &fonts, &theme);
    view.set_placeholder("Message the agent", &fonts, &theme);
    ScrollView::new(view)
}

impl Composer {
    pub(crate) fn document(&self) -> &crate::Document {
        &self.input.content().document
    }

    pub(crate) fn document_mut(&mut self) -> &mut crate::Document {
        &mut self.input.content_mut().document
    }

    pub(crate) fn editor(&self) -> ::editor::EditorId {
        self.input.content().editor
    }

    pub(crate) fn new() -> Self {
        let mut input = fresh_input();
        input.content_mut().focus_text();
        Self {
            input,
            expanded: false,
            blurred: false,
            band: Animation::done(
                0.0,
                Motion::Ease {
                    duration_ms: 180.0,
                    easing: Easing::EaseInOut,
                },
            ),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.input.content().document.text().byte_count() == 0
    }

    pub(crate) fn text(&self) -> String {
        document_text(&self.input.content().document)
    }

    pub(crate) fn content_height(&self) -> f32 {
        self.input.content().content_height()
    }

    pub(crate) fn expanded(&self) -> bool {
        self.expanded
    }

    pub(crate) fn clear(&mut self) {
        self.input = fresh_input();
        self.input.content_mut().focus_text();
    }

    pub(crate) fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, ComposerCommand>) {
        fx.scope(ComposerCommand::Editor, |fx| self.input.destroy(store, fx));
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ComposerCommand,
        fx: &mut Effects<'_, ComposerCommand>,
    ) {
        match command {
            ComposerCommand::Editor(command) => {
                if let ScrollCommand::Content(command) = &command {
                    if !matches!(
                        command,
                        EditorCommand::ApplyRepair(_)
                            | EditorCommand::ApplyReparse(_)
                            | EditorCommand::ApplyEnrichment(_)
                            | EditorCommand::Retheme { .. }
                            | EditorCommand::Viewport { .. }
                    ) {
                        self.input.content_mut().focus_text();
                    }
                }
                fx.scope(ComposerCommand::Editor, |fx| {
                    View::perform(&mut self.input, store, ui, command, fx)
                });
            }
            ComposerCommand::ToggleExpand => {
                self.expanded = !self.expanded;
                self.band.set(if self.expanded { 1.0 } else { 0.0 });
            }
            ComposerCommand::Tick(now) => self.band.advance(now),
            ComposerCommand::Rewrap(width) => {
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                let editor = self.input.content().editor;
                fx.scope(ComposerCommand::Editor, |fx| {
                    fx.scope(ScrollCommand::Content, |fx| {
                        self.input
                            .content_mut()
                            .document
                            .resize(editor, width, 0, &fonts, &theme, fx);
                    })
                });
            }

            ComposerCommand::Submit | ComposerCommand::Stop => {}
        }
    }

    fn metrics(chrome: &ChatChrome) -> (f32, f32) {
        let pad = chrome.pad;
        (pad, pad * 0.75)
    }

    fn editor_height(&self, chrome: &ChatChrome, panel_height: f32) -> f32 {
        let (pad, box_pad) = Self::metrics(chrome);
        let one_line = chrome.title_size * 1.6;
        if self.blurred {
            return one_line;
        }
        let grown = self
            .content_height()
            .clamp(one_line, chrome.input_max_height);
        let full = (panel_height - pad * 2.0 - box_pad * 2.0).max(one_line);
        grown + (full - grown) * self.band.value()
    }

    pub(crate) fn set_blurred(&mut self, blurred: bool) {
        self.blurred = blurred;
    }

    pub(crate) fn blurred(&self) -> bool {
        self.blurred
    }

    pub(crate) fn band_height(&self, chrome: &ChatChrome, panel_height: f32) -> f32 {
        let (_pad, box_pad) = Self::metrics(chrome);
        self.editor_height(chrome, panel_height) + box_pad * 2.0
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
        panel_height: f32,
        props: ComposerProps,
    ) -> impl imba::Thunk<'a, ComposerCommand> + 'a {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let (_pad, box_pad) = Self::metrics(&chrome);
        let pad = chrome.pad;
        let band_h = self.band_height(&chrome, panel_height);
        let editor_h = self.editor_height(&chrome, panel_height);

        let editor_w = (width - pad * 2.0).max(120.0);
        let mut band = container(arena, Size::new(width, band_h));
        let focused = props.focused;

        band.place(
            pad,
            box_pad,
            imba::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(editor_w, editor_h),
                    max: Size::new(editor_w, editor_h),
                },
            )
            .map(ComposerCommand::Editor)
            .focus_scope(focused),
        );

        let rewrap =
            ((self.input.content().layout_width() - editor_w).abs() > 1.0).then_some(editor_w);

        let animating = self.band.running();
        let expanded = self.expanded;
        band.wrap_realized(move |inner| ComposerWidget {
            inner,
            rewrap,
            animating,
        })
        .commands(move || {
            vec![imba::PresentableCommand::new(
                "chat.toggle-composer",
                match expanded {
                    true => "Shrink Chat Input",
                    false => "Expand Chat Input to Full View",
                },
                ComposerCommand::ToggleExpand,
            )]
        })
    }
}

pub(crate) struct ComposerWidget<'a> {
    inner: imba::container::RealizedContainer<'a, ComposerCommand>,
    rewrap: Option<f32>,
    animating: bool,
}

impl<'a> Widget<'a, ComposerCommand> for ComposerWidget<'a> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, ComposerCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ComposerCommand> {
        match event {
            Event::Paint { .. } => {
                let result = self.inner.handle_event(arena, event, viewport);
                match self.rewrap {
                    Some(width) => {
                        result.merge(EventResult::Command(ComposerCommand::Rewrap(width)))
                    }
                    None => result,
                }
            }
            Event::AnimationClock { now } => {
                let result = self.inner.handle_event(arena, event, viewport);
                if self.animating {
                    return result.merge(EventResult::Command(ComposerCommand::Tick(*now)));
                }
                result
            }
            Event::MouseDown { .. }
            | Event::MouseDrag { .. }
            | Event::MouseUp { .. }
            | Event::Scroll { .. }
            | Event::ThemeChanged => self.inner.handle_event(arena, event, viewport),

            _ => self.inner.route_to(EDITOR_CHILD, arena, event, viewport),
        }
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, ComposerCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}
