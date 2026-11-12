// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Sessions drawer's LINES (the Sessions mock): a host is a
//! heading with its kind word and its own action; a session is a
//! mark in the gutter, a title with its age, and the folders it works
//! in; the "Older · N" fold parts the live rows from the rest. Every
//! line is a list row, so the keyboard walk and the flat selection
//! wash cover them all.

use gfx::{Rect, Size};
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    layout::{Alignment, Column, Insets, Layout, LayoutBox, LayoutExt as _, Row},
    store::Store,
    ui::UiCtx,
    Thunk, ThunkBox, View, Widget, WidgetBox,
};

/// How a session's title reads: the session's own words, or a quiet
/// italic stand-in when it has none worth showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TitleForm {
    Plain,
    /// No title yet — "untitled", italic ink 3.
    Untitled,
    /// A slash command stands as the title — italic, it is a verb,
    /// not a subject.
    Command,
}

/// What the age column says besides the age, and in which ink.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AgeInk {
    Quiet,
    /// The open session, a blocked turn: the live accent.
    Live,
    /// The last turn failed.
    Failed,
}

/// The gutter mark: what the session wants from the user.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    None,
    /// A turn finished (or is blocked) and nobody looked: the live
    /// dot.
    Attention,
    /// A turn is streaming: the quiet ring.
    Running,
}

/// A host heading's state, which picks its inks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HostState {
    Connected,
    Connecting,
    Failed,
}

#[derive(Clone)]
pub(crate) enum LineKind {
    /// A host heading: the machine's name, its kind word (the OS,
    /// or its state when it is not connected), and the trailing
    /// action the heading's activation performs.
    Host {
        name: String,
        kind: String,
        state: HostState,
        action: String,
        /// The mono key after the action, when the gesture has one.
        key: Option<String>,
    },
    /// The host's error, in the margin register under its heading.
    Error(String),
    Session {
        title: String,
        form: TitleForm,
        mark: Mark,
        /// The title takes the attention weight — changed since the
        /// user last looked.
        attention: bool,
        age: String,
        ink: AgeInk,
        /// The session's folders, " · "-joined.
        roots: String,
        /// The window's open session: the bright title, the live age.
        here: bool,
        /// A row under the Older fold: a size down, no folders line
        /// (its group's heading names them).
        old: bool,
        /// Indent steps in from the content edge — the rows under a
        /// folders heading stand two in.
        indent: u8,
    },
    /// The fold between the live rows and the rest: "Older · N",
    /// and the word that toggles it.
    Fold { count: usize, expanded: bool },
    /// A group heading under the fold: the folders its rows work in
    /// (" · "-joined), one step in from the fold.
    Folders(String),
}

/// The air a line keeps above itself — whitespace groups, a hairline
/// parts hosts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lead {
    None,
    /// The first host's heading: air, no rule.
    Top,
    /// Every host heading after the first: air, a hairline, air.
    Host,
}

#[derive(Clone)]
pub(crate) struct DrawerLine {
    pub(crate) kind: LineKind,
    pub(crate) lead: Lead,
}

impl DrawerLine {
    pub(crate) fn new(kind: LineKind, lead: Lead) -> Self {
        Self { kind, lead }
    }

    /// The word the line is FOR — what speed-search matches and the
    /// test projection reads.
    pub(crate) fn text(&self) -> &str {
        match &self.kind {
            LineKind::Host { name, .. } => name,
            LineKind::Error(error) => error,
            LineKind::Session { title, .. } => title,
            LineKind::Fold { .. } => "Older",
            LineKind::Folders(label) => label,
        }
    }

    /// The line's second word: a session's folders, a host's kind,
    /// the fold's count.
    pub(crate) fn detail(&self) -> String {
        match &self.kind {
            LineKind::Session { roots, .. } => roots.clone(),
            LineKind::Host { kind, .. } => kind.clone(),
            LineKind::Fold { count, .. } => count.to_string(),
            LineKind::Error(_) | LineKind::Folders(_) => String::new(),
        }
    }

    /// The trailing word: a session's age, a host's action, the
    /// fold's toggle.
    pub(crate) fn trail(&self) -> &str {
        match &self.kind {
            LineKind::Session { age, .. } => age,
            LineKind::Host { action, .. } => action,
            LineKind::Fold { expanded, .. } => match expanded {
                true => "show less",
                false => "show all",
            },
            LineKind::Error(_) | LineKind::Folders(_) => "",
        }
    }

