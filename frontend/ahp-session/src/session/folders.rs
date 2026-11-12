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

/// The folders a host's sessions work in, most recently used first
/// and each once — what a NEW session on that host is likely to
/// want. Raw folder uris, as the summaries carry them; the live
/// mirror's folders count too (a channel may have moved ahead of its
/// summary).
pub fn host_recent_folders(store: &Store, host: ahp_wire::client::HostId) -> Vec<String> {
    let Some(record) = Hosts::host_ref(store, host) else {
        return Vec::new();
    };
    let mut sessions: Vec<&ahp_types::state::SessionSummary> = record.sessions.iter().collect();
    sessions.sort_by_key(|summary| std::cmp::Reverse(super::summary::modified_stamp(summary)));
    let mut seen = std::collections::HashSet::new();
    let mut folders = Vec::new();
    let mut take = |folder: &str| {
        if !folder.is_empty() && seen.insert(folder.to_owned()) {
            folders.push(folder.to_owned());
        }
    };
    for summary in sessions {
        let session = ahp_wire::client::SessionUri::new(summary.resource.clone());
        if let Some(channel) = record.states.get(&session) {
            for folder in channel.working_directories.iter() {
                take(folder);
            }
        }
        for folder in summary.working_directories.iter().flatten() {
            take(folder);
        }
    }
    folders
}

/// A folder's short name — the last path segment, which is what a row
/// has room for. Trailing slashes are not a segment.
pub fn folder_label(folder: &str) -> String {
    let trimmed = folder.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().filter(|name| !name.is_empty());
    name.unwrap_or(trimmed).to_owned()
}
