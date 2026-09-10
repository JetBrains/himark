use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) struct HostTrace {
    pub(crate) tag: String,

    alive: AtomicU64,
    seq: AtomicU64,

    inflight: Mutex<std::collections::HashMap<u64, (u64, u64, String, Instant)>>,
    watchdog: AtomicBool,
}

impl HostTrace {
    pub(crate) fn new() -> Arc<Self> {
        static INSTANCE: AtomicU64 = AtomicU64::new(1);
        let tag = format!("host#{}", INSTANCE.fetch_add(1, Ordering::Relaxed));
        Arc::new(Self {
            tag,
            alive: AtomicU64::new(unix_ms()),
            seq: AtomicU64::new(0),
            inflight: Mutex::new(std::collections::HashMap::new()),
            watchdog: AtomicBool::new(false),
        })
    }

    pub(crate) fn begin(&self, connection: u64, id: u64, method: &str) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = self.inflight.lock() {
            held.insert(seq, (connection, id, method.to_owned(), Instant::now()));
        }
        seq
    }

    pub(crate) fn end(&self, seq: u64) -> Option<Duration> {
        let (.., started) = self.inflight.lock().ok()?.remove(&seq)?;
        Some(started.elapsed())
    }

    fn inflight_brief(&self) -> Vec<String> {
        self.inflight
            .lock()
            .map(|held| {
                held.values()
                    .map(|(connection, id, method, started)| {
                        format!(
                            "{method}#{id}@c{connection} {:.1}s",
                            started.elapsed().as_secs_f32()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn ensure_watchdog(self: &Arc<Self>) {
        if self.watchdog.swap(true, Ordering::Relaxed) {
            return;
        }
        let stamp = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let Some(trace) = Weak::upgrade(&stamp) else {
                    break;
                };
                trace.alive.store(unix_ms(), Ordering::Relaxed);
            }
        });
        let watched: Weak<HostTrace> = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("hihost-watchdog".into())
            .spawn(move || {
                let mut beat = 0u64;
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let Some(trace) = Weak::upgrade(&watched) else {
                        break;
                    };
                    beat += 1;
                    let lag = unix_ms().saturating_sub(trace.alive.load(Ordering::Relaxed));
                    let inflight = trace.inflight_brief();

                    if lag > 3_000 {
                        tracing::error!(
                            target: "ahp_host",
                            host = %trace.tag,
                            lag_ms = lag,
                            ?inflight,
                            "WATCHDOG: RUNTIME DEAF — tasks are not being polled"
                        );
                    } else if !inflight.is_empty() {
                        tracing::info!(
                            target: "ahp_host",
                            host = %trace.tag,
                            beat,
                            ?inflight,
                            "watchdog heartbeat"
                        );
                    } else if beat % 12 == 0 {
                        tracing::debug!(
                            target: "ahp_host",
                            host = %trace.tag,
                            beat,
                            lag_ms = lag,
                            "watchdog heartbeat, idle"
                        );
                    }
                }
            });
    }
}
