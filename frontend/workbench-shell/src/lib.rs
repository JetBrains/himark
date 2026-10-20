// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The WORKBENCH SHELL's chrome: the dock toggles, their toolbar
//! buttons, the window modal helpers and the terminal pane — the
//! placement half of the features, pulled out of the engine. The
//! engine halves (collections, lanes, session commands) stay in
//! himark; what lives here holds windows.

use std::sync::Arc;

pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod hihistory;
pub mod hisearch;
pub mod hiterminal;
pub mod modal;

/// The chrome's shell DECORATOR: places only a workbench can show
/// (the search-dock feed) are claimed here and handed to their
/// windowed vehicles; everything else falls through the chain.
struct ChromeFrontend {
    next: Arc<dyn himark::himark::Shell>,
}

impl himark::himark::Shell for ChromeFrontend {
    fn navigate(
        &self,
        origin: himark::bridge::ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        let window = ::workbench::window::WindowId::from_raw(origin.raw());
        if ::workbench::window::Windows::window_ref(store, window).is_some() {
            if let Some(place) = place.place::<himark::places::FeedPlace>() {
                // The feed fronts in the search dock — the windowed
                // vehicle rides the follow-up lane for its own scope.
                fx.follow_up(imba::command::Verb::shell(Box::new(
                    himark::shell::windowed(
                        window,
                        Arc::new(hisearch::ShowFeedInDock {
                            lists: place.lists,
                            feed: place.feed,
                        }),
                    ),
                )));
                return true;
            }
        }
        self.next.navigate(origin, place, store, ui, fx)
    }
}

/// Seats the chrome's decorator — runs with the other plugins, so
/// a native shell created later still wraps OVER it and claims
/// first.
pub struct ShellPlugin;

impl himark::plugin::Plugin for ShellPlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        app.seat_shell(|next| Arc::new(ChromeFrontend { next }));
    }
}

/// Every chrome plugin, in registration order — the workbench
/// host's one-line boot addition.
pub fn plugins() -> Vec<Box<dyn himark::plugin::Plugin>> {
    vec![
        Box::new(ShellPlugin),
        Box::new(hichanges::ChromePlugin),
        Box::new(hihistory::ChromePlugin),
        Box::new(hicomments::ChromePlugin),
        Box::new(hisearch::ChromePlugin),
        Box::new(hifiles::ChromePlugin),
        Box::new(hiterminal::ChromePlugin),
    ]
}