    /// Hosts stand at depth 0, what they head at 1, and under a
    /// folders heading one deeper.
    pub(crate) fn depth(&self) -> usize {
        match &self.kind {
            LineKind::Host { .. } => 0,
            LineKind::Session { indent, .. } => 1 + usize::from(*indent).saturating_sub(1),
            _ => 1,
        }
    }
}

impl View for DrawerLine {
    type Command = std::convert::Infallible;

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
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        // The line body answers no press: an unclaimed click is the
        // LIST's — `Select` then `Activate(Click)`
        // (docs/ui/list-keyboard.md §2). A heading's action word is
        // what its activation DOES, not a button of its own.
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            let themes = ::editor::env::Themes::of(store);
            let chrome = themes.part(crate::chrome::SessionsChrome::KEY);
            let rule = themes.part(::editor::theme::InkChrome::KEY).rule.0;
            // Measuring passes lay rows unbounded; the body then
            // takes its natural width.
            let room = (constraints.max.width < f32::MAX / 2.0)
                .then(|| (constraints.max.width - 2.0 * chrome.inset).max(1.0));
            let body = self.body(arena, store, ui, room);
            let body: ThunkBox<'a, Self::Command> = match self.lead {
                // A host after the first stands apart: air, a
                // hairline across the content, the heading's own air.
                Lead::Host => ThunkBox::new(
                    arena,
                    hikit::ui::Hairline::new(rule).top().wrap(
                        imba::layout::fixed(body)
                            .align(Alignment::TopStart)
                            .width(room.unwrap_or(f32::NAN))
                            .layout(arena, constraints.loosen()),
                    ),
                ),
                Lead::None | Lead::Top => body,
            };
            let lead = match self.lead {
                Lead::None | Lead::Top => 0.0,
                Lead::Host => chrome.host_rule_gap,
            };
            imba::layout::fixed(body)
                .pad_insets(Insets {
                    left: chrome.inset,
                    top: lead,
                    right: chrome.inset,
                    bottom: 0.0,
                })
                .layout(arena, constraints)
        })
    }
}

