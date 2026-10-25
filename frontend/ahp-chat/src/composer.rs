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

type ChatChrome = ::hikit::chrome::ChatChrome;

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
    // The input speaks in the PROMPT register — the same type your
    // message wears once sent (docs/Design.md). The covering style
    // is greedy, so everything typed wears it.
    let mut markup = editor::markup::Markup::new();
    markup.push_styled_covering(0..0, editor::theme::StyleId::ChatPrompt);
    let document = editor::document::Document::new(text::text::Text::from_string_exact(""), markup)
        .with_syntax(
            editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
            &[],
        );
    let theme = editor::theme::embedded();
    let mut view = EditorView::of_document(document, 600.0, store, ui, &theme);
    // The LIVE mark at the input's start is the editor's own caret
    // in the input dress (the Find query's), not a painted bar.
    view.document
        .mark_input(view.editor, editor::theme::StyleId::ChatPrompt);
    view.set_placeholder("Message the agent", ui, &theme);
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
                let theme = env::Themes::of(store);
                let editor = self.input.content().editor;
                fx.scope(ComposerCommand::Editor, |fx| {
                    fx.scope(ScrollCommand::Content, |fx| {
                        self.input
                            .content_mut()
                            .document
                            .resize(editor, width, 0, store, ui, &theme, fx);
                    })
                });
            }

            ComposerCommand::Submit | ComposerCommand::Stop => {}
        }
    }

    /// One line of the prompt register — the input's line box.
    fn prompt_line(store: &Store) -> f32 {
        let prompt = env::Themes::of(store)
            .part(::editor::theme::TypeChrome::KEY)
            .prompt;
        (prompt.size * prompt.line_height).round()
    }

    fn editor_height(&self, store: &Store, chrome: &ChatChrome, panel_height: f32) -> f32 {
        let line = Self::prompt_line(store);
        let rest = line * chrome.input_lines.max(1.0);
        if self.blurred {
            return line;
        }
        let grown = self
            .content_height()
            .clamp(rest, chrome.input_max_height.max(rest));
        let full = (panel_height - Self::chrome_height(chrome)).max(rest);
        grown + (full - grown) * self.band.value()
    }

    /// The footer's furniture around the input: the air above it, and
    /// the rule's gap and hairline under it.
    fn chrome_height(chrome: &ChatChrome) -> f32 {
        chrome.footer_top + chrome.input_rule_gap + 1.0
    }

    pub(crate) fn set_blurred(&mut self, blurred: bool) {
        self.blurred = blurred;
    }

    pub(crate) fn blurred(&self) -> bool {
        self.blurred
    }

    pub(crate) fn band_height(&self, store: &Store, chrome: &ChatChrome, panel_height: f32) -> f32 {
        self.editor_height(store, chrome, panel_height) + Self::chrome_height(chrome)
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
        let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        let grid = crate::cell::Grid::of(store, width, false);
        let band_h = self.band_height(store, &chrome, panel_height);
        let editor_h = self.editor_height(store, &chrome, panel_height);

        // The input row of the footer's grid: "YOU" in the gutter, the
        // text at the column's edge (its LIVE caret marks the start),
        // and a strong rule under it — no box, no fill.
        let editor_x = grid.edge;
        let editor_w = grid.column.max(120.0);
        let mut band = container(arena, Size::new(width, band_h));
        let focused = props.focused;

        band.place(
            editor_x,
            chrome.footer_top,
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
        {
            use imba::layout::Layout as _;
            if chrome.voice_gutter {
                band.place_boxed(
                    0.0,
                    chrome.footer_top,
                    crate::cell::voice::<ComposerCommand>(
                        arena,
                        store,
                        ui,
                        &chrome,
                        grid,
                        Self::prompt_line(store),
                        "YOU",
                    )
                    .layout(
                        arena,
                        Constraints::tight(Size::new(grid.edge, editor_h)).loosen(),
                    ),
                );
            }
            band.place_boxed(
                grid.edge,
                chrome.footer_top + editor_h + chrome.input_rule_gap,
                hikit::ui::rule_h::<ComposerCommand>(ink.rule_strong.0, grid.column)
                    .layout(arena, Constraints::tight(Size::new(width, band_h)).loosen()),
            );
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
}

impl ComposerBand {
    pub(crate) fn new(store: &imba::store::Store, ui: &imba::ui::UiCtx) -> Self {
        Self {
            composer: Composer::new(store, ui),
            toolbar: crate::session_toolbar::SessionToolbar::new(store, ui),
        }
    }

    /// (input, stack, options) heights — the caller sizes panes and
    /// containers from these.
    pub(crate) fn heights(
        &self,
        store: &imba::store::Store,
        stack: &crate::stack::WidgetStack,
        nominal_height: f32,
    ) -> (f32, f32, f32) {
        let theme = env::Themes::of(store);
        let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
        (
            self.composer.band_height(store, &chrome, nominal_height),
            stack.height(store, &chrome),
            Self::options_height(&chrome),
        )
    }

    /// The options row under the input, with the footer's air below.
    pub(crate) fn options_height(chrome: &ChatChrome) -> f32 {
        chrome.options_top + chrome.options_line + chrome.footer_bottom
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
        use imba::thunk_ext::ThunkExt as _;
        use imba::Thunk as _;

        imba::layout::laid(
            move |arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let (band_h, stack_h, toolbar_h) =
                    self.heights(store, props.stack, props.nominal_height);
                let theme = env::Themes::of(store);
                let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
                let ink = theme.part(::editor::theme::InkChrome::KEY);
                let size = Size::new(constraints.max.width.max(1.0), band_h + stack_h + toolbar_h);
                let grid = crate::cell::Grid::of(store, size.width, false);
                let mut panel = imba::container::container(arena, size);
                let composer_empty = self.composer.is_empty();

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
                            .layout(arena, store, ui, &chrome, size.width)
                            .map(ChatPanelCommand::Stack),
                    );
                }

                // The footer's hairline: the transcript ends here.
                panel.place_boxed(
                    0.0,
                    0.0,
                    hikit::ui::rule_h::<ChatPanelCommand>(ink.rule.0, size.width)
                        .layout(arena, Constraints::tight(Size::new(size.width, 1.0))),
                );

                // The OPTIONS row: the session's combos as bare words
                // from the text column's edge, SEND at its far end —
                // one baseline across (the combo cell centers its
                // value; everything else meets it there).
                let row_y = size.height - toolbar_h + chrome.options_top;
                let row_h = chrome.options_line;
                let baseline =
                    row_h * 0.5 + theme.part(::hikit::chrome::ComboChrome::KEY).value_size * 0.35;
                let on_baseline = |style: &hikit::ui::TextStyle, label: String| {
                    let ascent = -style.font.metrics().1.ascent;
                    hikit::ui::text(style, label).pad_insets(imba::layout::Insets {
                        top: (baseline - ascent).max(0.0),
                        ..Default::default()
                    })
                };

                let busy = props.busy;
                let stop = busy && composer_empty;
                // SEND steps down an ink while the link cannot carry a
                // message; stopping a live turn reads in the removed ink.
                let send_style = hikit::ui::TextStyle {
                    font: hikit::fonts::role_font(ui, &chrome.send),
                    color: match (stop, props.link_ready || busy) {
                        (true, _) => ink.removed.0,
                        (false, true) => ink.ink.0,
                        (false, false) => ink.ink_3.0,
                    },
                    tracking: chrome.send.tracking,
                    shaper: imba::layout::TextShaper::of(ui),
                };
                let key_style = hikit::ui::TextStyle {
                    font: hikit::fonts::mono_font(ui, chrome.send_key_size),
                    color: ink.ink_3.0,
                    tracking: 0.0,
                    shaper: imba::layout::TextShaper::of(ui),
                };
                let label = match (stop, busy) {
                    (true, _) => "\u{25A0} Stop",
                    (false, true) => "Steer",
                    (false, false) => "Send",
                };
                let mut send = imba::layout::Row::new(arena)
                    .gap(chrome.send_key_gap)
                    .child(on_baseline(&send_style, label.to_owned()));
                if !stop {
                    send = send.child(on_baseline(&key_style, "\u{2318}\u{23CE}".to_owned()));
                }
                let send = send
                    .on_click(move || {
                        ChatPanelCommand::Composer(match stop {
                            true => ComposerCommand::Stop,
                            false => ComposerCommand::Submit,
                        })
                    })
                    .layout(
                        arena,
                        Constraints::tight(Size::new(size.width, row_h)).loosen(),
                    );
                let send_w = send.size().width;
                let send_x = grid.edge + grid.column - send_w;

                let toolbar_cells_right = self.toolbar.place(
                    arena,
                    &mut panel,
                    store,
                    ui,
                    grid.edge,
                    row_y,
                    row_h,
                    (send_x - chrome.options_gap).max(0.0),
                );
                panel.place_boxed(send_x, row_y, send);

                // The shortcut legend (or the link's status), brief,
                // right against SEND — it yields when the words need
                // the room.
                let legend = match props.status.is_empty() {
                    true => "\u{238B} chat".to_owned(),
                    false => props.status.clone(),
                };
                let brief = hikit::ui::brief(store, ui);
                let legend_w = imba::layout::text_advance(ui, &brief.font, &legend);
                let legend_x = send_x - chrome.options_gap * 2.0 - legend_w;
                if legend_x >= toolbar_cells_right + chrome.options_gap {
                    panel.place_boxed(
                        legend_x,
                        row_y,
                        on_baseline(&brief, legend).layout(
                            arena,
                            Constraints::tight(Size::new(legend_w, row_h)).loosen(),
                        ),
                    );
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
