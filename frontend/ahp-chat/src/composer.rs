// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::{editor_view::EditorCommand, editor_view::EditorView, env};
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
    ui::UiCtx,
    View, Widget,
};
use skia_safe::{Rect, Size};

use crate::cell::document_text;

type ChatChrome = editor::theme::ChatChrome;

#[derive(Clone)]
pub enum ComposerCommand {
    Editor(ScrollCommand<EditorCommand>),

    Submit,

    Stop,

    ToggleExpand,

    Tick(AnimationClock),

    Rewrap(f32),
}

impl std::fmt::Display for ComposerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposerCommand::Editor(command) => command.fmt(out),
            ComposerCommand::Submit => out.write_str("composer submit"),
            ComposerCommand::Stop => out.write_str("composer stop"),
            ComposerCommand::ToggleExpand => out.write_str("composer toggle expand"),
            ComposerCommand::Tick(_) => out.write_str("composer tick"),
            ComposerCommand::Rewrap(_) => out.write_str("composer rewrap"),
        }
    }
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

fn fresh_input(store: &imba::store::Store, ui: &imba::ui::UiCtx) -> ScrollView<EditorView> {
    let document = editor::document::Document::new(
        text::text::Text::from_string_exact(""),
        editor::markup::Markup::new(),
    )
    .with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    );
    let fonts = hikit::fonts::source()();
    let theme = editor::theme::Theme::embedded();
    let mut view = EditorView::of_document(document, 600.0, store, ui, &fonts, &theme);
    view.set_placeholder("Message the agent", &fonts, &theme);
    ScrollView::new(view)
}

impl Composer {
    pub(crate) fn document(&self) -> &editor::document::Document {
        &self.input.content().document
    }

    pub(crate) fn document_mut(&mut self) -> &mut editor::document::Document {
        &mut self.input.content_mut().document
    }

    pub(crate) fn editor(&self) -> ::editor::editor::EditorId {
        self.input.content().editor
    }

    pub(crate) fn new(store: &imba::store::Store, ui: &imba::ui::UiCtx) -> Self {
        let mut input = fresh_input(store, ui);
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

    pub(crate) fn clear(&mut self, store: &imba::store::Store, ui: &imba::ui::UiCtx) {
        self.input = fresh_input(store, ui);
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
                            .resize(editor, width, 0, store, ui, &fonts, &theme, fx);
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
            imba::layout::Layout::layout(
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

        // The input row's companions (docs/himark-islands-default):
        // attach and mention, both funneling into the mention road —
        // a mentioned file IS the send's attachment.
        {
            use imba::leaf::leaf;
            let icon_color = theme.ui().peeker.dim_text.0;
            let shaper = imba::layout::TextShaper::of(ui);
            let at_font = hikit::fonts::ui_text_font(ui, 16.0);
            let mention = move || {
                ComposerCommand::Editor(ScrollCommand::Content(EditorCommand::Paste {
                    text: "@".to_owned(),
                }))
            };
            let at = leaf::<ComposerCommand>(22.0, 22.0)
                .paint_below({
                    let shaper = shaper.clone();
                    let at_font = at_font.clone();
                    move |_arena, canvas, rect| {
                        shaper.draw(
                            canvas,
                            &at_font,
                            "@",
                            icon_color,
                            0.0,
                            rect.left + 3.0,
                            rect.top + 16.0,
                        );
                    }
                })
                .event(move |_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::command(mention()),
                    _ => EventResult::Ignored,
                });
            let clip = leaf::<ComposerCommand>(22.0, 22.0)
                .paint_below(move |_arena, canvas, rect| {
                    let mut paint = skia_safe::Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.4);
                    paint.set_stroke_cap(skia_safe::PaintCap::Round);
                    paint.set_color(icon_color);
                    let (x, y) = (rect.left, rect.top + 2.0);
                    let mut path = skia_safe::PathBuilder::new();
                    path.move_to((x + 6.2, y + 6.0));
                    path.line_to((x + 6.2, y + 11.0));
                    path.quad_to((x + 6.2, y + 14.0), (x + 9.0, y + 14.0));
                    path.quad_to((x + 11.8, y + 14.0), (x + 11.8, y + 11.0));
                    path.line_to((x + 11.8, y + 5.5));
                    path.quad_to((x + 11.8, y + 2.8), (x + 9.4, y + 2.8));
                    path.quad_to((x + 7.0, y + 2.8), (x + 7.0, y + 5.5));
                    path.line_to((x + 7.0, y + 10.6));
                    canvas.draw_path(&path.detach(), &paint);
                })
                .event(move |_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::command(mention()),
                    _ => EventResult::Ignored,
                });
            band.place(width - pad - 24.0, box_pad + 4.0, at);
            band.place(width - pad - 50.0, box_pad + 4.0, clip);
        }

