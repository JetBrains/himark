// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The canvas ROWS (docs/editor/diff-canvas.md): per changed file a
//! header row — the FILE BAND — and its diff row (a placeholder that
//! arms its build on its first paint, the built diff, a failure
//! note), their layouts, and the paint-time asks that ride a row's
//! thunk.

use crate::diff_canvas::CanvasFile;
use editor::env;
use editor::{location::ResourceLocation, unified_diff::UnifiedDiffCommand};
use imba::effect::Effects;
use imba::event::{Event, EventResult};
use imba::thunk_ext::ThunkExt;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, Thunk, View, Widget};
use skia_safe::{Paint, Rect, Size};

const MIN_EST_LINES: i64 = 4;
const MAX_EST_LINES: i64 = 60;
const EST_CONTEXT_LINES: i64 = 4;

/// What a header row's face offers — pressed on the in-flow row or on
/// its planted sticky copy alike.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderAction {
    /// The header body (and cmd-enter): open the live file in a pane.
    OpenFile,

    /// The side-by-side button: the standalone diff pane.
    OpenPane,

    /// The layout button: inline ⇄ split for this file's diff.
    ToggleFace,

    /// The chevron: fold the diff row away / bring it back.
    ToggleCollapse,
}

#[derive(Clone)]
pub enum RowCommand {
    /// The placeholder painted un-armed: build me, at this width.
    Arm(f32),

    Header(HeaderAction),

    Diff(UnifiedDiffCommand),

    Rewrap(f32),
}

impl std::fmt::Display for RowCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RowCommand::Diff(command) => command.fmt(out),
            RowCommand::Arm(_) => out.write_str("row arm"),
            RowCommand::Header(_) => out.write_str("row header"),
            RowCommand::Rewrap(_) => out.write_str("row rewrap"),
        }
    }
}

#[derive(Clone)]
pub(crate) enum RowBody {
    Placeholder { armed: bool },
    Failed(String),
    Built { pane: crate::diff_pane::PairPane },
}

#[derive(Clone)]
pub enum CanvasRow {
    Header(HeaderRow),
    Diff(DiffRow),
}

#[derive(Clone)]
pub struct HeaderRow {
    pub(crate) file: CanvasFile,
    pub(crate) collapsed: bool,

    /// Whether a diff stands behind this header — the face toggle
    /// only shows then.
    pub(crate) built: bool,

    /// "file N of M" — restamped when the membership moves.
    pub(crate) ordinal: (usize, usize),

    /// The diff wears the split face — the face word offers inline.
    pub(crate) split: bool,
}

#[derive(Clone)]
pub struct DiffRow {
    pub(crate) file: CanvasFile,
    pub(crate) body: RowBody,

    /// The last rewrap target seen. A live window drag changes the
    /// width EVERY frame; resizing three editors per row per frame
    /// (plus the SetHeight/settle churn each resize drags in) is the
    /// resize storm. A rewrap only runs once the same target arrives
    /// twice — i.e. the width held still for a frame.
    pub(crate) rewrap_ask: Option<f32>,

    /// The width the standing build ran at — the reconcile relaunches
    /// a stale row at this width so the fresh build lands into the
    /// same geometry (the old view keeps showing until it does).
    pub(crate) built_width: Option<f32>,
}

pub(crate) fn trace_wrap() -> bool {
    std::env::var_os("HIMARK_TRACE_WRAP").is_some()
}

/// The file band's height — the insets around the name's line box.
pub(crate) fn header_band(theme: &editor::theme::Theme) -> f32 {
    crate::diff_header::file_band_height_of(&theme.part(::hikit::chrome::DiffChrome::KEY))
}

pub(crate) fn reserved_body(theme: &editor::theme::Theme, file: &CanvasFile) -> f32 {
    let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
    let line = chrome.title_size * 1.5;
    let known = file.added.is_some() || file.removed.is_some();
    let est = match known {
        true => (file.added.unwrap_or(0) + file.removed.unwrap_or(0) + EST_CONTEXT_LINES)
            .clamp(MIN_EST_LINES, MAX_EST_LINES),
        false => 12,
    };
    est as f32 * line + body_air(theme)
}

/// The AIR under every diff (Multi-breaks.dc.html): the next file's
/// hairline stands this far below the last line, so whitespace groups
/// the files. It is the DIFF row's tail, never the band's head — the
/// band is the sticky row, and air inside it would pin with it. Every
/// writer of a diff row's height adds it: the reserved estimate, the
/// landing, the resize, and the painted row.
pub(crate) fn body_air(theme: &editor::theme::Theme) -> f32 {
    theme.part(::hikit::chrome::DiffChrome::KEY).band_air
}

