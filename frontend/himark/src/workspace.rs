// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::app::{AppCommand, AppFx};
use crate::ResourceLocation;

pub struct StoreDocumentEffect {
    pub location: ResourceLocation,
    pub text: String,
}

impl Effect for StoreDocumentEffect {
    type Result = bool;
}

pub struct ListDirectoryEffect {
    pub location: ResourceLocation,
}

impl Effect for ListDirectoryEffect {
    type Result = Option<Vec<ResourceLocation>>;
}

pub struct PickSaveEffect {
    pub suggested: String,
}

impl Effect for PickSaveEffect {
    type Result = Option<ResourceLocation>;
}

pub struct BuildDocumentEffect {
    pub location: ResourceLocation,
    pub text: String,
}

impl Effect for BuildDocumentEffect {
    type Result = BuiltDocument;
}

#[derive(Clone)]
pub struct BuiltDocument {
    pub document: crate::Document,
}

pub struct OpenByLocationEffect {
    pub window: crate::WindowId,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<crate::LineCol>>,

    /// Focus the opened editor (a deliberate jump) or just show it.
    pub focus: bool,
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
    pub old: DiffSideInput,
    pub new: DiffSideInput,
}

impl Effect for OpenDiffByLocationsEffect {
    type Result = AppCommand;
}

/// The ONE off-thread step both diff roads share (docs/editor/diff-canvas.md
/// §4): ensure each side is a REGISTERED document. An OPEN side passes
/// through by id (no fetch, no build); a CLOSED side is fetched and
/// built here and registered at the landing — the standard open road.
/// It does NOT diff: the diff view's normalize lane computes the diff
/// from the registered documents (docs/no-diff-on-ui-thread). The
/// canvas has no business with Texts, parses, or operations.
pub struct OpenDiffPairEffect {
    pub old: DiffSideInput,
    pub new: DiffSideInput,
    /// The half width to lay the editors at (the canvas's content width).
    pub width: f32,
}

impl Effect for OpenDiffPairEffect {
    type Result = OpenedDiffPair;
}

/// One side to open, resolved on the UI thread at launch — a reference,
/// never content.
pub enum DiffSideInput {
    /// Already a registered document — use it as-is.
    Open(crate::DocumentId),
    /// Closed — the handler fetches and builds it, the landing registers.
    Fetch(ResourceLocation),
}

impl DiffSideInput {
    pub fn resolve(store: &imba::store::Store, location: ResourceLocation) -> Self {
        match crate::OpenDocuments::by_location(store, &location) {
            Some(document) => DiffSideInput::Open(document),
            None => DiffSideInput::Fetch(location),
        }
    }
}

#[derive(Clone)]
pub struct OpenedDiffPair {
    pub old: DiffSide,
    pub new: DiffSide,
    pub width: f32,
    /// Both sides gone — the caller reports instead of mounting.
    pub failed: bool,
}

/// What the landing does with a side: reuse the registered document, or
/// register the freshly-built one at its location (register-at-display —
/// the standard `BuiltDocument` payload).
#[derive(Clone)]
pub enum DiffSide {
    Open(crate::DocumentId),
    Built {
        location: ResourceLocation,
        document: BuiltDocument,
    },
}

pub fn open_by_location_effect(
    window: crate::WindowId,
    location: ResourceLocation,
    primary: bool,
    focus: bool,
    target: Option<std::ops::Range<crate::LineCol>>,
) -> crate::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        window,
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
    for location in &locations {
        if let Some(document) = crate::OpenDocuments::by_location(store, location) {
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

    pub session: String,
}

impl SessionId {
    pub fn local_default(store: &Store) -> SessionId {
        let host = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0)
            .unwrap_or(crate::higent::HostId::LOCAL);
        SessionId {
            host,
            session: host_discovery::LOCAL_FS_SESSION.to_owned(),
        }
    }

    pub fn mint_scratch(store: &mut Store) -> SessionId {
        let mut minted = 0;
        store.update::<ScratchSpaces>(|spaces| {
            spaces.0 += 1;
            minted = spaces.0;
        });
        SessionId {
            host: Self::local_default(store).host,
            session: format!("scratch-space:{minted}"),
        }
    }

    pub fn names_session(&self) -> bool {
        self.session != host_discovery::LOCAL_FS_SESSION
            && !self.session.starts_with("scratch-space:")
    }
}

#[derive(Clone, Default)]
pub struct ScratchSpaces(u64);

/// The quick-open path find: fuzzy over names, capped, one answer.
/// Content search is the streaming locations channel
/// (`SearchLocationsEffect`), not a Find target.
pub struct FindEffect {
    pub folders: Vec<ResourceLocation>,
    pub term: String,
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
    pub channel: String,
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

impl Effect for LspLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}