        let rewrap =
            ((self.input.content().layout_width() - editor_w).abs() > 1.0).then_some(editor_w);

        let animating = self.band.running();
        band.wrap_realized(move |inner| ComposerWidget {
            inner,
            rewrap,
            animating,
        })
    }

    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, ComposerCommand> {
        let own = imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
            "chat.toggle-composer",
            match self.expanded {
                true => "Shrink Chat Input",
                false => "Expand Chat Input to Full View",
            },
            ComposerCommand::ToggleExpand,
        )]);
        self.input
            .focus_data(store, ui)
            .map(ComposerCommand::Editor)
            .merge_under(own)
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
                        result.merge(EventResult::command(ComposerCommand::Rewrap(width)))
                    }
                    None => result,
                }
            }
            Event::AnimationClock { now } => {
                let result = self.inner.handle_event(arena, event, viewport);
                if self.animating {
                    return result.merge(EventResult::command(ComposerCommand::Tick(*now)));
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

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, ComposerCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

// ─── the BAND: composer + toolbar + send, one component ───

/// The composer BAND — the input box, the session toolbar, the
/// SEND cell and the ask stack's seat — as a view of its own,
/// sized to its natural footprint. The workbench places it under
/// the transcript; a native shell mounts it standalone at the
/// height it measures. It speaks the panel's command vocabulary
/// (the toolbar places itself with it).
pub(crate) struct ComposerBand {
    pub(crate) composer: Composer,
    pub(crate) toolbar: crate::session_toolbar::SessionToolbar,
}

impl Clone for ComposerBand {
    fn clone(&self) -> Self {
        Self {
            composer: self.composer.clone(),
            toolbar: self.toolbar.clone(),
        }
    }
}

/// What the band shows of the PANEL's state — props in, commands
/// out; the band never reads the conversation itself.
pub(crate) struct BandProps<'a> {
    pub(crate) status: String,
    pub(crate) busy: bool,
    pub(crate) link_ready: bool,
    pub(crate) focused: bool,
    pub(crate) stack: &'a crate::stack::WidgetStack,
    /// The height the composer's growth caps against — the
    /// window's where there is one, a nominal where the host asks
    /// loose.
    pub(crate) nominal_height: f32,
    /// The WELL behind the input and the toolbar row, or nothing.
    /// Whoever stands the band up decides: the workbench hands the
    /// rounded card, a mounted host that already drew one (the
    /// islands app's SwiftUI card) hands nothing — two of them on
    /// the same edge is two borders.
    pub(crate) well: Option<hikit::ui::Surface>,
}

impl ComposerBand {
    pub(crate) fn new(store: &imba::store::Store, ui: &imba::ui::UiCtx) -> Self {
        Self {
            composer: Composer::new(store, ui),
            toolbar: crate::session_toolbar::SessionToolbar::new(store, ui),
        }
    }

    /// (band, stack, toolbar) heights — the caller sizes panes and
    /// containers from these.
    pub(crate) fn heights(
        &self,
        store: &imba::store::Store,
        stack: &crate::stack::WidgetStack,
        nominal_height: f32,
    ) -> (f32, f32, f32) {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        (
            self.composer.band_height(&chrome, nominal_height),
            stack.height(&chrome),
            theme.ui().toolbar.height,
        )
    }

    pub(crate) fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        ui: &'a imba::ui::UiCtx,
        props: BandProps<'a>,
    ) -> impl imba::layout::Layout<'a, crate::chat::ChatPanelCommand> + imba::layout::LayoutValue + 'a
    {
        use crate::chat::ChatPanelCommand;
        use imba::layout::Layout as _;
        use imba::layout::LayoutExt as _;
        use imba::leaf::leaf;
        use imba::thunk_ext::ThunkExt as _;
        

        imba::layout::laid(
            move |arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let (band_h, stack_h, toolbar_h) =
                    self.heights(store, props.stack, props.nominal_height);
                let theme = env::Themes::of(store);
                let chrome = theme.ui().chat.clone();
                let size = Size::new(constraints.max.width.max(1.0), band_h + stack_h + toolbar_h);
                let mut panel = imba::container::container(arena, size);
                let status = props.status.clone();
                let composer_empty = self.composer.is_empty();
                let inset = chrome.pad;
                // ONE rounded container holds the input and the
                // toolbar row (docs/himark-islands-default.png) —
                // placed FIRST so everything paints over it; the
                // hairline-ruled strip is gone.
                if let Some(well) = props.well {
                    panel.place(
                        inset,
                        stack_h,
                        well.wrap(leaf::<ChatPanelCommand>(
                            size.width - inset * 2.0,
                            (size.height - stack_h - inset * 0.5).max(1.0),
                        )),
                    );
                }
                panel.place(
                    0.0,
                    stack_h,
                    self.composer
                        .layout(
                            arena,
                            store,
                            ui,
                            size.width,
                            size.height,
                            ComposerProps {
                                focused: props.focused,
                            },
                        )
                        .map(ChatPanelCommand::Composer),
                );

                if stack_h > 0.0 {
                    panel.place(
                        0.0,
                        0.0,
                        props
                            .stack
                            .layout(arena, ui, &chrome, size.width)
                            .map(ChatPanelCommand::Stack),
                    );
                }

                // The SEND cell's width, computed up front: the combo
                // strip's budget ends where SEND begins — the button
                // always fits and nothing ever covers it.
                let send_cell_width = {
                    let ui_theme = theme.ui();
                    let busy = props.busy;
                    let stop = busy && composer_empty;
                    let label = if stop {
                        "■ Stop"
                    } else if busy {
                        "Steer"
                    } else {
                        "Send"
                    };
                    let caps_font = hikit::fonts::ui_font(ui, ui_theme.combo.label_size * 1.1);
                    let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                    label
                        .chars()
                        .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                        .sum::<f32>()
                        + imba::layout::text_advance(ui, &key_font, "⌘⏎")
                        + ui_theme.combo.gap
                        + ui_theme.combo.pad * 2.0
                };
                let toolbar_cells_right = self.toolbar.place(
                    arena,
                    &mut panel,
                    store,
                    ui,
                    inset,
                    size.height - toolbar_h + 1.0,
                    toolbar_h - 1.0,
                    (size.width - send_cell_width - theme.ui().combo.gap - inset).max(0.0),
                );

                {
                    let ui_theme = theme.ui();
                    let busy = props.busy;
                    let sendable = !composer_empty && props.link_ready;
                    let stop = busy && composer_empty;
                    let label = if stop {
                        "■ Stop"
                    } else if busy {
                        "Steer"
                    } else {
                        "Send"
                    };
                    let caps_font = hikit::fonts::ui_font(ui, ui_theme.combo.label_size * 1.1);
                    let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                    let pad = ui_theme.combo.pad;
                    let cell_width = send_cell_width;
                    // Send is GO-green; steering and stopping a live
                    // turn are red (docs/himark-islands mock).
                    let accent = if stop || busy {
                        chrome.stop_color.0
                    } else {
                        chrome.accent.0
                    };
                    let on_accent = chrome.on_accent.0;
                    let accent_soft = ui_theme.peeker.dim_text.0;
                    let gap = ui_theme.combo.gap;
                    // The cell's two texts as a Row of `Text`s at exact
                    // baseline parity: the old per-char loop advanced by
                    // glyph width + 1.5 (= `.tracking(1.5)`) from x =
                    // left + pad, and drew the key hint a `gap` after the
                    // label's last advance (= the Row's `.gap`). Each text
                    // pads down so its baseline lands on the old
                    // mid + font.size() * 0.35 line. The accent fill and
                    // left rule stay a backdrop painter; the press is
                    // `.on_click`, minting Stop or Submit like the old
                    // event closure.
                    let cell_h = toolbar_h - 1.0;
                    let mid = cell_h * 0.5;
                    let caps_ascent = -caps_font.metrics().1.ascent;
                    let key_ascent = -key_font.metrics().1.ascent;
                    let mut row = imba::layout::Row::new(arena).gap(gap).child(
                        imba::layout::text(ui, label, caps_font.clone(), on_accent)
                            .tracking(0.2)
                            .pad_insets(imba::layout::Insets {
                                left: 0.0,
                                top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            }),
                    );
                    if !stop {
                        row = row.child(
                            imba::layout::text(ui, "⌘⏎", key_font.clone(), accent_soft).pad_insets(
                                imba::layout::Insets {
                                    left: 0.0,
                                    top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                    right: 0.0,
                                    bottom: 0.0,
                                },
                            ),
                        );
                    }
                    let cell = row
                        .pad_insets(imba::layout::Insets {
                            left: pad,
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                        })
                        .sized(cell_width, cell_h)
                        .backdrop(
                            move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                                // A full CAPSULE, not a rounded cell
                                // (docs/himark-islands-default.png).
                                let mut paint = skia_safe::Paint::default();
                                paint.set_anti_alias(true);
                                let mut fill = accent;
                                if !sendable && !busy {
                                    fill = fill.with_a(0x50);
                                }
                                paint.set_color(fill);
                                let pill = skia_safe::Rect::from_ltrb(
                                    rect.left,
                                    rect.top + 6.0,
                                    rect.right - 5.0,
                                    rect.bottom - 6.0,
                                );
                                let radius = pill.height() * 0.5;
                                canvas.draw_round_rect(pill, radius, radius, &paint);
                            },
                        )
                        .on_click(move || {
                            ChatPanelCommand::Composer(match stop {
                                true => ComposerCommand::Stop,
                                false => ComposerCommand::Submit,
                            })
                        });
                    panel.place_boxed(
                        size.width - cell_width - inset,
                        size.height - toolbar_h + 1.0,
                        cell.layout(arena, Constraints::tight(Size::new(cell_width, cell_h))),
                    );

                    // The shortcut legend (or the link's status) sits on
                    // the toolbar, right against the SEND cell. The ⌘⏎
                    // hint already lives on the cell itself.
                    let legend = match status.is_empty() {
                        true => "⎋ chat".to_owned(),
                        false => status.clone(),
                    };
                    let legend_w = imba::layout::text_advance(ui, &key_font, &legend);
                    let legend_x = size.width - cell_width - inset - gap - legend_w;
                    // Squeezed out by the combo cells? The legend yields.
                    if legend_x >= toolbar_cells_right + gap {
                        panel.place_boxed(
                            legend_x,
                            size.height - toolbar_h + 1.0,
                            imba::layout::text(ui, legend, key_font.clone(), accent_soft)
                                .pad_insets(imba::layout::Insets {
                                    left: 0.0,
                                    top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                    right: 0.0,
                                    bottom: 0.0,
                                })
                                .layout(arena, Constraints::tight(Size::new(legend_w, cell_h))),
                        );
                    }
                }
                panel
            },
        )
    }
}

