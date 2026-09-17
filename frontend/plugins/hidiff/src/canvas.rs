// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff canvas (docs/diff-canvas.md): one huge list — per changed
//! file a Header-1 band and the file's diff beneath it. Diffs build
//! lazily when their row paints, entirely off-thread, and land
//! atomically: the swap and the height move in one perform, and the
//! settle pulse re-aims the viewport before the frame paints.

use himark::diff_canvas::{
    canvas_files, canvas_generation, CanvasFile, CanvasListing, CanvasReveal, CanvasSource,
};
use himark::{env, EditorView, ResourceLocation, UnifiedDiffCommand};
use imba::effect::{AnyEffect, Effects};
use imba::event::{Event, EventResult, Placement};
use imba::list::{ListCommand, ListSlice, ListView};
use imba::scroll::{ScrollCommand, ScrollView};
use imba::thunk_ext::ThunkExt;
use imba::{arena::Arena, constraints::Constraints, store::Store, Thunk, UiCtx, View, Widget};
use skia_safe::{Paint, Rect, Size};

const MIN_EST_LINES: i64 = 4;
const MAX_EST_LINES: i64 = 60;
const EST_CONTEXT_LINES: i64 = 4;

type CanvasRows = ListView<CanvasRow, ResourceLocation>;
type RowsCommand = ScrollCommand<ListCommand<RowCommand>>;

pub enum CanvasCommand {
    Rows(RowsCommand),

    /// The paint probe saw the feed's generation move.
    Refresh,

    /// The paint probe saw an armed reveal for this source.
    PickupReveal,

    Landed {
        key: ResourceLocation,
        built: himark::BuiltFileDiff,
    },
}

#[derive(Clone)]
pub struct DiffCanvasView {
    source: CanvasSource,
    rows: ScrollView<CanvasRows>,
    files: rpds::HashTrieMapSync<ResourceLocation, CanvasFile>,

    note: Option<String>,
    seen: Option<u64>,
    populated: bool,
    request: Option<himark::PanelRequest>,

    phases: rpds::HashTrieMapSync<ResourceLocation, RowPhase>,
}

/// A row's lifecycle, panel-tracked — the test oracle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowPhase {
    Placeholder,
    Built,
    Failed,
}

impl DiffCanvasView {
    pub fn fresh(source: CanvasSource) -> Self {
        Self {
            source,
            rows: ScrollView::new(ListView::empty()),
            files: rpds::HashTrieMapSync::new_sync(),
            note: None,
            seen: None,
            populated: false,
            request: None,
            phases: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub fn source(&self) -> &CanvasSource {
        &self.source
    }

    #[doc(hidden)]
    pub fn probe_note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    #[doc(hidden)]
    pub fn probe_scroll_top(&self) -> f32 {
        self.rows.scroll_y()
    }

    /// (title, phase, reserved/current height), in list order.
    #[doc(hidden)]
    pub fn probe_rows(&self) -> Vec<(String, RowPhase, f32)> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| {
                let key = rows.key_at(index)?;
                let file = self.files.get(key)?;
                Some((
                    file.title.clone(),
                    self.phases
                        .get(key)
                        .copied()
                        .unwrap_or(RowPhase::Placeholder),
                    rows.height_at(index)?,
                ))
            })
            .collect()
    }