impl DrawerLine {
    fn body<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        room: Option<f32>,
    ) -> ThunkBox<'a, std::convert::Infallible> {
        let themes = ::editor::env::Themes::of(store);
        let chrome = themes.part(crate::chrome::SessionsChrome::KEY);
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let loose = Constraints {
            min: Size::default(),
            max: Size::new(room.unwrap_or(f32::MAX), f32::MAX),
        };
        let brief = hikit::ui::brief(store, ui);
        match &self.kind {
            LineKind::Host {
                name,
                kind,
                state,
                action,
                key,
            } => {
                let connected = *state == HostState::Connected;
                let name_style = hikit::ui::TextStyle {
                    font: hikit::fonts::role_font(ui, &chrome.host_name),
                    color: match connected {
                        true => ink.ink.0,
                        false => ink.ink_3.0,
                    },
                    tracking: chrome.host_name.tracking,
                    shaper: imba::layout::TextShaper::of(ui),
                };
                let kind_style = brief
                    .clone()
                    .sized(chrome.host_kind_size)
                    .colored(match state {
                        HostState::Failed => ink.removed.0,
                        _ => ink.ink_3.0,
                    });
                let action_style =
                    brief
                        .clone()
                        .sized(chrome.host_action_size)
                        .colored(match connected {
                            true => ink.ink_2.0,
                            false => ink.placeholder.0,
                        });
                let key_style = hikit::ui::margin(store, ui)
                    .sized(chrome.host_key_size)
                    .colored(ink.placeholder.0);
                let kind_word: LayoutBox<'a, std::convert::Infallible> = match state {
                    HostState::Failed => LayoutBox::new(
                        arena,
                        Oblique::new(
                            hikit::ui::text(&kind_style, kind.clone()),
                            chrome.oblique_skew,
                        ),
                    ),
                    _ => LayoutBox::new(arena, hikit::ui::text(&kind_style, kind.clone())),
                };
                let mut row = Row::new(arena)
                    .gap(chrome.host_word_gap)
                    .child_by_baseline(hikit::ui::text(&name_style, name.clone()))
                    .child_by_baseline(kind_word)
                    .weighted(1.0, imba::layout::Fill::new().height(0.0))
                    .child_by_baseline(hikit::ui::text(&action_style, action.clone()));
                if let Some(key) = key {
                    row = row.child_by_baseline(
                        hikit::ui::text(&key_style, key.clone()).pad_insets(Insets {
                            left: chrome.host_key_gap - chrome.host_word_gap,
                            ..Insets::default()
                        }),
                    );
                }
                row.pad_insets(Insets {
                    top: chrome.host_top,
                    bottom: chrome.host_bottom,
                    ..Insets::default()
                })
                .width(room.unwrap_or(f32::NAN))
                .layout(arena, loose)
            }
            LineKind::Error(error) => hikit::ui::text(
                &hikit::ui::margin(store, ui)
                    .sized(chrome.error_size)
                    .colored(ink.placeholder.0),
                error.clone(),
            )
            .pad_insets(Insets {
                left: chrome.error_inset,
                bottom: chrome.error_gap,
                ..Insets::default()
            })
            .layout(arena, loose),
            LineKind::Session {
                title,
                form,
                mark,
                attention,
                age,
                ink: age_ink,
                roots,
                here,
                old,
                indent,
            } => {
                let accent = match age_ink {
                    AgeInk::Quiet => ink.ink_3.0,
                    AgeInk::Live => ink.live.0,
                    AgeInk::Failed => ink.removed.0,
                };
                let title_size = match old {
                    true => chrome.old_title_size,
                    false => chrome.title_size,
                };
                let title_color = match (here, attention, old) {
                    (true, _, _) | (_, true, _) => ink.ink_bright.0,
                    (false, false, true) => chrome.old_color.0,
                    (false, false, false) => chrome.title_color.0,
                };
                let title_style = match form {
                    TitleForm::Untitled | TitleForm::Command => {
                        brief.clone().sized(title_size).colored(ink.ink_3.0)
                    }
                    TitleForm::Plain => {
                        let role = ::editor::theme::TypeRole {
                            size: title_size,
                            weight: match attention {
                                true => chrome.attention_weight,
                                false => 400.0,
                            },
                            width: 100.0,
                            tracking: 0.0,
                            line_height: 1.0,
                        };
                        hikit::ui::TextStyle {
                            font: hikit::fonts::role_font(ui, &role),
                            color: title_color,
                            tracking: 0.0,
                            shaper: imba::layout::TextShaper::of(ui),
                        }
                    }
                };
                let title: LayoutBox<'a, std::convert::Infallible> = match form {
                    TitleForm::Plain => {
                        LayoutBox::new(arena, hikit::ui::text(&title_style, title.clone()))
                    }
                    TitleForm::Untitled | TitleForm::Command => LayoutBox::new(
                        arena,
                        Oblique::new(
                            hikit::ui::text(&title_style, title.clone()),
                            chrome.oblique_skew,
                        ),
                    ),
                };
                let age_style = brief.clone().sized(chrome.meta_size).colored(accent);
                let age: ThunkBox<'a, std::convert::Infallible> = match age_ink {
                    AgeInk::Live if !here => Layout::layout(
                        Oblique::new(
                            hikit::ui::text(&age_style, age.clone()),
                            chrome.oblique_skew,
                        ),
                        arena,
                        loose,
                    ),
                    _ => Layout::layout(hikit::ui::text(&age_style, age.clone()), arena, loose),
                };
                let inset = f32::from(*indent) * chrome.old_indent;
                let text_room =
                    room.map(|room| (room - inset - chrome.gutter - chrome.column_gap).max(0.0));
                let title_width = text_room.map_or(f32::NAN, |room| {
                    (room - chrome.column_gap - age.size().width).max(0.0)
                });
                let head = Row::new(arena)
                    .gap(chrome.column_gap)
                    .child_by_baseline(title.width(title_width))
                    .child_by_baseline(imba::layout::fixed(age))
                    .align(Alignment::CenterStart)
                    .height(chrome.row_line);
                let mut words = Column::new(arena).child(head);
                if !old && !roots.is_empty() {
                    let roots_style = brief.clone().sized(chrome.roots_size);
                    words = words.child(
                        hikit::ui::text(&roots_style, roots.clone())
                            .align(Alignment::CenterStart)
                            .width(text_room.unwrap_or(f32::NAN))
                            .height(chrome.roots_line),
                    );
                }
                let mut gutter = Column::new(arena);
                let disc = match mark {
                    Mark::None => None,
                    Mark::Attention => Some(hikit::ui::Disc::filled(ink.live.0, chrome.mark)),
                    Mark::Running => Some(hikit::ui::Disc::ring(
                        ink.ink_3.0,
                        chrome.mark,
                        chrome.mark_stroke,
                    )),
                };
                if let Some(disc) = disc {
                    gutter = gutter.child(disc.pad_insets(Insets {
                        top: ((chrome.row_line - chrome.mark) * 0.5).max(0.0),
                        ..Insets::default()
                    }));
                }
                Row::new(arena)
                    .gap(chrome.column_gap)
                    .child(gutter.width(chrome.gutter))
                    .child(words.width(text_room.unwrap_or(f32::NAN)))
                    .pad_insets(Insets {
                        left: inset,
                        top: chrome.row_pad,
                        right: 0.0,
                        bottom: chrome.row_pad,
                    })
                    .layout(arena, loose)
            }
            LineKind::Folders(label) => {
                hikit::ui::text(&brief.clone().sized(chrome.roots_size), label.clone())
                    .pad_insets(Insets {
                        left: chrome.old_indent + chrome.gutter + chrome.column_gap,
                        top: chrome.group_top,
                        bottom: chrome.group_bottom,
                        ..Insets::default()
                    })
                    .layout(arena, loose)
            }
            LineKind::Fold { count, expanded } => {
                let word = brief.clone().sized(chrome.fold_size);
                let toggle = match expanded {
                    true => "show less",
                    false => "show all",
                };
                Row::new(arena)
                    .gap(chrome.fold_gap)
                    .child_by_baseline(hikit::ui::text(&word, "Older"))
                    .child_by_baseline(hikit::ui::text(
                        &word.clone().colored(ink.ink_4.0),
                        count.to_string(),
                    ))
                    .weighted(1.0, imba::layout::Fill::new().height(0.0))
                    .child_by_baseline(hikit::ui::text(&word.clone().colored(ink.ink_2.0), toggle))
                    .pad_insets(Insets {
                        left: chrome.gutter + chrome.column_gap,
                        top: chrome.fold_top,
                        bottom: chrome.fold_bottom,
                        ..Insets::default()
                    })
                    .width(room.unwrap_or(f32::NAN))
                    .layout(arena, loose)
            }
        }
    }
}

