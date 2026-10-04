// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The comments WIRE driver: owns the annotations channel (the one
//! feed a family's comments ride), the send-to-agent turn, and
//! every outbound announce — and applies landings through the
//! `Comments` collection's doors in its MIRROR types. The model
//! never dispatches: its mutations leave ANNOUNCE notes beside the
//! card work, and the batch-tail comments lane (`sync`) drains both
//! — the dressing-lane pattern, so a clean collection costs a map
//! read.

use std::sync::Arc;

use crate::hicomments::{
    AnnotationId, Announce, CommentDelta, CommentRecord, CommentSeed, Comments, EntryRecord,
};
use crate::higent::ahp_types::actions::{
    AnnotationsEntrySetAction, AnnotationsRemovedAction, AnnotationsSetAction,
    AnnotationsUpdatedAction, StateAction,
};
use crate::higent::ahp_types::common::StringOrMarkdown;
use crate::higent::ahp_types::state::{
    Annotation, AnnotationEntry, MessageAnnotationsAttachment, MessageAttachment, TextPosition,
    TextRange,
};
use crate::higent::AhpServer;
use crate::higent::SessionUri as Uri;
use crate::{AppCommand, ResourceLocation};
use imba::{effect::AnyEffect, store::Store};

/// The one channel a family's comments ride.
#[derive(Clone)]
struct ChannelWire {
    server: crate::higent::HostId,
    session: Uri,
    seat: Arc<dyn AhpServer>,
    live: bool,
}

#[derive(Clone)]
pub struct CommentsWire {
    comments: imba::store::Id<Comments>,
    uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,
    channel: Option<ChannelWire>,
}

impl CommentsWire {
    pub fn wired(
        comments: imba::store::Id<Comments>,
        uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,
    ) -> Self {
        Self {
            comments,
            uris,
            channel: None,
        }
    }

    pub(crate) fn stamp_uris(
        store: &mut Store,
        wire: imba::store::Id<CommentsWire>,
        uris: &Arc<dyn crate::higent::ResourceUriMap>,
    ) {
        update(store, wire, |row| row.uris = Some(Arc::clone(uris)));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.channel.is_none()
    }
}

fn of(store: &Store, wire: imba::store::Id<CommentsWire>) -> Option<&CommentsWire> {
    store.entity(wire)
}

pub(crate) fn drives(
    store: &Store,
    wire: imba::store::Id<CommentsWire>,
    comments: imba::store::Id<Comments>,
) -> bool {
    of(store, wire).is_some_and(|row| row.comments == comments)
}

