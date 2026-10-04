// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! Go-to reference's SHELL half: the editor command that mints the
//! feed and mounts the card, and the deferred window ask its picks
//! ride — the card itself is `locations::peek` (the view lives with
//! its model, docs/entities.md). The injected closures carry the
//! shell verbs: `open` queues `OpenPicked`, `promote` fronts the
//! feed in the Search dock tab.


use ::locations::peek::{PeekCommand, PeekView};

use std::sync::Arc;

use imba::store::Store;

use locations::open_feed;
use locations::FeedId;
use locations::LocationLists;
use editor::document::Document;
use editor::editor_view::EditorCommand;
use editor::editor_view::EditorFocus;
use editor::markup::Inlay;
use editor::markup::InlayMode;
use locations::peek::caret_anchor;


const FALLBACK_WIDTH: f32 = 600.0;

fn peek_markup() -> editor::markup::MarkupId {
    static ID: std::sync::OnceLock<editor::markup::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(editor::markup::MarkupId::mint)
}

pub struct GoToReference;

/// Closes over the pane's ids (docs/entities.md law 3): the card's
/// host is the document the command runs in, no owner is resolved.
impl documents::dynamic::DocumentCommand for GoToReference {
    fn id(&self) -> &'static str {
        "code.go-to-reference"
    }

    fn name(&self) -> String {
        "Go to Reference".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<documents::OpenDocuments>,
        document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        let _ = payload;
        // Phase one: mint the feed, mount the card NOW — the ask's
        // outcome lands into the visible card, never into silence.
        // The lists collection is the documents' sibling — a catalog
        // consult at a DocumentCommand border, the save.rs/fsroute
        // debt class: burns when generic document commands learn
        // their session (the gating keeps this one boot-global).
        let Some((lists, wire)) = ahp_session::session::state::Hosts::owner_of_documents(store, _documents)
            .map(|state| (state.lists(), state.locations_wire()))
        else {
            return;
        };
        let caret = document.caret_byte(editor);
        let Some(anchor) = caret_anchor(document, caret) else {
            return;
        };
        let position = {
            let mut view = document.text().view();
            documents::text_ext::line_col_at(&mut view, caret as usize)
        };
        let feed = FeedId::mint();
        open_feed(store, lists, feed, "References".to_owned(), String::new());

        let fonts = ::editor::env::Fonts::of(store)();
        let theme = ::editor::env::Themes::of(store);
        let width = match document.layout_width(editor) {
            width if width > 1.0 => width,
            _ => FALLBACK_WIDTH,
        };
        let host = Some(document_id);
        // The open verb the card emits — windowless card, shell
        // window in the closure: the targeted open is queued as a
        // deferred window ask and the request drain lands it.
        let open: Arc<
            dyn Fn(&mut Store, editor::location::ResourceLocation, std::ops::Range<documents::text_ext::LineCol>)
                + Send
                + Sync,
        > = Arc::new(move |store, location, target| {
            crate::commands::AppRequests::push(
                store,
                Arc::new(OpenPicked {
                    location,
                    target: Some(target),
                }),
            );
        });
        let promote: Arc<dyn Fn(&mut Store) + Send + Sync> = Arc::new(move |store| {
            crate::commands::AppRequests::push(
                store,
                Arc::new(crate::hisearch::ShowFeedInDock { lists, wire, feed }),
            );
        });
        let view = PeekView::new(store, host, width, lists, feed, promote, open);
        let markup = peek_markup();
        document.ensure_document_markup(markup);
        let key = document.push_inlay(
            markup,
            anchor.clone(),
            Inlay::new(InlayMode::Under, view.clone()),
            store,
            ui,
            &fonts,
            &theme,
            fx,
        );
        document.swap_inlay(key, anchor, Inlay::new(InlayMode::Under, view.keyed(key)));
        document.set_focus(editor, EditorFocus::Inlay(key));

        let _ = (fx, wire);
        LocationLists::ask(
            store,
            lists,
            locations::LocationsAsk::Lsp {
                feed,
                kind: locations::LspKind::References,
                location: location.clone(),
                position,
            },
        );
    }
}

/// The peek's deliberate open — a deferred window ask: the request
/// drain supplies whichever window the gesture ran in.
struct OpenPicked {
    location: editor::location::ResourceLocation,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
}

impl crate::commands::DynamicCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "peek.open-picked"
    }
    fn name(&self) -> String {
        "Open Reference".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: crate::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(documents) =
            crate::window::Windows::session_state(store, window).map(|state| state.documents())
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