/// A synthetic OBLIQUE: the chrome face has no italic cut, so the
/// label paints skewed about its own baseline — and claims the
/// overhang its slanted top reaches past the upright box.
pub(crate) struct Oblique<L> {
    inner: L,
    skew: f32,
}

impl<L> Oblique<L> {
    pub(crate) fn new(inner: L, skew: f32) -> Self {
        Self { inner, skew }
    }
}

impl<L> imba::layout::LayoutValue for Oblique<L> {}

impl<'a, Command: 'a, L: Layout<'a, Command> + 'a> Layout<'a, Command> for Oblique<L> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        // The overhang is the slant over the ascent: the box widens
        // by it so the last glyph's slanted top is not clipped.
        let probe = self.inner.layout(arena, constraints);
        let baseline = probe.first_baseline().unwrap_or(probe.size().height);
        let overhang = (self.skew.abs() * baseline).ceil();
        let size = probe.size();
        let width = (size.width + overhang).min(constraints.max.width.max(size.width));
        ThunkBox::new(
            arena,
            ObliqueThunk {
                inner: probe,
                skew: self.skew,
                baseline,
                size: Size::new(width, size.height),
            },
        )
    }
}

struct ObliqueThunk<Inner> {
    inner: Inner,
    skew: f32,
    baseline: f32,
    size: Size,
}

impl<'a, Command: 'a, Inner: Thunk<'a, Command> + 'a> Thunk<'a, Command> for ObliqueThunk<Inner> {
    fn size(&self) -> Size {
        self.size
    }

    fn first_baseline(&self) -> Option<f32> {
        Some(self.baseline)
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        WidgetBox::new(
            arena,
            ObliqueWidget {
                inner: self.inner.realize(arena, viewport),
                skew: self.skew,
                baseline: self.baseline,
                size: self.size,
            },
        )
    }
}

struct ObliqueWidget<'a, Command> {
    inner: WidgetBox<'a, Command>,
    skew: f32,
    baseline: f32,
    size: Size,
}

impl<'a, Command: 'a> Widget<'a, Command> for ObliqueWidget<'a, Command> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        let Event::Paint { canvas, .. } = event else {
            return self.inner.handle_event(arena, event, viewport);
        };
        canvas.save();
        canvas.translate(0.0, self.baseline);
        canvas.skew(self.skew, 0.0);
        canvas.translate(0.0, -self.baseline);
        let result = self.inner.handle_event(arena, event, viewport);
        canvas.restore();
        result
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}
