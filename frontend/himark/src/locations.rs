// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's door to the `locations` crate: the collection, the
//! forests, the washes and the feed disposal all live there — the
//! stream pump is `drivers::locations`, the dock/peek window glue
//! `hisearch`/`hipeek`.

use ::locations::views::{
    dispose_feed, files_forest, locations_forest, LocationsWashHook, WashDocument,
};
use ::locations::{
    open_feed, FeedId, FoundLocation, LocationKey, LocationLists, LocationsAsk, LocationsFeedRow,
    LspKind,
};
