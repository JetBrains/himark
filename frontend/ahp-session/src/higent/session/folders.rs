// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::state::Hosts;
use crate::SessionId;
use imba::store::Store;

pub fn session_folders(store: &Store, session: &SessionId) -> Vec<editor::location::ResourceLocation> {
    let Some(uris) = Hosts::uris(store, session.host) else {
        return Vec::new();
    };
    let Some(host) = Hosts::host_ref(store, session.host) else {
        return Vec::new();
    };
    let mirrored: Vec<String> = match host.states.get(&session.session) {
        Some(channel) => channel.working_directories.iter().cloned().collect(),
        None => host
            .summary(&session.session)
            .and_then(|summary| summary.working_directories.clone())
            .unwrap_or_default(),
    };
    mirrored
        .iter()
        .filter_map(|uri| folder_location(uris.as_ref(), session, uri))
        .collect()
}

pub fn all_session_folders(store: &Store) -> Vec<editor::location::ResourceLocation> {
    let mut seen = std::collections::HashSet::new();
    let mut folders = Vec::new();
    for (id, host) in Hosts::list(store) {
        let mut sessions: Vec<crate::higent::SessionUri> = host
            .sessions
            .iter()
            .map(|s| crate::higent::SessionUri::new(s.resource.clone()))
            .collect();
        for (session, _) in host.states.iter() {
            sessions.push(session.clone());
        }
        for session in sessions {
            let key = SessionId { host: id, session };
            for folder in session_folders(store, &key) {
                if seen.insert(folder.clone()) {
                    folders.push(folder);
                }
            }
        }
    }
    folders
}

fn folder_location(
    uris: &dyn crate::higent::client::ResourceUriMap,
    key: &SessionId,
    uri: &str,
) -> Option<editor::location::ResourceLocation> {
    uris.location_of(
        &crate::higent::client::ResourceUri::new(uri),
        editor::location::ResourceType::directory(),
        &editor::location::Authority::new(crate::higent::client::authority(key.host, &key.session)),
    )
    .filter(|location| !location.path().is_empty())
}
