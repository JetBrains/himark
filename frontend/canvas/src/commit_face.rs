// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A COMMIT's face (Main.dc.html): the COMMIT BLOCK — the byline, the
//! message's first line in the headline rung, its body in a markdown
//! box — and under it the bare canvas of the commit's diffs. The
//! block is the face's: it reads the commit off the history and builds
//! its body box over the face's own effect road.

use changesview::hichanges::{CanvasSource, Changes};
use editor::editor_view::{EditorCommand, EditorView};
use editor::env;
use imba::effect::Effects;
use imba::thunk_ext::ThunkExt as _;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, View};
use skia_safe::Size;

use crate::canvas::{CanvasCommand, DiffCanvasView};
use crate::diff_canvas::CanvasPlace;
use crate::faces::{byline_row, FaceFocus, FocusOn};
use crate::rows::AskOnPaint;

#[derive(Clone)]
pub struct CommitFace {
    canvas: DiffCanvasView,
    block: Option<CommitBlock>,
    body: Body,
    focus: FaceFocus,
    request: Option<hikit::panel::PanelRequest>,
}

/// What the block says: the message (the probes read it whole), its
/// first line as the headline, the author.
#[derive(Clone)]
struct CommitBlock {
    message: String,
    headline: String,
    author: String,
    author_name: String,
}

/// The message body's box: none for a one-line message; the text
/// until the paint asks for the build; then a real editor that
/// focuses on click.
#[derive(Clone)]
enum Body {
    None,
    Pending(String),
    Built(EditorView),
}

#[derive(Clone)]
pub enum CommitCommand {
    Canvas(CanvasCommand),

    /// The body box's own commands — and the async landings of its
    /// Bounded build and markdown reparse.
    Body(EditorCommand),

    /// The first paint: read the commit off the history if the face
    /// opened before it landed, build the body box.
    Boot,

    /// The paint probe saw the body wrapped at the wrong width.
    Rewrap(f32),

    /// A press in one of the face's areas claims the keyboard for it.
    Focus(FaceFocus),
}

impl std::fmt::Display for CommitCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitCommand::Canvas(command) => command.fmt(out),
            CommitCommand::Body(command) => command.fmt(out),
            CommitCommand::Boot => out.write_str("commit boot"),
            CommitCommand::Rewrap(_) => out.write_str("commit rewrap"),
            CommitCommand::Focus(_) => out.write_str("commit focus"),
        }
    }
}

impl CommitFace {
    /// The face over a commit canvas: the block as the history tells
    /// it now, the body built on the first paint.
    pub fn over(store: &Store, canvas: DiffCanvasView) -> Self {
        let mut face = Self {
            canvas,
            block: None,
            body: Body::None,
            focus: FaceFocus::Canvas,
            request: None,
        };
        face.read(store);
        face
    }

    pub fn canvas(&self) -> &DiffCanvasView {
        &self.canvas
    }

    /// (message, author) of the block.
    #[doc(hidden)]
    pub fn probe_block(&self) -> Option<(String, String)> {
        self.block
            .as_ref()
            .map(|block| (block.message.clone(), block.author.clone()))
    }

    /// TEST SUPPORT: a block the history never sent, its body built
    /// at once.
    #[doc(hidden)]
    pub fn with_block_for_tests(
        mut self,
        store: &Store,
        ui: &UiCtx,
        message: &str,
        author_name: &str,
    ) -> Self {
        self.adopt(
            message.to_owned(),
            author_name.to_owned(),
            author_name.to_owned(),
        );
        let mut throwaway = imba::effect::Batch::new();
        self.build_body(store, ui, &mut throwaway.effects());
        self
    }

    fn commit_id(&self) -> Option<&changesview::hichanges::Revision> {
        match self.canvas.source() {
            CanvasSource::Commit { id, .. } => Some(id),
            CanvasSource::WorkingCopy { .. } => None,
        }
    }

    /// The commit off the history, when it has landed.
    fn read(&mut self, store: &Store) {
        if self.block.is_some() {
            return;
        }
        let Some(id) = self.commit_id() else {
            return;
        };
        let Some(history) = Changes::of(store, self.canvas.changes()).map(|held| held.history())
        else {
            return;
        };
        let Some(held) =
            changesview::hihistory::History::folder(store, history, self.canvas.source().folder())
        else {
            return;
        };
        let Some(commit) = held.commits.iter().find(|commit| commit.id == id.as_str()) else {
            return;
        };
        let message = commit
            .message
            .clone()
            .unwrap_or_else(|| commit.summary.clone());
        let author = match &commit.author.email {
            Some(email) => format!("{} <{}>", commit.author.name, email),
            None => commit.author.name.clone(),
        };
        self.adopt(message, author, commit.author.name.clone());
    }

    fn adopt(&mut self, message: String, author: String, author_name: String) {
        let (headline, body) = split_message(&message);
        self.body = match body {
            Some(body) => Body::Pending(body),
            None => Body::None,
        };
        self.block = Some(CommitBlock {
            message,
            headline,
            author,
            author_name,
        });
    }

