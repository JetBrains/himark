// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::app::{AppCommand, AppFx};
use crate::ResourceLocation;

pub use documents::{
    CreateDocumentEffect, DeleteResourceEffect, ListDirectoryEffect, MoveResourceEffect,
    PickSaveEffect, StoreDocumentEffect,
};

pub use documents::{BuildDocumentEffect, BuiltDocument};

pub struct OpenByLocationEffect {
    pub window: crate::WindowId,
    /// The collection the open lands into — stamped at launch.
    pub documents: imba::store::Id<crate::OpenDocuments>,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<crate::LineCol>>,

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

/// The pane road's boundary type (himark → hiahp): the changes view's
/// "Open Diff" and the diff navigator resolve the two sides on the UI
/// thread and launch this; the handler opens both sides and lands the
/// pane-open command.
pub struct OpenDiffByLocationsEffect {
    pub window: crate::WindowId,
    /// The collection both sides register into — stamped at launch.
    pub documents: imba::store::Id<crate::OpenDocuments>,
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

pub use documents::diff_views::{DiffSide, DiffSideInput, OpenDiffPairEffect, OpenedDiffPair};

pub fn open_by_location_effect(
    window: crate::WindowId,
    documents: imba::store::Id<crate::OpenDocuments>,
    location: ResourceLocation,
    primary: bool,
    focus: bool,
    target: Option<std::ops::Range<crate::LineCol>>,
) -> crate::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        window,
        documents,
        location,
        primary,
        target,
        focus,
    })
}

pub fn open_locations(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    locations: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) {
    let folders = crate::Windows::window_ref(store, window)
        .map(|entity| crate::higent::session_folders(store, &entity.current_session()))
        .unwrap_or_default();
    let locations: Vec<ResourceLocation> = locations
        .iter()
        .map(|location| {
            if location.authority().as_str() != "local" {
                return location.clone();
            }
            match folders
                .iter()
                .find(|folder| location.path().starts_with(folder.path()))
            {
                Some(folder) => ResourceLocation::new(
                    location.kind().clone(),
                    folder.authority().clone(),
                    location.path().to_vec(),
                ),
                None => location.clone(),
            }
        })
        .collect();
    let mut primary = true;
    let Some(documents) =
        crate::Windows::session_family(store, window).map(|family| family.documents())
    else {
        return;
    };
    for location in &locations {
        if let Some(document) = crate::OpenDocuments::by_location(store, documents, location) {
            if primary {
                if let Some(mut window_entity) = crate::Windows::window(store, window) {
                    window_entity.show_document(store, ui, window, document, None, false, fx);
                    crate::Windows::put(store, window, window_entity);
                }
            }
            primary = false;
            continue;
        }
        fx.push(open_by_location_effect(
            window,
            documents,
            location.clone(),
            primary,
            false,
            None,
        ));
        primary = false;
    }
}

pub use ::hiahp::{
    FindEffect, LocationsChannel, LspLocationsEffect, LspLocationsKind, SearchLocationsEffect,
    SessionId,
};
