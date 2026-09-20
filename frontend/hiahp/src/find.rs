// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::fs::SeatDirectory;
use himark::higent::{SearchAsk, SearchKind, SearchTarget};
use himark::{FindEffect, ResourceLocation, ResourceType};
use imba::effect::EffectHandler;

const PATH_CAP: usize = 128;

pub struct NativeFindHandler {
    pub directory: Arc<SeatDirectory>,
}

impl EffectHandler<FindEffect> for NativeFindHandler {
    async fn handle(&self, effect: FindEffect) -> Vec<ResourceLocation> {
        let cap = PATH_CAP;
        if effect.term.is_empty() {
            return Vec::new();
        }

        let (kind, target) = (SearchKind::Fuzzy, SearchTarget::Path);
        let mut found = Vec::new();
        for folder in &effect.folders {
            let remaining = cap - found.len();
            if remaining == 0 {
                break;
            }
            let Some((seat, session)) = crate::fsroute::seat_of(&self.directory, folder) else {
                continue;
            };
            let ask = SearchAsk {
                folders: vec![himark::higent::ResourceUriMap::uri_of(
                    &crate::uris::FileUris,
                    folder,
                )
                .into_string()],
                query: effect.term.clone(),
                kind,
                case_sensitive: false,
                target,
                limit: remaining,
            };
            let Some(answer) = seat.search(session, ask).await else {
                continue;
            };
            for uri in answer.hits {
                if let Some(location) = location_under(folder, &uri) {
                    found.push(location);
                }
            }
        }
        found
    }
}

fn location_under(folder: &ResourceLocation, uri: &str) -> Option<ResourceLocation> {
    let prefix =
        himark::higent::ResourceUriMap::uri_of(&crate::uris::FileUris, folder).into_string();
    let rest = uri.strip_prefix(&prefix)?.strip_prefix('/')?;
    let mut location = folder.clone();
    let mut segments = rest
        .split('/')
        .filter(|segment| !segment.is_empty())
        .peekable();
    segments.peek()?;
    while let Some(segment) = segments.next() {
        let kind = match segments.peek() {
            Some(_) => ResourceType::directory(),
            None => ResourceType::document(),
        };
        location = location.child(kind, segment);
    }
    Some(location)
}
