// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::state::Hosts;
use ahp_wire::SessionId;
use imba::store::Store;

pub fn session_folders(
    store: &Store,
    session: &SessionId,
) -> Vec<editor::location::ResourceLocation> {
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
        let mut sessions: Vec<ahp_wire::client::SessionUri> = host
            .sessions
            .iter()
            .map(|s| ahp_wire::client::SessionUri::new(s.resource.clone()))
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
    uris: &dyn ahp_wire::client::ResourceUriMap,
    key: &SessionId,
    uri: &str,
) -> Option<editor::location::ResourceLocation> {
    uris.location_of(
        &ahp_wire::client::ResourceUri::new(uri),
        editor::location::ResourceType::directory(),
        &editor::location::Authority::new(ahp_wire::client::authority(key.host, &key.session)),
    )
    .filter(|location| !location.path().is_empty())
}

/// A folder's short name — the last path segment, which is what a row
/// has room for. Trailing slashes are not a segment.
pub fn folder_label(folder: &str) -> String {
    let trimmed = folder.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().filter(|name| !name.is_empty());
    name.unwrap_or(trimmed).to_owned()
}

/// The short names of a session's folders, in order.
pub fn folders_label(folders: &[String]) -> String {
    folders
        .iter()
        .map(|folder| folder_label(folder))
        .collect::<Vec<_>>()
        .join(", ")
}
