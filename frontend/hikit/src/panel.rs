// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The panel face: what a workbench slot asks of the view it hosts.
//! Panels hold ids and mind their collections; windows, docks and
//! slots are the shell's — nothing here names one.

use imba::store::Store;
use imba::UiCtx;

use crate::{FamilyRow, NavigationLocation, Place};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidgetOrigin {
    Pane(usize),
    Family,
}

#[derive(Clone)]
pub enum PanelRequest {
    OpenLocations(Vec<editor::ResourceLocation>),

    Perform(std::sync::Arc<dyn imba::command::DynamicCommand>),

    /// The shell-payload escape: an application-level ask the kit
    /// cannot name (a window-coupled command), carried opaquely and
    /// interpreted by the shell's drain. Requests live on CLONE-able
    /// panels, so the escape rides an Arc, not the verb's box.
    Shell(std::sync::Arc<dyn std::any::Any + Send + Sync>),
}

pub trait PanelView: imba::CloneDynView + Clone + Sized + 'static {
    type Place: Place;

    fn title(&self, store: &Store) -> String;

    fn dismantle(&mut self, store: &mut Store);

    /// The pane LEFT ITS SLOT without being closed: another panel took
    /// the slot, or the slot walked elsewhere. The instance is dropped
    /// right after — a pane that keeps state outside itself (the chat's
    /// laid mount) hands it back here, so the walk back adopts it
    /// instead of rebuilding. Closing is `dismantle`; this is not.
    fn displaced(&mut self, store: &mut Store) {
        let _ = store;
    }

    fn take_request(&mut self) -> Option<PanelRequest> {
        None
    }

    fn collapsed_height(&self, _store: &Store, _nominal_height: f32) -> Option<f32> {
        None
    }

    fn family_row(&self) -> Option<FamilyRow> {
        None
    }

    fn as_any(&self) -> &dyn std::any::Any;

    fn full_bleed(&self) -> bool {
        false
    }

    fn navigation_location(&self, store: &Store) -> Option<Self::Place> {
        let _ = store;
        None
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &Self::Place,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        let _ = (store, place, fx);
        false
    }

    fn drawer_view(&self, store: &Store, ui: &UiCtx) -> Option<Box<dyn crate::ModalView>> {
        let _ = (store, ui);
        None
    }
}

pub trait DynPanelView: imba::CloneDynView {
    fn clone_panel(&self) -> Box<dyn DynPanelView>;
    fn title(&self, store: &Store) -> String;
    fn dismantle(&mut self, store: &mut Store);
    fn displaced(&mut self, store: &mut Store);
    fn take_request(&mut self) -> Option<PanelRequest>;
    fn family_row(&self) -> Option<FamilyRow>;
    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn full_bleed(&self) -> bool;
    fn navigation_location_dyn(&self, store: &Store) -> Option<NavigationLocation>;
    fn navigate_to_dyn(
        &mut self,
        store: &mut Store,
        location: &NavigationLocation,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool;
    fn drawer_view_dyn(&self, store: &Store, ui: &UiCtx) -> Option<Box<dyn crate::ModalView>>;
}

impl<P: PanelView> DynPanelView for P {
    fn clone_panel(&self) -> Box<dyn DynPanelView> {
        Box::new(self.clone())
    }
    fn title(&self, store: &Store) -> String {
        PanelView::title(self, store)
    }
    fn dismantle(&mut self, store: &mut Store) {
        PanelView::dismantle(self, store)
    }
    fn displaced(&mut self, store: &mut Store) {
        PanelView::displaced(self, store)
    }
    fn take_request(&mut self) -> Option<PanelRequest> {
        PanelView::take_request(self)
    }
    fn family_row(&self) -> Option<FamilyRow> {
        PanelView::family_row(self)
    }
    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        PanelView::collapsed_height(self, store, nominal_height)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        PanelView::as_any(self)
    }
    fn full_bleed(&self) -> bool {
        PanelView::full_bleed(self)
    }
    fn navigation_location_dyn(&self, store: &Store) -> Option<NavigationLocation> {
        self.navigation_location(store).map(NavigationLocation::new)
    }
    fn navigate_to_dyn(
        &mut self,
        store: &mut Store,
        location: &NavigationLocation,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        match location.place::<P::Place>() {
            Some(place) => self.navigate_to(store, place, fx),
            None => false,
        }
    }
    fn drawer_view_dyn(&self, store: &Store, ui: &UiCtx) -> Option<Box<dyn crate::ModalView>> {
        self.drawer_view(store, ui)
    }
}

impl Clone for Box<dyn DynPanelView> {
    fn clone(&self) -> Self {
        self.clone_panel()
    }
}
