// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Go-to reference's SHELL half: the editor command that mints the
//! feed and mounts the card, and the deferred window ask its picks
//! ride — the card itself is `locations::peek` (the view lives with
//! its model, docs/entities.md). The injected closures carry the
//! shell verbs: `open` queues `OpenPicked`, `promote` fronts the
//! feed in the Search dock tab.

use std::sync::Arc;

use imba::store::Store;

use crate::locations::{open_feed, FeedId, LocationLists};
use crate::{Document, EditorCommand, EditorFocus, Inlay, InlayMode};
use locations::peek::caret_anchor;

pub use ::locations::peek::{PeekCommand, PeekView};

const FALLBACK_WIDTH: f32 = 600.0;

fn peek_markup() -> crate::MarkupId {
    static ID: std::sync::OnceLock<crate::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(crate::MarkupId::mint)
}

pub struct GoToReference;

/// Closes over the pane's ids (docs/entities.md law 3): the card's
/// host is the document the command runs in, no owner is resolved.
impl documents::DocumentCommand for GoToReference {
    fn id(&self) -> &'static str {
        "code.go-to-reference"
    }

    fn name(&self) -> String {
        "Go to Reference".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::UiCtx,
        _documents: imba::store::Id<crate::OpenDocuments>,
        document_id: crate::DocumentId,
        document: &mut Document,
        editor: crate::EditorId,
        location: &crate::ResourceLocation,
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
        let Some((lists, wire)) = crate::higent::Hosts::owner_of_documents(store, _documents)
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
            crate::line_col_at(&mut view, caret as usize)
        };
        let feed = FeedId::mint();
        open_feed(store, lists, feed, "References".to_owned(), String::new());

        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let width = match document.layout_width(editor) {
            width if width > 1.0 => width,
            _ => FALLBACK_WIDTH,
        };
        let host = Some(document_id);
        // The open verb the card emits — windowless card, shell
        // window in the closure: the targeted open is queued as a
        // deferred window ask and the request drain lands it.
        let open: Arc<
            dyn Fn(&mut Store, crate::ResourceLocation, std::ops::Range<crate::LineCol>)
                + Send
                + Sync,
        > = Arc::new(move |store, location, target| {
            crate::AppRequests::push(
                store,
                Arc::new(OpenPicked {
                    location,
                    target: Some(target),
                }),
            );
        });
        let promote: Arc<dyn Fn(&mut Store) + Send + Sync> = Arc::new(move |store| {
            crate::AppRequests::push(
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
            crate::locations::LocationsAsk::Lsp {
                feed,
                kind: crate::locations::LspKind::References,
                location: location.clone(),
                position,
            },
        );
    }
}

/// The peek's deliberate open — a deferred window ask: the request
/// drain supplies whichever window the gesture ran in.
struct OpenPicked {
    location: crate::ResourceLocation,
    target: Option<std::ops::Range<crate::LineCol>>,
}

impl crate::DynamicCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "peek.open-picked"
    }
    fn name(&self) -> String {
        "Open Reference".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(documents) =
            crate::Windows::session_state(store, window).map(|state| state.documents())
        else {
            return;
        };
        fx.push(crate::open_by_location_effect(
            window,
            documents,
            self.location.clone(),
            true,
            true,
            self.target.clone(),
        ));
    }
}
