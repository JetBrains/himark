// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use editor::location::ResourceLocation;

use crate::client::{Client, HostId, ResourceClient, WatchHandle};

const SUBSCRIPTION_BASE: u64 = 1 << 48;

pub struct ClientDirectory {
    clients: Mutex<HashMap<HostId, Client>>,

    local: Mutex<Option<HostId>>,
    watches: Mutex<HashMap<u64, (Arc<dyn ResourceClient>, WatchHandle)>>,
    next: AtomicU64,

    deliver: Arc<dyn Fn(u64) + Send + Sync>,
}

impl ClientDirectory {
    pub fn new(deliver: Arc<dyn Fn(u64) + Send + Sync>) -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            local: Mutex::new(None),
            watches: Mutex::new(HashMap::new()),
            next: AtomicU64::new(SUBSCRIPTION_BASE),
            deliver,
        }
    }

    pub fn set_local(&self, server: HostId) {
        *self.local.lock().expect("client directory") = Some(server);
    }

    pub(crate) fn local_client(&self) -> Option<(Client, crate::client::SessionUri)> {
        let server = (*self.local.lock().expect("client directory"))?;
        let client = self.client(server)?;
        Some((
            client,
            crate::client::SessionUri::new(crate::LOCAL_FS_SESSION),
        ))
    }

    pub fn record(&self, server: HostId, client: Client) {
        self.clients
            .lock()
            .expect("client directory")
            .insert(server, client);
    }

    pub fn client(&self, server: HostId) -> Option<Client> {
        self.clients
            .lock()
            .expect("client directory")
            .get(&server)
            .cloned()
    }

    pub fn adopt_watch(&self, client: Arc<dyn ResourceClient>, handle: WatchHandle) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.watches
            .lock()
            .expect("watch table")
            .insert(id, (client, handle));
        id
    }

    pub fn release_watch(
        &self,
        subscription: u64,
    ) -> Option<(Arc<dyn ResourceClient>, WatchHandle)> {
        self.watches
            .lock()
            .expect("watch table")
            .remove(&subscription)
    }

    pub fn deliver(&self) -> Arc<dyn Fn(u64) + Send + Sync> {
        Arc::clone(&self.deliver)
    }
}

const LOCAL_AUTHORITY: &str = "local";

pub fn client_of_authority(
    directory: &ClientDirectory,
    authority: &str,
) -> Option<(crate::client::Client, crate::client::SessionUri)> {
    if crate::client::scoped(authority) {
        let (server, session) = crate::client::parse(authority)?;
        let client = directory.client(server)?;
        return Some((client, session));
    }
    if authority == LOCAL_AUTHORITY {
        return directory.local_client();
    }
    None
}

pub fn client_of(
    directory: &ClientDirectory,
    location: &ResourceLocation,
) -> Option<(crate::client::Client, crate::client::SessionUri)> {
    client_of_authority(directory, location.authority().as_str())
}

pub fn served(location: &ResourceLocation) -> bool {
    let authority = location.authority().as_str();
    crate::client::scoped(authority) || authority == LOCAL_AUTHORITY
}
