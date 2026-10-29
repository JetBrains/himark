// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The WORKING COPY's face (Commit.dc.html): the summary strip, the
//! COMMIT CONTROL — the byline, the subject typed where the commit's
//! headline will stand, the body, the action row — and under them the
//! bare canvas of the folder's diffs. The control is the face's: its
//! boxes, its focus, the commit ask it posts; a draft survives a walk
//! away, parked by folder.

use std::sync::Arc;

use changesview::hichanges::Changes;
use editor::editor_view::{EditorCommand, EditorView};
use editor::env;
use editor::location::ResourceLocation;
use imba::effect::Effects;
use imba::event::EventResult;
use imba::thunk_ext::ThunkExt as _;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, View};
use skia_safe::Size;

use crate::canvas::{CanvasCommand, DiffCanvasView};
use crate::diff_canvas::{CanvasPlace, Totals};
use crate::faces::{byline_row, FaceFocus, FocusOn};
use crate::rows::AskOnPaint;

/// Git's conventional subject length: the margin's count turns to the
/// removed ink once the line runs past it.
const SUBJECT_SOFT_LIMIT: usize = 72;

#[derive(Clone)]
pub struct ChangesFace {
    canvas: DiffCanvasView,
    folder: ResourceLocation,
    composer: Composer,
    focus: FaceFocus,
    request: Option<hikit::panel::PanelRequest>,
}

#[derive(Clone)]
pub enum ChangesCommand {
    Canvas(CanvasCommand),
    Composer(ComposerCommand),
    /// A press in one of the face's areas claims the keyboard for it.
    Focus(FaceFocus),
}

impl std::fmt::Display for ChangesCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangesCommand::Canvas(command) => command.fmt(out),
            ChangesCommand::Composer(command) => command.fmt(out),
            ChangesCommand::Focus(_) => out.write_str("changes focus"),
        }
    }
}

#[derive(Clone)]
pub enum ComposerCommand {
    /// The SUBJECT and BODY boxes: two editors, each routing its own
    /// commands, so a click says which box it hit.
    Subject(EditorCommand),
    Body(EditorCommand),

    /// A press on the control outside both boxes: the subject takes
    /// the keyboard.
    Focus,

    /// ⌘⏎ or the Commit word: the face posts the CommitHistory ask
    /// and resets its boxes.
    Commit,

    /// The paint probe saw a box wrapped at the wrong width: the
    /// subject's and the body's measures, as laid.
    Rewrap {
        subject: f32,
        body: f32,
    },
}

impl std::fmt::Display for ComposerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposerCommand::Subject(command) => command.fmt(out),
            ComposerCommand::Body(command) => command.fmt(out),
            ComposerCommand::Focus => out.write_str("composer focus"),
            ComposerCommand::Commit => out.write_str("composer commit"),
            ComposerCommand::Rewrap { .. } => out.write_str("composer rewrap"),
        }
    }
}

/// Which of the control's two boxes holds the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ComposerFocus {
    None,
    Subject,
    Body,
}

/// The control's state: the two boxes and which holds the keyboard.
#[derive(Clone)]
pub struct Composer {
    subject: EditorView,
    body: EditorView,
    focus: ComposerFocus,
}

/// Drafts parked by displaced faces, by folder — the message being
/// written survives a walk away and back.
#[derive(Clone, Default)]
struct CommitDrafts(rpds::HashTrieMapSync<ResourceLocation, Composer>);

impl ChangesFace {
    /// The face over a working-copy canvas: the parked draft for its
    /// folder, else fresh boxes.
    pub fn over(store: &mut Store, ui: &UiCtx, canvas: DiffCanvasView) -> Self {
        let folder = canvas.source().folder().clone();
        let mut parked = None;
        store.update::<CommitDrafts>(|drafts| {
            parked = drafts.0.get(&folder).cloned();
            drafts.0.remove_mut(&folder);
        });
        Self {
            canvas,
            folder,
            composer: parked.unwrap_or_else(|| Composer::fresh(store, ui)),
            focus: FaceFocus::Canvas,
            request: None,
        }
    }

    pub fn canvas(&self) -> &DiffCanvasView {
        &self.canvas
    }

    /// (focused, message) of the commit control.
    #[doc(hidden)]
    pub fn probe_composer(&self) -> (bool, String) {
        (
            self.composer.focus != ComposerFocus::None,
            self.composer.message(),
        )
    }

