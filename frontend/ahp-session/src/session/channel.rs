// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The session CHANNEL mirror: the wire's session-level actions
//! (chats added and retitled, folders granted, config and changesets
//! moved) applied to the local catalog, and their side effects fanned
//! out to the owning collections. TWO standing pollers drain the one
//! shared wire feed for a session — the driver's and hichanges' — and
//! whichever wakes first takes the whole batch, so both must route
//! every action kind through here; a partial handler silently loses
//! the rest of the batch for everyone.

use std::sync::Arc;

use ahp_types::actions::StateAction;
use ahp_types::state::ChatSummary;
use ahp_wire::client::{ChatUri, SessionChannel};
use ahp_wire::SessionId;
use imba::command::Fx;
use imba::store::Store;

use super::agents::Agents;

/// Adopt a subscribe answer whole: the host's session state becomes
/// the channel mirror — chats, default chat, folders, config.
pub fn adopt_subscribed(
    store: &mut Store,
    key: &SessionId,
    state: &ahp_types::state::SessionState,
) {
    Agents::set_channel(
        store,
        key,
        SessionChannel {
            provider: state.provider.clone(),
            chats: state.chats.iter().cloned().collect(),
            default_chat: state.default_chat.clone().map(ChatUri::new),
            working_directories: state
                .working_directories
                .iter()
                .flatten()
                .cloned()
                .collect(),
            config: state.config.clone().map(Arc::new),
        },
    );
}

pub fn apply_channel_actions(
    store: &mut Store,
    key: &SessionId,
    actions: &[StateAction],
    fx: &mut Fx<'_>,
) {
    let mut channel = Agents::channel(store, key).unwrap_or_default();
    let mut folders_grew = false;
    for action in actions {
        match action {
            StateAction::SessionChatAdded(added) => {
                let kept: rpds::VectorSync<ChatSummary> = channel
                    .chats
                    .iter()
                    .filter(|held| held.resource != added.summary.resource)
                    .cloned()
                    .collect();
                let mut chats = kept;
                chats.push_back_mut(added.summary.clone());
                channel.chats = chats;
            }
            StateAction::SessionChatRemoved(removed) => {
                channel.chats = channel
                    .chats
                    .iter()
                    .filter(|held| held.resource != removed.chat)
                    .cloned()
                    .collect();
            }
            StateAction::SessionChatUpdated(updated) => {
                channel.chats = channel
                    .chats
                    .iter()
                    .map(|held| {
                        let mut held = held.clone();
                        if held.resource == updated.chat {
                            merge_chat_summary(&mut held, updated);
                        }
                        held
                    })
                    .collect();
            }
            StateAction::SessionWorkingDirectorySet(set) => {
                if !channel
                    .working_directories
                    .iter()
                    .any(|held| held == &set.directory)
                {
                    channel
                        .working_directories
                        .push_back_mut(set.directory.clone());
                    folders_grew = true;
                }
            }
            StateAction::SessionWorkingDirectoryRemoved(removed) => {
                channel.working_directories = channel
                    .working_directories
                    .iter()
                    .filter(|held| **held != removed.directory)
                    .cloned()
                    .collect();
            }

            StateAction::SessionConfigChanged(changed) => {
                if let Some(held) = &channel.config {
                    let mut config = (**held).clone();
                    if changed.replace.unwrap_or(false) {
                        config.values = changed.config.clone();
                    } else {
                        for (key, value) in &changed.config {
                            config.values.insert(key.clone(), value.clone());
                        }
                    }
                    channel.config = Some(Arc::new(config));
                }
            }
            StateAction::SessionChangesetsChanged(changed) => {
                // The session channel's catalog names the session it
                // serves; its session's DRIVER takes the entries.
                if let Some(wire) =
                    super::state::Hosts::state(store, key).map(|state| state.changes_wire())
                {
                    ahp_changes::changes::adopt_session_catalog(store, key, wire, changed, fx);
                }
            }
            _ => {}
        }
    }
    Agents::set_channel(store, key, channel);

    if folders_grew {
        // The attach path (`EnterSessionWork`) arms comments for
        // every folder; one added mid-session gets the same here.
        // The channel names the session it serves; the session's
        // collection takes the folder.
        if let Some(state) = super::state::Hosts::state(store, key) {
            let (comments, diagnostics) = (state.comments_wire(), state.diagnostics_wire());
            for folder in super::folders::session_folders(store, key) {
                ahp_comments::ensure(store, comments, &folder, fx);
                ahp_lsp::diagnostics::ensure(store, diagnostics, &folder, fx);
            }
        }
    }
}

fn merge_chat_summary(
    summary: &mut ChatSummary,
    updated: &ahp_types::actions::SessionChatUpdatedAction,
) {
    let changes = &updated.changes;
    if let Some(title) = &changes.title {
        summary.title = title.clone();
    }
    if let Some(status) = changes.status {
        summary.status = status;
    }
    if let Some(activity) = &changes.activity {
        summary.activity = Some(activity.clone());
    }
    if let Some(modified) = &changes.modified_at {
        summary.modified_at = modified.clone();
    }
}