    /// The body box — the CHAT CELL's markdown recipe (ahp-chat's
    /// `build_text`): a markdown document over the exact text, a
    /// Bounded build whose tail repairs and reparse land over the
    /// `Body` road. A normal editor: it focuses on click and takes
    /// the keyboard.
    fn build_body(&mut self, store: &Store, ui: &UiCtx, fx: &mut Effects<'_, CommitCommand>) {
        let Body::Pending(text) = &self.body else {
            return;
        };
        let theme = env::Themes::of(store);
        let mut document = editor::document::Document::new(
            text::text::Text::from_string_exact(text),
            editor::markup::Markup::new(),
        )
        .with_syntax(
            editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
            &[],
        );
        let view = fx.scope(CommitCommand::Body, |fx| {
            let editor = document.add_editor(
                600.0,
                None,
                editor::document::EditorBuild::Bounded,
                &[],
                store,
                ui,
                &theme,
                fx,
            );
            if let Some(parsers) = env::Parsers::of(store) {
                document.launch_reparse(parsers, fx);
            }
            EditorView {
                document,
                editor,
                reports_geometry: false,
                location: None,
                gutter_width: 0.0,
                base: None,
            }
        });
        self.body = Body::Built(view);
    }
}

impl View for CommitFace {
    type Command = CommitCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CommitCommand> {
        match (self.focus, &self.body) {
            (FaceFocus::Head, Body::Built(body)) => {
                body.focus_data(store, ui).map(CommitCommand::Body)
            }
            (FaceFocus::Head, _) => imba::focus::FocusData::default(),
            (FaceFocus::Canvas, _) => self.canvas.focus_data(store, ui).map(CommitCommand::Canvas),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: CommitCommand,
        fx: &mut Effects<'_, CommitCommand>,
    ) {
        match command {
            CommitCommand::Canvas(command) => {
                fx.scope(CommitCommand::Canvas, |fx| {
                    self.canvas.perform(store, ui, command, fx)
                });
                if let Some(request) = self.canvas.take_request() {
                    self.request = Some(request);
                }
            }
            CommitCommand::Body(command) => {
                let Body::Built(body) = &mut self.body else {
                    return;
                };
                if matches!(command, EditorCommand::Click { .. }) {
                    body.focus_text();
                }
                fx.scope(CommitCommand::Body, |fx| {
                    body.perform(store, ui, command, fx)
                });
            }
            CommitCommand::Boot => {
                self.read(store);
                self.build_body(store, ui, fx);
            }
            CommitCommand::Rewrap(width) => {
                let Body::Built(body) = &mut self.body else {
                    return;
                };
                let theme = env::Themes::of(store);
                let editor = body.editor;
                fx.scope(CommitCommand::Body, |fx| {
                    body.document
                        .resize(editor, width, 0, store, ui, &theme, fx)
                });
            }
            CommitCommand::Focus(area) => {
                self.focus = area;
                if let (FaceFocus::Canvas, Body::Built(body)) = (area, &mut self.body) {
                    body.blur();
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, CommitCommand> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{Column, Insets, Layout as _, LayoutExt as _};
            let width = constraints.max.width.max(1.0);
            let theme = env::Themes::of(store);
            let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
            let ink = theme.part(::editor::theme::InkChrome::KEY);
            let lead = theme
                .part(::editor::theme::WindowChrome::KEY)
                .code_text_edge;
            let measure = (width - lead - diff.commit_pad_right).max(1.0);

            // The COMMIT BLOCK (Main.dc.html): the byline, then the
            // message's first line in the HEADLINE rung, wrapped at
            // its measure — one Text per line, each line box the
            // rung's line height, the glyphs centered in it — and a
            // message body in its own markdown box.
            let mut column = Column::new(arena);
            if let Some(block) = &self.block {
                column = column.child(byline_row(
                    arena,
                    store,
                    ui,
                    Some(&block.author_name),
                    None,
                    self.canvas.totals(store),
                ));
                let headline_style = hikit::ui::headline(store, ui);
                let ramp = theme.part(::editor::theme::TypeChrome::KEY);
                let line = (ramp.headline.size * ramp.headline.line_height).ceil();
                let half_leading = ((line - headline_style.line_height()) * 0.5).floor();
                let mut headline = Column::new(arena);
                for text in wrap_words(
                    &headline_style,
                    &block.headline,
                    measure.min(diff.headline_max_width),
                ) {
                    headline = headline.child(
                        hikit::ui::text(&headline_style, text)
                            .pad_insets(Insets {
                                top: half_leading,
                                ..Default::default()
                            })
                            .height(line),
                    );
                }
                column = column.child(headline.pad_insets(Insets {
                    top: diff.headline_gap,
                    ..Default::default()
                }));
            }
            let mut rewrap = None;
            if let Body::Built(body) = &self.body {
                rewrap = ((body.layout_width() - measure).abs() > 1.0)
                    .then_some(CommitCommand::Rewrap(measure));
                let laid = imba::layout::Layout::layout(
                    body.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(measure, body.content_height().max(1.0))),
                )
                .map(CommitCommand::Body)
                .focus_scope(self.focus == FaceFocus::Head);
                column = column.child(imba::layout::fixed(laid).pad_insets(Insets {
                    top: diff.headline_gap,
                    ..Default::default()
                }));
            }
            // The first paint boots the block: a commit the history
            // had not landed yet, a body still unbuilt.
            let boot = self.block.is_none() || matches!(self.body, Body::Pending(_));
            let head = column
                .pad_insets(Insets {
                    left: lead,
                    top: diff.commit_pad_top,
                    right: diff.commit_pad_right,
                    bottom: diff.commit_pad_bottom,
                })
                .width(width)
                .backdrop(hikit::ui::ground(ink.ground.0))
                .layout(
                    arena,
                    Constraints {
                        min: Size::new(width, 0.0),
                        max: Size::new(width, f32::INFINITY),
                    },
                )
                .wrap(|inner| FocusOn {
                    inner,
                    ask: CommitCommand::Focus(FaceFocus::Head),
                })
                .wrap(move |inner| AskOnPaint {
                    inner,
                    ask: match boot {
                        true => Some(CommitCommand::Boot),
                        false => rewrap,
                    },
                });
            // The canvas takes the ceiling the shell gave, less the
            // head above it — the list's own viewport arithmetic.
            let head_height = imba::Thunk::size(&head).height;
            let canvas = imba::layout::Layout::layout(
                self.canvas.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(width, (constraints.min.height - head_height).max(0.0)),
                    max: Size::new(width, (constraints.max.height - head_height).max(0.0)),
                },
            )
            .map(CommitCommand::Canvas)
            .focus_scope(self.focus == FaceFocus::Canvas)
            .wrap(|inner| FocusOn {
                inner,
                ask: CommitCommand::Focus(FaceFocus::Canvas),
            });
            Column::new(arena)
                .child(imba::layout::fixed(head))
                .child(imba::layout::fixed(canvas))
                .width(width)
                .layout(arena, constraints)
        })
    }
}

