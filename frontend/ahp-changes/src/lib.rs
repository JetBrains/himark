// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The changes + history drivers: the wire coroutines over the
//! changeset and history channels, driving the `changesview` models
//! through their public mutation doors. The two share a crate the way
//! their models do — the folders catalog, digestion, and the commit
//! dispatches are one fabric.

pub mod changes;
pub mod history;
