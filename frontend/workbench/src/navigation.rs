// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use hikit::{navigation::NavigationLocation, navigation::Navigator, navigation::Place};

use std::any::TypeId;
use std::sync::Arc;

use imba::store::Store;

use crate::workbench_node::Panel;


/// The WINDOWED navigators — the editor and diff OPEN roads, which
/// resolve the window's session and land panes into it. Shell-side by
/// nature; they shrink away as opening becomes content-addressed.
pub trait WindowedNavigator: Send + Sync + 'static {
    type Place: Place;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: crate::window::WindowId,
        place: &Self::Place,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<Panel>;
}

type ErasedNavigate = Arc<
    dyn Fn(
            &mut Store,
            &imba::ui::UiCtx,
            crate::window::WindowId,
            &NavigationLocation,
            &mut imba::command::Fx<'_>,
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
            navigator.navigate(store, ui, place, fx).map(Panel::Plugin)
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
        ui: &imba::ui::UiCtx,
        window: crate::window::WindowId,
        location: &NavigationLocation,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<Panel> {
        let entry = crate::registry::Registry::of(store)?
            .navigators
            .0
            .get(&location.place_type())
            .cloned()?;
        entry(store, ui, window, location, fx)
    }
}
