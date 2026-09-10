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
use imba::effect::AnyEffect;
use imba::store::Store;

use crate::hicomments::{comments_markup, CommentView};

type Uri = String;

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

#[derive(Clone, Default)]
pub struct Comments {
    records: rpds::HashTrieMapSync<AnnotationId, CommentRecord>,

    cards: rpds::HashTrieMapSync<AnnotationId, (DocumentId, InlayKey)>,

    channel: Option<ChannelFeed>,

    generation: u64,

    minted: u64,
}

#[derive(Clone, Default)]
pub struct CommentsInstall;

impl Comments {
    pub fn install(store: &mut Store) {
        store.put(CommentsInstall);
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<CommentsInstall>().is_some()
    }

    fn channel_for(&self, session: &str) -> Option<&ChannelFeed> {
        self.channel.as_ref().filter(|feed| feed.session == session)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.cards.is_empty() && self.channel.is_none()
    }

    pub fn generation(store: &Store) -> u64 {
        store
            .get::<Comments>()
            .map(|comments| comments.generation)
            .unwrap_or(0)
    }

    pub fn record(store: &Store, id: &AnnotationId) -> Option<CommentRecord> {
        store.get::<Comments>()?.records.get(id).cloned()
    }

    pub fn records(store: &Store) -> Vec<(AnnotationId, CommentRecord)> {
        store
            .get::<Comments>()
            .map(|comments| {
                comments
                    .records
                    .iter()
                    .map(|(id, record)| (id.clone(), record.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn card(store: &Store, id: &AnnotationId) -> Option<(DocumentId, InlayKey)> {
        store.get::<Comments>()?.cards.get(id).copied()
    }

    fn update_record(
        store: &mut Store,
        id: &AnnotationId,
        change: impl FnOnce(&mut CommentRecord),
    ) {
        store.update::<Comments>(|comments| {
            let Some(mut record) = comments.records.get(id).cloned() else {
                return;
            };
            change(&mut record);
            comments.records.insert_mut(id.clone(), record);
            comments.generation += 1;
        });
    }

    fn mint(store: &mut Store) -> AnnotationId {
        let mut id = String::new();
        store.update::<Comments>(|comments| {
            comments.minted += 1;
            id = format!("hc-{}-{}", std::process::id(), comments.minted);
        });
        id
    }

    pub fn ensure(
        store: &mut Store,
        window: WindowId,
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
        let known = store
            .get::<Comments>()
            .is_some_and(|comments| comments.channel_for(&session).is_some());
        if known {
            return;
        }
        store.update::<Comments>(|comments| {
            comments.channel = Some(ChannelFeed {
                session: session.clone(),
                server,
                seat: seat.clone(),
                live: false,
            });
        });
        let scope = crate::SessionId {
            host: server,
            session: session.clone(),
        };
        fx.push(
            AnyEffect::new(crate::higent::SubscribeAnnotationsEffect {
                seat,
                session: session.clone(),
            })
            .map(move |result| {
                AppCommand::dynamic_in(
                    scope.clone(),
                    window,
                    Arc::new(SnapshotLanded {
                        session: session.clone(),
                        result,
                    }),
                )
            }),
        );
    }

    pub fn created(
        store: &mut Store,
        location: &ResourceLocation,
        range: Range<LineCol>,
    ) -> Option<AnnotationId> {
        if !Self::installed(store) {
            return None;
        }
        let (server, session) = crate::higent::seat::route(store, location.authority().as_str())?;
        let id = Self::mint(store);
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

        let feed = store
            .get::<Comments>()
            .and_then(|comments| comments.channel_for(&record.session).cloned())
            .filter(|feed| feed.live);
        if let Some(feed) = &feed {
            record.synced = true;
            dispatch_set(store, &feed.seat, &id, &record);
        }
        store.update::<Comments>(|comments| {
            comments.records.insert_mut(id.clone(), record.clone());
            comments.generation += 1;
        });
        Some(id)
    }

    pub fn card_born(store: &mut Store, id: &AnnotationId, document: DocumentId, key: InlayKey) {
        let id = id.clone();
        store.update::<Comments>(|comments| {
            comments.cards.insert_mut(id, (document, key));
        });
    }

    pub fn removed(store: &mut Store, id: &AnnotationId) {
        let Some(record) = Self::record(store, id) else {
            return;
        };
        if record.synced {
            if let Some(feed) = store
                .get::<Comments>()
                .and_then(|comments| comments.channel_for(&record.session).cloned())
            {
                feed.seat.dispatch_annotations(
                    &record.session,
                    StateAction::AnnotationsRemoved(AnnotationsRemovedAction {
                        annotation_id: id.clone(),
                    }),
                );
            }
        }
        store.update::<Comments>(|comments| {
            comments.records.remove_mut(id);
            comments.cards.remove_mut(id);
            comments.generation += 1;
        });
    }

    pub fn send_to_agent(
        store: &mut Store,
        window: WindowId,
        ids: Vec<AnnotationId>,
        fx: &mut AppFx<'_>,
    ) {
        let mut by_session: Vec<(Uri, Vec<AnnotationId>)> = Vec::new();
        for id in ids {
            let Some(record) = Self::record(store, &id) else {
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
            let Some(feed) = store
                .get::<Comments>()
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
                Self::update_record(store, id, |record| record.sending = true);
            }
            let sent = group.clone();

            let scope = key.clone();
            fx.push(
                AnyEffect::new(crate::higent::StartTurnEffect {
                    seat: feed.seat.clone(),
                    chat,
                    text: format!("Please address the attached review {noun}."),
                    attachments: Some(vec![attachment]),
                    model: None,
                })
                .map(move |result| {
                    AppCommand::dynamic_in(
                        scope.clone(),
                        window,
                        Arc::new(Sent {
                            ids: sent.clone(),
                            result,
                        }),
                    )
                }),
            );
        }
    }

    pub fn text_edited(store: &mut Store, id: &AnnotationId, text: crate::Text) {
        let Some(record) = Self::record(store, id) else {
            return;
        };
        let Some(own) = record.own_entry() else {
            return;
        };
        let own_id = own.id.clone();
        {
            let own_id = own_id.clone();
            let text = text.clone();
            Self::update_record(store, id, move |record| {
                let mut entries: Vec<EntryRecord> = record.entries.iter().cloned().collect();
                if let Some(entry) = entries.iter_mut().find(|entry| entry.id == own_id) {
                    entry.text = text;
                }
                record.entries = entries.into_iter().collect();
            });
        }
        if record.synced {
            if let Some(feed) = store
                .get::<Comments>()
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

    pub fn resolve(store: &mut Store, id: &AnnotationId, resolved: bool) {
        let Some(record) = Self::record(store, id) else {
            return;
        };
        Self::update_record(store, id, |record| record.resolved = resolved);
        if record.synced {
            if let Some(feed) = store
                .get::<Comments>()
                .and_then(|comments| comments.channel_for(&record.session).cloned())
            {
                feed.seat.dispatch_annotations(
                    &record.session,
                    StateAction::AnnotationsUpdated(AnnotationsUpdatedAction {
                        annotation_id: id.clone(),
                        turn_id: None,
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

struct SnapshotLanded {
    session: Uri,
    result: Result<AnnotationsState, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "comments.snapshot-landed"
    }
    fn name(&self) -> String {
        "Comments Snapshot Landed".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(feed) = store
            .get::<Comments>()
            .and_then(|comments| comments.channel_for(&self.session).cloned())
        else {
            return;
        };
        let state = match &self.result {
            Ok(state) => state,
            Err(_) => {
                store.update::<Comments>(|comments| {
                    if comments.channel_for(&self.session).is_some() {
                        comments.channel = None;
                    }
                });
                return;
            }
        };

        let placed: Vec<(Annotation, ResourceLocation)> = state
            .annotations
            .iter()
            .filter_map(|annotation| {
                place(store, &self.session, annotation)
                    .map(|location| (annotation.clone(), location))
            })
            .collect();
        store.update::<Comments>(|comments| {
            if let Some(mut feed) = comments.channel_for(&self.session).cloned() {
                feed.live = true;
                comments.channel = Some(feed);
            }
            for (annotation, location) in &placed {
                fold_set(comments, feed.server, &self.session, annotation, location);
            }
            comments.generation += 1;
        });
        settle(store, &self.session, fx);
        relaunch_poll(window, &self.session, &feed, fx);
    }
}

struct Polled {
    session: Uri,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "comments.polled"
    }
    fn name(&self) -> String {
        "Comments Actions Landed".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(feed) = store
            .get::<Comments>()
            .and_then(|comments| comments.channel_for(&self.session).cloned())
        else {
            return;
        };

        let placed: std::collections::HashMap<AnnotationId, ResourceLocation> = self
            .actions
            .iter()
            .filter_map(|action| match action {
                StateAction::AnnotationsSet(set) => place(store, &self.session, &set.annotation)
                    .map(|location| (set.annotation.id.clone(), location)),
                _ => None,
            })
            .collect();
        let mut dead_cards: Vec<(DocumentId, InlayKey)> = Vec::new();
        store.update::<Comments>(|comments| {
            for action in &self.actions {
                match action {
                    StateAction::AnnotationsSet(set) => {
                        let Some(location) = placed.get(&set.annotation.id) else {
                            continue;
                        };
                        fold_set(
                            comments,
                            feed.server,
                            &self.session,
                            &set.annotation,
                            location,
                        );
                    }
                    StateAction::AnnotationsUpdated(updated) => {
                        let Some(mut record) =
                            comments.records.get(&updated.annotation_id).cloned()
                        else {
                            continue;
                        };
                        if let Some(turn) = &updated.turn_id {
                            record.turn_id = turn.clone();
                        }
                        if let Some(range) = &updated.range {
                            record.range = Some(from_wire(range));
                        }
                        if let Some(resolved) = updated.resolved {
                            record.resolved = resolved;
                        }
                        comments
                            .records
                            .insert_mut(updated.annotation_id.clone(), record);
                    }
                    StateAction::AnnotationsRemoved(removed) => {
                        if let Some(card) = comments.cards.get(&removed.annotation_id) {
                            dead_cards.push(*card);
                        }
                        comments.records.remove_mut(&removed.annotation_id);
                        comments.cards.remove_mut(&removed.annotation_id);
                    }
                    StateAction::AnnotationsEntrySet(set) => {
                        let Some(mut record) = comments.records.get(&set.annotation_id).cloned()
                        else {
                            continue;
                        };
                        let text = wire_text(&set.entry.text);
                        let mut entries: Vec<EntryRecord> =
                            record.entries.iter().cloned().collect();
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
                        comments
                            .records
                            .insert_mut(set.annotation_id.clone(), record);
                    }
                    StateAction::AnnotationsEntryRemoved(removed) => {
                        let Some(mut record) =
                            comments.records.get(&removed.annotation_id).cloned()
                        else {
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
                        comments
                            .records
                            .insert_mut(removed.annotation_id.clone(), record);
                    }
                    _ => {}
                }
            }
            comments.generation += 1;
        });

        for (document, key) in dead_cards {
            remove_card(store, document, key, fx);
        }
        settle(store, &self.session, fx);
        relaunch_poll(window, &self.session, &feed, fx);
    }
}

fn settle(store: &mut Store, session: &Uri, fx: &mut AppFx<'_>) {
    let feed = store
        .get::<Comments>()
        .and_then(|comments| comments.channel_for(session).cloned());
    let records: Vec<(AnnotationId, CommentRecord)> = Comments::records(store)
        .into_iter()
        .filter(|(_, record)| &record.session == session)
        .collect();
    for (id, record) in records {
        if !record.synced {
            if let Some(feed) = feed.as_ref().filter(|feed| feed.live) {
                dispatch_set(store, &feed.seat, &id, &record);
                Comments::update_record(store, &id, |record| record.synced = true);
            }
        }
        match Comments::card(store, &id) {
            Some(_) => refresh_card(store, &id, &record),
            None => {
                if let Some(document) = crate::OpenDocuments::by_location(store, &record.location) {
                    materialize(store, &id, &record, document, fx);
                }
            }
        }
    }
}

fn relaunch_poll(window: WindowId, session: &Uri, feed: &ChannelFeed, fx: &mut AppFx<'_>) {
    let scope = crate::SessionId {
        host: feed.server,
        session: session.clone(),
    };
    let session = session.clone();
    let landing = session.clone();
    fx.push(
        AnyEffect::new(crate::higent::PollAnnotationsEffect {
            seat: feed.seat.clone(),
            session,
        })
        .map(move |actions| {
            AppCommand::dynamic_in(
                scope.clone(),
                window,
                Arc::new(Polled {
                    session: landing.clone(),
                    actions,
                }),
            )
        }),
    );
}

pub(crate) struct Sent {
    pub(crate) ids: Vec<AnnotationId>,
    pub(crate) result: Result<(), String>,
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
        _app: &mut crate::Application,
        store: &mut Store,
        _window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        if let Err(error) = &self.result {
            eprintln!("[comments] send failed, comments kept: {error}");
            for id in &self.ids {
                Comments::update_record(store, id, |record| record.sending = false);
            }
            return;
        }
        for id in &self.ids {
            let card = Comments::card(store, id);
            Comments::removed(store, id);
            let Some((document, key)) = card else {
                continue;
            };
            let Some(mut doc) = crate::OpenDocuments::document(store, document) else {
                continue;
            };
            let fonts = crate::env::Fonts::of(store)();
            let theme = crate::env::Themes::of(store);
            fx.scope(
                move |command| AppCommand::Entity(document, command),
                |fx| doc.remove_inlay(key, &fonts, &theme, fx),
            );
            crate::OpenDocuments::put_document(store, document, doc);
        }
    }
}

fn place(store: &Store, session: &Uri, annotation: &Annotation) -> Option<ResourceLocation> {
    let comments = store.get::<Comments>()?;
    if let Some(held) = comments.records.get(&annotation.id) {
        return Some(held.location.clone());
    }
    let server = comments.channel_for(session)?.server;
    let uris = crate::higent::Hosts::uris(store, server)?;
    let authority = crate::higent::seat::route_authority(server, session);
    uris.location_of(
        &crate::higent::ResourceUri::new(annotation.resource.clone()),
        crate::ResourceType::document(),
        &authority,
    )
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
        turn_id: annotation.turn_id.clone(),
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

fn remove_card(store: &mut Store, document: DocumentId, key: InlayKey, fx: &mut AppFx<'_>) {
    let Some(mut doc) = crate::OpenDocuments::document(store, document) else {
        return;
    };
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    fx.scope(
        move |command| AppCommand::Entity(document, command),
        |fx| doc.remove_inlay(key, &fonts, &theme, fx),
    );
    crate::OpenDocuments::put_document(store, document, doc);
}

pub struct CommentsHook;

impl crate::DocumentHook for CommentsHook {
    fn opened(&self, store: &mut Store, document: DocumentId) {
        let Some(location) = crate::OpenDocuments::location(store, document) else {
            return;
        };
        let owes = store.get::<Comments>().is_some_and(|comments| {
            comments
                .records
                .iter()
                .any(|(id, record)| record.location == location && !comments.cards.contains_key(id))
        });
        if owes {
            crate::AppRequests::push(store, Arc::new(MaterializeFor { document }));
        }
    }

    fn closing(&self, store: &mut Store, document: DocumentId) {
        let cards: Vec<(AnnotationId, InlayKey)> = store
            .get::<Comments>()
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
            if let Some(range) = live_card_range(store, document, key) {
                Comments::update_record(store, &id, move |record| {
                    record.range = Some(range);
                });
            }
            store.update::<Comments>(|comments| {
                comments.cards.remove_mut(&id);
            });
        }
    }
}

struct MaterializeFor {
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
        _app: &mut crate::Application,
        store: &mut Store,
        _window: WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(location) = crate::OpenDocuments::location(store, self.document) else {
            return;
        };
        let owed: Vec<(AnnotationId, CommentRecord)> = Comments::records(store)
            .into_iter()
            .filter(|(id, record)| {
                record.location == location && Comments::card(store, id).is_none()
            })
            .collect();
        for (id, record) in owed {
            materialize(store, &id, &record, self.document, fx);
        }
    }
}

fn live_card_range(store: &Store, document: DocumentId, key: InlayKey) -> Option<Range<LineCol>> {
    let doc = crate::OpenDocuments::document_ref(store, document)?;
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
    id: &AnnotationId,
    record: &CommentRecord,
    document: DocumentId,
    fx: &mut AppFx<'_>,
) {
    let Some(mut doc) = crate::OpenDocuments::document(store, document) else {
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
        Some(document),
        crate::hicomments::FALLBACK_WIDTH,
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
        move |command| AppCommand::Entity(document, command),
        |fx| {
            let key = doc.push_inlay(
                markup,
                range.clone(),
                crate::Inlay::new(crate::InlayMode::Under, view.clone()),
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
    crate::OpenDocuments::put_document(store, document, doc);
    if let Some(key) = minted {
        Comments::card_born(store, id, document, key);
    }
}

fn refresh_card(store: &mut Store, id: &AnnotationId, record: &CommentRecord) {
    let Some((document, key)) = Comments::card(store, id) else {
        return;
    };
    let Some(doc) = crate::OpenDocuments::document_ref(store, document) else {
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
        Some(document),
        crate::hicomments::FALLBACK_WIDTH,
        &fonts,
        &theme,
        id.clone(),
        live_text.or(record.own_entry().map(|entry| entry.text.clone())),
        record.foreign_texts(),
        record.thread_stamp,
    )
    .keyed(key);
    let mut doc = crate::OpenDocuments::document(store, document).expect("held above");
    doc.swap_inlay(
        key,
        range,
        crate::Inlay::new(crate::InlayMode::Under, rebuilt),
    );
    crate::OpenDocuments::put_document(store, document, doc);
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
                turn_id: record.turn_id.clone(),
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
) -> Option<Uri> {
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
