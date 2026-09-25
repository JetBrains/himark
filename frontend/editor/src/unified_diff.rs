// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::constraints::Constraints;
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{UiCtx, View as _};
use skia_safe::Size;

use crate::document::EditorBuild;
use crate::editor::EditorId;
use crate::editor_view::{EditorCommand, EditorView};
use crate::split_diff::{fold, SplitDiffCommand, SplitDiffView};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffLayout {
    Split,
    Inline,
}

impl DiffLayout {
    pub fn other(self) -> DiffLayout {
        match self {
            DiffLayout::Split => DiffLayout::Inline,
            DiffLayout::Inline => DiffLayout::Split,
        }
    }
}

pub enum UnifiedDiffCommand {
    Split(SplitDiffCommand),

    Inline(EditorCommand),
    SetLayout(DiffLayout),
}

pub type UnifiedDiffEffects<'a> = imba::effect::Effects<'a, UnifiedDiffCommand>;

#[derive(Clone)]
pub struct UnifiedDiffView {
    pub split: SplitDiffView,
    pub layout: DiffLayout,
    pub inline_editor: Option<EditorId>,
}

impl UnifiedDiffView {
    pub fn new(split: SplitDiffView) -> Self {
        let layout = split.state.unified_layout();
        let inline_editor = split.state.inline_editor();
        Self {
            split,
            layout,
            inline_editor,
        }
    }

    fn inline_face(&self, store: &Store) -> Option<EditorView> {
        let editor = self.inline_editor?;
        Some(EditorView {
            document: self.split.right.document.clone(),
            editor,
            reports_geometry: true,
            location: self.split.right.location.clone(),

            gutter_width: crate::env::Themes::of(store).ui().editor_gutter.width,
            base: Some((self.split.left.document.clone(), self.split.state.diff_id())),
        })
    }

    fn set_layout(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        next: DiffLayout,
        fx: &mut UnifiedDiffEffects<'_>,
    ) {
        if next == self.layout {
            return;
        }
        if next == DiffLayout::Inline && self.inline_editor.is_none() {
            let editor = self.build_inline_editor(store, ui, fx);
            self.inline_editor = Some(editor);
            self.split.state.note_inline_built();
        }
        self.layout = next;
        self.split
            .state
            .set_unified(self.layout, self.inline_editor);
    }

    /// A bounded build of the inline face off the CURRENT dressing: it
    /// shows the hunk washes and the fold strips (`right_marks`), and
    /// its before-cards are expanded from the pane's live diff
    /// operation. Never diffs (docs/no-diff-on-ui-thread).
    fn build_inline_editor(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut UnifiedDiffEffects<'_>,
    ) -> EditorId {
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let width = self
            .split
            .right
            .document
            .layout_width(self.split.right.editor)
            .max(200.0);
        let shown = [
            self.split.state.hunk_markup(),
            self.split.state.right_marks(),
        ];
        let diff = self.split.state.diff_id();
        let base = self.split.left.document.clone();
        let right = &mut self.split.right.document;
        fx.scope(UnifiedDiffCommand::Inline, |fx| {
            let editor = right.add_editor(
                width,
                None,
                EditorBuild::Bounded,
                &shown,
                store,
                ui,
                &fonts,
                &theme,
                fx,
            );
            right.expand_before_inlays(editor, &base, diff, store, ui, &fonts, &theme, fx);
            editor
        })
    }

