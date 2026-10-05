// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! Go-to reference's SHELL half: the roads the command cannot know —
//! the catalog consult (documents → its session's lists), the
//! deferred window ask its picks ride, and the dock promote. The
//! command and the card both live in `locations::peek` now (the view
//! with its model, docs/entities.md).

use std::sync::Arc;

use imba::store::Store;

/// Wire the command with the shell's three roads. The lists consult
/// is the save.rs/fsroute debt class — a catalog consult at a
/// DocumentCommand border; burns when generic document commands learn
/// their session.
pub fn go_to_reference() -> Arc<locations::peek::GoToReference> {
    Arc::new(locations::peek::GoToReference {
        lists: Arc::new(|store, documents| {
            ahp_session::session::state::Hosts::owner_of_documents(store, documents)
                .map(|state| state.lists())
        }),
        open: Arc::new(|store, location, target| {
            crate::commands::AppRequests::push(
                store,
                Arc::new(OpenPicked {
                    location,
                    target: Some(target),
                }),
            );
        }),
        promote: Arc::new(|store, lists, feed| {
            // The dock tab needs the feed's WIRE too — the session
            // whose lists these are has it on the same row.
            let Some(wire) = ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .find(|state| state.lists() == lists)
                .map(|state| state.locations_wire())
            else {
                return;
            };
            crate::commands::AppRequests::push(
                store,
                Arc::new(crate::hisearch::ShowFeedInDock { lists, wire, feed }),
            );
        }),
    })
}

/// The peek's deliberate open — a deferred window ask: the request
/// drain supplies whichever window the gesture ran in.
struct OpenPicked {
    location: editor::location::ResourceLocation,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
}

impl crate::commands::WindowedCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "peek.open-picked"
    }
    fn name(&self) -> String {
        "Open Reference".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(documents) =
            crate::grip::session_state(store, window).map(|state| state.documents())
        else {
            return;
        };
        fx.push(crate::workspace::open_by_location_effect(
            window,
            documents,
            self.location.clone(),
            true,
            true,
            self.target.clone(),
        ));
    }
}
