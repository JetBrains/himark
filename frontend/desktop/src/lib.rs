// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod connector;
#[cfg(unix)]
pub mod unix_transport;
#[cfg(unix)]
use std::time::{Duration, Instant};

use himark::app::AppFonts;

pub fn app_fonts() -> AppFonts {
    let started = Instant::now();
    let fonts = AppFonts::platform();
    startup_profile_log("fonts.app_fonts", started.elapsed());
    fonts
}

pub fn is_insertable_text(text: &str) -> bool {
    text.chars()
        .all(|ch| ch == '\t' || ch == '\n' || !ch.is_control())
}

pub fn startup_profile_log(label: &str, elapsed: Duration) {
    if std::env::var_os("HIMARK_PROFILE_STARTUP").is_some() {
        eprintln!(
            "[startup] {label}: {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}
