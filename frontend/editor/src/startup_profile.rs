use std::{sync::OnceLock, time::Instant};

pub(crate) type Start = Option<Instant>;

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_PROFILE_STARTUP").is_some())
}

pub(crate) fn start() -> Start {
    enabled().then(Instant::now)
}

pub(crate) fn log(label: &str, start: Start) {
    if let Some(start) = start {
        let elapsed = start.elapsed();
        eprintln!(
            "[startup] {label}: {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}
