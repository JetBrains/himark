// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The AHP wire: the transport, the `WireHost` protocol client, the
//! per-domain client facets, and the thin effect handlers between
//! them — no catalog, no collections, no window anywhere.

pub mod client;
pub mod effects;
pub mod fs;
pub mod registry;
pub mod transport;
pub mod uris;
pub mod wire;

pub fn uuid_v4() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let stack = &seed as *const _ as usize as u128;
    let mut bits = seed ^ stack.rotate_left(64) ^ (std::process::id() as u128) << 96;
    let mut nibbles = String::with_capacity(36);
    for index in 0..32 {
        let nibble = (bits & 0xf) as u32;
        bits = bits >> 4 | (u128::from(nibble.wrapping_mul(2654435769)) << 100);
        match index {
            8 | 12 | 16 | 20 => nibbles.push('-'),
            _ => {}
        }
        let value = match index {
            12 => 4,
            16 => 8 | (nibble & 0x3),
            _ => nibble,
        };
        nibbles.push(char::from_digit(value, 16).expect("nibble"));
    }
    nibbles
}

/// The LOCAL fs session's uri — the daemon's lockfile library
/// (`host-discovery`) carries its own copy of this literal; the two
/// must agree.
pub const LOCAL_FS_SESSION: &str = "hihost-fs:/local";

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionId {
    pub host: crate::client::HostId,

    pub session: crate::client::SessionUri,
}

impl SessionId {
    /// Which session OWNS a location: the one whose client routes its
    /// authority, else the local workspace — the address-derived
    /// owner, never an ambient scope. A plain file's state belongs to
    /// the local session, not to nothing.
    pub fn of_location(
        store: &imba::store::Store,
        location: &editor::location::ResourceLocation,
    ) -> SessionId {
        if let Some((host, session)) = crate::client::route(store, location.authority().as_str()) {
            return SessionId { host, session };
        }
        Self::local_default(store)
    }

    pub fn local_default(store: &imba::store::Store) -> SessionId {
        let host = store
            .get::<crate::client::LocalHost>()
            .and_then(|local| local.0)
            .unwrap_or(crate::client::HostId::LOCAL);
        SessionId {
            host,
            session: crate::client::SessionUri::new(LOCAL_FS_SESSION),
        }
    }

    pub fn mint_scratch(store: &mut imba::store::Store) -> SessionId {
        // Monotonic and never reused — the `Id::mint` pattern; a
        // store-held counter bought nothing but a component.
        static MINT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let minted = MINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SessionId {
            host: Self::local_default(store).host,
            session: crate::client::SessionUri::new(format!("scratch-space:{minted}")),
        }
    }

    pub fn names_session(&self) -> bool {
        self.session.as_str() != LOCAL_FS_SESSION
            && !self.session.as_str().starts_with("scratch-space:")
    }
}

/// The shell's catalog-actions road, installed at boot: session
/// channel actions (folders joining/leaving, chats appearing) apply
/// against WINDOWS, so the application lives shell-side; the wire
/// drains hand the batch through here.
#[derive(Clone)]
pub struct ChannelActionsRoad(
    pub  std::sync::Arc<
        dyn Fn(&mut imba::store::Store, &crate::SessionId, Vec<ahp_types::actions::StateAction>)
            + Send
            + Sync,
    >,
);
