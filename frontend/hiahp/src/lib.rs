// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The protocol crate: the AHP adapter (transport, session wire,
//! fs/doc/LSP routes, docsync), the HIGENT catalog (hosts, seats,
//! sessions, the chat) and the DRIVERS — every coroutine between a
//! himark shell and its agent hosts, with no window anywhere.

pub mod completion;
pub mod docsync;
pub mod drivers;
pub mod higent;

pub use completion::{
    Completion, CompletionCommand, CompletionFound, CompletionPopupView, PickedFile,
};
pub mod find;
pub mod fs;
pub mod fsroute;
pub mod locations;
pub mod lsproute;
pub mod open;
pub mod registry;
pub mod transport;
pub mod uris;
pub mod wire;

pub fn uuid_v4() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let stack = &seed as *const _ as usize as u128;
    let mut bits = seed ^ stack.rotate_left(64) ^ (std::process::id() as u128) << 96;
    let mut nibbles = String::with_capacity(36);
    for index in 0..32 {
        let nibble = (bits & 0xf) as u32;
        bits = bits >> 4 | (u128::from(nibble.wrapping_mul(2654435769)) << 100);
        match index {
            8 | 12 | 16 | 20 => nibbles.push('-'),
            _ => {}
        }
        let value = match index {
            12 => 4,
            16 => 8 | (nibble & 0x3),
            _ => nibble,
        };
        nibbles.push(char::from_digit(value, 16).expect("nibble"));
    }
    nibbles
}

/// The LOCAL fs session's uri — the daemon's lockfile library
/// (`host-discovery`) carries its own copy of this literal; the two
/// must agree.
pub const LOCAL_FS_SESSION: &str = "hihost-fs:/local";

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
    pub fn of_location(
        store: &imba::store::Store,
        location: &editor::ResourceLocation,
    ) -> SessionId {
        if let Some((host, session)) =
            crate::higent::seat::route(store, location.authority().as_str())
        {
            return SessionId { host, session };
        }
        Self::local_default(store)
    }

    pub fn local_default(store: &imba::store::Store) -> SessionId {
        let host = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0)
            .unwrap_or(crate::higent::HostId::LOCAL);
        SessionId {
            host,
            session: crate::higent::SessionUri::new(LOCAL_FS_SESSION),
        }
    }

    pub fn mint_scratch(store: &mut imba::store::Store) -> SessionId {
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
        self.session.as_str() != LOCAL_FS_SESSION
            && !self.session.as_str().starts_with("scratch-space:")
    }
}

/// The quick-open path find: fuzzy over names, capped, one answer.
/// Content search is the streaming locations channel
/// (`SearchLocationsEffect`), not a Find target.
pub struct FindEffect {
    pub folders: Vec<editor::ResourceLocation>,
    pub term: String,
}

impl std::fmt::Display for FindEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "find paths {}", self.term)
    }
}

impl imba::effect::Effect for FindEffect {
    type Result = Vec<editor::ResourceLocation>;
}

/// A live `ahp-locations:/…` result stream, as the ask effects
/// answer it: the seat and channel to subscribe/poll/unsubscribe
/// (docs/ahp/ahp-locations.md), plus the route's way back from the
/// stream's resource URIs to locations — the shell never parses URIs.
#[derive(Clone)]
pub struct LocationsChannel {
    pub seat: std::sync::Arc<dyn crate::higent::AhpServer>,
    pub channel: crate::higent::ChannelUri,
    pub resolve: std::sync::Arc<dyn Fn(&str) -> Option<editor::ResourceLocation> + Send + Sync>,
}

/// The streaming content search ask. Answers the channel; results
/// stream as `LocationList` batches; unsubscribing cancels the walk.
pub struct SearchLocationsEffect {
    pub folders: Vec<editor::ResourceLocation>,
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

impl imba::effect::Effect for SearchLocationsEffect {
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
    pub location: editor::ResourceLocation,
    pub position: documents::LineCol,
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

impl imba::effect::Effect for LspLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}

#[derive(Clone, Debug)]
pub struct LspAnswer {
    pub items: Vec<LspItem>,

    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub struct LspItem {
    pub label: String,
    pub detail: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,

    pub edit: Option<(std::ops::Range<documents::LineCol>, String)>,
    pub insert_text: Option<String>,
}

pub struct LspCompletionEffect {
    pub location: editor::ResourceLocation,
    pub position: documents::LineCol,
}

impl std::fmt::Display for LspCompletionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "lsp completion /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for LspCompletionEffect {
    type Result = Option<LspAnswer>;
}

/// The watch lane, Verb-flavored: landings route At the documents
/// collection — no window, no app command.
pub fn sync_document_watches(
    store: &mut imba::store::Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    fx: &mut imba::command::Fx<'_>,
) {
    documents::watch::sync_document_watches(store, documents, fx, move |document, subscription| {
        imba::command::Verb::at(
            documents,
            documents::DocumentsCommand::Watched(document, subscription),
        )
    });
}

/// Refetch one document from its host — the landing routes At the
/// collection like every other watch landing.
pub fn refetch_document(
    store: &mut imba::store::Store,
    documents_id: imba::store::Id<documents::OpenDocuments>,
    document: documents::DocumentId,
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(location) = documents::OpenDocuments::location(store, documents_id, document) else {
        return;
    };
    if documents::is_synthetic(&location) {
        return;
    }
    let serial = documents::OpenDocuments::stamp_refetch(store, documents_id, document);
    let _ = fx.push(
        imba::effect::AnyEffect::new(documents::FetchDocumentEffect { location }).map(
            move |text| {
                imba::command::Verb::at(
                    documents_id,
                    documents::DocumentsCommand::Refetched {
                        document,
                        serial,
                        text,
                    },
                )
            },
        ),
    );
}

/// Ask bases for every registered document that has none yet — the
/// landings route At the collection (himark re-exports this as its
/// own lane door).
pub fn sync_stripe_bases(
    store: &mut imba::store::Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    ui: &imba::UiCtx,
    fx: &mut imba::command::Fx<'_>,
) {
    fx.scope(
        move |command| imba::command::Verb::at(documents, command),
        |fx| {
            documents::diffs::sync_stripe_bases(
                store,
                documents,
                fx,
                |store, document, base, fx| {
                    documents::diffs::land_base_located(store, documents, ui, document, base, fx);
                },
            );
        },
    );
}
