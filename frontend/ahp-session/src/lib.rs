// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The session catalog: hosts and their session states, the mint/
//! dispose ceremony that wires every collection and driver row, the
//! fs-effect routes' watch border — the one crate that knows the
//! whole protocol family, sitting on top of the domain crates.

pub mod fsroute;
pub mod session;
