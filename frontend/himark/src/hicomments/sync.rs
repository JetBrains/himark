// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;
use std::sync::Arc;

use crate::higent::ahp_types::actions::{
    AnnotationsEntrySetAction, AnnotationsRemovedAction, AnnotationsSetAction,
    AnnotationsUpdatedAction, StateAction,
};
use crate::higent::ahp_types::common::StringOrMarkdown;
use crate::higent::ahp_types::state::{
    Annotation, AnnotationEntry, AnnotationsState, MessageAnnotationsAttachment, MessageAttachment,
    TextPosition, TextRange,
};
use crate::{AppCommand, AppFx, DocumentId, InlayKey, LineCol, ResourceLocation, WindowId};
use imba::effect::{AnyEffect, Effects};
use imba::store::Store;

use crate::hicomments::{comments_markup, CommentView};

use crate::higent::SessionUri as Uri;

pub type AnnotationId = String;

#[derive(Clone)]
pub struct EntryRecord {
    pub id: String,

    pub text: crate::Text,

    pub ours: bool,
}

#[derive(Clone)]
pub struct CommentRecord {
    pub server: crate::higent::HostId,
    pub session: Uri,
    pub location: ResourceLocation,

    pub range: Option<Range<LineCol>>,
    pub turn_id: String,
    pub resolved: bool,
    pub entries: rpds::VectorSync<EntryRecord>,

    pub thread_stamp: u64,

    pub synced: bool,

    pub sending: bool,
}

impl CommentRecord {
    pub fn own_entry(&self) -> Option<&EntryRecord> {
        self.entries.iter().find(|entry| entry.ours)
    }

    pub fn foreign_texts(&self) -> Vec<crate::Text> {
        self.entries
            .iter()
            .filter(|entry| !entry.ours)
            .map(|entry| entry.text.clone())
            .collect()
    }
}

#[derive(Clone)]
struct ChannelFeed {
    session: Uri,
    server: crate::higent::HostId,
    seat: Arc<dyn crate::higent::AhpServer>,
    live: bool,
}

#[derive(Clone)]
pub struct Comments {
    /// The documents the cards live in — wired at the family mint
    /// (docs/entities.md law 4).
    documents: imba::store::Id<crate::OpenDocuments>,

    records: rpds::HashTrieMapSync<AnnotationId, CommentRecord>,

    cards: rpds::HashTrieMapSync<AnnotationId, (DocumentId, InlayKey)>,

    channel: Option<ChannelFeed>,

    generation: u64,

    minted: u64,
}

#[derive(Clone, Default)]
pub struct CommentsInstall;

/// What the collection answers to behind its `At` address
/// (docs/entities.md law 5): the annotations feed's landings and the
/// send-turn's answer, each stamped with the collection id at launch.
#[derive(Clone)]
pub enum CommentsCommand {
    /// The annotations subscribe answered for the feed's session.
    Snapshot {
        session: Uri,
        result: Result<AnnotationsState, String>,
    },
    /// The annotations poll drained for the feed's session.
    Polled {
        session: Uri,
        actions: Vec<StateAction>,
    },
    /// The send-to-agent turn answered for a batch of comments.
    Sent {
        ids: Vec<AnnotationId>,
        result: Result<(), String>,
    },
}

impl std::fmt::Display for CommentsCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentsCommand::Snapshot { .. } => out.write_str("comments snapshot"),
            CommentsCommand::Polled { .. } => out.write_str("comments polled"),
            CommentsCommand::Sent { .. } => out.write_str("comments sent"),
        }
    }
}

/// A note the collection leaves for the application road after a
/// landing (the `BaseRearms` shape): cards whose records died drop
/// their inlays, and the surviving records settle into cards. The
/// entity holds no document-addressed effects of its own; the
/// `AtComments` arm consumes this with the ones it has.
#[derive(Clone, Default)]
pub struct CardWork {
    pub dead: Vec<(DocumentId, InlayKey)>,
    pub settle: bool,
}

impl imba::store::Entity for Comments {
    type Command = CommentsCommand;