    /// Per Built row: (title, host/inline focus, each card's focus).
    #[doc(hidden)]
    pub fn probe_focus(&self) -> Vec<(String, String, Vec<String>)> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| {
                let key = rows.key_at(index)?;
                let title = self.files.get(key)?.title.clone();
                let row = rows.view_at(index)?;
                let RowBody::Built { view } = &row.body else {
                    return None;
                };
                let inline = view.inline_editor?;
                let host = format!("{:?}", view.split.right.document.focus(inline));
                let cards = view
                    .split
                    .right
                    .document
                    .before_inlay_views(inline)
                    .into_iter()
                    .map(|(_, card)| format!("{:?}", card.card_focus()))
                    .collect();
                Some((title, host, cards))
            })
            .collect()
    }

    /// Per Built row: (host text head, each card's text head).
    #[doc(hidden)]
    pub fn probe_texts(&self) -> Vec<(String, Vec<String>)> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| {
                let row = rows.view_at(index)?;
                let RowBody::Built { view } = &row.body else {
                    return None;
                };
                let inline = view.inline_editor?;
                let host = {
                    let text = view.split.right.document.text();
                    let end = text.byte_count().min(120) as u32;
                    text.view().substring(0..end)
                };
                let cards = view
                    .split
                    .right
                    .document
                    .before_inlay_views(inline)
                    .into_iter()
                    .map(|(_, card)| card.shown_text())
                    .collect();
                Some((host, cards))
            })
            .collect()
    }

    /// Geometry oracle: (content_height, [(anchor_byte, y_of_anchor)]).
    #[doc(hidden)]
    pub fn probe_geometry(&self) -> Vec<(f32, Vec<(u32, f32)>)> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| {
                let row = rows.view_at(index)?;
                let RowBody::Built { view } = &row.body else {
                    return None;
                };
                let inline = view.inline_editor?;
                let document = &view.split.right.document;
                let content = document.content_height(inline);
                let anchors = document
                    .before_inlays(inline)
                    .into_iter()
                    .map(|(range, _, _)| (range.start, document.height_before(inline, range.start)))
                    .collect();
                Some((content, anchors))
            })
            .collect()
    }

    fn refresh(&mut self, store: &mut Store) {
        let (generation, listing) = canvas_files(store, &self.source);
        self.seen = Some(generation);
        match listing {
            CanvasListing::Pending(text) => {
                if !self.populated {
                    self.note = Some(text);
                }
            }
            CanvasListing::Ready(files) => {
                // Reconcile of an already-populated canvas is phase 6
                // (docs/diff-canvas.md §7); v1 populates exactly once.
                if self.populated {
                    return;
                }
                self.populated = true;
                self.note = None;
                let theme = env::Themes::of(store);
                let mut slice: ListSlice<CanvasRow, ResourceLocation> = ListSlice::new();
                for file in &files {
                    slice.push_keyed_sized(
                        file.new.clone(),
                        CanvasRow {
                            file: file.clone(),
                            body: RowBody::Placeholder { armed: false },
                            rewrap_ask: None,
                        },
                        reserved_height(&theme, file),
                    );
                }
                for file in files {
                    self.phases
                        .insert_mut(file.new.clone(), RowPhase::Placeholder);
                    self.files.insert_mut(file.new.clone(), file);
                }
                self.rows.content_mut().splice_slice(0..0, slice);
            }
        }
    }

    fn launch(&self, index: usize, width: f32, fx: &mut Effects<'_, CanvasCommand>) {
        let Some(key) = self.rows.content().key_at(index).cloned() else {
            return;
        };
        let Some(file) = self.files.get(&key) else {
            return;
        };
        fx.push(
            AnyEffect::new(himark::BuildFileDiffEffect {
                old: file.old.clone(),
                new: file.new.clone(),
                width,
            })
            .map(move |built| CanvasCommand::Landed {
                key: key.clone(),
                built,
            }),
        );
    }

    fn land(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ResourceLocation,
        built: himark::BuiltFileDiff,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(range) = self.rows.content().row_range(&key) else {
            return;
        };
        let index = range.start;
        let Some(file) = self.files.get(&key).cloned() else {
            return;
        };
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let (body, body_height) = match built.failed.clone() {
            Some(error) => (RowBody::Failed(error), chrome.title_size * 3.0),
            None => {
                let (view, height) = fx.scope(
                    move |command: RowCommand| {
                        CanvasCommand::Rows(ScrollCommand::Content(ListCommand::Child(
                            index, command,
                        )))
                    },
                    |fx| mounted(store, ui, built, fx),
                );
                (RowBody::Built { view }, height)
            }
        };
        self.phases.insert_mut(
            key.clone(),
            match &body {
                RowBody::Failed(_) => RowPhase::Failed,
                _ => RowPhase::Built,
            },
        );
        let height = header_band(&theme) + body_height + chrome.gap;
        let mut slice: ListSlice<CanvasRow, ResourceLocation> = ListSlice::new();
        slice.push_keyed_sized(
            key.clone(),
            CanvasRow {
                file,
                body,
                rewrap_ask: None,
            },
            height,
        );
        self.rows
            .content_mut()
            .splice_slice(index..index + 1, slice);
        // The swap is a height mutation like any other: the door
        // noted the anchor, the pulse re-aims before this frame
        // paints. Zero wrong frames (docs/diff-canvas.md §4).
        fx.settle();
    }
}

