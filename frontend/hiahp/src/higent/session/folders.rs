// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::state::Hosts;
use crate::SessionId;
use imba::store::Store;

pub fn session_folders(store: &Store, session: &SessionId) -> Vec<editor::ResourceLocation> {
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

pub fn all_session_folders(store: &Store) -> Vec<editor::ResourceLocation> {
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
    uris: &dyn crate::higent::seat::ResourceUriMap,
    key: &SessionId,
    uri: &str,
) -> Option<editor::ResourceLocation> {
    uris.location_of(
        &crate::higent::seat::ResourceUri::new(uri),
        editor::ResourceType::directory(),
        &editor::Authority::new(crate::higent::seat::authority(key.host, &key.session)),
    )
    .filter(|location| !location.path().is_empty())
}