/// The composer-band PANE: the band standalone at its natural
/// height — the native shell's bottom component; the workbench
/// never shows this flavor.
#[derive(Clone)]
pub struct ComposerPane {
    seat: crate::chats::PaneSeat,
}

impl ComposerPane {
    pub fn new(
        chats: imba::store::Id<crate::chats::Chats>,
        chat: ahp_wire::client::ChatUri,
    ) -> Self {
        Self {
            seat: crate::chats::PaneSeat::new(chats, chat),
        }
    }
}

impl imba::View for ComposerPane {
    type Command = crate::chat::ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w imba::store::Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.seat.focus_data(store, ui)
    }

    fn destroy(
        &mut self,
        store: &mut imba::store::Store,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.seat.park(store);
    }

    fn perform(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.seat.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                match self.seat.view.and_then(|view| {
                    crate::chats::Chats::chat_ref(store, self.seat.chats, &self.seat.chat)
                        .and_then(|panel| panel.display_band(arena, store, ui, view))
                }) {
                    Some(laid) => imba::ThunkBox::new(
                        arena,
                        imba::layout::Layout::layout(laid, arena, constraints),
                    ),
                    None => crate::chats::PaneSeat::boot_leaf(arena, constraints),
                }
            },
        )
    }
}

impl hikit::panel::PanelView for ComposerPane {
    type Place = crate::chats::ChatPlace;

    fn title(&self, store: &imba::store::Store) -> String {
        self.seat.title(store)
    }

    fn dismantle(&mut self, store: &mut imba::store::Store) {
        self.seat.dismantle(store);
    }

    fn displaced(&mut self, store: &mut imba::store::Store) {
        self.seat.park(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
