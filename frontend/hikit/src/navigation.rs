// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Navigation places, erased: a pane records WHERE it stands as a
//! typed place; the walk stack carries them erased and asks panes
//! and navigators by downcast. The windowed navigators (the editor
//! and diff OPEN roads) stay with the shell — a navigator here minds
//! collections, never windows.

use std::any::{Any, TypeId};
use std::sync::Arc;

use imba::store::Store;

pub trait Place: Clone + PartialEq + Send + Sync + 'static {}

#[derive(Clone, PartialEq)]
pub enum NoPlace {}

impl Place for NoPlace {}

trait ErasedPlace: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn same(&self, other: &dyn Any) -> bool;
}

impl<P: Place> ErasedPlace for P {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn same(&self, other: &dyn Any) -> bool {
        other.downcast_ref::<P>().is_some_and(|other| other == self)
    }
}

#[derive(Clone)]
pub struct NavigationLocation {
    place_type: TypeId,
    payload: Arc<dyn ErasedPlace>,
}

impl NavigationLocation {
    pub fn new<P: Place>(place: P) -> Self {
        Self {
            place_type: TypeId::of::<P>(),
            payload: Arc::new(place),
        }
    }

    pub fn place_type(&self) -> TypeId {
        self.place_type
    }

    pub fn place<P: Place>(&self) -> Option<&P> {
        self.payload.as_any().downcast_ref::<P>()
    }

    pub fn same(&self, other: &NavigationLocation) -> bool {
        self.payload.same(other.payload.as_any())
    }
}

/// The walk-back road for a place no live pane answers: re-mint the
/// pane off its collection. Windowless — a navigator reaches its
/// rows by the ids the place carries; where the pane LANDS is the
/// shell's business.
pub trait Navigator: Send + Sync + 'static {
    type Place: Place;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        place: &Self::Place,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn crate::DynPanelView>>;
}

/// The editor pane's place: the located document, the caret and the
/// scroll to restore. The one place every editor navigator speaks.
#[derive(Clone, Debug)]
pub struct EditorPlace {
    pub location: editor::location::ResourceLocation,
    pub caret: u32,
    pub scroll_y: f32,
}

impl PartialEq for EditorPlace {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location && self.caret == other.caret
    }
}

impl Place for EditorPlace {}
