// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::fs::SeatDirectory;
use himark::higent::{LocationsAsk, ResourceUriMap, SearchKind};
use himark::{
    Authority, LocationsChannel, LspLocationsEffect, LspLocationsKind, ResourceLocation,
    ResourceType, SearchLocationsEffect,
};
use imba::effect::EffectHandler;

pub struct RouteSearchLocations {
    pub directory: Arc<SeatDirectory>,
}

impl EffectHandler<SearchLocationsEffect> for RouteSearchLocations {
    async fn handle(&self, effect: SearchLocationsEffect) -> Result<LocationsChannel, String> {
        let Some(first) = effect.folders.first() else {
            return Err("no folders to search".to_owned());
        };
        let Some((seat, session)) = crate::fsroute::seat_of(&self.directory, first) else {
            return Err(format!("no seat serves {}", first.authority().as_str()));
        };
        // A session's folders live on one seat; a stray foreign
        // authority in the list is skipped, not multiplexed.
        let authority = first.authority().clone();
        let folders: Vec<String> = effect
            .folders
            .iter()
            .filter(|folder| folder.authority().as_str() == authority.as_str())
            .map(|folder| ResourceUriMap::uri_of(&crate::uris::FileUris, folder).into_string())
            .collect();
        let ask = LocationsAsk {
            folders,
            query: effect.query,
            kind: match effect.regex {
                true => SearchKind::Regex,
                false => SearchKind::Text,
            },
            case_sensitive: effect.case_sensitive,
            limit: effect.limit,
        };
        let channel = seat.search_locations(session, ask).await?;
        Ok(LocationsChannel {
            seat,
            channel,
            resolve: resolver(authority),
        })
    }
}

pub struct RouteLspLocations {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn ResourceUriMap>,
}

impl EffectHandler<LspLocationsEffect> for RouteLspLocations {
    async fn handle(&self, effect: LspLocationsEffect) -> Result<LocationsChannel, String> {
        let Some((seat, session)) = crate::fsroute::seat_of(&self.directory, &effect.location)
        else {
            let authority = effect.location.authority().as_str();
            tracing::warn!(target: "ahp_wire", %authority, "lsp/locations: no seat serves the asked document");
            return Err(format!("no seat serves {authority}"));
        };
        let uri = self.uris.uri_of(&effect.location).into_string();
        let mut params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": effect.position.line, "character": effect.position.col },
        });
        let method = match effect.kind {
            LspLocationsKind::References => {
                params["context"] = serde_json::json!({ "includeDeclaration": true });
                "textDocument/references"
            }
            LspLocationsKind::Implementations => "textDocument/implementation",
        };
        let channel = match seat.lsp_locations(session, method.to_owned(), params).await {
            Ok(channel) => channel,
            Err(error) => {
                tracing::warn!(target: "ahp_wire", %method, %error, "lsp/locations ask failed");
                return Err(error);
            }
        };
        Ok(LocationsChannel {
            seat,
            channel,
            resolve: resolver(effect.location.authority().clone()),
        })
    }
}

/// The way back from a stream's resource URIs to locations — the
/// asked location's authority is reused, so remote-seat results stay
/// on the remote authority (the lsproute precedent).
fn resolver(authority: Authority) -> Arc<dyn Fn(&str) -> Option<ResourceLocation> + Send + Sync> {
    Arc::new(move |uri| {
        ResourceUriMap::location_of(
            &crate::uris::FileUris,
            &himark::higent::seat::ResourceUri::new(uri),
            ResourceType::document(),
            &authority,
        )
    })
}
