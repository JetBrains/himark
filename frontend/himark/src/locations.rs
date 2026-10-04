// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's door to the `locations` crate: the collection, the
//! forests, the washes and the feed disposal all live there — the
//! stream pump is `drivers::locations`, the dock/peek window glue
//! `hisearch`/`hipeek`.

pub use ::locations::{
    dispose_feed, files_forest, locations_forest, open_feed, FeedId, FoundLocation, LocationKey,
    LocationLists, LocationsAsk, LocationsFeedRow, LocationsWashHook, LspKind, WashDocument,
};
