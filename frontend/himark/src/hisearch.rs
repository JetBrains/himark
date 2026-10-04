// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The Search dock tab's SHELL half: the dock toggle/focus commands,
//! the LSP feed opener and the toolbar button — window glue over the
//! `locations::search` face (the view itself lives with its model,
//! docs/entities.md).


use ::locations::search::{SearchArea, SearchCommand, SearchView};

use std::sync::Arc;

use imba::store::Store;

use ahp_locations::driver::LocationsWire;
use locations::open_feed;
use locations::FeedId;
use locations::LocationLists;
use locations::LocationsAsk;


/// The dock owner id — the toggle command's, shared by everything
/// that lands content into this tab.
pub const OWNER: &str = "search.view";

/// An LSP locations ask opening the Search dock tab at ASK time —
/// the whole chain in ONE window command: the window's session names
/// the lists collection, the feed is minted and fronted here, and
/// the ask's landing arrives with `(lists, feed)` stamped at launch
/// (docs/entities.md law 3) — no payload re-entry through the
/// editor command, no scope re-derived later.
pub struct OpenLspFeed {
    pub kind: ahp_locations::LspLocationsKind,
    pub title: String,
    pub location: editor::location::ResourceLocation,
    pub position: documents::text_ext::LineCol,
}

impl crate::commands::DynamicCommand for OpenLspFeed {
    fn id(&self) -> &'static str {
        "search.open-lsp-feed"
    }

    fn name(&self) -> String {
        "Stream Locations into Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some((lists, wire)) = ::workbench::window::Windows::window_ref(store, window)
            .map(|entity| (entity.state().lists(), entity.state().locations_wire()))
        else {
            return;
        };
        let feed = FeedId::mint();
        open_feed(store, lists, feed, self.title.clone(), String::new());
        LocationLists::ask(
            store,
            lists,
            LocationsAsk::Lsp {
                feed,
                kind: match self.kind {
                    ahp_locations::LspLocationsKind::References => locations::LspKind::References,
                    ahp_locations::LspLocationsKind::Implementations => {
                        locations::LspKind::Implementations
                    }
                },
                location: self.location.clone(),
                position: self.position,
            },
        );
        ShowFeedInDock { lists, wire, feed }.perform(app, store, window, fx);
    }
}

/// Front a feed in the Search dock tab — the one door every producer
/// uses: a references ask opening the tab at ASK time, and the
/// peek's promote button, which reuses the standing feed instead of
/// asking again.
pub struct ShowFeedInDock {
    /// The feed's HOME collection and its wire — carried with the
    /// feed so a promote fronts the right rows even if the window
    /// moved on.
    pub lists: imba::store::Id<LocationLists>,
    pub wire: imba::store::Id<LocationsWire>,
    pub feed: FeedId,
}

impl crate::commands::DynamicCommand for ShowFeedInDock {
    fn id(&self) -> &'static str {
        "search.show-feed"
    }

    fn name(&self) -> String {
        "Show Locations in Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        if let Some(previous) = LocationLists::search(store, self.lists) {
            if previous != self.feed {
                LocationLists::ask(store, self.lists, LocationsAsk::Dispose(previous));
            }
        }
        LocationLists::set_search(store, self.lists, self.feed);

        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = SearchView::open(store, &app.ui_ctx(), self.lists);
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), OWNER, fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub struct ToggleSearchView;

impl crate::commands::DynamicCommand for ToggleSearchView {
    fn id(&self) -> &'static str {
        OWNER
    }

    fn name(&self) -> String {
        "Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }

        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let session = entity.current_session();
        let lists = entity.state().lists();
        let folders = ahp_session::session::folders::session_folders(store, &session);
        LocationLists::adopt_folders(store, lists, &folders);
        let panel = SearchView::open(store, &app.ui_ctx(), lists);
        let owner = self.id();
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

/// The KEYBINDING's road (cmd-shift-f, `search.focus`): open the
/// dock if it is away, and FOCUS the query input if it already
/// fronts — never a toggle-away; the toolbar button (`search.view`,
/// [`ToggleSearchView`]) keeps the toggle every dock button has.
pub struct FocusSearchView;

impl crate::commands::DynamicCommand for FocusSearchView {
    fn id(&self) -> &'static str {
        "search.focus"
    }

    fn name(&self) -> String {
        "Focus Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(OWNER) {
            entity.focus_dock();
            if let Some(panel) = entity.dock_panel_mut() {
                let ui = app.ui_ctx();
                fx.scope(
                    move |command| {
                        crate::app::AppCommand::Content(
                            window,
                            ::workbench::window::WindowCommand::Dock(imba::dyn_view::DynCommand::new(
                                ::workbench::dock::DockCommand::Content(command),
                            )),
                        )
                    },
                    |fx| {
                        imba::dyn_view::DynView::perform_dyn(
                            panel.as_mut(),
                            store,
                            &ui,
                            imba::dyn_view::DynCommand::new(SearchCommand::Focus(SearchArea::Input, None)),
                            fx,
                        )
                    },
                );
            }
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        ::workbench::window::Windows::put(store, window, entity);
        ToggleSearchView.perform(app, store, window, fx);
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: OWNER,
        order: 0.5,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let radius = rect.width() * 0.32;
            let center = skia_safe::Point::new(
                rect.left + rect.width() * 0.42,
                rect.top + rect.height() * 0.42,
            );
            canvas.draw_circle(center, radius, &paint);
            let start = skia_safe::Point::new(
                center.x + radius * std::f32::consts::FRAC_1_SQRT_2,
                center.y + radius * std::f32::consts::FRAC_1_SQRT_2,
            );
            canvas.draw_line(
                start,
                skia_safe::Point::new(
                    rect.right - rect.width() * 0.08,
                    rect.bottom - rect.height() * 0.08,
                ),
                &paint,
            );
        }),
    }
}