/// The whole off-thread build arrives here; this mounts it — bounded
/// editors over the PRE-BUILT documents, the seeded pair road exactly
/// as the chat diff cell mounts (higent/cell.rs `resolve_diff`).
/// A row-level ask inside a rows command, seen from the panel — digs
/// through the list's `Focus(_, Some(..))` press wrapping.
fn row_ask(command: &RowsCommand) -> Option<(usize, &RowCommand)> {
    let ScrollCommand::Content(inner) = command else {
        return None;
    };
    fn dig<C>(list: &ListCommand<C>) -> Option<(usize, &C)> {
        match list {
            ListCommand::Child(index, command) => Some((*index, command)),
            ListCommand::Focus(_, Some(inner)) => dig(inner),
            _ => None,
        }
    }
    dig(inner)
}

fn mounted(
    store: &mut Store,
    ui: &UiCtx,
    built: himark::BuiltFileDiff,
    fx: &mut Effects<'_, RowCommand>,
) -> (himark::UnifiedDiffView, f32) {
    let fonts = env::Fonts::of(store)();
    let theme = env::Themes::of(store);
    let mut before_doc = built.old;
    let mut after_doc = built.new;
    let operation = built.operation;
    let prepared = built.marks;

    let diff_id = after_doc.add_diff(operation.clone(), before_doc.revision());
    after_doc.install_normalized_diff(diff_id, operation, before_doc.revision());
    let hunks = after_doc.diff(diff_id).expect("just added").markup();

    let gutter = theme.ui().editor_gutter.width;
    let editor_width = (built.width - gutter).max(120.0);
    let mut throwaway = imba::effect::Batch::new();
    let quiet = &mut throwaway.effects();

    let left_marks = before_doc.add_markup();
    before_doc.replace_markup(
        left_marks,
        prepared.left.clone(),
        &[],
        &fonts,
        &theme,
        quiet,
    );
    let left_editor = before_doc.add_editor(
        editor_width,
        None,
        himark::EditorBuild::Bounded,
        &[left_marks],
        &fonts,
        &theme,
        quiet,
    );
    before_doc.manage_repairs_in_pair(left_editor);

    let right_editor = after_doc.add_editor(
        editor_width,
        None,
        himark::EditorBuild::Bounded,
        &[hunks],
        &fonts,
        &theme,
        quiet,
    );
    after_doc.manage_repairs_in_pair(right_editor);
    let right_extras = after_doc.add_owned_markup(right_editor);
    after_doc.replace_markup(
        right_extras,
        prepared.right.clone(),
        &[],
        &fonts,
        &theme,
        quiet,
    );

    if let Some(parsers) = env::Parsers::of(store) {
        fx.scope(
            |command: himark::EditorCommand| {
                RowCommand::Diff(UnifiedDiffCommand::Split(himark::SplitDiffCommand::Left(
                    command,
                )))
            },
            |fx| before_doc.launch_reparse(parsers.clone(), fx),
        );
        fx.scope(
            |command: himark::EditorCommand| {
                RowCommand::Diff(UnifiedDiffCommand::Split(himark::SplitDiffCommand::Right(
                    command,
                )))
            },
            |fx| after_doc.launch_reparse(parsers, fx),
        );
    }

    let state = himark::DiffState::attach(
        diff_id,
        &before_doc,
        &after_doc,
        left_marks,
        right_extras,
        Some(prepared.window),
    )
    .expect("the entry was just installed");
    let left = EditorView {
        document: before_doc,
        editor: left_editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let right = EditorView {
        document: after_doc,
        editor: right_editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let mut view = himark::UnifiedDiffView::new(himark::SplitDiffView::new(left, right, state));
    fx.scope(RowCommand::Diff, |fx| {
        view.perform(
            store,
            ui,
            UnifiedDiffCommand::SetLayout(himark::DiffLayout::Inline),
            fx,
        )
    });
    let height = {
        let frame = Arena::default();
        let thunk = imba::Layout::layout(
            view.display(&frame, store, ui),
            &frame,
            Constraints {
                min: Size::new(editor_width, 0.0),
                max: Size::new(editor_width + gutter, f32::MAX),
            },
        );
        Thunk::size(&thunk).height
    };
    (view, height)
}

impl View for DiffCanvasView {
    type Command = CanvasCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            CanvasCommand::Refresh => self.refresh(store),
            CanvasCommand::PickupReveal => {
                if let Some(key) = CanvasReveal::take_for(store, &self.source) {
                    self.rows
                        .content_mut()
                        .reveal_row(key, Placement::TopLeftAt);
                }
            }
            CanvasCommand::Landed { key, built } => self.land(store, ui, key, built, fx),
            CanvasCommand::Rows(command) => {
                match row_ask(&command) {
                    Some((index, RowCommand::Arm(width))) => self.launch(index, *width, fx),
                    Some((index, RowCommand::OpenFull)) => {
                        // The header opens the standalone pane — the
                        // pre-canvas road, still reachable per file.
                        let file = self
                            .rows
                            .content()
                            .key_at(index)
                            .and_then(|key| self.files.get(key));
                        if let Some(file) = file {
                            self.request = Some(himark::PanelRequest::Perform(
                                std::sync::Arc::new(himark::hichanges::OpenDiffForPair {
                                    old: file.old.clone(),
                                    new: file.new.clone(),
                                }),
                            ));
                        }
                    }
                    _ => {}
                }
                fx.scope(CanvasCommand::Rows, |fx| {
                    self.rows.perform(store, ui, command, fx)
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        let _ = arena;
        imba::laid(move |arena: &'a Arena, constraints: Constraints| {
            let refresh = self.seen != Some(canvas_generation(store, &self.source));
            let reveal = CanvasReveal::pending_for(store, &self.source);
            let inner: imba::ThunkBox<'a, CanvasCommand> = match &self.note {
                Some(note) => {
                    let chrome = env::Themes::of(store).ui().chat.clone();
                    let text = note.clone();
                    let font = himark::fonts::ui_text_font(ui, chrome.title_size);
                    let color = chrome.loader_color.0;
                    let size = constraints.max;
                    imba::ThunkBox::new(
                        arena,
                        imba::leaf::leaf::<CanvasCommand>(size.width, size.height.min(240.0))
                            .paint_instead(move |_arena, canvas, rect| {
                                let mut paint = Paint::default();
                                paint.set_anti_alias(true);
                                paint.set_color(color);
                                let width = font.measure_str(&text, None).0;
                                canvas.draw_str(
                                    &text,
                                    (
                                        rect.left + (rect.width() - width) / 2.0,
                                        rect.top + rect.height() * 0.5,
                                    ),
                                    &font,
                                    &paint,
                                );
                            }),
                    )
                }
                None => imba::ThunkBox::new(
                    arena,
                    imba::Layout::layout(self.rows.display(arena, store, ui), arena, constraints)
                        .map(CanvasCommand::Rows),
                ),
            };
            inner.wrap(move |widget| CanvasProbe {
                inner: widget,
                refresh,
                reveal,
            })
        })
    }
}

/// The ReconcileShell of the canvas: paint compares retained state
/// against the store and answers with commands — mutation stays in
/// perform.
struct CanvasProbe<Inner> {
    inner: Inner,
    refresh: bool,
    reveal: bool,
}

impl<'a, Inner: Widget<'a, CanvasCommand>> Widget<'a, CanvasCommand> for CanvasProbe<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CanvasCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CanvasCommand> {
        let mut result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) {
            if self.refresh {
                result = result.merge(EventResult::Command(CanvasCommand::Refresh));
            }
            if self.reveal {
                result = result.merge(EventResult::Command(CanvasCommand::PickupReveal));
            }
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, CanvasCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

// ---------------------------------------------------------------- rows

pub enum RowCommand {
    /// The placeholder painted un-armed: build me, at this width.
    Arm(f32),

    /// The header band was pressed: open the standalone diff pane.
    OpenFull,

    Diff(UnifiedDiffCommand),

    Rewrap(f32),
}

#[derive(Clone)]
enum RowBody {
    Placeholder { armed: bool },
    Failed(String),
    Built { view: himark::UnifiedDiffView },
}

#[derive(Clone)]
pub(crate) struct CanvasRow {
    file: CanvasFile,
    body: RowBody,

    /// The last rewrap target seen. A live window drag changes the
    /// width EVERY frame; resizing three editors per row per frame
    /// (plus the SetHeight/settle churn each resize drags in) is the
    /// resize storm. A rewrap only runs once the same target arrives
    /// twice — i.e. the width held still for a frame.
    rewrap_ask: Option<f32>,
}

fn header_band(theme: &himark::Theme) -> f32 {
    let h1 = theme.resolve([himark::StyleId::Header(1)]);
    h1.font_size.unwrap_or(48.0) + h1.block_gap.unwrap_or(24.0)
}

fn reserved_height(theme: &himark::Theme, file: &CanvasFile) -> f32 {
    let chrome = theme.ui().chat.clone();
    let line = chrome.title_size * 1.5;
    let known = file.added.is_some() || file.removed.is_some();
    let est = match known {
        true => (file.added.unwrap_or(0) + file.removed.unwrap_or(0) + EST_CONTEXT_LINES)
            .clamp(MIN_EST_LINES, MAX_EST_LINES),
        false => 12,
    };
    header_band(theme) + chrome.gap + est as f32 * line
}

impl View for CanvasRow {
    type Command = RowCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        if let RowBody::Built { view } = &mut self.body {
            fx.scope(RowCommand::Diff, |fx| view.destroy(store, fx));
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            RowCommand::OpenFull => {}
            RowCommand::Arm(_) => {
                if let RowBody::Placeholder { armed } = &mut self.body {
                    *armed = true;
                }
            }
            RowCommand::Diff(command) => {
                let RowBody::Built { view } = &mut self.body else {
                    return;
                };
                fx.scope(RowCommand::Diff, |fx| view.perform(store, ui, command, fx));
            }
            RowCommand::Rewrap(width) => {
                if self.rewrap_ask != Some(width) {
                    self.rewrap_ask = Some(width);
                    return;
                }
                let RowBody::Built { view } = &mut self.body else {
                    return;
                };
                let fonts = env::Fonts::of(store)();
                let theme = env::Themes::of(store);
                let left_editor = view.split.left.editor;
                let right_editor = view.split.right.editor;
                let inline = view.inline_editor;
                fx.scope(
                    |command: himark::EditorCommand| {
                        RowCommand::Diff(UnifiedDiffCommand::Split(himark::SplitDiffCommand::Left(
                            command,
                        )))
                    },
                    |fx| {
                        view.split
                            .left
                            .document
                            .resize(left_editor, width, 0, &fonts, &theme, fx)
                    },
                );
                fx.scope(
                    |command: himark::EditorCommand| {
                        RowCommand::Diff(UnifiedDiffCommand::Split(
                            himark::SplitDiffCommand::Right(command),
                        ))
                    },
                    |fx| {
                        view.split
                            .right
                            .document
                            .resize(right_editor, width, 0, &fonts, &theme, fx)
                    },
                );
                if let Some(inline) = inline {
                    fx.scope(
                        |command: himark::EditorCommand| {
                            RowCommand::Diff(UnifiedDiffCommand::Inline(command))
                        },
                        |fx| {
                            view.split
                                .right
                                .document
                                .resize(inline, width, 0, &fonts, &theme, fx)
                        },
                    );
                }
                fx.scope(RowCommand::Diff, |fx| {
                    view.perform(
                        store,
                        ui,
                        UnifiedDiffCommand::Split(himark::SplitDiffCommand::Resync),
                        fx,
                    )
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        RowFrame {
            row: self,
            store,
            ui,
        }
    }
}

struct RowFrame<'a> {
    row: &'a CanvasRow,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for RowFrame<'_> {}

impl<'a> imba::Layout<'a, RowCommand> for RowFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, RowCommand> {
        use imba::LayoutExt as _;
        let RowFrame { row, store, ui } = self;
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let width = constraints.max.width.max(1.0);
        let band = header_band(&theme);
        let gutter = theme.ui().editor_gutter.width;

        let header = header_layout(arena, store, ui, &row.file, band, width);

        match &row.body {
            RowBody::Placeholder { armed } => {
                let reserved = reserved_height(&theme, &row.file);
                let body_height = (reserved - band - chrome.gap).max(0.0);
                let skeleton = skeleton_layout(arena, &theme, &row.file, width, body_height);
                let card = imba::ZBox::new(arena)
                    .child(imba::spacer(width, reserved - chrome.gap))
                    .child(imba::fixed(header).on_click(|| RowCommand::OpenFull))
                    .child(imba::fixed(skeleton).pad_insets(imba::Insets {
                        left: 0.0,
                        top: band,
                        right: 0.0,
                        bottom: 0.0,
                    }))
                    .pad_insets(imba::Insets {
                        left: 0.0,
                        top: 0.0,
                        right: 0.0,
                        bottom: chrome.gap,
                    })
                    .layout(arena, constraints);
                let armed = *armed;
                imba::ThunkBox::new(
                    arena,
                    card.wrap(move |inner| ArmedPlaceholder { inner, armed }),
                )
            }
            RowBody::Failed(error) => {
                let text = format!("{} — {error}", row.file.title);
                let font = himark::fonts::ui_text_font(ui, chrome.title_size * 0.85);
                let color = chrome.loader_color.0;
                let body = imba::leaf::leaf::<RowCommand>(width, chrome.title_size * 3.0)
                    .paint_instead(move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        paint.set_color(color);
                        canvas.draw_str(
                            &text,
                            (rect.left + 16.0, rect.top + rect.height() * 0.5),
                            &font,
                            &paint,
                        );
                    });
                imba::ZBox::new(arena)
                    .child(imba::spacer(width, band + chrome.title_size * 3.0))
                    .child(imba::fixed(header).on_click(|| RowCommand::OpenFull))
                    .child(imba::fixed(body).pad_insets(imba::Insets {
                        left: 0.0,
                        top: band,
                        right: 0.0,
                        bottom: 0.0,
                    }))
                    .pad_insets(imba::Insets {
                        left: 0.0,
                        top: 0.0,
                        right: 0.0,
                        bottom: chrome.gap,
                    })
                    .layout(arena, constraints)
            }
            RowBody::Built { view } => {
                let editor_target = (width - gutter).max(120.0);
                let body = imba::Layout::layout(
                    view.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(editor_target, 0.0),
                        max: Size::new(editor_target + gutter, f32::MAX),
                    },
                )
                .map(RowCommand::Diff);
                let body_height = Thunk::size(&body).height;
                let laid = view
                    .split
                    .right
                    .document
                    .layout_width(view.split.right.editor);
                let rewrap = ((laid - editor_target).abs() > 1.0).then_some(editor_target);
                let card = imba::ZBox::new(arena)
                    .child(imba::spacer(width, band + body_height))
                    .child(imba::fixed(header).on_click(|| RowCommand::OpenFull))
                    .child(imba::fixed(body).pad_insets(imba::Insets {
                        left: 0.0,
                        top: band,
                        right: 0.0,
                        bottom: 0.0,
                    }))
                    .pad_insets(imba::Insets {
                        left: 0.0,
                        top: 0.0,
                        right: 0.0,
                        bottom: chrome.gap,
                    })
                    .layout(arena, constraints);
                imba::ThunkBox::new(
                    arena,
                    card.wrap(move |inner| RewrapOnPaint { inner, rewrap }),
                )
            }
        }
    }
}

/// The Header-1 band: the file name in the markdown Header 1
/// attributes, right-aligned; the `+N −M` trail at the left edge in
/// the tree's colors.
fn header_layout<'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    file: &CanvasFile,
    band: f32,
    width: f32,
) -> imba::ThunkBox<'a, RowCommand> {
    let theme = env::Themes::of(store);
    let chrome = theme.ui().chat.clone();
    let h1 = theme.resolve([himark::StyleId::Header(1)]);
    let size = h1.font_size.unwrap_or(48.0);
    let mut font = himark::fonts::ui_text_font(ui, size);
    if h1.bold {
        font.set_embolden(true);
    }
    let color = h1.color.unwrap_or(chrome.text_color.0);
    let trail_font = himark::fonts::ui_text_font(ui, chrome.title_size);
    let added = file.added.filter(|n| *n > 0);
    let removed = file.removed.filter(|n| *n > 0);
    let (added_color, removed_color) = (chrome.added_color.0, chrome.removed_color.0);
    let title = file.title.clone();
    let inset = chrome.pad;
    let thunk =
        imba::leaf::leaf::<RowCommand>(width, band).paint_instead(move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            let title_width = font.measure_str(&title, None).0;
            let baseline = rect.top + size;
            canvas.draw_str(
                &title,
                ((rect.right - inset - title_width).max(rect.left), baseline),
                &font,
                &paint,
            );
            let mut x = rect.left + inset;
            if let Some(added) = added {
                paint.set_color(added_color);
                let label = format!("+{added}");
                canvas.draw_str(&label, (x, baseline), &trail_font, &paint);
                x += trail_font.measure_str(&label, None).0 + 8.0;
            }
            if let Some(removed) = removed {
                paint.set_color(removed_color);
                canvas.draw_str(&format!("−{removed}"), (x, baseline), &trail_font, &paint);
            }
        });
    imba::ThunkBox::new(arena, thunk)
}

