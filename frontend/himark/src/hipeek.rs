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
/// The peek feature's document command as a plugin.
pub struct PeekPlugin;

impl crate::plugin::Plugin for PeekPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        app.register_document_command(go_to_reference());
    }
}

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
            // Only promote a feed whose lists a live session owns.
            if !ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .any(|state| state.lists() == lists)
            {
                return;
            }
            crate::commands::AppRequests::push(store, Arc::new(PromoteFeed { lists, feed }));
        }),
    })
}

/// The peek's deliberate open — a deferred window ask: the request
/// drain supplies whichever window the gesture ran in.
struct OpenPicked {
    location: editor::location::ResourceLocation,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
}

impl crate::commands::HimarkCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "peek.open-picked"
    }
    fn name(&self) -> String {
        "Open Reference".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SEATED SHELL opens the pick — any shell's.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::DocumentPlace {
                    location: self.location.clone(),
                    target: self.target.clone(),
                    focus: true,
                },
                fx,
            )
        });
    }
}

/// Front a reference feed — the SEATED SHELL decides how: the
/// workbench's chrome claims the place into its search dock.
struct PromoteFeed {
    lists: imba::store::Id<locations::LocationLists>,
    feed: locations::FeedId,
}

impl crate::commands::HimarkCommand for PromoteFeed {
    fn id(&self) -> &'static str {
        "peek.promote-feed"
    }
    fn name(&self) -> String {
        "Show All References".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::FeedPlace {
                    lists: self.lists,
                    feed: self.feed,
                },
                fx,
            )
        });
    }
}