    fn park(&self, store: &mut Store) {
        let (folder, composer) = (self.folder.clone(), self.composer.clone());
        store.update::<CommitDrafts>(|drafts| drafts.0.insert_mut(folder, composer));
    }

    fn perform_composer(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ComposerCommand,
        fx: &mut Effects<'_, ChangesCommand>,
    ) {
        let composer = &mut self.composer;
        match command {
            // Enter leaves the one-line subject for the body: git's
            // blank line is the control's, never typed.
            ComposerCommand::Subject(EditorCommand::Enter { .. }) => {
                composer.focus(ComposerFocus::Body)
            }
            ComposerCommand::Subject(command) => {
                if matches!(command, EditorCommand::Click { .. }) {
                    composer.focus(ComposerFocus::Subject);
                }
                fx.scope(
                    |command| ChangesCommand::Composer(ComposerCommand::Subject(command)),
                    |fx| composer.subject.perform(store, ui, command, fx),
                );
            }
            ComposerCommand::Body(command) => {
                if matches!(command, EditorCommand::Click { .. }) {
                    composer.focus(ComposerFocus::Body);
                }
                fx.scope(
                    |command| ChangesCommand::Composer(ComposerCommand::Body(command)),
                    |fx| composer.body.perform(store, ui, command, fx),
                );
            }
            ComposerCommand::Focus => composer.focus(ComposerFocus::Subject),
            // The ask carries the message; the boxes start over.
            ComposerCommand::Commit => {
                let message = composer.message();
                let history = Changes::of(store, self.canvas.changes()).map(|held| held.history());
                if let (false, Some(history)) = (message.trim().is_empty(), history) {
                    self.request = Some(hikit::panel::PanelRequest::Perform(Arc::new(
                        changesview::hihistory::CommitHistory {
                            history,
                            folder: self.folder.clone(),
                            message,
                        },
                    )));
                    self.composer = Composer::fresh(store, ui);
                }
            }
            ComposerCommand::Rewrap { subject, body } => {
                let theme = env::Themes::of(store);
                let editor = composer.subject.editor;
                fx.scope(
                    |command| ChangesCommand::Composer(ComposerCommand::Subject(command)),
                    |fx| {
                        composer
                            .subject
                            .document
                            .resize(editor, subject, 0, store, ui, &theme, fx)
                    },
                );
                let editor = composer.body.editor;
                fx.scope(
                    |command| ChangesCommand::Composer(ComposerCommand::Body(command)),
                    |fx| {
                        composer
                            .body
                            .document
                            .resize(editor, body, 0, store, ui, &theme, fx)
                    },
                );
            }
        }
    }
}

