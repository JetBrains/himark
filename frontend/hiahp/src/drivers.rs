// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The protocol DRIVERS: himark-side glue that owns every wire
//! coroutine — the subscribe/poll/relaunch chains, the landings, the
//! dispatch asks — and controls the collections through their public
//! mutation doors. Collections are passive models; it is not they
//! who run coroutines. Session/channel/host vocabulary is legal
//! here and nowhere inside a collection.

pub mod changes;
pub mod comments;
pub mod history;
pub mod locations;
