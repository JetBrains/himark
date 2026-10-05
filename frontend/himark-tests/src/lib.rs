// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's end-to-end suite lives in `tests/shell.rs` — this lib
//! is the crate's empty anchor. The tests drive the real himark
//! Application through its public face and the `test-support`
//! harness, exactly as an embedder does; nothing here may reach a
//! private seam.