impl View for ChangesFace {
    type Command = ChangesCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChangesCommand> {
        match self.focus {
            FaceFocus::Head => self
                .composer
                .focus_data(store, ui)
                .map(ChangesCommand::Composer),
            FaceFocus::Canvas => self
                .canvas
                .focus_data(store, ui)
                .map(ChangesCommand::Canvas),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ChangesCommand,
        fx: &mut Effects<'_, ChangesCommand>,
    ) {
        match command {
            ChangesCommand::Canvas(command) => {
                fx.scope(ChangesCommand::Canvas, |fx| {
                    self.canvas.perform(store, ui, command, fx)
                });
                if let Some(request) = self.canvas.take_request() {
                    self.request = Some(request);
                }
            }
            ChangesCommand::Composer(command) => self.perform_composer(store, ui, command, fx),
            ChangesCommand::Focus(area) => {
                self.focus = area;
                if area == FaceFocus::Canvas {
                    self.composer.focus(ComposerFocus::None);
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, ChangesCommand> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{Column, Insets, Layout as _, LayoutExt as _};
            let width = constraints.max.width.max(1.0);
            let totals = self.canvas.totals(store);
            let mut column = Column::new(arena);
            // The SUMMARY strip heads the face while files stand: the
            // totals, and a press toggling every file.
            if totals.files > 0 {
                let summary = crate::diff_summary::SummaryLine::new(
                    store,
                    ui,
                    crate::diff_summary::SummarySpec {
                        totals,
                        collapsed: self.canvas.collapsed(store),
                    },
                );
                let (lead, strip) = (summary.lead(), summary.strip_height());
                let laid = imba::layout::Layout::layout(
                    summary.on_press(|| ChangesCommand::Canvas(CanvasCommand::ToggleAll)),
                    arena,
                    Constraints::tight(Size::new((width - lead).max(1.0), strip)).loosen(),
                );
                column = column.child(imba::layout::fixed(laid).pad_insets(Insets {
                    left: lead,
                    ..Default::default()
                }));
            }
            let branch = working_branch(store, &self.canvas);
            let (head, rewrap) =
                self.composer
                    .display(arena, store, ui, width, totals, branch.as_deref());
            let head = head
                .map(ChangesCommand::Composer)
                .focus_scope(self.focus == FaceFocus::Head)
                .wrap(|inner| FocusOn {
                    inner,
                    ask: ChangesCommand::Focus(FaceFocus::Head),
                })
                .wrap(move |inner| AskOnPaint {
                    inner,
                    ask: rewrap.map(ChangesCommand::Composer),
                });
            // The canvas takes the ceiling the shell gave, less the
            // head above it — the list's own viewport arithmetic.
            let head_height = imba::Thunk::size(&head).height;
            column = column.child(imba::layout::fixed(imba::ThunkBox::new(arena, head)));
            let canvas = imba::layout::Layout::layout(
                self.canvas.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(width, (constraints.min.height - head_height).max(0.0)),
                    max: Size::new(width, (constraints.max.height - head_height).max(0.0)),
                },
            )
            .map(ChangesCommand::Canvas)
            .focus_scope(self.focus == FaceFocus::Canvas)
            .wrap(|inner| FocusOn {
                inner,
                ask: ChangesCommand::Focus(FaceFocus::Canvas),
            });
            column
                .child(imba::layout::fixed(canvas))
                .width(width)
                .layout(arena, constraints)
        })
    }
}

impl hikit::panel::PanelView for ChangesFace {
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
        // Reuse IN PLACE: arm the reveal on the shared canvas — the
        // paint probe picks it up next frame.
        if let Some(key) = &place.reveal {
            self.canvas.arm_reveal(store, key.clone());
        }
        true
    }

    fn title(&self, _store: &Store) -> String {
        "Changes".to_owned()
    }

    fn subtitle(&self, _store: &Store) -> Option<String> {
        Some(self.folder.name().to_string())
    }

    fn take_request(&mut self) -> Option<hikit::panel::PanelRequest> {
        self.request.take()
    }

    fn dismantle(&mut self, store: &mut Store) {
        self.park(store);
        self.canvas.release(store);
    }

