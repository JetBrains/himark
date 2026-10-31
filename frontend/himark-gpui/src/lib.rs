// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Himark's BRIDGES, adapted to gpui — the himark-macOS design with
//! gpui where AppKit stood. The public api is the same one the
//! Swift shell drives: an engine booted once per process, windows
//! minted from it, and each bridge mounted as a view
//! ([`BridgeView`] here, `HimarkBridgeView` there). The workbench
//! window is the degenerate case: one root bridge whose mounted
//! view is the whole composition.
//!
//! Being Rust, the adapter calls Himark directly — real
//! `imba::event::Event`s, `WindowId`s, the clipboard client — and
//! keeps the engine roads that are machinery, not marshaling: the
//! boot, the effect worker, the wake, the scroll coalescer.
//! Everything gpui-facing (painting, input translation, variable
//! font instancing) comes from `imba-gpui`.

mod bridge_view;
mod engine;

pub use bridge_view::BridgeView;
pub use engine::{Bridge, Engine, Window};

/// The embedding app's gpui — re-exported so a shell binary needs
/// only this crate.
pub use gpui;

/// Start the agent host when none stands — the same autostart every
/// shell performs before its first frame.
pub fn agent_host_autostart() -> bool {
    himark_api::agent_host_autostart()
}