pub(crate) fn open_commands(
    location: &ResourceLocation,
) -> Vec<imba::PresentableCommand<RowCommand>> {
    let _ = location;
    vec![
        imba::PresentableCommand::new(
            "workbench.open-in-full",
            "Open File in Full",
            RowCommand::Header(HeaderAction::OpenFile),
        ),
        imba::PresentableCommand::new(
            "diff.open-pane",
            "Diff: Open in Side-by-Side Panel",
            RowCommand::Header(HeaderAction::OpenPane),
        ),
    ]
}

impl View for CanvasRow {
    type Command = RowCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, RowCommand> {
        match self {
            CanvasRow::Header(header) => {
                imba::focus::FocusData::of_commands(open_commands(&header.file.new))
            }
            CanvasRow::Diff(diff) => {
                let own = imba::focus::FocusData::of_commands(open_commands(&diff.file.new));
                match &diff.body {
                    RowBody::Built { pane } => pane
                        .focus_data(store, ui)
                        .map(RowCommand::Diff)
                        .merge_under(own),
                    _ => own,
                }
            }
        }
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        let _ = fx;
        if let CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        }) = self
        {
            documents::diff_views::teardown_diff_view(store, pane.documents(), pane.id());
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        let diff = match self {
            // Header rows carry no state of their own: header actions
            // are the CANVAS's (it owns the splices and requests).
            CanvasRow::Header(_) => return,
            CanvasRow::Diff(diff) => diff,
        };
        match command {
            RowCommand::Header(HeaderAction::ToggleFace) => {
                let RowBody::Built { pane } = &diff.body else {
                    return;
                };
                let Some(view) =
                    crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                else {
                    return;
                };
                let next = view.layout.other();
                let mut pane = *pane;
                fx.scope(RowCommand::Diff, |fx| {
                    pane.perform(store, ui, UnifiedDiffCommand::SetLayout(next), fx)
                });
            }
            RowCommand::Header(_) => {}
            RowCommand::Arm(_) => {
                if let RowBody::Placeholder { armed } = &mut diff.body {
                    *armed = true;
                }
            }
            RowCommand::Diff(command) => {
                let RowBody::Built { pane } = &diff.body else {
                    return;
                };
                let mut pane = *pane;
                fx.scope(RowCommand::Diff, |fx| pane.perform(store, ui, command, fx));
            }
            RowCommand::Rewrap(width) => {
                if diff.rewrap_ask != Some(width) {
                    if trace_wrap() {
                        eprintln!("[wrap] ask recorded {}: {width}", diff.file.new.name());
                    }
                    diff.rewrap_ask = Some(width);
                    return;
                }
                let RowBody::Built { pane } = &diff.body else {
                    if trace_wrap() {
                        eprintln!("[wrap] rewrap on UNBUILT row {}", diff.file.new.name());
                    }
                    return;
                };
                if trace_wrap() {
                    eprintln!("[wrap] rewrap RUNS {}: {width}", diff.file.new.name());
                }
                let id = pane.id();
                let documents = pane.documents();
                fx.scope(RowCommand::Diff, |fx| {
                    documents::diff_views::rewrap_pair(store, documents, ui, id, width, fx)
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        RowFrame {
            row: self,
            store,
            ui,
        }
    }
}

pub(crate) struct RowFrame<'a> {
    pub(crate) row: &'a CanvasRow,
    pub(crate) store: &'a Store,
    pub(crate) ui: &'a UiCtx,
}

impl imba::layout::LayoutValue for RowFrame<'_> {}

impl<'a> imba::layout::Layout<'a, RowCommand> for RowFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, RowCommand> {
        use imba::layout::LayoutExt as _;
        let RowFrame { row, store, ui } = self;
        let theme = env::Themes::of(store);
        let width = constraints.max.width.max(1.0);
        let gutter = theme.part(::editor::theme::EditorGutterChrome::KEY).width;
        let ground = theme.part(::editor::theme::InkChrome::KEY).ground.0;

        match row {
            CanvasRow::Header(header) => {
                // The FILE BAND (Main.dc.html): the path's words on one
                // baseline, a hairline above. A press on the title
                // folds the file; the words at the right edge flip
                // the face, open the standalone pane, open the file.
                let band = crate::diff_header::FileBand::new(store, ui, &header.file.title)
                    .counts(header.file.added, header.file.removed)
                    .collapsed(header.collapsed)
                    .ordinal(header.ordinal.0, header.ordinal.1)
                    .on_title(|| RowCommand::Header(HeaderAction::ToggleCollapse));
                let band = match header.built {
                    true => band.word(
                        match header.split {
                            true => "Inline",
                            false => "Side by side",
                        },
                        || RowCommand::Header(HeaderAction::ToggleFace),
                    ),
                    false => band,
                };
                imba::layout::Layout::layout(
                    band.word("Open pane", || RowCommand::Header(HeaderAction::OpenPane))
                        .word("Open source", || RowCommand::Header(HeaderAction::OpenFile)),
                    arena,
                    constraints,
                )
            }
            CanvasRow::Diff(diff) => match &diff.body {
                RowBody::Placeholder { armed } => {
                    let body_height = reserved_body(&theme, &diff.file);
                    let armed = *armed;
                    imba::ThunkBox::new(
                        arena,
                        skeleton_layout(arena, &theme, &diff.file, width, body_height)
                            .wrap(move |inner| ArmedPlaceholder { inner, armed }),
                    )
                }
                RowBody::Failed(error) => {
                    // The failure in the brief rung at the code text
                    // edge, the band's air around it.
                    let diff_chrome = theme.part(::hikit::chrome::DiffChrome::KEY);
                    imba::layout::Layout::layout(
                        hikit::ui::text(
                            &hikit::ui::brief(store, ui),
                            format!("{} — {error}", diff.file.title),
                        )
                        .pad_insets(imba::layout::Insets {
                            left: theme
                                .part(::editor::theme::WindowChrome::KEY)
                                .code_text_edge,
                            top: diff_chrome.band_pad_top,
                            right: diff_chrome.band_pad_right,
                            bottom: diff_chrome.band_pad_bottom + diff_chrome.band_air,
                        })
                        .width(width),
                        arena,
                        constraints,
                    )
                }
                RowBody::Built { pane } => {
                    let editor_target = (width - gutter).max(120.0);
                    let body = imba::layout::Layout::layout(
                        pane.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(editor_target, 0.0),
                            max: Size::new(editor_target + gutter, f32::MAX),
                        },
                    )
                    .map(RowCommand::Diff);
                    // A fresh landing is a SEED. The pane mounts and
                    // paints underneath (its own probes drive the
                    // dressing lanes), but the ROW keeps the skeleton
                    // face and its reserved band until the view answers
                    // DRESSED — loader → diff is one swap, not a
                    // striptease of markup, folds and heights arriving
                    // separately. ONE swap only: an edit undresses the
                    // face for a beat while its marks re-land, and a
                    // row that has shown its diff keeps showing it
                    // through the re-dress (`ever_dressed`) — no blink
                    // back to the skeleton under the user's caret.
                    // The list clips the row, and the container
                    // reports the reserved size, so the taller
                    // undressed body neither bleeds nor fights the
                    // list's visible-resize measure.
                    let presentable =
                        crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                            .is_none_or(|view| view.dressed() || view.ever_dressed());
                    if !presentable {
                        let body_height = reserved_body(&theme, &diff.file);
                        let mut face =
                            imba::container::container(arena, Size::new(width, body_height));
                        face.place_boxed(0.0, 0.0, imba::ThunkBox::new(arena, body));
                        face.place_boxed(
                            0.0,
                            0.0,
                            skeleton_layout(arena, &theme, &diff.file, width, body_height),
                        );
                        return imba::ThunkBox::new(
                            arena,
                            imba::thunk_ext::ThunkExt::paint_below(face, hikit::ui::ground(ground)),
                        );
                    }
                    // The row-level rewrap governs the INLINE face
                    // only. On the split face the halves report their
                    // own painted geometry (`reports_geometry`) and
                    // resize themselves to the half-pane width — a
                    // row-level rewrap to the full width would fight
                    // them every frame.
                    let rewrap =
                        match crate::diff_pane::gathered_view(store, pane.documents(), pane.id()) {
                            Some(view) => match view.layout {
                                editor::unified_diff::DiffLayout::Split => None,
                                editor::unified_diff::DiffLayout::Inline => {
                                    let laid = view
                                        .split
                                        .right
                                        .document
                                        .layout_width(view.split.right.editor);
                                    let armed = (laid - editor_target).abs() > 1.0;
                                    if armed && trace_wrap() {
                                        eprintln!(
                                            "[wrap] arm {}: laid {laid} target {editor_target}",
                                            diff.file.new.name()
                                        );
                                    }
                                    armed.then_some(editor_target)
                                }
                            },
                            None => None,
                        };
                    // No card: the diff stands on the ground, its air
                    // under it, and the next file's hairline closes it.
                    let body_height = Thunk::size(&body).height + body_air(&theme);
                    let laid = imba::layout::ZBox::new(arena)
                        .child(imba::layout::spacer(width, body_height))
                        .child(imba::layout::fixed(body))
                        .backdrop(hikit::ui::ground(ground))
                        .layout(arena, constraints);
                    imba::ThunkBox::new(
                        arena,
                        laid.wrap(move |inner| AskOnPaint {
                            inner,
                            ask: rewrap.map(RowCommand::Rewrap),
                        }),
                    )
                }
            },
        }
    }
}