    /// The normalize lane landed a fresh generation and the inline face
    /// wears an older dressing. TWO very different cases:
    ///
    /// The face still wears the whole-replace SEED (built at mount,
    /// before the first honest diff): rebuild it wholesale — its one
    /// giant before-card and foldless height are the seed's shape, and
    /// a face frames old holds nobody's caret. This is the ONLY
    /// rebuild.
    ///
    /// An honest generation replaced an honest one (a keystroke's own
    /// landing included): heal IN PLACE, like the split face — folds
    /// and washes already arrived through the shared marks markup the
    /// inline editor shows, and the before-cards re-expand as inlay
    /// surgery. The editor survives, and with it the caret: tearing it
    /// down here is what snapped typing back to offset zero.
    fn refresh_inline_if_stale(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut UnifiedDiffEffects<'_>,
    ) {
        if !self.split.state.inline_stale() {
            return;
        }
        if self.split.state.inline_wears_the_seed() {
            let Some(stale) = self.inline_editor.take() else {
                return;
            };
            self.split.right.document.remove_editor(stale);
            let editor = self.build_inline_editor(store, ui, fx);
            self.inline_editor = Some(editor);
        } else {
            let Some(editor) = self.inline_editor else {
                return;
            };
            let fonts = crate::env::ui_collection(store, ui);
            let theme = crate::env::Themes::of(store);
            let diff = self.split.state.diff_id();
            let base = self.split.left.document.clone();
            let right = &mut self.split.right.document;
            fx.scope(UnifiedDiffCommand::Inline, |fx| {
                right.refresh_before_inlays(editor, &base, diff, store, ui, &fonts, &theme, fx)
            });
        }
        self.split.state.note_inline_built();
        self.split
            .state
            .set_unified(self.layout, self.inline_editor);
    }

    fn toggle_surface(&self) -> Vec<imba::PresentableCommand<UnifiedDiffCommand>> {
        vec![imba::PresentableCommand::new(
            "diff.toggle-layout",
            match self.layout {
                DiffLayout::Split => "Diff: Switch to Inline View",
                DiffLayout::Inline => "Diff: Switch to Split View",
            },
            UnifiedDiffCommand::SetLayout(self.layout.other()),
        )]
    }
}

impl imba::View for UnifiedDiffView {
    type Command = UnifiedDiffCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, UnifiedDiffCommand> {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        let inner = match self.layout {
            DiffLayout::Split => self
                .split
                .focus_data(store, ui)
                .map(UnifiedDiffCommand::Split),
            DiffLayout::Inline => {
                // The face view is MINTED per ask (documents are
                // persistent, the clone is cheap); handlers re-mint
                // per call because the semantic data may not outlive
                // a temporary.
                let (commands, location, seat) = match self.inline_face(store) {
                    Some(view) => {
                        let mut data = view.focus_data(store, ui);
                        (
                            std::mem::take(&mut data.commands)
                                .into_iter()
                                .map(|presentable| presentable.map(UnifiedDiffCommand::Inline))
                                .collect(),
                            data.location.take(),
                            data.seat.take(),
                        )
                    }
                    None => (Vec::new(), None, None),
                };
                let with_face = move |f: &mut dyn FnMut(
                    imba::focus::FocusData<'_, EditorCommand>,
                )
                    -> EventResult<EditorCommand>| {
                    match self.inline_face(store) {
                        Some(view) => f(view.focus_data(store, ui)).map(UnifiedDiffCommand::Inline),
                        None => EventResult::Ignored,
                    }
                };
                FocusData {
                    commands,
                    on_key: Some(Box::new(move |k, mods| {
                        with_face(&mut |mut data| data.key(k, mods))
                    })),
                    on_text: Some(Box::new(move |text| {
                        with_face(&mut |mut data| data.text(text))
                    })),
                    clipboard: Some(Box::new(move |visit| {
                        with_face(&mut |mut data| match data.clipboard.as_mut() {
                            Some(seat) => seat(visit),
                            None => EventResult::Ignored,
                        })
                    })),
                    location,
                    seat,
                }
            }
        };
        inner.merge_under(FocusData::of_commands(self.toggle_surface()))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: UnifiedDiffCommand,
        fx: &mut UnifiedDiffEffects<'_>,
    ) {
        match command {
            UnifiedDiffCommand::SetLayout(next) => self.set_layout(store, ui, next, fx),
            UnifiedDiffCommand::Split(command) => {
                fx.scope(UnifiedDiffCommand::Split, |fx| {
                    self.split.perform(store, ui, command, fx)
                });
            }
            UnifiedDiffCommand::Inline(command) => {
                let fold = match &command {
                    EditorCommand::Inlay { key, command } => command
                        .downcast_ref::<fold::FoldCommand>()
                        .map(|fold_command| (*key, *fold_command)),
                    _ => None,
                };
                match fold {
                    Some((key, fold_command)) => {
                        fx.scope(UnifiedDiffCommand::Split, |fx| {
                            self.split.adjust_fold(key, fold_command, store, ui, fx)
                        });
                    }
                    None => {
                        let Some(mut view) = self.inline_face(store) else {
                            return;
                        };
                        fx.scope(UnifiedDiffCommand::Inline, |fx| {
                            view.perform(store, ui, command, fx)
                        });

                        self.split.right.document = view.document;

                        fx.scope(UnifiedDiffCommand::Split, |fx| {
                            self.split.settle_after(None, None);
                            self.split.pair_lane(fx);
                        });
                    }
                }
            }
        }
        // A command may have adopted a freshly-normalized generation
        // (the pane's settle runs inside these handlers); the inline
        // face rebuilds off the new dressing if so.
        self.refresh_inline_if_stale(store, ui, fx);
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(UnifiedDiffCommand::Split, |fx| {
            self.split.destroy(store, fx)
        });
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, constraints: Constraints| {
                let face: imba::ThunkBox<'a, UnifiedDiffCommand> = match self.inline_face(store) {
                    Some(view) if self.layout == DiffLayout::Inline => {
                        // A real THUNK: the projected inlays and the
                        // face's own realization both happen at
                        // realize time, BOUNDED by the viewport the
                        // route delivers — never a full-extent build
                        // at display (the DiffCanvas.trace lesson).
                        // The pane hosts the projections itself.
                        imba::ThunkBox::new(
                            arena,
                            InlineThunk {
                                ui,
                                store,
                                arena,
                                view,
                                constraints,
                            }
                            .map(UnifiedDiffCommand::Inline)
                            .overlay_host(crate::markup::INLAY_HOST),
                        )
                    }

                    _ => imba::ThunkBox::new(
                        arena,
                        imba::Layout::layout(
                            self.split.display(arena, store, ui),
                            arena,
                            constraints,
                        )
                        .map(UnifiedDiffCommand::Split),
                    ),
                };
                face
            },
        )
    }
}

