// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, constraints::Constraints, store::Store, UiCtx, View};
use skia_safe::Size;

use crate::workbench_node::{NodeCommand, WorkbenchNode};

#[derive(Clone)]
pub struct Workbench {
    pub root: WorkbenchNode,

    dock: Option<crate::dock::Dock>,

    bottom: Option<crate::sheet::Sheet>,
}

impl Workbench {
    pub(crate) fn new(root: WorkbenchNode) -> Self {
        Self {
            root,
            dock: None,
            bottom: None,
        }
    }

    pub(crate) fn dock(&self) -> Option<&crate::dock::Dock> {
        self.dock.as_ref()
    }

    pub(crate) fn bottom(&self) -> Option<&crate::sheet::Sheet> {
        self.bottom.as_ref()
    }

    pub(crate) fn shown_bottom(&self) -> Option<&crate::sheet::Sheet> {
        self.bottom.as_ref().filter(|sheet| sheet.shown())
    }

    pub(crate) fn bottom_mut(&mut self) -> Option<&mut crate::sheet::Sheet> {
        self.bottom.as_mut()
    }

    pub(crate) fn open_sheet(&mut self, pane: Box<dyn crate::DynPanelView>) {
        self.bottom = Some(crate::sheet::Sheet::new(pane));
    }

    pub(crate) fn perform_sheet(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::UiCtx,
        command: imba::DynCommand,
        fx: &mut imba::effect::Effects<'_, imba::DynCommand>,
    ) -> Option<bool> {
        let bottom = self.bottom.as_mut()?;
        let mut keys = None;
        if matches!(
            command.downcast_ref::<crate::sheet::SheetCommand>(),
            Some(crate::sheet::SheetCommand::Toggle)
        ) {
            let expanding = !bottom.expanded();
            if expanding {
                keys = Some(true);
                fx.scope(
                    |command: crate::sheet::SheetCommand| -> imba::DynCommand { Box::new(command) },
                    |fx| bottom.set_blur(store, ui, false, fx),
                );
            }
        }
        imba::DynView::perform_dyn(bottom, store, ui, command, fx);
        keys
    }

    pub(crate) fn sheet_focus_changed(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::UiCtx,
        focused: bool,
        fx: &mut imba::effect::Effects<'_, crate::sheet::SheetCommand>,
    ) {
        if let Some(bottom) = &mut self.bottom {
            bottom.focus_changed(store, ui, focused, fx);
        }
    }

    pub(crate) fn perform_dock(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::UiCtx,
        command: imba::DynCommand,
        fx: &mut imba::effect::Effects<'_, imba::DynCommand>,
    ) {
        if let Some(dock) = &mut self.dock {
            imba::DynView::perform_dyn(dock, store, ui, command, fx);
        }
    }

    pub(crate) fn dock_mut(&mut self) -> &mut Option<crate::dock::Dock> {
        &mut self.dock
    }

    pub(crate) fn settle_dock(&mut self) {
        if let Some(dock) = &mut self.dock {
            if dock.target_width() > 0.0 {
                dock.settle();
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct WorkbenchGeometry {
    pub x: f32,
    pub top: f32,
    pub split_width: f32,
}

pub fn workbench_geometry(
    width: f32,
    height: f32,
    window: &::editor::theme::WindowChrome,
) -> WorkbenchGeometry {
    let margin = (width * window.margin_ratio).clamp(window.margin_min, window.margin_max);
    WorkbenchGeometry {
        x: margin,
        top: (height * window.top_ratio).clamp(window.top_min, window.top_max),
        split_width: (width - margin * 2.0).max(1.0),
    }
}

impl View for Workbench {
    type Command = NodeCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.root.perform(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        WorkbenchFrame {
            workbench: self,
            store,
            ui,
        }
    }
}

/// The workbench frame, reified: the root split placed by the
/// window's geometry rule over the cleared background.
struct WorkbenchFrame<'a> {
    workbench: &'a Workbench,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for WorkbenchFrame<'_> {}

impl<'a> imba::Layout<'a, NodeCommand> for WorkbenchFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, NodeCommand> {
        use imba::thunk_ext::ThunkExt;
        let WorkbenchFrame {
            workbench,
            store,
            ui,
        } = self;
        let size = constraints.max;
        let theme = ::editor::env::Themes::of(store);

        let geometry = match workbench.root.full_bleed() {
            true => WorkbenchGeometry {
                x: 0.0,
                top: 0.0,
                split_width: size.width,
            },
            false => workbench_geometry(size.width, size.height, &theme.ui().window),
        };
        let split_height = (size.height - geometry.top).max(1.0);
        let root = imba::Layout::layout(
            workbench.root.display(arena, store, ui),
            arena,
            Constraints::tight(Size::new(geometry.split_width, split_height)),
        );

        let mut container = imba::container::container(arena, size);
        container.place(geometry.x, geometry.top, root);

        imba::ThunkBox::new(
            arena,
            container.paint_below(move |_arena, canvas, _| {
                canvas.clear(theme.ui().window.background.0);
            }),
        )
    }
}
