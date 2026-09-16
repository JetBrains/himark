// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::any::{Any, TypeId};
use std::sync::Arc;

use imba::store::Store;

use crate::app::AppFx;
use crate::Panel;

pub trait Place: Clone + PartialEq + Send + Sync + 'static {}

#[derive(Clone, Debug)]
pub struct EditorPlace {
    pub location: crate::ResourceLocation,
    pub caret: u32,
    pub scroll_y: f32,
}

impl PartialEq for EditorPlace {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location && self.caret == other.caret
    }
}

impl Place for EditorPlace {}

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

    pub fn place<P: Place>(&self) -> Option<&P> {
        self.payload.as_any().downcast_ref::<P>()
    }

    pub fn same(&self, other: &NavigationLocation) -> bool {
        self.payload.same(other.payload.as_any())
    }
}

pub trait Navigator: Send + Sync + 'static {
    type Place: Place;

    fn navigate(
        &self,
        store: &mut Store,
        window: crate::WindowId,
        place: &Self::Place,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel>;
}

type ErasedNavigate = Arc<
    dyn Fn(&mut Store, crate::WindowId, &NavigationLocation, &mut AppFx<'_>) -> Option<Panel>
        + Send
        + Sync,
>;

#[derive(Clone, Default)]
pub struct Navigators(rpds::HashTrieMapSync<TypeId, ErasedNavigate>);

impl Navigators {
    pub fn register<N: Navigator>(store: &mut Store, navigator: N) {
        let navigator = Arc::new(navigator);
        let erased: ErasedNavigate = Arc::new(move |store, window, location, fx| {
            let place = location.place::<N::Place>()?;
            navigator.navigate(store, window, place, fx)
        });
        store.update::<Navigators>(|navigators| {
            navigators.0.insert_mut(TypeId::of::<N::Place>(), erased);
        });
    }

    pub fn navigate(
        store: &mut Store,
        window: crate::WindowId,
        location: &NavigationLocation,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel> {
        let entry = store
            .get::<Navigators>()?
            .0
            .get(&location.place_type)
            .cloned()?;
        entry(store, window, location, fx)
    }
}

#[derive(Clone, Default)]
pub struct RecentLocations(Vec<crate::ResourceLocation>);

impl RecentLocations {
    const CAP: usize = 100;

    pub fn touch(store: &mut Store, location: &crate::ResourceLocation) {
        store.update::<RecentLocations>(|recents| {
            recents.0.retain(|listed| listed != location);
            recents.0.insert(0, location.clone());
            recents.0.truncate(Self::CAP);
        });
    }

    pub fn replace(
        store: &mut Store,
        old: &crate::ResourceLocation,
        new: &crate::ResourceLocation,
    ) {
        store.update::<RecentLocations>(|recents| {
            recents.0.retain(|listed| listed != old && listed != new);
            recents.0.insert(0, new.clone());
            recents.0.truncate(Self::CAP);
        });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn list(store: &Store) -> Vec<crate::ResourceLocation> {
        store
            .get::<RecentLocations>()
            .map(|recents| recents.0.clone())
            .unwrap_or_default()
    }
}

pub(crate) struct EditorNavigator;

impl Navigator for EditorNavigator {
    type Place = EditorPlace;

    fn navigate(
        &self,
        store: &mut Store,
        window: crate::WindowId,
        place: &EditorPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel> {
        let Some(id) = crate::OpenDocuments::by_location(store, &place.location) else {
            fx.push(crate::open_by_location_effect(
                window,
                place.location.clone(),
                true,
                None,
            ));
            return None;
        };
        let mut document = crate::OpenDocuments::document(store, id)?;
        let width = crate::Windows::window_ref(store, window)
            .and_then(|entity| {
                crate::app::panel_width(store, entity.workbench().root.focused_pane())
            })
            .unwrap_or_else(|| crate::app::fallback_pane_editor_width(store));
        let editor = crate::app::entity_scope(id, fx, |fx| {
            let editor = crate::mount_editor(store, &mut document, width, None, fx);

            if place.caret > 0 {
                let fonts = ::editor::env::Fonts::of(store)();
                let theme = ::editor::env::Themes::of(store);
                document.reveal_at(editor, place.caret, &fonts, &theme, fx);
            }
            editor
        });
        documents::scroll_stripes::enable_scroll_stripes(store, id, &mut document, editor);
        crate::OpenDocuments::put_document(store, id, document);
        crate::OpenDocuments::touch(store, id);
        let mut pane =
            imba::scroll::ScrollView::new(crate::EditorIdView::new(id, editor).with_gutter());
        pane.set_scroll_y(place.scroll_y);
        Some(crate::Panel::Editor(pane))
    }
}