/// Mutate in place; a gone driver takes no write.
fn update(
    store: &mut Store,
    wire: imba::store::Id<CommentsWire>,
    mutate: impl FnOnce(&mut CommentsWire),
) {
    let Some(mut row) = store.entity::<CommentsWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// Attach the annotations feed of the wire that serves a folder —
/// the seat and AHP session come off the folder's authority.
pub fn ensure(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<CommentsWire>,
    location: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    if !crate::hicomments::installed(store) {
        return;
    }
    let Some((server, seat, session)) =
        crate::higent::seat::route_seat(store, location.authority().as_str())
    else {
        return;
    };
    let known = of(store, wire)
        .and_then(|row| row.channel.as_ref())
        .is_some_and(|held| held.session == session);
    if known {
        return;
    }
    update(store, wire, |row| {
        row.channel = Some(ChannelWire {
            server,
            session: session.clone(),
            seat: seat.clone(),
            live: false,
        });
    });
    fx.push(
        AnyEffect::new(crate::higent::SubscribeAnnotationsEffect {
            seat,
            session: session.clone(),
        })
        .map(move |result| {
            AppCommand::Dynamic(
                window,
                Arc::new(SnapshotLanded {
                    wire,
                    session: session.clone(),
                    result: result.map(|state| state.annotations),
                }),
            )
        }),
    );
}

fn seed_of(row: &CommentsWire, session: &Uri, annotation: &Annotation) -> Option<CommentSeed> {
    let location = place(row, session, annotation)?;
    Some(CommentSeed {
        id: annotation.id.clone(),
        location,
        range: annotation.range.as_ref().map(from_wire),
        turn_id: annotation.origin.turn_id.clone().unwrap_or_default(),
        resolved: annotation.resolved,
        entries: annotation
            .entries
            .iter()
            .map(|entry| EntryRecord {
                id: entry.id.clone(),
                text: wire_text(&entry.text),
                ours: false, // the model re-marks ours by held entry ids
            })
            .collect(),
    })
}

/// The annotation's document location through the host's uri map.
/// A record the model already holds keeps its location — the model
/// door enforces that; this is the fresh-placement half.
fn place(row: &CommentsWire, session: &Uri, annotation: &Annotation) -> Option<ResourceLocation> {
    let server = row.channel.as_ref()?.server;
    let uris = row.uris.clone()?;
    let authority = crate::higent::seat::route_authority(server, session);
    uris.location_of(
        &crate::higent::ResourceUri::new(annotation.resource.clone()),
        crate::ResourceType::document(),
        &authority,
    )
}

fn digest_deltas(row: &CommentsWire, session: &Uri, actions: &[StateAction]) -> Vec<CommentDelta> {
    actions
        .iter()
        .filter_map(|action| match action {
            StateAction::AnnotationsSet(set) => {
                seed_of(row, session, &set.annotation).map(CommentDelta::Set)
            }
            StateAction::AnnotationsUpdated(updated) => Some(CommentDelta::Updated {
                id: updated.annotation_id.clone(),
                // The action replaces provenance wholesale (AHP 0.9)
                // — adopt its turn, present or not.
                turn_id: updated
                    .origin
                    .as_ref()
                    .map(|origin| origin.turn_id.clone().unwrap_or_default()),
                range: updated.range.as_ref().map(from_wire),
                resolved: updated.resolved,
            }),
            StateAction::AnnotationsRemoved(removed) => Some(CommentDelta::Removed {
                id: removed.annotation_id.clone(),
            }),
            StateAction::AnnotationsEntrySet(set) => Some(CommentDelta::EntrySet {
                id: set.annotation_id.clone(),
                entry_id: set.entry.id.clone(),
                text: wire_text(&set.entry.text),
            }),
            StateAction::AnnotationsEntryRemoved(removed) => Some(CommentDelta::EntryRemoved {
                id: removed.annotation_id.clone(),
                entry_id: removed.entry_id.clone(),
            }),
            _ => None,
        })
        .collect()
}

struct SnapshotLanded {
    wire: imba::store::Id<CommentsWire>,
    session: Uri,
    result: Result<Vec<Annotation>, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "comments.snapshot"
    }
    fn name(&self) -> String {
        "Comments Snapshot".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(row) = of(store, self.wire).cloned() else {
            return;
        };
        let held = row
            .channel
            .as_ref()
            .filter(|held| held.session == self.session)
            .cloned();
        let Some(held) = held else {
            return;
        };
        match &self.result {
            Err(_) => {
                // The subscribe failed: drop the channel — the next
                // ensure re-dials.
                update(store, self.wire, |row| row.channel = None);
            }
            Ok(annotations) => {
                let seeds: Vec<CommentSeed> = annotations
                    .iter()
                    .filter_map(|annotation| seed_of(&row, &self.session, annotation))
                    .collect();
                update(store, self.wire, |row| {
                    if let Some(channel) = &mut row.channel {
                        channel.live = true;
                    }
                });
                Comments::land_seeds(store, row.comments, seeds);
                let _ = held;
                sync(store, self.wire, &app.ui_ctx(), fx);
                relaunch_poll(store, window, self.wire, fx);
            }
        }
    }
}

struct Polled {
    wire: imba::store::Id<CommentsWire>,
    session: Uri,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "comments.polled"
    }
    fn name(&self) -> String {
        "Comments Update".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(row) = of(store, self.wire).cloned() else {
            return;
        };
        if !row
            .channel
            .as_ref()
            .is_some_and(|held| held.session == self.session)
        {
            return;
        }
        let deltas = digest_deltas(&row, &self.session, &self.actions);
        Comments::fold_deltas(store, row.comments, deltas);
        sync(store, self.wire, &app.ui_ctx(), fx);
        relaunch_poll(store, window, self.wire, fx);
    }
}

fn relaunch_poll(
    store: &Store,
    window: crate::WindowId,
    wire: imba::store::Id<CommentsWire>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(held) = of(store, wire).and_then(|row| row.channel.clone()) else {
        return;
    };
    let session = held.session.clone();
    fx.push(
        AnyEffect::new(crate::higent::PollAnnotationsEffect {
            seat: held.seat,
            session: session.clone(),
        })
        .map(move |actions| {
            AppCommand::Dynamic(
                window,
                Arc::new(Polled {
                    wire,
                    session: session.clone(),
                    actions,
                }),
            )
        }),
    );
}

