// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The TAILNET as the catalog's neighbours see it: the client node's
//! state (one immutable value the shells render) and the flows a
//! shell fires to move it. The scanner that WRITES the state and
//! the connector behind the flows live in the embedder (`hitailnet`,
//! docs/ahp/tailnet.md); this module is only the vocabulary, placed
//! here so the sessions drawer and the share command can speak it
//! without knowing tailscale.

use std::sync::Arc;

/// What a shell renders of the tailnet. Immutable value, replaced
/// whole.
#[derive(Clone, PartialEq, Default, Debug)]
pub enum TailnetState {
    #[default]
    Off,
    Starting,
    NeedsLogin {
        auth_url: String,
    },
    WaitingApproval,
    Running,
}

impl TailnetState {
    /// The C ABI encoding — integers exist ONLY at this edge.
    /// Append-only once a header ships.
    pub fn code(&self) -> u32 {
        match self {
            TailnetState::Off => 0,
            TailnetState::Starting => 1,
            TailnetState::NeedsLogin { .. } => 2,
            TailnetState::WaitingApproval => 3,
            TailnetState::Running => 4,
        }
    }

    /// The word a status line shows while the node is on its way —
    /// `None` when there is nothing to say (off, or running).
    pub fn progress_word(&self) -> Option<&'static str> {
        match self {
            TailnetState::Off | TailnetState::Running => None,
            TailnetState::Starting => Some("Tailnet: starting…"),
            TailnetState::NeedsLogin { .. } => Some("Tailnet: sign in in the browser…"),
            TailnetState::WaitingApproval => Some("Tailnet: waiting for the admin's approval…"),
        }
    }
}

/// The store component the faces read. Written only by the
/// embedder's state landing.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct TailnetFaceState {
    pub state: TailnetState,
}

impl TailnetFaceState {
    pub fn of(store: &imba::store::Store) -> TailnetState {
        store
            .get::<TailnetFaceState>()
            .map(|held| held.state.clone())
            .unwrap_or_default()
    }
}

/// The shell's tailnet verbs, installed in the store at boot by the
/// embedder — the closures carry what they captured there (the
/// scanner's channel, the connector, the poster).
#[derive(Clone)]
pub struct TailnetFlows {
    /// Join the tailnet as a client: start the node, log in when it
    /// asks, and let discovery list the hosts.
    pub connect: Arc<dyn Fn() + Send + Sync>,
    /// The foregrounding kick: revive a suspension-wedged engine
    /// and nudge the Failed rows to redial.
    pub resume: Arc<dyn Fn() + Send + Sync>,
}

impl TailnetFlows {
    /// Fire the connect flow when an embedder installed one; answers
    /// whether anyone was there to hear it.
    pub fn connect(store: &imba::store::Store) -> bool {
        match store.get::<TailnetFlows>() {
            Some(flows) => {
                (flows.connect)();
                true
            }
            None => false,
        }
    }
}
