// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Sessions drawer's LINES: one recency list read as a three-column
//! grid — the repo in a right-aligned margin column, the title, the age
//! — so projects interleave and still read (the Sessions mock). Group
//! heads, the host foot and the "+ …" affordances are lines too, so the
//! list's keyboard walk and its flat selection wash cover them all.

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    layout::{Alignment, Insets, Layout, LayoutBox, LayoutExt as _, Row},
    store::Store,
    ui::UiCtx,
    Thunk, ThunkBox, View, Widget, WidgetBox,
};
use skia_safe::{Rect, Size};

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
    /// The open session, a running or blocked turn: the live accent.
    Live,
    /// The last turn failed.
    Failed,
}

#[derive(Clone)]
pub(crate) enum LineKind {
    /// A recency group's caps head ("TODAY").
    Group(&'static str),
    Session {
        repo: String,
        title: String,
        form: TitleForm,
        /// Changed since the user last looked — the title takes the
        /// primary ink instead of the row's quieter one.
        unviewed: bool,
        age: String,
        ink: AgeInk,
        /// The window's open session: the here row, a size up.
        here: bool,
    },
    /// A host that is not connected: its name and its state's word.
    Host {
        name: String,
        status: String,
        failed: bool,
    },
    /// The host's error, in the margin register under its name.
    Error(String),
    /// A quiet "+ …" line in the grid ("+ New Session…" under its
    /// host's name, "+ Add Host…").
    Affordance { repo: String, label: String },
}

/// The air a line keeps above itself — whitespace groups.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lead {
    None,
    /// The list's first line.
    Top,
    /// A group head after the first.
    Group,
    /// The first line of the foot: air, then a hairline, then air.
    Foot,
    /// A second host in the foot.
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
            LineKind::Group(caps) => caps,
            LineKind::Session { title, .. } => title,
            LineKind::Host { name, .. } => name,
            LineKind::Error(error) => error,
            LineKind::Affordance { label, .. } => label,
        }
    }

    /// The margin column's word, when the line has one.
    pub(crate) fn repo(&self) -> &str {
        match &self.kind {
            LineKind::Session { repo, .. } | LineKind::Affordance { repo, .. } => repo,
            _ => "",
        }
    }

    pub(crate) fn age(&self) -> &str {
        match &self.kind {
            LineKind::Session { age, .. } => age,
            _ => "",
        }
    }

    /// Heads (groups, hosts) stand at depth 0; what they head at 1.
    pub(crate) fn depth(&self) -> usize {
        match &self.kind {
            LineKind::Group(_) | LineKind::Host { .. } => 0,
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
        // (docs/ui/list-keyboard.md §2).
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            let themes = ::editor::env::Themes::of(store);
            let theme = themes.ui();
            let chrome = theme.sessions.clone();
            let rule = theme.ink.rule.0;
            // Measuring passes lay rows unbounded; the grid then
            // takes its natural width.
            let room = (constraints.max.width < f32::MAX / 2.0)
                .then(|| (constraints.max.width - 2.0 * chrome.inset).max(1.0));
            let body = self.body(arena, store, ui, room);
            let lead = match self.lead {
                Lead::None => 0.0,
                Lead::Top => chrome.top_gap,
                Lead::Group => chrome.group_gap,
                Lead::Foot => chrome.foot_gap,
                Lead::Host => chrome.host_gap,
            };
            let body: ThunkBox<'a, Self::Command> = match self.lead {
                // The foot stands apart: a hairline across the
                // content, its air under it.
                Lead::Foot => ThunkBox::new(
                    arena,
                    hikit::ui::Hairline::new(rule).top().wrap(
                        imba::layout::fixed(body)
                            .pad_insets(Insets {
                                top: chrome.foot_pad,
                                ..Insets::default()
                            })
                            .align(Alignment::TopStart)
                            .width(room.unwrap_or(f32::NAN))
                            .layout(arena, constraints.loosen()),
                    ),
                ),
                _ => body,
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
        let theme = themes.ui();
        let chrome = theme.sessions.clone();
        let ink = theme.ink.clone();
        let meta = hikit::ui::brief(store, ui).sized(chrome.meta_size);
        let loose = Constraints {
            min: Size::default(),
            max: Size::new(room.unwrap_or(f32::MAX), f32::MAX),
        };
        match &self.kind {
            LineKind::Group(caps) => hikit::ui::text(&hikit::ui::caps(store, ui), *caps)
                .pad_insets(Insets {
                    left: chrome.repo_width + chrome.column_gap,
                    ..Insets::default()
                })
                .layout(arena, loose),
            LineKind::Session {
                repo,
                title,
                form,
                unviewed,
                age,
                ink: age_ink,
                here,
            } => {
                let accent = match age_ink {
                    AgeInk::Quiet => ink.ink_3.0,
                    AgeInk::Live => ink.live.0,
                    AgeInk::Failed => ink.removed.0,
                };
                let repo_ink = match here {
                    true => ink.live.0,
                    false => ink.ink_3.0,
                };
                let title_style = match (here, form, unviewed) {
                    (_, TitleForm::Untitled | TitleForm::Command, _) => {
                        hikit::ui::row(store, ui).colored(ink.ink_3.0)
                    }
                    (true, TitleForm::Plain, _) => hikit::ui::TextStyle {
                        font: hikit::fonts::role_font(ui, &chrome.here_title),
                        color: ink.ink_bright.0,
                        tracking: chrome.here_title.tracking,
                        shaper: imba::layout::TextShaper::of(ui),
                    },
                    (false, TitleForm::Plain, true) => hikit::ui::row(store, ui).colored(ink.ink.0),
                    (false, TitleForm::Plain, false) => {
                        hikit::ui::row(store, ui).colored(chrome.title_color.0)
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
                let (line, top, bottom) = match here {
                    true => (
                        chrome.here_line,
                        chrome.here_pad_top,
                        chrome.here_pad_bottom,
                    ),
                    false => (chrome.row_line, chrome.row_pad, chrome.row_pad),
                };
                Grid {
                    repo: hikit::ui::text(&meta.clone().colored(repo_ink), repo.clone()),
                    title,
                    age: hikit::ui::text(&meta.clone().colored(accent), age.clone()),
                    repo_width: chrome.repo_width,
                    gap: chrome.column_gap,
                    room,
                }
                .align(Alignment::CenterStart)
                .height(line)
                .pad_insets(Insets {
                    top,
                    bottom,
                    ..Insets::default()
                })
                .layout(arena, loose)
            }
            LineKind::Affordance { repo, label } => Grid {
                repo: hikit::ui::text(&meta, repo.clone()),
                title: LayoutBox::new(
                    arena,
                    hikit::ui::text(&hikit::ui::brief(store, ui), label.clone()),
                ),
                age: hikit::ui::text(&meta, String::new()),
                repo_width: chrome.repo_width,
                gap: chrome.column_gap,
                room,
            }
            .align(Alignment::CenterStart)
            .height(chrome.row_line)
            .pad_insets(Insets::xy(0.0, chrome.row_pad))
            .layout(arena, loose),
            LineKind::Host {
                name,
                status,
                failed,
            } => {
                let name_style = hikit::ui::TextStyle {
                    font: hikit::fonts::role_font(ui, &chrome.host_name),
                    color: ink.ink_3.0,
                    tracking: chrome.host_name.tracking,
                    shaper: imba::layout::TextShaper::of(ui),
                };
                let status_style = hikit::ui::row(store, ui)
                    .sized(chrome.host_status_size)
                    .colored(match failed {
                        true => ink.removed.0,
                        false => ink.ink_3.0,
                    });
                Row::new(arena)
                    .gap(chrome.host_word_gap)
                    .child_by_baseline(hikit::ui::text(&name_style, name.clone()))
                    .child_by_baseline(Oblique::new(
                        hikit::ui::text(&status_style, status.clone()),
                        chrome.oblique_skew,
                    ))
                    .layout(arena, loose)
            }
            LineKind::Error(error) => hikit::ui::text(
                &hikit::ui::margin(store, ui).sized(chrome.error_size),
                error.clone(),
            )
            .pad_insets(Insets {
                top: chrome.error_gap,
                ..Insets::default()
            })
            .layout(arena, loose),
        }
    }
}

/// The session grid: the repo right-aligned in its margin column, the
/// title taking what the age leaves (and ellipsizing there), the age
/// at the right — one shared baseline.
struct Grid<'a> {
    repo: imba::layout::Text,
    title: LayoutBox<'a, std::convert::Infallible>,
    age: imba::layout::Text,
    repo_width: f32,
    gap: f32,
    /// The content width, when the line is laid for real.
    room: Option<f32>,
}

impl imba::layout::LayoutValue for Grid<'_> {}

impl<'a> Layout<'a, std::convert::Infallible> for Grid<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> ThunkBox<'a, std::convert::Infallible> {
        let Grid {
            repo,
            title,
            age,
            repo_width,
            gap,
            room,
        } = self;
        let age = Layout::<std::convert::Infallible>::layout(age, arena, constraints.loosen());
        let title_width = room.map_or(f32::NAN, |room| {
            (room - repo_width - 2.0 * gap - age.size().width).max(0.0)
        });
        Row::new(arena)
            .gap(gap)
            .child_by_baseline(repo.align(Alignment::TopEnd).width(repo_width))
            .child_by_baseline(title.width(title_width))
            .child_by_baseline(imba::layout::fixed(age))
            .layout(arena, constraints)
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
        canvas.translate((0.0, self.baseline));
        canvas.skew((self.skew, 0.0));
        canvas.translate((0.0, -self.baseline));
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