/// The batch-tail comments lane: drain the model's ANNOUNCE notes
/// onto the wire, sweep unsynced records while the feed is live,
/// and run the card work — a clean collection costs a map read.
pub fn sync(
    store: &mut Store,
    wire: imba::store::Id<CommentsWire>,
    ui: &imba::UiCtx,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire).cloned() else {
        return;
    };
    let comments = row.comments;
    if !Comments::owes_sync(store, comments) {
        return;
    }
    // Announces drain ONLY onto a live channel — taken earlier they
    // would be lost; they wait in the model's note until the feed
    // stands.
    if let (Some(held), Some(uris)) = (
        row.channel.as_ref().filter(|held| held.live),
        row.uris.as_ref(),
    ) {
        for announce in Comments::take_announces(store, comments) {
            match announce {
                Announce::Set(id) => {
                    if let Some(record) = Comments::record(store, comments, &id) {
                        dispatch_set(uris, &held.seat, &held.session, &id, &record);
                        Comments::update_record(store, comments, &id, |record| {
                            record.synced = true;
                        });
                    }
                }
                Announce::EntrySet { id, entry_id } => {
                    let entry = Comments::record(store, comments, &id).and_then(|record| {
                        record
                            .entries
                            .iter()
                            .find(|entry| entry.id == entry_id)
                            .cloned()
                    });
                    if let Some(entry) = entry {
                        held.seat.dispatch_annotations(
                            &held.session,
                            StateAction::AnnotationsEntrySet(AnnotationsEntrySetAction {
                                annotation_id: id.clone(),
                                entry: AnnotationEntry {
                                    id: entry.id.clone(),
                                    text: StringOrMarkdown::Markdown {
                                        markdown: wire_string(&entry.text),
                                    },
                                    meta: author_meta("user"),
                                },
                            }),
                        );
                    }
                }
                Announce::Meta(id) => {
                    if let Some(record) = Comments::record(store, comments, &id) {
                        held.seat.dispatch_annotations(
                            &held.session,
                            StateAction::AnnotationsUpdated(AnnotationsUpdatedAction {
                                annotation_id: id.clone(),
                                origin: None,
                                resource: None,
                                range: None,
                                resolved: Some(record.resolved),
                            }),
                        );
                    }
                }
                Announce::Removed(id) => {
                    held.seat.dispatch_annotations(
                        &held.session,
                        StateAction::AnnotationsRemoved(AnnotationsRemovedAction {
                            annotation_id: id,
                        }),
                    );
                }
            }
        }
    }
    let work = Comments::take_card_work(store, comments);
    crate::hicomments::run_card_work(store, ui, comments, work, fx);
}

/// Send a batch of comments to the agent as one turn with an
/// annotations attachment — the chat resolution and the ask are
/// wire business.
pub fn send_to_agent(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<CommentsWire>,
    ids: Vec<AnnotationId>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire).cloned() else {
        return;
    };
    let comments = row.comments;
    let Some(held) = row.channel.clone() else {
        return;
    };
    let group: Vec<AnnotationId> = ids
        .into_iter()
        .filter(|id| {
            Comments::record(store, comments, id)
                .is_some_and(|record| record.synced && !record.sending)
        })
        .collect();
    if group.is_empty() {
        return;
    }
    let session = held.session.clone();
    let key = crate::higent::SessionId {
        host: held.server,
        session: session.clone(),
    };
    let mut chat =
        crate::higent::Agents::channel(store, &key).and_then(|channel| channel.default_chat);
    if chat.is_none() {
        let bound = crate::Windows::window_ref(store, window)
            .map(|entity| entity.current_session())
            .and_then(|workspace| crate::higent::Agents::live_session(store, &workspace))
            .filter(|bound| bound.host == held.server);
        if let Some(bound) = bound {
            chat = crate::higent::Agents::channel(store, &bound)
                .and_then(|channel| channel.default_chat);
        }
    }
    let Some(chat) = chat else {
        eprintln!("[comments] no chat serves {session} — send skipped");
        return;
    };
    let noun = match group.len() {
        1 => "comment",
        _ => "comments",
    };
    let attachment = MessageAttachment::Annotations(MessageAnnotationsAttachment {
        label: format!("{} review {noun}", group.len()),
        range: None,
        display_kind: None,
        meta: None,
        resource: format!("{session}/annotations"),
        annotation_ids: Some(group.clone()),
    });
    for id in &group {
        Comments::update_record(store, comments, id, |record| record.sending = true);
    }
    let sent = group.clone();
    fx.push(
        AnyEffect::new(crate::higent::StartTurnEffect {
            seat: held.seat.clone(),
            chat,
            text: format!("Please address the attached review {noun}."),
            attachments: Some(vec![attachment]),
            model: None,
        })
        .map(move |result| {
            AppCommand::Dynamic(
                window,
                Arc::new(Sent {
                    wire,
                    ids: sent.clone(),
                    result,
                }),
            )
        }),
    );
}

