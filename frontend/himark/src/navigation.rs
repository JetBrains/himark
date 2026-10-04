// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::any::{Any, TypeId};
use std::sync::Arc;

use imba::store::Store;

use crate::app::AppFx;
use crate::Panel;

pub use hikit::{NavigationLocation, Navigator, NoPlace, Place};

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

/// The WINDOWED navigators — the editor and diff OPEN roads, which
/// resolve the window's family and land panes into it. Shell-side by
/// nature; they shrink away as opening becomes content-addressed.
pub trait WindowedNavigator: Send + Sync + 'static {
    type Place: Place;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        place: &Self::Place,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel>;
}

type ErasedNavigate = Arc<
    dyn Fn(
            &mut Store,
            &imba::UiCtx,
            crate::WindowId,
            &NavigationLocation,
            &mut AppFx<'_>,
        ) -> Option<Panel>
        + Send
        + Sync,
>;

#[derive(Clone, Default)]
pub struct Navigators(pub(crate) rpds::HashTrieMapSync<TypeId, ErasedNavigate>);

impl Navigators {
    /// Register a WINDOWLESS navigator (the kit trait): its verbs
    /// fold into the app stream; its pane lands wherever the shell
    /// decides.
    pub fn register<N: Navigator>(store: &mut Store, navigator: N) {
        let navigator = Arc::new(navigator);
        let erased: ErasedNavigate = Arc::new(move |store, ui, _window, location, fx| {
            let place = location.place::<N::Place>()?;
            fx.scope(crate::AppCommand::Verb, |fx| {
                navigator.navigate(store, ui, place, fx)
            })
            .map(Panel::Plugin)
        });
        crate::registry::Registry::update(store, |registry| {
            registry
                .navigators
                .0
                .insert_mut(TypeId::of::<N::Place>(), erased);
        });
    }

    pub fn register_windowed<N: WindowedNavigator>(store: &mut Store, navigator: N) {
        let navigator = Arc::new(navigator);
        let erased: ErasedNavigate = Arc::new(move |store, ui, window, location, fx| {
            let place = location.place::<N::Place>()?;
            navigator.navigate(store, ui, window, place, fx)
        });
        crate::registry::Registry::update(store, |registry| {
            registry
                .navigators
                .0
                .insert_mut(TypeId::of::<N::Place>(), erased);
        });
    }

    pub fn navigate(
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        location: &NavigationLocation,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel> {
        let entry = crate::registry::Registry::of(store)?
            .navigators
            .0
            .get(&location.place_type())
            .cloned()?;
        entry(store, ui, window, location, fx)
    }
}

#[derive(Clone, Default)]
pub struct RecentLocations(Vec<crate::ResourceLocation>);

/// Recents belong to the session you are working in; its family row
/// hands the id to whoever has that context (docs/entities.md law 3) —
/// this module never sees a `SessionId`.
impl RecentLocations {
    const CAP: usize = 100;

    pub fn touch(
        store: &mut Store,
        recents: imba::store::Id<Self>,
        location: &crate::ResourceLocation,
    ) {
        store.update_entity(recents, |recents| {
            let recents = &mut recents.0;
            recents.retain(|listed| listed != location);
            recents.insert(0, location.clone());
            recents.truncate(Self::CAP);
        });
    }

    pub fn replace(
        store: &mut Store,
        recents: imba::store::Id<Self>,
        old: &crate::ResourceLocation,
        new: &crate::ResourceLocation,
    ) {
        store.update_entity(recents, |recents| {
            let recents = &mut recents.0;
            recents.retain(|listed| listed != old && listed != new);
            recents.insert(0, new.clone());
            recents.truncate(Self::CAP);
        });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn list(store: &Store, recents: imba::store::Id<Self>) -> Vec<crate::ResourceLocation> {
        store
            .entity(recents)
            .map(|recents| recents.0.clone())
            .unwrap_or_default()
    }
}

pub(crate) struct EditorNavigator;

impl WindowedNavigator for EditorNavigator {
    type Place = EditorPlace;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        place: &EditorPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<Panel> {
        let documents = crate::Windows::session_family(store, window)
            .expect("navigation runs in a window with a session")
            .documents();
        let Some(id) = crate::OpenDocuments::by_location(store, documents, &place.location) else {
            fx.push(crate::open_by_location_effect(
                window,
                documents,
                place.location.clone(),
                true,
                false,
                None,
            ));
            return None;
        };
        let mut document = crate::OpenDocuments::document(store, documents, id)?;
        let width = crate::Windows::window_ref(store, window)
            .and_then(|entity| {
                crate::app::panel_width(store, entity.workbench().root.focused_pane())
            })
            .unwrap_or_else(|| crate::app::fallback_pane_editor_width(store));
        let editor = fx.scope(
            move |command| {
                crate::AppCommand::at(documents, crate::DocumentsCommand::Editor(id, command))
            },
            |fx| {
                let editor = crate::mount_editor(store, ui, &mut document, width, None, fx);

                if place.caret > 0 {
                    let fonts = ::editor::env::Fonts::of(store)();
                    let theme = ::editor::env::Themes::of(store);
                    document.reveal_at_instant(editor, place.caret, store, ui, &fonts, &theme, fx);
                }
                editor
            },
        );
        documents::scroll_stripes::enable_scroll_stripes(
            store,
            documents,
            id,
            &mut document,
            editor,
        );
        crate::OpenDocuments::put_document(store, documents, id, document);
        crate::OpenDocuments::touch(store, documents, id);
        let mut pane = imba::scroll::ScrollView::new(
            crate::EditorIdView::new(documents, id, editor).with_gutter(),
        );
        pane.set_scroll_y(place.scroll_y);
        Some(crate::Panel::Editor(pane))
    }
}