struct InlineThunk<'a> {
    ui: &'a UiCtx,
    store: &'a Store,
    arena: &'a imba::arena::Arena,
    view: EditorView,
    constraints: Constraints,
}

impl<'a> imba::Thunk<'a, EditorCommand> for InlineThunk<'a> {
    fn size(&self) -> Size {
        Size::new(
            (self.view.gutter_width + self.view.layout_width()).max(self.constraints.min.width),
            self.view.content_height().max(self.constraints.min.height),
        )
    }

    fn realize(
        self,
        arena: &'a imba::arena::Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, EditorCommand> {
        let InlineThunk {
            ui,
            store,
            arena: frame,
            view,
            constraints,
        } = self;
        let view: &'a EditorView = frame.alloc(view);
        // ONE build per frame: the editor's own realize derives the
        // shared viewport and mints the projected inlays from it —
        // this pane only FILTERS the emissions to the projections
        // (the strips and before-cards riding INLAY_HOST), exactly
        // what the old duplicate build re-minted by hand.
        let inner = imba::Layout::layout(view.display(frame, store, ui), frame, constraints)
            .realize(arena, viewport);
        imba::WidgetBox::new(arena, InlinePane { inner })
    }
}

/// The realized inline face: the editor widget, closed over its
/// bounded viewport. Every ask — events, focus, keys, IME,
/// clipboard — is a read.
struct InlinePane<'a> {
    inner: imba::WidgetBox<'a, EditorCommand>,
}

impl<'a> imba::Widget<'a, EditorCommand> for InlinePane<'a> {
    fn size(&self) -> Size {
        imba::Widget::size(&self.inner)
    }

    fn handle_event(
        &self,
        arena: &imba::arena::Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<EditorCommand> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
        // ONLY the projections surface on this face (the pane is the
        // host); the editor's popup/sticky emissions stay dropped,
        // as they always were here.
        let mut overlays = self.inner.overlays();
        overlays.retain(|overlay| overlay.host == crate::markup::INLAY_HOST);
        overlays
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, EditorCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}
