// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's app-facing re-exports — the stable paths shells and
//! features import. The workbench composition (boot, windows,
//! panes) lives in the `workbench-shell` crate.

use std::sync::OnceLock;

#[cfg(any(test, feature = "test-support"))]
pub use crate::himark::StoreMut;
pub use crate::himark::{AppFonts, Himark, InstalledShell, NoShell, Shell};
pub(crate) use crate::perform::{command_label, AppEffects};
pub use crate::perform::{shell_verb, AppCommand, AppFx};

pub(crate) fn trace_reconcile(source: &str, commands: &[AppCommand]) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RECONCILE").is_some()) {
        return;
    }
    let labels: Vec<_> = commands.iter().map(command_label).collect();
    eprintln!("[reconcile] {source} reported: {}", labels.join(", "));
}

pub(crate) fn trace_stalls_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_STALLS").is_some())
}
