// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The modal face: a standalone layer (peeker, palette, search-like
//! sheets) that answers the shell through REQUESTS. The requests
//! speak ids, locations and verbs — a window-coupled ask rides the
//! verb lane's shell escape, built by the shell's own helpers.

use imba::store::Store;
use imba::{arena::Arena, constraints::Constraints, DynCommand, UiCtx, View};

/// A one-slot mailbox for a view's outbound request. Cloning a view
/// never clones its pending ask — the clone starts empty.
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

    Perform(imba::command::Verb),

    /// Open one location, honoring a caret target — the shell
    /// supplies the window.
    OpenAt(
        editor::ResourceLocation,
        Option<std::ops::Range<documents::LineCol>>,
    ),

    ShowDocument(documents::DocumentId),

    OpenLocations(Vec<editor::ResourceLocation>),

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
