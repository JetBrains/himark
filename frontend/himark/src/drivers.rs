// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The drivers moved to the `hiahp` crate with the wire they pump;
//! the shell keeps the path (and the window-threaded rims that land
//! back here during the split).

pub use ::hiahp::drivers::{changes, comments, history, locations};
