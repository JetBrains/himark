// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! File watching over `jetbrains-fswatcher`.
//!
//! The process holds ONE `Watcher`: it owns the OS resources (on
//! macOS a single FSEvents stream), and every watch is a subscription
//! on it. A watch costs a parked task — no thread, no polling.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use futures_util::StreamExt;
use jetbrains_fswatcher::{Watcher, WatcherApi};

pub(crate) use jetbrains_fswatcher::Event;

/// What a watch reports.
#[derive(Clone, Copy)]
pub(crate) enum Scope {
    /// The entry itself and, for a directory, its direct children.
    Entry,
    /// One regular file, including writes made while the writer still
    /// holds it open.
    OpenFile,
}

/// A live watch. Dropping it ends the subscription.
pub(crate) struct Watch(tokio::task::AbortHandle);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn watcher() -> Result<Arc<Watcher>, String> {
    static WATCHER: OnceLock<Result<Arc<Watcher>, String>> = OnceLock::new();
    WATCHER
        .get_or_init(|| {
            Watcher::create_default()
                .map(Arc::new)
                .map_err(|error| format!("the file watcher failed to start: {error:#}"))
        })
        .clone()
}

/// Watch `path`, calling `on_event` for every event until the returned
/// `Watch` drops. The path need not exist yet. Events arrive in order;
/// `on_event` runs on the blocking pool, so it may read files and take
/// locks.
///
/// Must be called on the host's tokio runtime.
pub(crate) fn watch(
    path: &Path,
    scope: Scope,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Watch, String> {
    if !path.is_absolute() {
        return Err(format!("not an absolute path: {}", path.display()));
    }
    let runtime = tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?;
    let watcher = watcher()?;
    // The subscription registers here, before this function returns:
    // no change after this point is missed.
    let mut events = match scope {
        Scope::Entry => watcher.watch_one(path),
        Scope::OpenFile => watcher.watch_immediate(path),
    };
    let on_event = Arc::new(on_event);
    let task = runtime.spawn(async move {
        while let Some(event) = events.next().await {
            let on_event = Arc::clone(&on_event);
            let _ = tokio::task::spawn_blocking(move || on_event(event)).await;
        }
    });
    Ok(Watch(task.abort_handle()))
}
