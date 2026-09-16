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
            let editor = fx.scope(UnifiedDiffCommand::Inline, |fx| {
                let editor = right.add_editor(
                    width,
                    None,
                    EditorBuild::Bounded,
                    &shown,
                    &fonts,
                    &theme,
                    fx,
                );
                right.expand_before_inlays(editor, &base, diff, &fonts, &theme, fx);
                editor
            });
            self.inline_editor = Some(editor);
        }
        self.layout = next;
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
                if let EditorCommand::Inlay { key, command } = &command {
                    if let Some(fold_command) = command.downcast_ref::<fold::FoldCommand>() {
                        let key = *key;
                        let fold_command = *fold_command;
                        return fx.scope(UnifiedDiffCommand::Split, |fx| {
                            self.split.adjust_fold(key, fold_command, store, ui, fx)
                        });
                    }
                }
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
                        self.split
                            .layout(arena, store, ui, constraints)
                            .map(UnifiedDiffCommand::Split),
                    ),
                };
                face.commands(move || self.toggle_surface())
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
        // Projected inlays mint from the VISIBLE band's geometry —
        // strips scrolled out of view simply are not mounted this
        // frame, like everything else the viewport culls.
        let band = viewport.top.max(0.0)..viewport.bottom.max(viewport.top);
        let data = crate::viewport::EditorViewport::build(
            &view.document,
            view.editor,
            band,
            false,
            false,
            None,
            &crate::env::ui_collection(store, ui),
            &crate::env::Themes::of(store),
        );
        let projected = crate::popup::projected_overlays(
            &view.document,
            view.editor,
            frame,
            store,
            ui,
            &data,
            skia_safe::Point::new(0.0, 0.0),
        );
        let inner = view
            .layout(frame, store, ui, constraints)
            .realize(arena, viewport);
        imba::WidgetBox::new(arena, InlinePane { inner, projected })
    }
}

/// The realized inline face: the editor widget, closed over its
/// bounded viewport, plus the projections it minted. Every ask —
/// events, focus, keys, IME, clipboard — is a read.
struct InlinePane<'a> {
    inner: imba::WidgetBox<'a, EditorCommand>,
    projected: Vec<imba::overlay::Overlay<'a, EditorCommand>>,
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
        // ONLY the projections: the inner editor's own emissions were
        // always dropped on this face (the pane is the host).
        std::mem::take(&mut self.projected)
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, EditorCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}
