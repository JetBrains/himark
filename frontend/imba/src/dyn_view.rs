// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ui::UiCtx;
use crate::{arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, View};

pub type DynCommand = Box<dyn std::any::Any + Send + Sync>;

pub trait DynView {
    fn perform_dyn(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut crate::effect::Effects<'_, DynCommand>,
    );
    fn layout_dyn<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> crate::ThunkBox<'a, DynCommand>;

    fn destroy_dyn(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, DynCommand>);

    fn focus_data_dyn<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, DynCommand>;
}

impl<V> DynView for V
where
    V: View,
    V::Command: Send + Sync + 'static,
{
    fn perform_dyn(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut crate::effect::Effects<'_, DynCommand>,
    ) {
        match command.downcast::<V::Command>() {
            Ok(command) => fx.scope(
                |command: V::Command| Box::new(command) as DynCommand,
                |fx| self.perform(store, ui, *command, fx),
            ),
            Err(_) => {
                debug_assert!(false, "command routed to a view with another command type");
            }
        }
    }

    fn layout_dyn<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> crate::ThunkBox<'a, DynCommand> {
        crate::ThunkBox::new(
            arena,
            crate::Layout::layout(self.display(arena, store, ui), arena, constraints)
                .map(|command| Box::new(command) as DynCommand),
        )
    }

    fn destroy_dyn(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, DynCommand>) {
        fx.scope(
            |command: V::Command| Box::new(command) as DynCommand,
            |fx| self.destroy(store, fx),
        )
    }

    fn focus_data_dyn<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, DynCommand> {
        self.focus_data(store, ui)
            .map(|command| Box::new(command) as DynCommand)
    }
}

pub trait CloneDynView: DynView + Send + Sync {
    fn clone_dyn(&self) -> Box<dyn CloneDynView>;
}

impl<V> CloneDynView for V
where
    V: View + Clone + Send + Sync + 'static,
    V::Command: Send + Sync + 'static,
{
    fn clone_dyn(&self) -> Box<dyn CloneDynView> {
        Box::new(self.clone())
    }
}

impl View for Box<dyn DynView> {
    type Command = DynCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, DynCommand>) {
        self.as_mut().destroy_dyn(store, fx)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut crate::effect::Effects<'_, DynCommand>,
    ) {
        self.as_mut().perform_dyn(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
            self.as_ref().layout_dyn(arena, store, ui, constraints)
        })
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, DynCommand> {
        self.as_ref().focus_data_dyn(store, ui)
    }
}