/// The send-to-agent turn answered for a batch of comments.
struct Sent {
    wire: imba::store::Id<CommentsWire>,
    ids: Vec<AnnotationId>,
    result: Result<(), String>,
}

impl crate::DynamicCommand for Sent {
    fn id(&self) -> &'static str {
        "comments.sent"
    }
    fn name(&self) -> String {
        "Comments Sent".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(comments) = of(store, self.wire).map(|row| row.comments) else {
            return;
        };
        Comments::sent_outcome(store, comments, &self.ids, self.result.clone());
        sync(store, self.wire, &app.ui_ctx(), fx);
    }
}

/// The freshest turn stamp of the wire's session — a NEW comment's
/// provenance.
pub(crate) fn latest_turn(store: &Store, wire: imba::store::Id<CommentsWire>) -> String {
    of(store, wire)
        .and_then(|row| row.channel.as_ref())
        .and_then(|held| {
            crate::higent::Agents::latest_turn(
                store,
                &crate::higent::SessionId {
                    host: held.server,
                    session: held.session.clone(),
                },
            )
        })
        .unwrap_or_default()
}

fn dispatch_set(
    uris: &Arc<dyn crate::higent::ResourceUriMap>,
    seat: &Arc<dyn AhpServer>,
    session: &Uri,
    id: &AnnotationId,
    record: &CommentRecord,
) {
    let uri = uris.uri_of(&record.location).into_string();
    seat.dispatch_annotations(
        session,
        StateAction::AnnotationsSet(AnnotationsSetAction {
            annotation: Annotation {
                id: id.clone(),
                origin: crate::higent::ahp_types::state::AnnotationOrigin {
                    session: session.to_string(),
                    chat: None,
                    turn_id: (!record.turn_id.is_empty()).then(|| record.turn_id.clone()),
                },
                resource: uri,
                range: record.range.as_ref().map(to_wire),
                resolved: record.resolved,
                entries: record
                    .entries
                    .iter()
                    .map(|entry| AnnotationEntry {
                        id: entry.id.clone(),
                        text: StringOrMarkdown::Markdown {
                            markdown: wire_string(&entry.text),
                        },
                        meta: author_meta(if entry.ours { "user" } else { "agent" }),
                    })
                    .collect(),
                meta: None,
            },
        }),
    );
}

fn author_meta(author: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut himark = serde_json::Map::new();
    himark.insert(
        "author".to_owned(),
        serde_json::Value::String(author.to_owned()),
    );
    let mut meta = serde_json::Map::new();
    meta.insert("himark".to_owned(), serde_json::Value::Object(himark));
    Some(meta)
}

fn wire_text(text: &StringOrMarkdown) -> crate::Text {
    crate::Text::from_string_exact(match text {
        StringOrMarkdown::Plain(text) => text,
        StringOrMarkdown::Markdown { markdown } => markdown,
    })
}

fn wire_string(text: &crate::Text) -> String {
    let mut view = text.view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

fn to_wire(range: &std::ops::Range<crate::LineCol>) -> TextRange {
    TextRange {
        start: TextPosition {
            line: range.start.line as i64,
            character: range.start.col as i64,
        },
        end: TextPosition {
            line: range.end.line as i64,
            character: range.end.col as i64,
        },
    }
}

fn from_wire(range: &TextRange) -> std::ops::Range<crate::LineCol> {
    let position = |position: &TextPosition| crate::LineCol {
        line: position.line.max(0) as u32,
        col: position.character.max(0) as u32,
    };
    position(&range.start)..position(&range.end)
}