// ------------------------------------------------------------- header

/// The skeleton under a pending header: rounded bars at the line
/// rhythm, tinted from the diff palette, the mix proportioned to the
/// entry's added:removed ratio — it already READS like the diff it
/// stands for.
pub(crate) fn skeleton_layout<'a>(
    arena: &'a Arena,
    theme: &editor::theme::Theme,
    file: &CanvasFile,
    width: f32,
    height: f32,
) -> imba::ThunkBox<'a, RowCommand> {
    let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
    let line = chrome.title_size * 1.5;
    let added = file.added.unwrap_or(0).max(0) as f32;
    let removed = file.removed.unwrap_or(0).max(0) as f32;
    let green_share = match added + removed > 0.0 {
        true => added / (added + removed),
        false => 0.6,
    };
    let tint =
        |color: skia_safe::Color| skia_safe::Color::from_argb(40, color.r(), color.g(), color.b());
    let (added_color, removed_color) = (tint(chrome.added_color.0), tint(chrome.removed_color.0));
    // The bars start at the code text edge, where the diff's text
    // will.
    let inset = theme
        .part(::editor::theme::WindowChrome::KEY)
        .code_text_edge;
    let trailing = theme.part(::hikit::chrome::DiffChrome::KEY).band_pad_right;
    let thunk =
        imba::leaf::leaf::<RowCommand>(width, height).paint_instead(move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            let bars = ((rect.height() / line).floor() as usize).max(1);
            let widths = [0.72f32, 0.48, 0.63, 0.55, 0.42, 0.68];
            let green_bars = (bars as f32 * green_share).round() as usize;
            for bar in 0..bars {
                let top = rect.top + bar as f32 * line + line * 0.2;
                let bar_width = (rect.width() - inset - trailing) * widths[bar % widths.len()];
                paint.set_color(match bar < green_bars {
                    true => added_color,
                    false => removed_color,
                });
                let bar_rect = Rect::from_xywh(rect.left + inset, top, bar_width, line * 0.55);
                canvas.draw_round_rect(bar_rect, 4.0, 4.0, &paint);
            }
        });
    imba::ThunkBox::new(arena, thunk)
}

/// The lazy trigger: a placeholder that PAINTED is a placeholder the
/// user can see (the list culls everything else) — the first unarmed
/// paint asks for the build at the row's real width.
pub(crate) struct ArmedPlaceholder<Inner> {
    inner: Inner,
    armed: bool,
}

impl<'a, Inner: Widget<'a, RowCommand>> Widget<'a, RowCommand> for ArmedPlaceholder<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<RowCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && !self.armed {
            return result.merge(EventResult::command(RowCommand::Arm(
                self.inner.size().width,
            )));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

/// A paint-time ASK riding a thunk: the layout saw something to fix
/// once this frame painted (a box wrapped at the wrong width, a head
/// still unbuilt), and the paint report carries the command — the
/// chat composer's Rewrap ride.
pub(crate) struct AskOnPaint<Inner, Command> {
    pub inner: Inner,
    pub ask: Option<Command>,
}

impl<'a, Command: Clone + 'a, Inner: Widget<'a, Command>> Widget<'a, Command>
    for AskOnPaint<Inner, Command>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        let result = self.inner.handle_event(arena, event, viewport);
        if let (Event::Paint { .. }, Some(ask)) = (event, &self.ask) {
            return result.merge(EventResult::command(ask.clone()));
        }
        result
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
