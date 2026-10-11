// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The modal face: a standalone layer (peeker, palette, search-like
//! sheets) that answers the shell through REQUESTS. The requests
//! speak ids, locations and verbs — a window-coupled ask rides the
//! verb lane's shell escape, built by the shell's own helpers.

use imba::store::Store;
use imba::{arena::Arena, constraints::Constraints, dyn_view::DynCommand, ui::UiCtx, View};

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
    /// supplies the window. `focus` distinguishes the deliberate
    /// jump from a browsing show (the keyboard stays where it was).
    OpenAt {
        location: editor::location::ResourceLocation,
        target: Option<std::ops::Range<documents::text_ext::LineCol>>,
        focus: bool,
    },

    ShowDocument(documents::DocumentId),

    OpenLocations(Vec<editor::location::ResourceLocation>),

    SelectWidget(Box<dyn crate::panel::DynPanelView>),
}

pub trait ModalView: imba::dyn_view::DynView + Send + Sync {
    fn take_request(&mut self) -> Option<ModalRequest>;

    fn set_query(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _query: &str,
        _fx: &mut imba::effect::Effects<'_, DynCommand>,
    ) {
    }

    fn release_widgets(
        &mut self,
    ) -> Vec<(
        crate::panel::WidgetOrigin,
        Box<dyn crate::panel::DynPanelView>,
    )> {
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            self.as_ref().layout_dyn(arena, store, ui, constraints)
        })
    }
}

/// A scroller-free view under the TOP scroller — the desktop
/// shells' half of the law: content views own no scroll, and the
/// scroll sits where the view is MOUNTED (a native shell's
/// scroller, or this). Reveals from the content land in the
/// wrapped ScrollView through the regular result chain.
pub struct Scrolled<V>
where
    V: imba::View + Clone,
    V::Command: Send + 'static,
{
    scroll: imba::scroll::ScrollView<V>,
}

impl<V> Scrolled<V>
where
    V: imba::View + Clone,
    V::Command: Send + 'static,
{
    pub fn new(view: V) -> Self {
        Self {
            scroll: imba::scroll::ScrollView::new(view),
        }
    }
}

impl<V> Clone for Scrolled<V>
where
    V: imba::View + Clone,
    V::Command: Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            scroll: self.scroll.clone(),
        }
    }
}

impl<V> imba::View for Scrolled<V>
where
    V: imba::View + Clone,
    V::Command: Send + 'static,
{
    type Command = imba::scroll::ScrollCommand<V::Command>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.scroll.focus_data(store, ui)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        self.scroll.destroy(store, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.scroll.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        self.scroll.display(arena, store, ui)
    }
}

impl<V> ModalView for Scrolled<V>
where
    V: ModalView + imba::View + Clone + Send + Sync + 'static,
    V::Command: Send + 'static,
{
    fn take_request(&mut self) -> Option<ModalRequest> {
        self.scroll.content_mut().take_request()
    }

    fn set_query(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        query: &str,
        fx: &mut imba::effect::Effects<'_, DynCommand>,
    ) {
        self.scroll.content_mut().set_query(store, ui, query, fx);
    }

    fn release_widgets(
        &mut self,
    ) -> Vec<(
        crate::panel::WidgetOrigin,
        Box<dyn crate::panel::DynPanelView>,
    )> {
        self.scroll.content_mut().release_widgets()
    }

    fn focus_lost(&mut self) {
        self.scroll.content_mut().focus_lost();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }
}

impl<V> crate::panel::PanelView for Scrolled<V>
where
    V: crate::panel::PanelView + imba::View + Clone + Send + Sync + 'static,
    V::Command: Send + 'static,
{
    type Place = V::Place;

    fn title(&self, store: &Store) -> String {
        self.scroll.content().title(store)
    }

    fn dismantle(&mut self, store: &mut Store) {
        self.scroll.content_mut().dismantle(store);
    }

    fn displaced(&mut self, store: &mut Store) {
        self.scroll.content_mut().displaced(store);
    }

    fn take_request(&mut self) -> Option<crate::panel::PanelRequest> {
        self.scroll.content_mut().take_request()
    }

    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        self.scroll.content().collapsed_height(store, nominal_height)
    }

    fn pane_row(&self) -> Option<crate::pane_row::PaneRow> {
        self.scroll.content().pane_row()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn full_bleed(&self) -> bool {
        self.scroll.content().full_bleed()
    }

    fn navigation_location(&self, store: &Store) -> Option<Self::Place> {
        self.scroll.content().navigation_location(store)
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &Self::Place,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        self.scroll.content_mut().navigate_to(store, place, fx)
    }

    fn drawer_view(&self, store: &Store, ui: &imba::ui::UiCtx) -> Option<Box<dyn ModalView>> {
        self.scroll.content().drawer_view(store, ui)
    }
}
