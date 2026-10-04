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

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionId {
    pub host: crate::higent::HostId,

    pub session: crate::higent::SessionUri,
}

impl SessionId {
    /// Which session OWNS a location: the one whose seat routes its
    /// authority, else the local workspace — the address-derived
    /// owner, never an ambient scope. A plain file's state belongs to
    /// the local session, not to nothing.
    pub fn of_location(store: &Store, location: &crate::ResourceLocation) -> SessionId {
        if let Some((host, session)) =
            crate::higent::seat::route(store, location.authority().as_str())
        {
            return SessionId { host, session };
        }
        Self::local_default(store)
    }

    pub fn local_default(store: &Store) -> SessionId {
        let host = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0)
            .unwrap_or(crate::higent::HostId::LOCAL);
        SessionId {
            host,
            session: crate::higent::SessionUri::new(host_discovery::LOCAL_FS_SESSION),
        }
    }

    pub fn mint_scratch(store: &mut Store) -> SessionId {
        // Monotonic and never reused — the `Id::mint` pattern; a
        // store-held counter bought nothing but a component.
        static MINT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let minted = MINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SessionId {
            host: Self::local_default(store).host,
            session: crate::higent::SessionUri::new(format!("scratch-space:{minted}")),
        }
    }

    pub fn names_session(&self) -> bool {
        self.session.as_str() != host_discovery::LOCAL_FS_SESSION
            && !self.session.as_str().starts_with("scratch-space:")
    }
}

/// The quick-open path find: fuzzy over names, capped, one answer.
/// Content search is the streaming locations channel
/// (`SearchLocationsEffect`), not a Find target.
pub struct FindEffect {
    pub folders: Vec<ResourceLocation>,
    pub term: String,
}

impl std::fmt::Display for FindEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "find paths {}", self.term)
    }
}

impl Effect for FindEffect {
    type Result = Vec<ResourceLocation>;
}

/// A live `ahp-locations:/…` result stream, as the ask effects
/// answer it: the seat and channel to subscribe/poll/unsubscribe
/// (docs/ahp/ahp-locations.md), plus the route's way back from the
/// stream's resource URIs to locations — himark never parses URIs.
#[derive(Clone)]
pub struct LocationsChannel {
    pub seat: std::sync::Arc<dyn crate::higent::AhpServer>,
    pub channel: crate::higent::ChannelUri,
    pub resolve: std::sync::Arc<dyn Fn(&str) -> Option<ResourceLocation> + Send + Sync>,
}

/// The streaming content search ask. Answers the channel; results
/// stream as `LocationList` batches; unsubscribing cancels the walk.
pub struct SearchLocationsEffect {
    pub folders: Vec<ResourceLocation>,
    pub query: String,
    /// Literal by default; the query as a regular expression when set.
    pub regex: bool,
    pub case_sensitive: bool,
    pub limit: usize,
}

impl std::fmt::Display for SearchLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "search locations {}", self.query)
    }
}

impl Effect for SearchLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LspLocationsKind {
    References,
    Implementations,
}

/// The location-answering LSP asks, streamed over the same channel
/// shape as the content search.
pub struct LspLocationsEffect {
    pub location: ResourceLocation,
    pub position: crate::LineCol,
    pub kind: LspLocationsKind,
}

impl std::fmt::Display for LspLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "lsp locations {:?} /{}",
            self.kind,
            self.location.path().join("/")
        )
    }
}

impl Effect for LspLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}
