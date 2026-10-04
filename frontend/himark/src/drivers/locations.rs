// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The locations WIRE driver: owns every result stream's channel end
//! and its pump — the ask's landing, the subscribe → poll loop, the
//! stop and dispose teardowns — and folds RESOLVED batches through
//! the `LocationLists` collection's doors. The model holds results,
//! the dock front and the washes; seats, channels and poll tokens
//! live here, in the family's wire row.

use std::sync::Arc;

use imba::{effect::AnyEffect, store::Store};

use crate::locations::{FeedId, FoundLocation, LocationLists};
use crate::AppCommand;

/// One live stream: the channel end and the standing poll.
#[derive(Clone)]
struct FeedWire {
    channel: crate::LocationsChannel,
    poll: Option<imba::effect::CancellationToken>,
}

#[derive(Clone)]
pub struct LocationsWire {
    lists: imba::store::Id<LocationLists>,
    feeds: rpds::HashTrieMapSync<FeedId, FeedWire>,
}

impl LocationsWire {
    pub fn wired(lists: imba::store::Id<LocationLists>) -> Self {
        Self {
            lists,
            feeds: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.feeds.is_empty()
    }
}

fn of(store: &Store, wire: imba::store::Id<LocationsWire>) -> Option<&LocationsWire> {
    store.entity(wire)
}

/// Mutate in place; a gone driver takes no write.
fn update(
    store: &mut Store,
    wire: imba::store::Id<LocationsWire>,
    mutate: impl FnOnce(&mut LocationsWire),
) {
    let Some(mut row) = store.entity::<LocationsWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// Take a feed's wire out of the row — the teardown half of stop and
/// dispose: whoever takes it owns the cancel and the unsubscribe.
fn take_feed(
    store: &mut Store,
    wire: imba::store::Id<LocationsWire>,
    feed: FeedId,
) -> Option<FeedWire> {
    let held = of(store, wire)?.feeds.get(&feed).cloned();
    if held.is_some() {
        update(store, wire, |row| {
            row.feeds.remove_mut(&feed);
        });
    }
    held
}

/// Resolve a wire batch against its channel's route — locations
/// whose URI the route cannot place are dropped.
fn resolve_batch(
    channel: &crate::LocationsChannel,
    batch: himark_ahp_ext_types::LocationList,
) -> Vec<FoundLocation> {
    batch
        .locations
        .into_iter()
        .filter_map(|location| {
            Some(FoundLocation {
                location: (channel.resolve)(&location.uri)?,
                line: location.line,
                column: location.column,
                length: location.length,
                context: location.context,
                context_column_start: location.context_column_start,
            })
        })
        .collect()
}

/// Phase two of every ask: the channel landed — subscribe and start
/// the feed's own pump, view-independent. Pushed through
/// `AppRequests` by whichever surface asked. A failed ask resolves
/// the row cut-off in plain sight instead of a silent no-op.
pub struct AttachFeedStream {
    pub wire: imba::store::Id<LocationsWire>,
    pub feed: FeedId,
    pub outcome: Result<crate::LocationsChannel, String>,
}

impl crate::DynamicCommand for AttachFeedStream {
    fn id(&self) -> &'static str {
        "locations.attach-stream"
    }

    fn name(&self) -> String {
        "Attach Location Stream".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(lists) = of(store, self.wire).map(|row| row.lists) else {
            return;
        };
        if LocationLists::row_ref(store, lists, self.feed).is_none() {
            return; // disposed before the ask answered
        }
        match self.outcome.clone() {
            Err(_) => LocationLists::mark_cut(store, lists, self.feed),
            Ok(channel) => {
                update(store, self.wire, |row| {
                    row.feeds.insert_mut(
                        self.feed,
                        FeedWire {
                            channel: channel.clone(),
                            poll: None,
                        },
                    );
                });
                let (wire, feed) = (self.wire, self.feed);
                let _ = fx.push(
                    AnyEffect::new(crate::higent::SubscribeLocationsEffect {
                        seat: channel.seat,
                        channel: channel.channel,
                    })
                    .map(move |outcome| {
                        AppCommand::Landing(
                            window,
                            Box::new(FeedBatch {
                                wire,
                                feed,
                                batches: outcome.map(|snapshot| vec![snapshot]),
                            }),
                        )
                    }),
                );
            }
        }
    }
}

/// The pump's landing: resolve, fold through the model's door, then
/// poll again while the stream runs.
struct FeedBatch {
    wire: imba::store::Id<LocationsWire>,
    feed: FeedId,
    batches: Result<Vec<himark_ahp_ext_types::LocationList>, String>,
}

impl crate::LandingCommand for FeedBatch {
    fn perform(
        self: Box<Self>,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(row) = of(store, self.wire) else {
            return;
        };
        let lists = row.lists;
        let Some(held) = row.feeds.get(&self.feed).cloned() else {
            return; // stopped or disposed while in flight
        };
        match self.batches {
            Err(_) => LocationLists::mark_cut(store, lists, self.feed),
            Ok(batches) => {
                let done = batches.iter().any(|batch| batch.done);
                let truncated = batches.iter().any(|batch| batch.truncated);
                let locations: Vec<FoundLocation> = batches
                    .into_iter()
                    .flat_map(|batch| resolve_batch(&held.channel, batch))
                    .collect();
                LocationLists::fold_locations(store, lists, self.feed, locations, done, truncated);
            }
        }
        let running =
            LocationLists::row_ref(store, lists, self.feed).is_some_and(|row| !row.done);
        let (wire, feed) = (self.wire, self.feed);
        let poll = running.then(|| {
            fx.push(
                AnyEffect::new(crate::higent::PollLocationsEffect {
                    seat: Arc::clone(&held.channel.seat),
                    channel: held.channel.channel.clone(),
                })
                .map(move |batches| {
                    AppCommand::Landing(
                        window,
                        Box::new(FeedBatch {
                            wire,
                            feed,
                            batches: Ok(batches),
                        }),
                    )
                }),
            )
        });
        update(store, self.wire, |row| {
            if let Some(mut held) = row.feeds.get(&feed).cloned() {
                held.poll = poll;
                row.feeds.insert_mut(feed, held);
            }
        });
    }
}

/// Stop a feed's stream, keeping what landed: cancel the pump,
/// unsubscribe (the host-side cancel), resolve the row cut-off.
pub struct StopFeed {
    pub wire: imba::store::Id<LocationsWire>,
    pub feed: FeedId,
}

impl crate::DynamicCommand for StopFeed {
    fn id(&self) -> &'static str {
        "locations.stop-feed"
    }

