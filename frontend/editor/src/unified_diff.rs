use imba::constraints::Constraints;
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{Thunk as _, UiCtx, View as _};
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
            let shown = [self.split.state.right_marks()];
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

    fn layout<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl imba::Thunk<'a, Self::Command> + 'a {
        let face: imba::ThunkBox<'a, UnifiedDiffCommand> = match self.inline_face(store) {
            Some(view) if self.layout == DiffLayout::Inline => imba::ThunkBox::new(
                arena,
                imba::eager(InlinePane {
                    ui,
                    store,
                    arena,
                    view,
                    constraints,
                })
                .map(UnifiedDiffCommand::Inline),
            ),

            _ => imba::ThunkBox::new(
                arena,
                self.split
                    .layout(arena, store, ui, constraints)
                    .map(UnifiedDiffCommand::Split),
            ),
        };
        face.commands(move || self.toggle_surface())
    }
}

struct InlinePane<'a> {
    ui: &'a UiCtx,
    store: &'a Store,
    arena: &'a imba::arena::Arena,
    view: EditorView,
    constraints: Constraints,
}

impl<'a> imba::Widget<'a, EditorCommand> for InlinePane<'a> {
    fn size(&self) -> Size {
        Size::new(
            (self.view.gutter_width + self.view.layout_width()).max(self.constraints.min.width),
            self.view.content_height().max(self.constraints.min.height),
        )
    }

    fn handle_event(
        &self,
        arena: &imba::arena::Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<EditorCommand> {
        self.view
            .layout(arena, self.store, self.ui, self.constraints)
            .realize(arena, viewport)
            .handle_event(arena, event, viewport)
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, EditorCommand>
    where
        'a: 'w,
    {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        let view = &self.view;
        let store = self.store;
        let ui = self.ui;
        let arena = self.arena;
        let constraints = self.constraints;
        let with_chain = move |f: &mut dyn FnMut(
            FocusData<'_, EditorCommand>,
        ) -> EventResult<EditorCommand>|
              -> EventResult<EditorCommand> {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let result = f(imba::Widget::focus_data(&mut widget));
            drop(widget);
            result
        };
        let commands = {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let commands = std::mem::take(&mut imba::Widget::focus_data(&mut widget).commands);
            drop(widget);
            commands
        };
        let location = {
            let mut widget = view
                .layout(arena, store, ui, constraints)
                .realize(arena, skia_safe::Rect::default());
            let location = imba::Widget::focus_data(&mut widget).location.take();
            drop(widget);
            location
        };
        FocusData {
            commands,
            on_key: Some(Box::new(move |key, mods| {
                with_chain(&mut |mut data| data.key(key, mods))
            })),
            on_text: Some(Box::new(move |text| {
                with_chain(&mut |mut data| data.text(text))
            })),
            ime: Some(imba::focus::ImeSeat {
                origin: skia_safe::Point::default(),
                clip: None,
                ask: Box::new(move |origin, clip, visit| {
                    with_chain(&mut |mut data| match data.ime.take() {
                        Some(mut seat) => {
                            let at = skia_safe::Point::new(
                                origin.x + seat.origin.x,
                                origin.y + seat.origin.y,
                            );
                            (seat.ask)(at, clip, visit)
                        }
                        None => EventResult::Ignored,
                    })
                }),
            }),
            clipboard: Some(Box::new(move |visit| {
                with_chain(&mut |mut data| match data.clipboard.as_mut() {
                    Some(seat) => seat(visit),
                    None => EventResult::Ignored,
                })
            })),
            location,
        }
    }
}
