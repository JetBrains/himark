// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_wire::fs::{client_of, client_of_authority, served, ClientDirectory};

use documents::watch::{SubscribeEffect, Subscription, UnsubscribeEffect};
use documents::{
    CreateDocumentEffect, DeleteResourceEffect, FetchDocumentEffect, ListDirectoryEffect,
    MoveResourceEffect, StoreDocumentEffect,
};
use editor::location::ResourceLocation;
use imba::effect::EffectHandler;

pub struct RouteFetch {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<FetchDocumentEffect> for RouteFetch {
    async fn handle(&self, effect: FetchDocumentEffect) -> Option<String> {
        if let Some((origin, raw)) = changesview::hichanges::raw_ref(&effect.location) {
            let (client, session) = client_of_authority(&self.directory, &origin)?;
            return client
                .resources
                .resource_read(session, ahp_wire::client::ResourceUri::new(raw))
                .await;
        }
        let (client, session) = client_of(&self.directory, &effect.location)?;
        // NO channel side effects here: document channels ride
        // REGISTRATION (`DocsyncHook::opened`), never bare fetches —
        // a fetch for a not-yet-registered document (a diff side
        // being built) must not open a channel that adoption then
        // orphans (the 2026-09-15 double-subscription).
        client
            .resources
            .resource_read(session, self.uris.uri_of(&effect.location))
            .await
    }
}

pub struct RouteFetchBytes {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<documents::FetchResourceBytesEffect> for RouteFetchBytes {
    async fn handle(&self, effect: documents::FetchResourceBytesEffect) -> Option<Vec<u8>> {
        let (client, session) = client_of(&self.directory, &effect.origin)?;

        let uri = match absolute(&effect.reference) {
            true => ahp_wire::client::ResourceUri::new(effect.reference),
            false => {
                let location = relative_to(&effect.origin, &effect.reference)?;
                self.uris.uri_of(&location)
            }
        };
        client.resources.resource_read_bytes(session, uri).await
    }
}

fn absolute(reference: &str) -> bool {
    reference.starts_with("//")
        || reference
            .split_once("://")
            .is_some_and(|(scheme, _)| !scheme.is_empty() && !scheme.contains('/'))
}

fn relative_to(origin: &ResourceLocation, reference: &str) -> Option<ResourceLocation> {
    let path = reference.split(['?', '#']).next().unwrap_or(reference);
    let mut segments: Vec<String> = origin.path().to_vec();
    segments.pop();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            part => segments.push(part.to_owned()),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(ResourceLocation::new(
        editor::location::ResourceType::document(),
        origin.authority().clone(),
        segments,
    ))
}

pub struct RouteStore {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    pub channels: Arc<ahp_docsync::DocumentChannels>,
}

impl EffectHandler<StoreDocumentEffect> for RouteStore {
    async fn handle(&self, effect: StoreDocumentEffect) -> bool {
        // Mode one: the host mirrors this document — the mirror is
        // the source of truth, so the save flows through the channel,
        // ordered behind this client's edits, and the host dumps its
        // own text. Writing the resource raw here would look like a
        // foreign edit to the mirror's watcher.
        if let Some(handle) = self.channels.store_handle(&effect.location).await {
            return handle.store(self.uris.uri_of(&effect.location)).await;
        }
        // Mode two: no document channel — the resource is the truth.
        let Some((client, session)) = client_of(&self.directory, &effect.location) else {
            return false;
        };
        client
            .resources
            .resource_write(session, self.uris.uri_of(&effect.location), effect.text)
            .await
    }
}

pub struct RouteList {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<ListDirectoryEffect> for RouteList {
    async fn handle(&self, effect: ListDirectoryEffect) -> Option<Vec<ResourceLocation>> {
        let (client, session) = client_of(&self.directory, &effect.location)?;
        let entries = client
            .resources
            .resource_list(session, self.uris.uri_of(&effect.location))
            .await?;
        Some(
            entries
                .into_iter()
                .map(|(name, directory)| {
                    let kind = match directory {
                        true => editor::location::ResourceType::directory(),
                        false => editor::location::ResourceType::document(),
                    };
                    effect.location.child(kind, &name)
                })
                .collect(),
        )
    }
}

pub struct RouteCreate {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<CreateDocumentEffect> for RouteCreate {
    async fn handle(&self, effect: CreateDocumentEffect) -> bool {
        let Some((client, session)) = client_of(&self.directory, &effect.location) else {
            return false;
        };
        client
            .resources
            .resource_create(session, self.uris.uri_of(&effect.location))
            .await
    }
}

pub struct RouteDelete {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<DeleteResourceEffect> for RouteDelete {
    async fn handle(&self, effect: DeleteResourceEffect) -> bool {
        let Some((client, session)) = client_of(&self.directory, &effect.location) else {
            return false;
        };
        client
            .resources
            .resource_delete(
                session,
                self.uris.uri_of(&effect.location),
                effect.recursive,
            )
            .await
    }
}

pub struct RouteMove {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<MoveResourceEffect> for RouteMove {
    async fn handle(&self, effect: MoveResourceEffect) -> bool {
        // One client serves both ends: a move never crosses authorities.
        let Some((client, session)) = client_of(&self.directory, &effect.from) else {
            return false;
        };
        client
            .resources
            .resource_move(
                session,
                self.uris.uri_of(&effect.from),
                self.uris.uri_of(&effect.to),
            )
            .await
    }
}

pub struct RouteSubscribe {
    pub directory: Arc<ClientDirectory>,
    pub uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<SubscribeEffect> for RouteSubscribe {
    async fn handle(&self, effect: SubscribeEffect) -> Option<Subscription> {
        let (client, session) = client_of(&self.directory, &effect.location)?;

        let slot = Arc::new(std::sync::OnceLock::new());
        let deliver = self.directory.deliver();
        let events = {
            let slot = Arc::clone(&slot);
            Arc::new(move || {
                if let Some(id) = slot.get() {
                    deliver(*id);
                }
            }) as Arc<dyn Fn() + Send + Sync>
        };
        let resources = std::sync::Arc::clone(&client.resources);
        let handle = resources
            .resource_watch(session, self.uris.uri_of(&effect.location), events)
            .await?;
        let id = self.directory.adopt_watch(resources, handle);
        let _ = slot.set(id);
        Some(Subscription(id))
    }
}

/// The BASE RESOLVER (installed as `StripeBases`): a working file's
/// base ref, read straight off the `Changes` model at ask time —
/// synchronous, store in hand, no worker hop. (This replaced an async
/// handler fed through a shared Arc<Mutex<HashMap>>.)
pub fn resolve_base(
    store: &imba::store::Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: &ResourceLocation,
) -> Option<ResourceLocation> {
    if changesview::hichanges::scoped(location) || !served(location) {
        return None;
    }
    // The ask names its documents collection; the bases live in the
    // change sets next to it.
    let changes = crate::session::state::Hosts::owner_of_documents(store, documents)?.changes();
    let before = changesview::hichanges::Changes::base_ref(
        store,
        changes,
        &format!("/{}", location.path().join("/")),
    )?;
    let (origin, _) = changesview::hichanges::raw_ref(&before)?;
    (origin == location.authority().as_str()).then_some(before)
}

pub struct RouteUnsubscribe {
    pub directory: Arc<ClientDirectory>,
}

impl EffectHandler<UnsubscribeEffect> for RouteUnsubscribe {
    async fn handle(&self, effect: UnsubscribeEffect) {
        if let Some((client, handle)) = self.directory.release_watch(effect.subscription.0) {
            client.resource_unwatch(handle).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ResourceLocation {
        ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("local"),
            vec!["repo".to_owned(), "docs".to_owned(), "page.md".to_owned()],
        )
    }

    #[test]
    fn absolute_references_are_the_ones_carrying_a_scheme() {
        for reference in [
            "https://example.com/a.png",
            "http://example.com/a.png",
            "file:///tmp/a.png",
            "//cdn.example.com/a.png",
        ] {
            assert!(absolute(reference), "{reference:?}");
        }
        for reference in ["a.png", "./a.png", "../img/a.png", "img/a.png", "a:b.png"] {
            assert!(!absolute(reference), "{reference:?}");
        }
    }

    #[test]
    fn relative_references_resolve_against_the_document() {
        let base = page();
        let of = |reference: &str| {
            relative_to(&base, reference)
                .map(|location| location.path().to_vec())
                .expect("resolved")
        };
        assert_eq!(of("shot.png"), ["repo", "docs", "shot.png"]);
        assert_eq!(of("./shot.png"), ["repo", "docs", "shot.png"]);
        assert_eq!(of("img/shot.png"), ["repo", "docs", "img", "shot.png"]);
        assert_eq!(of("../shot.png"), ["repo", "shot.png"]);
        assert_eq!(of("shot.png?v=2"), ["repo", "docs", "shot.png"]);
        assert_eq!(of("shot.png#frag"), ["repo", "docs", "shot.png"]);

        assert!(relative_to(&base, "../../..").is_none());
    }
}
