// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Re-export shim — the terminal collection AND its pane live in the
//! `terminals` crate.

pub use terminals::*;

#[cfg(test)]
mod tests;
