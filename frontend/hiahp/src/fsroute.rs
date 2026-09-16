// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::fs::SeatDirectory;
use himark::higent::seat as fs;
use himark::{
    FetchDocumentEffect, ListDirectoryEffect, ResourceLocation, StoreDocumentEffect,
    SubscribeEffect, Subscription, UnsubscribeEffect,
};
use imba::effect::EffectHandler;

const LOCAL_AUTHORITY: &str = "local";

pub fn seat_of_authority(
    directory: &SeatDirectory,
    authority: &str,
) -> Option<(Arc<dyn himark::higent::AhpServer>, String)> {
    if fs::scoped(authority) {
        let (server, session) = fs::parse(authority)?;
        let seat = directory.seat(server)?;
        return Some((seat, session));
    }
    if authority == LOCAL_AUTHORITY {
        return directory.local_seat();
    }
    None
}

pub fn seat_of(
    directory: &SeatDirectory,
    location: &ResourceLocation,
) -> Option<(Arc<dyn himark::higent::AhpServer>, String)> {
    seat_of_authority(directory, location.authority().as_str())
}

pub fn served(location: &ResourceLocation) -> bool {
    let authority = location.authority().as_str();
    fs::scoped(authority) || authority == LOCAL_AUTHORITY
}

pub struct RouteFetch {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<FetchDocumentEffect> for RouteFetch {
    async fn handle(&self, effect: FetchDocumentEffect) -> Option<String> {
        if let Some((origin, raw)) = himark::hichanges::raw_ref(&effect.location) {
            let (seat, session) = seat_of_authority(&self.directory, &origin)?;
            return seat
                .resource_read(session, himark::higent::seat::ResourceUri::new(raw))
                .await;
        }
        let (seat, session) = seat_of(&self.directory, &effect.location)?;
        // NO channel side effects here: document channels ride
        // REGISTRATION (`DocsyncHook::opened`), never bare fetches —
        // a fetch for a not-yet-registered document (a diff side
        // being built) must not open a channel that adoption then
        // orphans (the 2026-09-15 double-subscription).
        seat.resource_read(session, self.uris.uri_of(&effect.location))
            .await
    }
}

pub struct RouteFetchBytes {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<himark::FetchResourceBytesEffect> for RouteFetchBytes {
    async fn handle(&self, effect: himark::FetchResourceBytesEffect) -> Option<Vec<u8>> {
        let (seat, session) = seat_of(&self.directory, &effect.origin)?;

        let uri = match absolute(&effect.reference) {
            true => himark::higent::seat::ResourceUri::new(effect.reference),
            false => {
                let location = relative_to(&effect.origin, &effect.reference)?;
                self.uris.uri_of(&location)
            }
        };
        seat.resource_read_bytes(session, uri).await
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
        himark::ResourceType::document(),
        origin.authority().clone(),
        segments,
    ))
}

pub struct RouteStore {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
    pub channels: Arc<crate::docsync::DocumentChannels>,
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
        let Some((seat, session)) = seat_of(&self.directory, &effect.location) else {
            return false;
        };
        seat.resource_write(session, self.uris.uri_of(&effect.location), effect.text)
            .await
    }
}

pub struct RouteList {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<ListDirectoryEffect> for RouteList {
    async fn handle(&self, effect: ListDirectoryEffect) -> Option<Vec<ResourceLocation>> {
        let (seat, session) = seat_of(&self.directory, &effect.location)?;
        let entries = seat
            .resource_list(session, self.uris.uri_of(&effect.location))
            .await?;
        Some(
            entries
                .into_iter()
                .map(|(name, directory)| {
                    let kind = match directory {
                        true => himark::ResourceType::directory(),
                        false => himark::ResourceType::document(),
                    };
                    effect.location.child(kind, &name)
                })
                .collect(),
        )
    }
}

pub struct RouteSubscribe {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<SubscribeEffect> for RouteSubscribe {
    async fn handle(&self, effect: SubscribeEffect) -> Option<Subscription> {
        let (seat, session) = seat_of(&self.directory, &effect.location)?;

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
        let handle = seat
            .clone()
            .resource_watch(session, self.uris.uri_of(&effect.location), events)
            .await?;
        let id = self.directory.adopt_watch(seat, handle);
        let _ = slot.set(id);
        Some(Subscription(id))
    }
}

pub struct RouteBase {
    pub refs: himark::hichanges::ChangeRefs,
}

impl EffectHandler<himark::FetchBaseEffect> for RouteBase {
    async fn handle(&self, effect: himark::FetchBaseEffect) -> Option<ResourceLocation> {
        if himark::hichanges::scoped(&effect.location) || !served(&effect.location) {
            return None;
        }
        let before = self
            .refs
            .lookup(&format!("/{}", effect.location.path().join("/")))?;

        let (origin, _) = himark::hichanges::raw_ref(&before)?;
        (origin == effect.location.authority().as_str()).then_some(before)
    }
}

pub struct RouteUnsubscribe {
    pub directory: Arc<SeatDirectory>,
}

impl EffectHandler<UnsubscribeEffect> for RouteUnsubscribe {
    async fn handle(&self, effect: UnsubscribeEffect) {
        if let Some((seat, handle)) = self.directory.release_watch(effect.subscription.0) {
            seat.resource_unwatch(handle).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ResourceLocation {
        ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("local"),
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
