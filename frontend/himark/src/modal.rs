// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, constraints::Constraints, store::Store, DynCommand, UiCtx, View};

use crate::app::AppCommand;

pub struct RequestSlot<T>(Option<T>);

impl<T> Default for RequestSlot<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> Clone for RequestSlot<T> {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl<T> RequestSlot<T> {
    pub fn file(&mut self, request: T) {
        self.0 = Some(request);
    }

    pub fn take(&mut self) -> Option<T> {
        self.0.take()
    }
}

pub enum ModalRequest {
    Close,

    Perform(AppCommand),

    ShowDocument(crate::DocumentId),

    OpenLocations(Vec<crate::ResourceLocation>),

    SelectWidget(Box<dyn crate::DynPanelView>),
}

pub trait ModalView: imba::DynView + Send + Sync {
    fn take_request(&mut self) -> Option<ModalRequest>;

    fn set_query(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        _query: &str,
        _fx: &mut imba::effect::Effects<'_, DynCommand>,
    ) {
    }

    fn release_widgets(&mut self) -> Vec<(crate::WidgetOrigin, Box<dyn crate::DynPanelView>)> {
        Vec::new()
    }

    fn focus_lost(&mut self) {}
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable reach-through for the PUSH lanes: a batch-tail sync may
    /// refresh a store-held dock panel in place, no paint involved.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    fn clone_modal(&self) -> Box<dyn ModalView>;
}

impl Clone for Box<dyn ModalView> {
    fn clone(&self) -> Self {
        self.clone_modal()
    }
}

impl View for Box<dyn ModalView> {
    type Command = DynCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, DynCommand> {
        self.as_ref().focus_data_dyn(store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut imba::effect::Effects<'_, DynCommand>,
    ) {
        self.as_mut().perform_dyn(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            self.as_ref().layout_dyn(arena, store, ui, constraints)
        })
    }
}

pub fn modal_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| AppCommand::Content(window, crate::WindowCommand::Modal(command))
}

pub fn dock_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::WindowCommand::Dock(Box::new(crate::dock::DockCommand::Content(command))),
        )
    }
}

pub fn side_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::WindowCommand::Side(Box::new(crate::drawer::DrawerCommand::Content(command))),
        )
    }
}
