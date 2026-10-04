// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ui::UiCtx;
use crate::{arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, View};

/// The type-erased command a `DynView` routes — `Box<dyn Any>` with
/// its `Command` citizenship kept: it clones (`clone_box`) and prints
/// (`Display` forwards to the erased command) like the typed command
/// it wraps, so erased views satisfy the same `View::Command` bound
/// as everyone else.
pub struct DynCommand(Box<dyn ErasedCommand>);

trait ErasedCommand: Send + Sync {
    fn clone_box(&self) -> Box<dyn ErasedCommand>;
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

impl<C: crate::Command> ErasedCommand for C {
    fn clone_box(&self) -> Box<dyn ErasedCommand> {
        Box::new(self.clone())
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, out)
    }
}

impl DynCommand {
    pub fn new<C: crate::Command>(command: C) -> Self {
        Self(Box::new(command))
    }

    /// The routed command back in its own type — `None` is the
    /// misroute (a command delivered to a view of another command
    /// type), the caller's debug_assert.
    pub fn downcast<C: 'static>(self) -> Option<C> {
        self.0
            .into_any()
            .downcast::<C>()
            .ok()
            .map(|command| *command)
    }

    /// A by-reference probe — command routing that only needs to peek
    /// (fold ticks, passivity checks) without unwrapping.
    pub fn downcast_ref<C: 'static>(&self) -> Option<&C> {
        self.0.as_any().downcast_ref::<C>()
    }
}

impl Clone for DynCommand {
    fn clone(&self) -> Self {
        Self(self.0.clone_box())
    }
}

impl std::fmt::Display for DynCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(out)
    }
}

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
{
    fn perform_dyn(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut crate::effect::Effects<'_, DynCommand>,
    ) {
        match command.downcast::<V::Command>() {
            Some(command) => fx.scope(DynCommand::new::<V::Command>, |fx| {
                self.perform(store, ui, command, fx)
            }),
            None => {
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
            crate::layout::Layout::layout(self.display(arena, store, ui), arena, constraints)
                .map(DynCommand::new),
        )
    }

    fn destroy_dyn(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, DynCommand>) {
        fx.scope(DynCommand::new::<V::Command>, |fx| self.destroy(store, fx))
    }

    fn focus_data_dyn<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, DynCommand> {
        self.focus_data(store, ui).map(DynCommand::new)
    }
}

pub trait CloneDynView: DynView + Send + Sync {
    fn clone_dyn(&self) -> Box<dyn CloneDynView>;
}

impl<V> CloneDynView for V
where
    V: View + Clone + Send + Sync + 'static,
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
    ) -> impl crate::layout::Layout<'a, Self::Command> + crate::layout::LayoutValue + 'a {
        crate::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
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
