use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use himark::higent::{AhpServer, HostId, WatchHandle};

const SUBSCRIPTION_BASE: u64 = 1 << 48;

pub struct SeatDirectory {
    seats: Mutex<HashMap<HostId, Arc<dyn AhpServer>>>,

    local: Mutex<Option<HostId>>,
    watches: Mutex<HashMap<u64, (Arc<dyn AhpServer>, WatchHandle)>>,
    next: AtomicU64,

    deliver: Arc<dyn Fn(u64) + Send + Sync>,
}

impl SeatDirectory {
    pub fn new(deliver: Arc<dyn Fn(u64) + Send + Sync>) -> Self {
        Self {
            seats: Mutex::new(HashMap::new()),
            local: Mutex::new(None),
            watches: Mutex::new(HashMap::new()),
            next: AtomicU64::new(SUBSCRIPTION_BASE),
            deliver,
        }
    }

    pub fn set_local(&self, server: HostId) {
        *self.local.lock().expect("seat directory") = Some(server);
    }

    pub fn local_seat(&self) -> Option<(Arc<dyn AhpServer>, String)> {
        let server = (*self.local.lock().expect("seat directory"))?;
        let seat = self.seat(server)?;
        Some((seat, host_discovery::LOCAL_FS_SESSION.to_owned()))
    }

    pub fn record(&self, server: HostId, seat: Arc<dyn AhpServer>) {
        self.seats
            .lock()
            .expect("seat directory")
            .insert(server, seat);
    }

    pub fn seat(&self, server: HostId) -> Option<Arc<dyn AhpServer>> {
        self.seats
            .lock()
            .expect("seat directory")
            .get(&server)
            .cloned()
    }

    pub fn adopt_watch(&self, seat: Arc<dyn AhpServer>, handle: WatchHandle) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.watches
            .lock()
            .expect("watch table")
            .insert(id, (seat, handle));
        id
    }

    pub fn release_watch(&self, subscription: u64) -> Option<(Arc<dyn AhpServer>, WatchHandle)> {
        self.watches
            .lock()
            .expect("watch table")
            .remove(&subscription)
    }

    pub fn deliver(&self) -> Arc<dyn Fn(u64) + Send + Sync> {
        Arc::clone(&self.deliver)
    }
}