    fn perform(
        &mut self,
        _id: imba::store::Id<Self>,
        command: CommentsCommand,
        store: &mut Store,
        _ui: &imba::UiCtx,
        fx: &mut Effects<'_, CommentsCommand>,
    ) {
        match command {
            CommentsCommand::Snapshot { session, result } => {
                let Some(feed) = self.channel_for(&session).cloned() else {
                    return;
                };
                let state = match result {
                    Ok(state) => state,
                    Err(_) => {
                        self.channel = None;
                        return;
                    }
                };
                let placed: Vec<(Annotation, ResourceLocation)> = state
                    .annotations
                    .iter()
                    .filter_map(|annotation| {
                        self.place(store, &session, annotation)
                            .map(|location| (annotation.clone(), location))
                    })
                    .collect();
                let mut live = feed.clone();
                live.live = true;
                self.channel = Some(live);
                for (annotation, location) in &placed {
                    fold_set(self, feed.server, &session, annotation, location);
                }
                self.generation += 1;
                self.note_card_work(store, Vec::new());
                self.relaunch_poll(&session, fx);
            }
            CommentsCommand::Polled { session, actions } => {
                let Some(feed) = self.channel_for(&session).cloned() else {
                    return;
                };
                let placed: std::collections::HashMap<AnnotationId, ResourceLocation> = actions
                    .iter()
                    .filter_map(|action| match action {
                        StateAction::AnnotationsSet(set) => self
                            .place(store, &session, &set.annotation)
                            .map(|location| (set.annotation.id.clone(), location)),
                        _ => None,
                    })
                    .collect();
                let dead = self.fold_polled(feed.server, &session, &actions, &placed);
                self.generation += 1;
                self.note_card_work(store, dead);
                self.relaunch_poll(&session, fx);
            }
            CommentsCommand::Sent { ids, result } => {
                if let Err(error) = &result {
                    eprintln!("[comments] send failed, comments kept: {error}");
                    for id in &ids {
                        let Some(mut record) = self.records.get(id).cloned() else {
                            continue;
                        };
                        record.sending = false;
                        self.records.insert_mut(id.clone(), record);
                    }
                    return;
                }
                let mut dead = Vec::new();
                for id in &ids {
                    if let Some(card) = self.cards.get(id) {
                        dead.push(*card);
                    }
                    self.remove_in_place(id);
                }
                store.update::<CardWork>(|work| work.dead.extend(dead));
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {
        // Records and cards are the collection's PRIVATE schema —
        // nothing to retract; the feed dies with the drop.
    }
}

impl crate::AppEntity for Comments {
    /// The landing's note: cards whose records died drop their inlays
    /// and the records settle into cards — document-addressed effects
    /// the entity itself does not hold.
    fn after_route(
        store: &mut Store,
        ui: &imba::UiCtx,
        id: imba::store::Id<Self>,
        fx: &mut crate::AppFx<'_>,
    ) {
        if let Some(work) = store.take::<CardWork>() {
            run_card_work(store, ui, id, work, fx);
        }
    }
}

impl Comments {
    pub fn install(store: &mut Store) {
        store.put(CommentsInstall);
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<CommentsInstall>().is_some()
    }

    fn channel_for(&self, session: &Uri) -> Option<&ChannelFeed> {
        self.channel
            .as_ref()
            .filter(|feed| feed.session == *session)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.cards.is_empty() && self.channel.is_none()
    }

    /// A collection wired to the documents its cards live in — minted
    /// by the family ceremony.
    pub fn wired(documents: imba::store::Id<crate::OpenDocuments>) -> Self {
        Self {
            documents,
            records: rpds::HashTrieMapSync::new_sync(),
            cards: rpds::HashTrieMapSync::new_sync(),
            channel: None,
            generation: 0,
            minted: 0,
        }
    }

    pub fn documents(&self) -> imba::store::Id<crate::OpenDocuments> {
        self.documents
    }

    /// The collection by its id — `None` is gone.
    fn of(store: &Store, comments: imba::store::Id<Comments>) -> Option<&Comments> {
        store.entity(comments)
    }

    /// The documents collection this one's cards live in.
    pub(crate) fn documents_of(
        store: &Store,
        comments: imba::store::Id<Comments>,
    ) -> Option<imba::store::Id<crate::OpenDocuments>> {
        Some(Self::of(store, comments)?.documents)
    }

    /// Mutate in place; a gone collection takes no write.
    fn update(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        mutate: impl FnOnce(&mut Comments),
    ) {
        let Some(mut row) = store.entity(comments).cloned() else {
            return;
        };
        mutate(&mut row);
        store.put_entity(comments, row);
    }

    pub fn generation(store: &Store, comments: imba::store::Id<Comments>) -> u64 {
        Self::of(store, comments)
            .map(|comments| comments.generation)
            .unwrap_or(0)
    }

    pub fn record(
        store: &Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
    ) -> Option<CommentRecord> {
        Self::of(store, comments)?.records.get(id).cloned()
    }

    pub fn records(
        store: &Store,
        comments: imba::store::Id<Comments>,
    ) -> Vec<(AnnotationId, CommentRecord)> {
        Self::of(store, comments)
            .map(|comments| {
                comments
                    .records
                    .iter()
                    .map(|(id, record)| (id.clone(), record.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn card(
        store: &Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
    ) -> Option<(DocumentId, InlayKey)> {
        Self::of(store, comments)?.cards.get(id).copied()
    }

    fn update_record(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
        change: impl FnOnce(&mut CommentRecord),
    ) {
        Self::update(store, comments, |comments| {
            let Some(mut record) = comments.records.get(id).cloned() else {
                return;
            };
            change(&mut record);
            comments.records.insert_mut(id.clone(), record);
            comments.generation += 1;
        });
    }

    fn mint(store: &mut Store, comments: imba::store::Id<Comments>) -> AnnotationId {
        let mut id = String::new();
        Self::update(store, comments, |comments| {
            comments.minted += 1;
            id = format!("hc-{}-{}", std::process::id(), comments.minted);
        });
        id
    }

    /// The annotation's document location through the host's uri
    /// map — an existing record keeps the location it had.
    fn place(
        &self,
        store: &Store,
        session: &Uri,
        annotation: &Annotation,
    ) -> Option<ResourceLocation> {
        if let Some(held) = self.records.get(&annotation.id) {
            return Some(held.location.clone());
        }
        let server = self.channel_for(session)?.server;
        let uris = crate::higent::Hosts::uris(store, server)?;
        let authority = crate::higent::seat::route_authority(server, session);
        uris.location_of(
            &crate::higent::ResourceUri::new(annotation.resource.clone()),
            crate::ResourceType::document(),
            &authority,
        )
    }

    /// Fold the feed's streamed actions; cards whose records died come
    /// back for the application road to drop.
    fn fold_polled(
        &mut self,
        server: crate::higent::HostId,
        session: &Uri,
        actions: &[StateAction],
        placed: &std::collections::HashMap<AnnotationId, ResourceLocation>,
    ) -> Vec<(DocumentId, InlayKey)> {
        let mut dead_cards: Vec<(DocumentId, InlayKey)> = Vec::new();
        for action in actions {
            match action {
                StateAction::AnnotationsSet(set) => {
                    let Some(location) = placed.get(&set.annotation.id) else {
                        continue;
                    };
                    fold_set(self, server, session, &set.annotation, location);
                }
                StateAction::AnnotationsUpdated(updated) => {
                    let Some(mut record) = self.records.get(&updated.annotation_id).cloned() else {
                        continue;
                    };
                    if let Some(origin) = &updated.origin {
                        // The action replaces provenance wholesale
                        // (AHP 0.9) — adopt its turn, present or not.
                        record.turn_id = origin.turn_id.clone().unwrap_or_default();
                    }
                    if let Some(range) = &updated.range {
                        record.range = Some(from_wire(range));
                    }
                    if let Some(resolved) = updated.resolved {
                        record.resolved = resolved;
                    }
                    self.records
                        .insert_mut(updated.annotation_id.clone(), record);
                }
                StateAction::AnnotationsRemoved(removed) => {
                    if let Some(card) = self.cards.get(&removed.annotation_id) {
                        dead_cards.push(*card);
                    }
                    self.records.remove_mut(&removed.annotation_id);
                    self.cards.remove_mut(&removed.annotation_id);
                }
                StateAction::AnnotationsEntrySet(set) => {
                    let Some(mut record) = self.records.get(&set.annotation_id).cloned() else {
                        continue;
                    };
                    let text = wire_text(&set.entry.text);
                    let mut entries: Vec<EntryRecord> = record.entries.iter().cloned().collect();
                    let mut foreign_touch = true;
                    match entries.iter_mut().find(|held| held.id == set.entry.id) {
                        Some(held) => {
                            foreign_touch = !held.ours;
                            held.text = text;
                        }
                        None => entries.push(EntryRecord {
                            id: set.entry.id.clone(),
                            text,
                            ours: false,
                        }),
                    }
                    record.entries = entries.into_iter().collect();
                    record.thread_stamp += u64::from(foreign_touch);
                    self.records.insert_mut(set.annotation_id.clone(), record);
                }
                StateAction::AnnotationsEntryRemoved(removed) => {
                    let Some(mut record) = self.records.get(&removed.annotation_id).cloned() else {
                        continue;
                    };
                    let foreign_touch = record
                        .entries
                        .iter()
                        .any(|held| held.id == removed.entry_id && !held.ours);
                    record.entries = record
                        .entries
                        .iter()
                        .filter(|held| held.id != removed.entry_id)
                        .cloned()
                        .collect();
                    record.thread_stamp += u64::from(foreign_touch);
                    self.records
                        .insert_mut(removed.annotation_id.clone(), record);
                }
                _ => {}
            }
        }
        dead_cards
    }

    /// The landing's own poll relaunch — the next batch of the feed's
    /// annotations channel comes home as `Polled`, stamped by the
    /// router.
    fn relaunch_poll(&self, session: &Uri, fx: &mut Effects<'_, CommentsCommand>) {
        let Some(feed) = self.channel_for(session) else {
            return;
        };
        let landing = session.clone();
        fx.push(
            AnyEffect::new(crate::higent::PollAnnotationsEffect {
                seat: feed.seat.clone(),
                session: session.clone(),
            })
            .map(move |actions| CommentsCommand::Polled {
                session: landing.clone(),
                actions,
            }),
        );
    }

    /// Remove a record (announcing it to the live feed) and forget its
    /// card; the inlay itself is the application road's to drop.
    fn remove_in_place(&mut self, id: &AnnotationId) {
        let Some(record) = self.records.get(id) else {
            return;
        };
        if record.synced {
            if let Some(feed) = self.channel_for(&record.session) {
                feed.seat.dispatch_annotations(
                    &record.session,
                    StateAction::AnnotationsRemoved(AnnotationsRemovedAction {
                        annotation_id: id.clone(),
                    }),
                );
            }
        }
        self.records.remove_mut(id);
        self.cards.remove_mut(id);
        self.generation += 1;
    }

    /// Leave the card note for the application road: a landing's
    /// document work (inlay mint and removal) runs behind the lease.
    fn note_card_work(&self, store: &mut Store, dead: Vec<(DocumentId, InlayKey)>) {
        store.update::<CardWork>(|work| {
            work.dead.extend(dead);
            work.settle = true;
        });
    }

    /// Attach the annotations feed of the wire that serves a folder —
    /// the collection is the caller's (the window's family), the seat
    /// and AHP session come off the folder's authority.
    pub fn ensure(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        location: &ResourceLocation,
        fx: &mut AppFx<'_>,
    ) {
        if !Self::installed(store) {
            return;
        }
        let Some((server, seat, session)) =
            crate::higent::seat::route_seat(store, location.authority().as_str())
        else {
            return;
        };
        let known = Self::of(store, comments)
            .is_some_and(|comments| comments.channel_for(&session).is_some());
        if known {
            return;
        }
        Self::update(store, comments, |comments| {
            comments.channel = Some(ChannelFeed {
                session: session.clone(),
                server,
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
                AppCommand::at(
                    comments,
                    CommentsCommand::Snapshot {
                        session: session.clone(),
                        result,
                    },
                )
            }),
        );
    }

    pub fn created(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        location: &ResourceLocation,
        range: Range<LineCol>,
    ) -> Option<AnnotationId> {
        if !Self::installed(store) {
            return None;
        }
        let (server, session) = crate::higent::seat::route(store, location.authority().as_str())?;
        let id = Self::mint(store, comments);
        let entry_id = format!("{id}-e1");
        let record = CommentRecord {
            server,
            session,
            location: location.clone(),
            range: Some(range),
            turn_id: String::new(),
            resolved: false,
            entries: rpds::VectorSync::new_sync().push_back(EntryRecord {
                id: entry_id,
                text: crate::Text::from_string_exact(""),
                ours: true,
            }),
            thread_stamp: 0,
            synced: false,
            sending: false,
        };
        let stamp = latest_turn(store, record.server, &record.session);
        let mut record = record;
        record.turn_id = stamp;

        let feed = Self::of(store, comments)
            .and_then(|comments| comments.channel_for(&record.session).cloned())
            .filter(|feed| feed.live);
        if let Some(feed) = &feed {
            record.synced = true;
            dispatch_set(store, &feed.seat, &id, &record);
        }
        Self::update(store, comments, |comments| {
            comments.records.insert_mut(id.clone(), record.clone());
            comments.generation += 1;
        });
        Some(id)
    }

    pub fn card_born(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
        document: DocumentId,
        key: InlayKey,
    ) {
        let id = id.clone();
        Self::update(store, comments, |comments| {
            comments.cards.insert_mut(id, (document, key));
        });
    }

    pub fn removed(store: &mut Store, comments: imba::store::Id<Comments>, id: &AnnotationId) {
        Self::update(store, comments, |comments| comments.remove_in_place(id));
    }

    pub fn send_to_agent(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        window: WindowId,
        ids: Vec<AnnotationId>,
        fx: &mut AppFx<'_>,
    ) {
        let mut by_session: Vec<(Uri, Vec<AnnotationId>)> = Vec::new();
        for id in ids {
            let Some(record) = Self::record(store, comments, &id) else {
                continue;
            };
            if !record.synced || record.sending {
                continue;
            }
            match by_session
                .iter_mut()
                .find(|(session, _)| session == &record.session)
            {
                Some((_, group)) => group.push(id),
                None => by_session.push((record.session.clone(), vec![id])),
            }
        }
        for (session, group) in by_session {
            let Some(feed) = Self::of(store, comments)
                .and_then(|comments| comments.channel_for(&session).cloned())
            else {
                continue;
            };

            let key = crate::higent::SessionId {
                host: feed.server,
                session: session.clone(),
            };
            let mut chat = crate::higent::Agents::channel(store, &key)
                .and_then(|channel| channel.default_chat);
            if chat.is_none() {
                let bound = crate::Windows::window_ref(store, window)
                    .map(|entity| entity.current_session())
                    .and_then(|workspace| crate::higent::Agents::live_session(store, &workspace))
                    .filter(|bound| bound.host == feed.server);
                if let Some(bound) = bound {
                    chat = crate::higent::Agents::channel(store, &bound)
                        .and_then(|channel| channel.default_chat);
                }
            }
            let Some(chat) = chat else {
                eprintln!("[comments] no chat serves {session} — send skipped");
                continue;
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
                Self::update_record(store, comments, id, |record| record.sending = true);
            }
            let sent = group.clone();

            fx.push(
                AnyEffect::new(crate::higent::StartTurnEffect {
                    seat: feed.seat.clone(),
                    chat,
                    text: format!("Please address the attached review {noun}."),
                    attachments: Some(vec![attachment]),
                    model: None,
                })
                .map(move |result| {
                    AppCommand::at(
                        comments,
                        CommentsCommand::Sent {
                            ids: sent.clone(),
                            result,
                        },
                    )
                }),
            );
        }
    }

    pub fn text_edited(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
        text: crate::Text,
    ) {
        let Some(record) = Self::record(store, comments, id) else {
            return;
        };
        let Some(own) = record.own_entry() else {
            return;
        };
        let own_id = own.id.clone();
        {
            let own_id = own_id.clone();
            let text = text.clone();
            Self::update_record(store, comments, id, move |record| {
                let mut entries: Vec<EntryRecord> = record.entries.iter().cloned().collect();
                if let Some(entry) = entries.iter_mut().find(|entry| entry.id == own_id) {
                    entry.text = text;
                }
                record.entries = entries.into_iter().collect();
            });
        }
        if record.synced {
            if let Some(feed) = Self::of(store, comments)
                .and_then(|comments| comments.channel_for(&record.session).cloned())
                .filter(|feed| feed.live)
            {
                feed.seat.dispatch_annotations(
                    &record.session,
                    StateAction::AnnotationsEntrySet(AnnotationsEntrySetAction {
                        annotation_id: id.clone(),
                        entry: AnnotationEntry {
                            id: own_id,
                            text: StringOrMarkdown::Markdown {
                                markdown: wire_string(&text),
                            },
                            meta: author_meta("user"),
                        },
                    }),
                );
            }
        }
    }

    pub fn resolve(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        id: &AnnotationId,
        resolved: bool,
    ) {
        let Some(record) = Self::record(store, comments, id) else {
            return;
        };
        Self::update_record(store, comments, id, |record| record.resolved = resolved);
        if record.synced {
            if let Some(feed) = Self::of(store, comments)
                .and_then(|comments| comments.channel_for(&record.session).cloned())
            {
                feed.seat.dispatch_annotations(
                    &record.session,
                    StateAction::AnnotationsUpdated(AnnotationsUpdatedAction {
                        annotation_id: id.clone(),
                        origin: None,
                        resource: None,
                        range: None,
                        resolved: Some(resolved),
                    }),
                );
            }
        }
    }
}

fn latest_turn(store: &Store, server: crate::higent::HostId, session: &Uri) -> String {
    crate::higent::Agents::latest_turn(
        store,
        &crate::higent::SessionId {
            host: server,
            session: session.clone(),
        },
    )
    .unwrap_or_default()
}

/// The `AtComments` arm's drain: a landing's card note runs with the
/// application effects the entity does not hold.
pub(crate) fn run_card_work(
    store: &mut Store,
    ui: &imba::UiCtx,
    comments: imba::store::Id<Comments>,
    work: CardWork,
    fx: &mut AppFx<'_>,
) {
    if let Some(documents) = Comments::documents_of(store, comments) {
        for (document, key) in work.dead {
            remove_card(store, documents, ui, document, key, fx);
        }
    }
    if work.settle {
        settle_cards(store, ui, comments, fx);
    }
}

/// Behind a landing, after the lease: unsynced records reach the live
/// feed, and every record whose document is open settles into a card.
fn settle_cards(
    store: &mut Store,
    ui: &imba::UiCtx,
    comments: imba::store::Id<Comments>,
    fx: &mut AppFx<'_>,
) {
    let Some(feed) = Comments::of(store, comments).and_then(|comments| comments.channel.clone())
    else {
        return;
    };
    let session = feed.session.clone();
    let records: Vec<(AnnotationId, CommentRecord)> = Comments::records(store, comments)
        .into_iter()
        .filter(|(_, record)| record.session == session)
        .collect();
    for (id, record) in records {
        if !record.synced && feed.live {
            dispatch_set(store, &feed.seat, &id, &record);
            Comments::update_record(store, comments, &id, |record| record.synced = true);
        }
        match Comments::card(store, comments, &id) {
            Some(_) => refresh_card(store, ui, comments, &id, &record),
            None => {
                if let Some(document) =
                    Comments::documents_of(store, comments).and_then(|documents| {
                        crate::OpenDocuments::by_location(store, documents, &record.location)
                    })
                {
                    materialize(store, ui, comments, &id, &record, document, fx);
                }
            }
        }
    }
}

fn fold_set(
    comments: &mut Comments,
    server: crate::higent::HostId,
    session: &Uri,
    annotation: &Annotation,
    location: &ResourceLocation,
) {
    let held = comments.records.get(&annotation.id);
    let ours: std::collections::HashSet<String> = held
        .map(|held| {
            held.entries
                .iter()
                .filter(|entry| entry.ours)
                .map(|entry| entry.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let stamp = held.map(|held| held.thread_stamp).unwrap_or(0);
    let foreign_touch = annotation
        .entries
        .iter()
        .any(|entry| !ours.contains(&entry.id));
    let record = CommentRecord {
        server,
        session: session.clone(),
        location: location.clone(),
        range: annotation.range.as_ref().map(from_wire),
        turn_id: annotation.origin.turn_id.clone().unwrap_or_default(),
        resolved: annotation.resolved,
        entries: annotation
            .entries
            .iter()
            .map(|entry| EntryRecord {
                id: entry.id.clone(),
                text: wire_text(&entry.text),
                ours: ours.contains(&entry.id),
            })
            .collect(),
        thread_stamp: stamp + u64::from(foreign_touch),
        synced: true,

        sending: comments
            .records
            .get(&annotation.id)
            .is_some_and(|held| held.sending),
    };
    comments.records.insert_mut(annotation.id.clone(), record);
}

fn remove_card(
    store: &mut Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    ui: &imba::UiCtx,
    document: DocumentId,
    key: InlayKey,
    fx: &mut AppFx<'_>,
) {
    let Some(mut doc) = crate::OpenDocuments::document(store, documents, document) else {
        return;
    };
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    fx.scope(
        move |command| {
            AppCommand::at(
                documents,
                crate::app::DocumentsCommand::Editor(document, command),
            )
        },
        |fx| doc.remove_inlay(key, store, ui, &fonts, &theme, fx),
    );
    crate::OpenDocuments::put_document(store, documents, document, doc);
}

pub struct CommentsHook;

impl crate::DocumentHook for CommentsHook {
    fn opened(
        &self,
        store: &mut Store,
        documents: imba::store::Id<crate::OpenDocuments>,
        document: DocumentId,
        location: Option<&crate::ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        // The hook holds a documents id; the cards' collection is the
        // sibling next to it.
        let Some(comments) = crate::higent::Hosts::family_of_documents(store, documents)
            .map(|family| family.comments())
        else {
            return;
        };
        let owes = Comments::of(store, comments).is_some_and(|comments| {
            comments.records.iter().any(|(id, record)| {
                record.location == *location && !comments.cards.contains_key(id)
            })
        });
        if owes {
            crate::AppRequests::push(store, Arc::new(MaterializeFor { comments, document }));
        }
    }

    fn closing(
        &self,
        store: &mut Store,
        documents: imba::store::Id<crate::OpenDocuments>,
        document: DocumentId,
        _location: Option<&crate::ResourceLocation>,
        doc: &crate::Document,
    ) {
        let Some(comments) = crate::higent::Hosts::family_of_documents(store, documents)
            .map(|family| family.comments())
        else {
            return;
        };
        let cards: Vec<(AnnotationId, InlayKey)> = Comments::of(store, comments)
            .map(|comments| {
                comments
                    .cards
                    .iter()
                    .filter(|(_, (held, _))| *held == document)
                    .map(|(id, (_, key))| (id.clone(), *key))
                    .collect()
            })
            .unwrap_or_default();
        for (id, key) in cards {
            if let Some(range) = live_card_range(doc, key) {
                Comments::update_record(store, comments, &id, move |record| {
                    record.range = Some(range);
                });
            }
            Comments::update(store, comments, |comments| {
                comments.cards.remove_mut(&id);
            });
        }
    }
}

struct MaterializeFor {
    comments: imba::store::Id<Comments>,
    document: DocumentId,
}

impl crate::DynamicCommand for MaterializeFor {
    fn id(&self) -> &'static str {
        "comments.materialize"
    }
    fn name(&self) -> String {
        "Materialize Comments".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let _ = window;
        let ui = &app.ui_ctx();
        let Some(documents) = Comments::documents_of(store, self.comments) else {
            return;
        };
        let Some(location) = crate::OpenDocuments::location(store, documents, self.document) else {
            return;
        };
        let owed: Vec<(AnnotationId, CommentRecord)> = Comments::records(store, self.comments)
            .into_iter()
            .filter(|(id, record)| {
                record.location == location && Comments::card(store, self.comments, id).is_none()
            })
            .collect();
        for (id, record) in owed {
            materialize(store, ui, self.comments, &id, &record, self.document, fx);
        }
    }
}

/// The card's live range read off the CLOSING row itself — the hook
/// is handed the document; a store read here would be lease
/// reentrancy (docs/entities.md law 5).
fn live_card_range(doc: &crate::Document, key: InlayKey) -> Option<Range<LineCol>> {
    let markup = doc.feature_markup(comments_markup())?;

    let (range, _) = markup.inlay_at_key(comments_markup(), key)?;
    let mut view = doc.text().view();
    Some(
        crate::line_col_at(&mut view, range.start as usize)
            ..crate::line_col_at(&mut view, range.end as usize),
    )
}

fn materialize(
    store: &mut Store,
    ui: &imba::UiCtx,
    comments: imba::store::Id<Comments>,
    id: &AnnotationId,
    record: &CommentRecord,
    document: DocumentId,
    fx: &mut AppFx<'_>,
) {
    let Some(documents) = Comments::documents_of(store, comments) else {
        return;
    };
    let Some(mut doc) = crate::OpenDocuments::document(store, documents, document) else {
        return;
    };
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let range = match &record.range {
        Some(range) => {
            let mut view = doc.text().view();
            let start = crate::offset_at(&mut view, range.start).min(byte_count as usize) as u32;
            let end = crate::offset_at(&mut view, range.end).min(byte_count as usize) as u32;
            start..end.max(start)
        }
        None => 0..byte_count,
    };
    let view = CommentView::materialized(
        comments,
        Some(document),
        crate::hicomments::FALLBACK_WIDTH,
        store,
        ui,
        &fonts,
        &theme,
        id.clone(),
        record.own_entry().map(|entry| entry.text.clone()),
        record.foreign_texts(),
        record.thread_stamp,
    );
    let markup = comments_markup();
    doc.ensure_document_markup(markup);
    let mut minted = None;
    fx.scope(
        move |command| {
            AppCommand::at(
                documents,
                crate::app::DocumentsCommand::Editor(document, command),
            )
        },
        |fx| {
            let key = doc.push_inlay(
                markup,
                range.clone(),
                crate::Inlay::new(crate::InlayMode::Under, view.clone()),
                store,
                ui,
                &fonts,
                &theme,
                fx,
            );
            doc.swap_inlay(
                key,
                range.clone(),
                crate::Inlay::new(crate::InlayMode::Under, view.clone().keyed(key)),
            );
            minted = Some(key);
        },
    );
    crate::OpenDocuments::put_document(store, documents, document, doc);
    if let Some(key) = minted {
        Comments::card_born(store, comments, id, document, key);
    }
}

fn refresh_card(
    store: &mut Store,
    ui: &imba::UiCtx,
    comments: imba::store::Id<Comments>,
    id: &AnnotationId,
    record: &CommentRecord,
) {
    let Some((document, key)) = Comments::card(store, comments, id) else {
        return;
    };
    let Some(documents) = Comments::documents_of(store, comments) else {
        return;
    };
    let Some(doc) = crate::OpenDocuments::document_ref(store, documents, document) else {
        return;
    };
    let Some(markup) = doc.feature_markup(comments_markup()) else {
        return;
    };

    let Some((range, inlay)) = markup.inlay_at_key(comments_markup(), key) else {
        return;
    };
    let view = inlay.view_as::<CommentView>();

    if !view.is_some_and(|view| view.thread_stamp() != record.thread_stamp) {
        return;
    }
    let live_text = view.map(|view| view.text_rope());
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    let rebuilt = CommentView::materialized(
        comments,
        Some(document),
        crate::hicomments::FALLBACK_WIDTH,
        store,
        ui,
        &fonts,
        &theme,
        id.clone(),
        live_text.or(record.own_entry().map(|entry| entry.text.clone())),
        record.foreign_texts(),
        record.thread_stamp,
    )
    .keyed(key);
    let mut doc = crate::OpenDocuments::document(store, documents, document).expect("held above");
    doc.swap_inlay(
        key,
        range,
        crate::Inlay::new(crate::InlayMode::Under, rebuilt),
    );
    crate::OpenDocuments::put_document(store, documents, document, doc);
}

fn dispatch_set(
    store: &Store,
    seat: &Arc<dyn crate::higent::AhpServer>,
    id: &AnnotationId,
    record: &CommentRecord,
) {
    let Some(uri) = uri_of(store, record.server, &record.location) else {
        return;
    };
    seat.dispatch_annotations(
        &record.session,
        StateAction::AnnotationsSet(AnnotationsSetAction {
            annotation: Annotation {
                id: id.clone(),
                origin: crate::higent::ahp_types::state::AnnotationOrigin {
                    session: record.session.to_string(),
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

fn uri_of(
    store: &Store,
    server: crate::higent::HostId,
    location: &ResourceLocation,
) -> Option<String> {
    Some(
        crate::higent::Hosts::uris(store, server)?
            .uri_of(location)
            .into_string(),
    )
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

fn to_wire(range: &Range<LineCol>) -> TextRange {
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

fn from_wire(range: &TextRange) -> Range<LineCol> {
    let position = |position: &TextPosition| LineCol {
        line: position.line.max(0) as u32,
        col: position.character.max(0) as u32,
    };
    position(&range.start)..position(&range.end)
}
