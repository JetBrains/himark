// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's OPEN ROADS — session machinery, origin-typed, shell
//! agnostic: open a document by location, open a diff pair by
//! locations, and the OPENED landing that files the result into the
//! collection stamped at launch and hands the SEATED SHELL a ready
//! place. Placement is the shell's answer (`crate::himark::Shell`);
//! the iOS shell rides these without a workbench in the build.

use std::sync::Arc;

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::bridge::ViewBridgeId;
use crate::perform::{AppCommand, AppFx};
use documents::diff_views::DiffSideInput;
use documents::OpenDocuments;
use editor::document::Document;
use editor::location::ResourceLocation;

pub struct OpenByLocationEffect {
    /// The gesture's origin bridge — the landing navigates FROM it.
    pub origin: ViewBridgeId,
    /// The collection the open lands into — stamped at launch.
    pub documents: imba::store::Id<OpenDocuments>,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,

    /// Focus the opened editor (a deliberate jump) or just show it.
    pub focus: bool,
}

impl std::fmt::Display for OpenByLocationEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "open by location /{}", self.location.path().join("/"))
    }
}

impl Effect for OpenByLocationEffect {
    type Result = AppCommand;
}

/// The pane road's boundary type (himark → hiahp): a diff ask
/// resolves its two sides on the UI thread and launches this; the
/// handler opens both sides and lands the ready place at the origin.
pub struct OpenDiffByLocationsEffect {
    pub origin: ViewBridgeId,
    /// The collection both sides register into — stamped at launch.
    pub documents: imba::store::Id<OpenDocuments>,
    pub old: DiffSideInput,
    pub new: DiffSideInput,
}

impl std::fmt::Display for OpenDiffByLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("open diff by locations")
    }
}

impl Effect for OpenDiffByLocationsEffect {
    type Result = AppCommand;
}

pub fn open_by_location_effect(
    origin: ViewBridgeId,
    documents: imba::store::Id<OpenDocuments>,
    location: ResourceLocation,
    primary: bool,
    focus: bool,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
) -> crate::effects::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        origin,
        documents,
        location,
        primary,
        target,
        focus,
    })
}

/// A fetched document, landing.
pub struct OpenedDocument {
    /// The collection the open was launched FOR — stamped at the
    /// gesture, so the landing files the document into the session the
    /// user acted in, not whatever the origin shows by then.
    pub documents: imba::store::Id<OpenDocuments>,

    pub name: String,
    pub document: Document,

    pub location: Option<ResourceLocation>,

    pub primary: bool,

    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,

    /// Move the keyboard to the opened editor (a deliberate jump —
    /// a click or Enter) — or just show it, leaving the keyboard
    /// where it stands (a selection move browsing results).
    pub focus: bool,
}

/// The OPENED landing as an origin command: file the document into
/// the stamped collection, run the document lanes, and when the open
/// was primary hand the seated shell the READY place.
pub fn opened(origin: ViewBridgeId, document: OpenedDocument) -> AppCommand {
    AppCommand::AtOrigin(origin, Arc::new(OpenedLanding(std::sync::Mutex::new(Some(document)))))
}

struct OpenedLanding(std::sync::Mutex<Option<OpenedDocument>>);

impl crate::commands::HimarkCommand for OpenedLanding {
    fn id(&self) -> &'static str {
        "file.opened"
    }
    fn name(&self) -> String {
        "File Opened".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut AppFx<'_>,
    ) {
        let Some(opened) = self.0.lock().expect("the opened landing").take() else {
            eprintln!("[opening] an opened landing was already spent — dropped");
            return;
        };
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        // The landing files into the collection stamped at launch —
        // never the origin's CURRENT session, which may have
        // switched while the open was in flight.
        let documents = opened.documents;
        let document = opened.document;
        let saved_revision = document.revision();

        let document_id = match opened
            .location
            .as_ref()
            .and_then(|location| OpenDocuments::by_location(store, documents, location))
        {
            Some(existing) => existing,
            None => OpenDocuments::register(
                store,
                documents,
                document.clone(),
                opened.location,
                opened.name,
                saved_revision,
            ),
        };

        fx.scope(AppCommand::Verb, |fx| {
            documents::lanes::sync_document_watches(store, documents, fx)
        });
        fx.scope(AppCommand::Verb, |fx| {
            documents::lanes::sync_stripe_bases(store, documents, ui, fx)
        });

        if opened.primary {
            fx.scope(AppCommand::Verb, |fx| {
                crate::shell::navigate_origin(
                    store,
                    ui,
                    origin,
                    crate::places::OpenDocumentPlace {
                        document: document_id,
                        target: opened.target,
                        focus: opened.focus,
                    },
                    fx,
                )
            });
        }
    }
}