    /// Displacement ends this view as surely as closing does (the
    /// walk back mints a fresh face through the navigator, which
    /// retains the canvas again and adopts the parked draft).
    fn displaced(&mut self, store: &mut Store) {
        self.park(store);
        self.canvas.release(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The branch the working copy stands on — the byline's word.
fn working_branch(store: &Store, canvas: &DiffCanvasView) -> Option<String> {
    let history = Changes::of(store, canvas.changes())?.history();
    changesview::hihistory::History::folder(store, history, canvas.source().folder())?
        .head
        .branch
}

/// TEST SUPPORT: the commit control as it lays over empty boxes —
/// what the first file band sits under (after the summary strip).
#[doc(hidden)]
pub fn composer_band_height(store: &Store, ui: &UiCtx) -> f32 {
    Composer::fresh(store, ui).band(store, ui)
}

impl Composer {
    fn fresh(store: &Store, ui: &UiCtx) -> Self {
        Self {
            subject: fresh_input_box(
                store,
                ui,
                editor::theme::StyleId::Headline,
                None,
                "Name the change",
            ),
            body: fresh_input_box(
                store,
                ui,
                editor::theme::StyleId::Chat,
                Some("markdown"),
                "Why, not what — optional",
            ),
            focus: ComposerFocus::None,
        }
    }

    /// The message git gets: the subject on one line (a pasted break
    /// folds to a space), the body past the blank line when there is
    /// one.
    fn message(&self) -> String {
        let subject = editor_text(&self.subject)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let body = editor_text(&self.body);
        let body = body.trim();
        match body.is_empty() {
            true => subject,
            false => format!("{subject}\n\n{body}"),
        }
    }

    /// Hand the keyboard to one box — the other blurs, so one caret
    /// stands.
    fn focus(&mut self, next: ComposerFocus) {
        if self.focus == next {
            return;
        }
        self.focus = next;
        match next {
            ComposerFocus::Subject => {
                self.body.blur();
                self.subject.focus_text();
            }
            ComposerFocus::Body => {
                self.subject.blur();
                self.body.focus_text();
            }
            ComposerFocus::None => {
                self.subject.blur();
                self.body.blur();
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ComposerCommand> {
        // ⌘⏎ commits from either box; text and keys ride the focused
        // box's own focus data.
        let own = imba::focus::FocusData {
            on_key: Some(Box::new(|key, mods| match key {
                imba::event::Key::Enter if mods.command => {
                    EventResult::command(ComposerCommand::Commit)
                }
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        };
        match self.focus {
            ComposerFocus::Subject => own.merge_under(
                self.subject
                    .focus_data(store, ui)
                    .map(ComposerCommand::Subject),
            ),
            ComposerFocus::Body => {
                own.merge_under(self.body.focus_data(store, ui).map(ComposerCommand::Body))
            }
            ComposerFocus::None => own,
        }
    }

    /// The two boxes' laid heights: each its text's height, never
    /// under its minimum lines (Commit.dc.html's two-row fields) —
    /// the lines in the box's own register.
    fn boxes(&self, theme: &editor::theme::Theme) -> (f32, f32) {
        use editor::theme::ThemeStyles as _;
        let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
        let line = |id: editor::theme::StyleId| {
            let attributes = theme.resolve([id]);
            attributes.font_size.unwrap_or(0.0) * attributes.line_height.unwrap_or(1.0)
        };
        let subject = self
            .subject
            .content_height()
            .max(diff.commit_subject_min_lines as f32 * line(editor::theme::StyleId::Headline));
        let body = self
            .body
            .content_height()
            .max(diff.commit_body_min_lines as f32 * line(editor::theme::StyleId::Chat));
        (subject, body)
    }

    /// The control's height as `display` lays it: the insets, the
    /// byline, the boxes, the action row under its hairline.
    fn band(&self, store: &Store, ui: &UiCtx) -> f32 {
        let theme = env::Themes::of(store);
        let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        let byline = hikit::ui::brief(store, ui).line_height();
        let (subject, body) = self.boxes(&theme);
        let action = action_style(ui, &diff, &ink).line_height();
        diff.commit_pad_top
            + byline
            + diff.commit_subject_gap
            + subject
            + diff.commit_body_gap
            + body
            + diff.commit_action_gap
            + 1.0
            + diff.commit_action_pad
            + action
            + diff.commit_form_pad_bottom
    }

    /// The COMMIT CONTROL (Commit.dc.html): the byline, then the
    /// message typed where its headline will stand — the SUBJECT box
    /// in the headline register, its length counted in the
    /// line-number margin — the BODY box under it in the prose
    /// register, and the ACTION row: a hairline, the Commit word and
    /// its key flush right. Each box keeps room for a couple of lines
    /// and grows UNBOUNDED with its text; a press anywhere else on the
    /// control hands the subject the keyboard. Alongside: the rewrap
    /// the paint should ask for, when a box is laid at a new width.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
        totals: Totals,
        branch: Option<&str>,
    ) -> (imba::ThunkBox<'a, ComposerCommand>, Option<ComposerCommand>) {
        use imba::layout::{Alignment, Column, Fill, Insets, Layout as _, LayoutExt as _, Row};
        let theme = env::Themes::of(store);
        let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        let lead = theme
            .part(::editor::theme::WindowChrome::KEY)
            .code_text_edge;
        let measure = (width - lead - diff.commit_pad_right).max(1.0);
        let subject_w = measure.min(diff.headline_max_width);
        let body_w = measure.min(diff.commit_body_max_width);
        let (subject_h, body_h) = self.boxes(&theme);

        let byline = byline_row(arena, store, ui, None, branch, totals).pad_insets(Insets {
            left: lead,
            ..Default::default()
        });

        // The margin counts the subject: right-aligned before the text
        // edge, centered in its line box at the subject's top — nothing
        // while the box is empty, the removed ink past git's line.
        let length = editor_text(&self.subject).chars().count();
        let count = hikit::ui::margin(store, ui).colored(match length > SUBJECT_SOFT_LIMIT {
            true => ink.removed.0,
            false => ink.mono_dim.0,
        });
        let counted = match length {
            0 => String::new(),
            n => n.to_string(),
        };
        let margin = hikit::ui::text(&count, counted)
            .align(Alignment::CenterEnd)
            .pad_insets(Insets {
                right: diff.commit_count_inset,
                ..Default::default()
            })
            .sized(lead, diff.commit_count_line);
        let subject = imba::layout::fixed(
            imba::layout::Layout::layout(
                self.subject.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(subject_w, subject_h)),
            )
            .map(ComposerCommand::Subject)
            .focus_scope(self.focus == ComposerFocus::Subject),
        );
        let subject_row = Row::new(arena)
            .child(margin)
            .child(subject)
            .pad_insets(Insets {
                top: diff.commit_subject_gap,
                ..Default::default()
            });

        let body = imba::layout::fixed(
            imba::layout::Layout::layout(
                self.body.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(body_w, body_h)),
            )
            .map(ComposerCommand::Body)
            .focus_scope(self.focus == ComposerFocus::Body),
        )
        .pad_insets(Insets {
            left: lead,
            top: diff.commit_body_gap,
            ..Default::default()
        });

        let action = action_style(ui, &diff, &ink);
        let words = Row::new(arena)
            .gap(diff.commit_key_gap)
            .child_by_baseline(hikit::ui::text(&action, "Commit"))
            .child_by_baseline(hikit::ui::text(&hikit::ui::margin(store, ui), "⌘⏎"))
            .on_click(|| ComposerCommand::Commit);
        let action_row = Column::new(arena)
            .child(hikit::ui::rule_h(ink.rule_strong.0, measure))
            .child(
                Row::new(arena)
                    .weighted(1.0, Fill::new())
                    .child(words)
                    .height(action.line_height())
                    .pad_insets(Insets {
                        top: diff.commit_action_pad,
                        ..Default::default()
                    }),
            )
            .pad_insets(Insets {
                left: lead,
                top: diff.commit_action_gap,
                right: diff.commit_pad_right,
                ..Default::default()
            });

        let laid = Column::new(arena)
            .child(byline)
            .child(subject_row)
            .child(body)
            .child(action_row)
            .pad_insets(Insets {
                top: diff.commit_pad_top,
                bottom: diff.commit_form_pad_bottom,
                ..Default::default()
            })
            .width(width)
            .backdrop(hikit::ui::ground(ink.ground.0))
            .on_click(|| ComposerCommand::Focus)
            .layout(
                arena,
                Constraints {
                    min: Size::new(width, 0.0),
                    max: Size::new(width, f32::INFINITY),
                },
            );
        let rewrap = ((self.subject.layout_width() - subject_w).abs() > 1.0
            || (self.body.layout_width() - body_w).abs() > 1.0)
            .then_some(ComposerCommand::Rewrap {
                subject: subject_w,
                body: body_w,
            });
        (laid, rewrap)
    }
}

/// An editor's whole text.
pub(crate) fn editor_text(view: &EditorView) -> String {
    let text = view.document.text();
    let end = text.byte_count().min(u32::MAX as usize) as u32;
    text.view().substring(0..end)
}

/// The ACTION word's style — the Commit word's own rung, primary ink.
fn action_style(
    ui: &UiCtx,
    diff: &::hikit::chrome::DiffChrome,
    ink: &editor::theme::InkChrome,
) -> hikit::ui::TextStyle {
    hikit::ui::TextStyle {
        font: hikit::fonts::role_font(ui, &diff.commit_action),
        color: ink.ink.0,
        tracking: diff.commit_action.tracking,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// An INPUT box wearing one covering style (greedy: everything typed
/// wears it), the placeholder in the same register, the caret live.
fn fresh_input_box(
    store: &Store,
    ui: &UiCtx,
    style: editor::theme::StyleId,
    syntax: Option<&str>,
    placeholder: &str,
) -> EditorView {
    let theme = env::Themes::of(store);
    let mut markup = editor::markup::Markup::new();
    markup.push_styled_covering(0..0, style, editor::markup::StyleWeight::UNDERLAY);
    let mut document =
        editor::document::Document::new(text::text::Text::from_string_exact(""), markup);
    if let Some(language) = syntax {
        document = document.with_syntax(
            editor::markup::Syntax::new(language, None, editor::markup::Markup::new()),
            &[],
        );
    }
    let mut view = EditorView::of_document(document, 600.0, store, ui, &theme);
    view.document.mark_input(view.editor, style);
    view.set_placeholder(placeholder, ui, &theme);
    view
}