    fn name(&self) -> String {
        "Stop Location Stream".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(lists) = of(store, self.wire).map(|row| row.lists) else {
            return;
        };
        if let Some(held) = take_feed(store, self.wire, self.feed) {
            if let Some(token) = held.poll {
                fx.cancel(token);
            }
            unsubscribe(window, held.channel, fx);
        }
        LocationLists::mark_cut(store, lists, self.feed);
    }
}

/// Dispose a feed: cancel its pump, unsubscribe its channel (the
/// host-side cancel), then let the model drop the row and its
/// washes. Pushed through `AppRequests`.
pub struct DisposeFeed {
    pub wire: imba::store::Id<LocationsWire>,
    pub feed: FeedId,
}

impl crate::DynamicCommand for DisposeFeed {
    fn id(&self) -> &'static str {
        "locations.dispose-feed"
    }

    fn name(&self) -> String {
        "Dispose Location Feed".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(lists) = of(store, self.wire).map(|row| row.lists) else {
            return;
        };
        if let Some(held) = take_feed(store, self.wire, self.feed) {
            if let Some(token) = held.poll {
                fx.cancel(token);
            }
            unsubscribe(window, held.channel, fx);
        }
        crate::locations::dispose_feed(store, &app.ui_ctx(), lists, self.feed, fx);
    }
}

fn unsubscribe(
    window: crate::WindowId,
    channel: crate::LocationsChannel,
    fx: &mut crate::app::AppFx<'_>,
) {
    let _ = fx.push(
        AnyEffect::new(crate::higent::UnsubscribeLocationsEffect {
            seat: channel.seat,
            channel: channel.channel,
        })
        .map(move |()| AppCommand::Landing(window, Box::new(NothingLanded))),
    );
}

struct NothingLanded;

impl crate::LandingCommand for NothingLanded {
    fn perform(
        self: Box<Self>,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
    }
}
