// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Himark's BRIDGES, adapted to gpui — a PURE adapter. The app
//! layer composes the engine (bare + plugins, or the full workbench
//! shell) and hands it to [`Engine::adopt`]; from then on every
//! minted `ViewBridgeId` mounts as a [`BridgeView`]: input lands as
//! real `imba::event::Event`s through the engine's bridge dispatch,
//! frames go out through `gfx_gpui::GpuiPainter`, IME and clipboard
//! ride the engine's typed seats. No window vocabulary, no C types,
//! no composition — those belong to the apps.

mod bridge_view;
mod engine;

pub use bridge_view::BridgeView;
pub use engine::Engine;

/// The embedding app's gpui — re-exported so a shell binary needs
/// only this crate for the view side.
pub use gpui;