impl hikit::panel::PanelView for CommitFace {
    type Place = CanvasPlace;

    fn pane_row(&self) -> Option<hikit::pane_row::PaneRow> {
        Some(hikit::pane_row::PaneRow::new(crate::CanvasRow(
            self.canvas.source().clone(),
        )))
    }

    fn navigation_location(&self, _store: &Store) -> Option<CanvasPlace> {
        Some(CanvasPlace {
            changes: self.canvas.changes(),
            source: self.canvas.source().clone(),
            reveal: None,
        })
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &CanvasPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        if place.source != *self.canvas.source() {
            return false;
        }
        if let Some(key) = &place.reveal {
            self.canvas.arm_reveal(store, key.clone());
        }
        true
    }

    fn title(&self, _store: &Store) -> String {
        "Commit".to_owned()
    }

    /// The commit's short id and the branch it sits on (its first
    /// ref, else the history's head branch).
    fn subtitle(&self, store: &Store) -> Option<String> {
        let id = self.commit_id()?;
        let short: String = id.as_str().chars().take(8).collect();
        let branch = Changes::of(store, self.canvas.changes())
            .and_then(|held| {
                changesview::hihistory::History::folder(
                    store,
                    held.history(),
                    self.canvas.source().folder(),
                )
            })
            .and_then(|held| {
                held.commits
                    .iter()
                    .find(|commit| commit.id == id.as_str())
                    .and_then(|commit| commit.refs.first().map(|at| at.name.clone()))
                    .or_else(|| held.head.branch.clone())
            });
        Some(match branch {
            Some(branch) => format!("{short} · {branch}"),
            None => short,
        })
    }

    fn take_request(&mut self) -> Option<hikit::panel::PanelRequest> {
        self.request.take()
    }

    fn dismantle(&mut self, store: &mut Store) {
        self.canvas.release(store);
    }

    fn displaced(&mut self, store: &mut Store) {
        self.canvas.release(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A commit message's headline (its first line) and its body (the
/// rest, past the blank line git puts between them), if any.
fn split_message(message: &str) -> (String, Option<String>) {
    let mut lines = message.trim_end().splitn(2, '\n');
    let headline = lines.next().unwrap_or_default().trim().to_owned();
    let body = lines
        .next()
        .map(|rest| rest.trim_matches('\n').trim_end().to_owned())
        .filter(|rest| !rest.trim().is_empty());
    (headline, body)
}

/// Greedy word wrap at `width`: the headline's lines. A word wider
/// than the measure stands alone on its line (the label ellipsizes).
fn wrap_words(style: &hikit::ui::TextStyle, content: &str, width: f32) -> Vec<String> {
    let measure = |line: &str| {
        style
            .shaper
            .tracked_advance(&style.font, line, style.tracking)
    };
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in content.split_whitespace() {
        let candidate = match line.is_empty() {
            true => word.to_owned(),
            false => format!("{line} {word}"),
        };
        if !line.is_empty() && measure(&candidate) > width {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}