/// The skeleton under a pending header: rounded bars at the line
/// rhythm, tinted from the diff palette, the mix proportioned to the
/// entry's added:removed ratio — it already READS like the diff it
/// stands for.
fn skeleton_layout<'a>(
    arena: &'a Arena,
    theme: &himark::Theme,
    file: &CanvasFile,
    width: f32,
    height: f32,
) -> imba::ThunkBox<'a, RowCommand> {
    let chrome = theme.ui().chat.clone();
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
    let inset = chrome.pad;
    let thunk =
        imba::leaf::leaf::<RowCommand>(width, height).paint_instead(move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            let bars = ((rect.height() / line).floor() as usize).max(1);
            let widths = [0.72f32, 0.48, 0.63, 0.55, 0.42, 0.68];
            let green_bars = (bars as f32 * green_share).round() as usize;
            for bar in 0..bars {
                let top = rect.top + bar as f32 * line + line * 0.2;
                let bar_width = (rect.width() - inset * 2.0) * widths[bar % widths.len()];
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
struct ArmedPlaceholder<Inner> {
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
            return result.merge(EventResult::Command(RowCommand::Arm(
                self.inner.size().width,
            )));
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

struct RewrapOnPaint<Inner> {
    inner: Inner,
    rewrap: Option<f32>,
}

impl<'a, Inner: Widget<'a, RowCommand>> Widget<'a, RowCommand> for RewrapOnPaint<Inner> {
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
        if let (Event::Paint { .. }, Some(width)) = (event, self.rewrap) {
            return result.merge(EventResult::Command(RowCommand::Rewrap(width)));
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

// ---------------------------------------------------------------- panel

#[derive(Clone, PartialEq)]
pub struct CanvasPlace {
    pub source: CanvasSource,
}

impl himark::Place for CanvasPlace {}

impl himark::PanelView for DiffCanvasView {
    type Place = CanvasPlace;

    fn family_row(&self) -> Option<himark::FamilyRow> {
        Some(himark::FamilyRow::Canvas(self.source.clone()))
    }

    fn navigation_location(&self, _store: &Store) -> Option<CanvasPlace> {
        Some(CanvasPlace {
            source: self.source.clone(),
        })
    }

    fn navigate_to(
        &mut self,
        _store: &mut Store,
        place: &CanvasPlace,
        _fx: &mut himark::AppFx<'_>,
    ) -> bool {
        place.source == self.source
    }

    fn title(&self, store: &Store) -> String {
        self.source.title(store)
    }

    fn take_request(&mut self) -> Option<himark::PanelRequest> {
        self.request.take()
    }

    fn dismantle(&mut self, _store: &mut Store) {
        // Row documents and editors are ROW-owned (the chat cell
        // discipline) — they die with the views. Nothing is
        // registered anywhere to retract.
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
