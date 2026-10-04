// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;
use std::sync::Arc;

use crate::{AppCommand, AppFx, DocumentId, InlayKey, LineCol, ResourceLocation, WindowId};
use imba::store::Store;

use crate::hicomments::{comments_markup, CommentView};

pub type AnnotationId = String;

#[derive(Clone)]
pub struct EntryRecord {
    pub id: String,

    pub text: crate::Text,

    pub ours: bool,
}

#[derive(Clone)]
pub struct CommentRecord {
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

/// The model's MIRROR of an incoming annotation — the driver
/// digests the wire shape into this; the model folds values.
#[derive(Clone)]
pub struct CommentSeed {
    pub id: AnnotationId,
    pub location: ResourceLocation,
    pub range: Option<Range<LineCol>>,
    pub turn_id: String,
    pub resolved: bool,
    pub entries: Vec<EntryRecord>,
}

/// A streamed comments update, mirrored.
#[derive(Clone)]
pub enum CommentDelta {
    Set(CommentSeed),
    Updated {
        id: AnnotationId,
        turn_id: Option<String>,
        range: Option<Range<LineCol>>,
        resolved: Option<bool>,
    },
    Removed {
        id: AnnotationId,
    },
    EntrySet {
        id: AnnotationId,
        entry_id: String,
        text: crate::Text,
    },
    EntryRemoved {
        id: AnnotationId,
        entry_id: String,
    },
}

/// An outbound intent the model NOTED — the wire driver's lane
/// drains these onto a live channel; the model never dispatches.
#[derive(Clone)]
pub enum Announce {
    /// The whole record (a fresh comment, or one born offline).
    Set(AnnotationId),
    /// Our entry's text changed.
    EntrySet { id: AnnotationId, entry_id: String },
    /// Resolution (or other record meta) changed.
    Meta(AnnotationId),
    /// The record left — announced so the host forgets it too.
    Removed(AnnotationId),
}

#[derive(Clone)]
pub struct Comments {
    /// The documents the cards live in — wired at the family mint
    /// (docs/entities.md law 4).
    documents: imba::store::Id<crate::OpenDocuments>,

    records: rpds::HashTrieMapSync<AnnotationId, CommentRecord>,

    cards: rpds::HashTrieMapSync<AnnotationId, (DocumentId, InlayKey)>,

    /// A landing's pending card work (inlay mint and removal) — the
    /// collection's own note to the wire driver's lane, drained the
    /// same batch. Private schema, not a store component.
    work: CardWork,

    /// Outbound intents awaiting a live channel — the other half of
    /// the note.
    announce: Vec<Announce>,

    generation: u64,

    minted: u64,
}

/// A landing's pending card work: cards whose records died drop
/// their inlays, and the surviving records settle into cards. Lives
/// ON the collection row — written behind the lease, drained by
/// `after_route` the same batch; the entity holds no
/// document-addressed effects of its own.
#[derive(Clone, Default)]
pub struct CardWork {
    pub dead: Vec<(DocumentId, InlayKey)>,
    pub settle: bool,
}

impl CardWork {
    fn is_empty(&self) -> bool {
        self.dead.is_empty() && !self.settle
    }
}

impl Comments {
    pub fn install(store: &mut Store) {
        crate::registry::Registry::update(store, |registry| registry.comments = true);
    }

    pub fn installed(store: &Store) -> bool {
        crate::registry::Registry::of(store).is_some_and(|registry| registry.comments)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.cards.is_empty()
    }

    /// A collection wired to the documents its cards live in — minted
    /// by the family ceremony.
    pub fn wired(documents: imba::store::Id<crate::OpenDocuments>) -> Self {
        Self {
            documents,
            records: rpds::HashTrieMapSync::new_sync(),
            cards: rpds::HashTrieMapSync::new_sync(),
            work: CardWork::default(),
            announce: Vec::new(),
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

    pub(crate) fn update_record(
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

    /// Fold the feed's streamed actions; cards whose records died come
    /// back for the application road to drop.
    /// Land a snapshot's seeds — the driver digested the wire; this
    /// is value folding (existing records keep their location, their
    /// ours-marks and their send flags).
    pub(crate) fn land_seeds(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        seeds: Vec<CommentSeed>,
    ) {
        Self::update(store, comments, |held| {
            for seed in &seeds {
                held.fold_seed(seed);
            }
            held.generation += 1;
            held.note_card_work(Vec::new());
        });
    }

    /// Fold the driver's mirrored deltas; cards whose records died
    /// ride the card-work note for the lane to drop.
    pub(crate) fn fold_deltas(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        deltas: Vec<CommentDelta>,
    ) {
        Self::update(store, comments, |held| {
            let mut dead_cards: Vec<(DocumentId, InlayKey)> = Vec::new();
            for delta in &deltas {
                match delta {
                    CommentDelta::Set(seed) => held.fold_seed(seed),
                    CommentDelta::Updated {
                        id,
                        turn_id,
                        range,
                        resolved,
                    } => {
                        let Some(mut record) = held.records.get(id).cloned() else {
                            continue;
                        };
                        if let Some(turn) = turn_id {
                            record.turn_id = turn.clone();
                        }
                        if let Some(range) = range {
                            record.range = Some(range.clone());
                        }
                        if let Some(resolved) = resolved {
                            record.resolved = *resolved;
                        }
                        held.records.insert_mut(id.clone(), record);
                    }
                    CommentDelta::Removed { id } => {
                        if let Some(card) = held.cards.get(id) {
                            dead_cards.push(*card);
                        }
                        held.records.remove_mut(id);
                        held.cards.remove_mut(id);
                    }
                    CommentDelta::EntrySet { id, entry_id, text } => {
                        let Some(mut record) = held.records.get(id).cloned() else {
                            continue;
                        };
                        let mut entries: Vec<EntryRecord> =
                            record.entries.iter().cloned().collect();
                        let mut foreign_touch = true;
                        match entries.iter_mut().find(|entry| entry.id == *entry_id) {
                            Some(entry) => {
                                foreign_touch = !entry.ours;
                                entry.text = text.clone();
                            }
                            None => entries.push(EntryRecord {
                                id: entry_id.clone(),
                                text: text.clone(),
                                ours: false,
                            }),
                        }
                        record.entries = entries.into_iter().collect();
                        record.thread_stamp += u64::from(foreign_touch);
                        held.records.insert_mut(id.clone(), record);
                    }
                    CommentDelta::EntryRemoved { id, entry_id } => {
                        let Some(mut record) = held.records.get(id).cloned() else {
                            continue;
                        };
                        let foreign_touch = record
                            .entries
                            .iter()
                            .any(|entry| entry.id == *entry_id && !entry.ours);
                        record.entries = record
                            .entries
                            .iter()
                            .filter(|entry| entry.id != *entry_id)
                            .cloned()
                            .collect();
                        record.thread_stamp += u64::from(foreign_touch);
                        held.records.insert_mut(id.clone(), record);
                    }
                }
            }
            held.generation += 1;
            held.note_card_work(dead_cards);
        });
    }

    /// One incoming annotation onto the records — the existing
    /// record's location, ours-marks, thread stamp and send flag
    /// survive the fold (the fold_set discipline).
    fn fold_seed(&mut self, seed: &CommentSeed) {
        let held = self.records.get(&seed.id);
        let ours: std::collections::HashSet<String> = held
            .map(|held| {
                held.entries
                    .iter()
                    .filter(|entry| entry.ours)
                    .map(|entry| entry.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let location = held
            .map(|held| held.location.clone())
            .unwrap_or_else(|| seed.location.clone());
        let stamp = held.map(|held| held.thread_stamp).unwrap_or(0);
        let sending = held.is_some_and(|held| held.sending);
        let foreign_touch = seed.entries.iter().any(|entry| !ours.contains(&entry.id));
        let record = CommentRecord {
            location,
            range: seed.range.clone(),
            turn_id: seed.turn_id.clone(),
            resolved: seed.resolved,
            entries: seed
                .entries
                .iter()
                .map(|entry| EntryRecord {
                    id: entry.id.clone(),
                    text: entry.text.clone(),
                    ours: ours.contains(&entry.id),
                })
                .collect(),
            thread_stamp: stamp + u64::from(foreign_touch),
            synced: true,
            sending,
        };
        self.records.insert_mut(seed.id.clone(), record);
    }

    /// Remove a record (announcing it to the live feed) and forget its
    /// card; the inlay itself is the application road's to drop.
    fn remove_in_place(&mut self, id: &AnnotationId) {
        let Some(record) = self.records.get(id) else {
            return;
        };
        if record.synced {
            self.announce.push(Announce::Removed(id.clone()));
        }
        self.records.remove_mut(id);
        self.cards.remove_mut(id);
        self.generation += 1;
    }

    /// Leave the card note on the row: a landing's document work
    /// (inlay mint and removal) runs behind the lease, so the work
    /// waits for `after_route`.
    fn note_card_work(&mut self, dead: Vec<(DocumentId, InlayKey)>) {
        self.work.dead.extend(dead);
        self.work.settle = true;
    }

    /// Attach the annotations feed of the wire that serves a folder —
    /// the collection is the caller's (the window's family), the seat
    /// and AHP session come off the folder's authority.
    /// A fresh comment record — unsynced; the wire lane announces it
    /// onto a live channel. `turn_id` is the provenance stamp the
    /// caller resolved (the driver knows the session's latest turn).
    pub fn created(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        location: &ResourceLocation,
        range: Range<LineCol>,
        turn_id: String,
    ) -> Option<AnnotationId> {
        if !Self::installed(store) {
            return None;
        }
        let id = Self::mint(store, comments);
        let entry_id = format!("{id}-e1");
        let record = CommentRecord {
            location: location.clone(),
            range: Some(range),
            turn_id,
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
        Self::update(store, comments, |comments| {
            comments.records.insert_mut(id.clone(), record.clone());
            comments.announce.push(Announce::Set(id.clone()));
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

    /// The send-to-agent turn answered — Err keeps the comments and
    /// clears the flags; Ok removes them (announcing the removals so
    /// the host forgets them too).
    pub fn sent_outcome(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        ids: &[AnnotationId],
        result: Result<(), String>,
    ) {
        Self::update(store, comments, |held| match &result {
            Err(error) => {
                eprintln!("[comments] send failed, comments kept: {error}");
                for id in ids {
                    let Some(mut record) = held.records.get(id).cloned() else {
                        continue;
                    };
                    record.sending = false;
                    held.records.insert_mut(id.clone(), record);
                }
            }
            Ok(()) => {
                let mut dead = Vec::new();
                for id in ids {
                    if let Some(card) = held.cards.get(id) {
                        dead.push(*card);
                    }
                    held.remove_in_place(id);
                }
                held.work.dead.extend(dead);
            }
        });
    }

    /// The wire lane's gates and drains: the model notes, the driver
    /// moves.
    pub(crate) fn owes_sync(store: &Store, comments: imba::store::Id<Comments>) -> bool {
        Self::of(store, comments)
            .is_some_and(|held| !held.announce.is_empty() || !held.work.is_empty())
    }

    pub(crate) fn take_announces(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
    ) -> Vec<Announce> {
        let Some(mut held) = store.entity::<Comments>(comments).cloned() else {
            return Vec::new();
        };
        let announces = std::mem::take(&mut held.announce);
        store.put_entity(comments, held);
        announces
    }

    pub(crate) fn take_card_work(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
    ) -> CardWork {
        let Some(mut held) = store.entity::<Comments>(comments).cloned() else {
            return CardWork::default();
        };
        let work = std::mem::take(&mut held.work);
        store.put_entity(comments, held);
        work
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
            Self::update(store, comments, |held| {
                held.announce.push(Announce::EntrySet {
                    id: id.clone(),
                    entry_id: own_id,
                });
            });
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
            Self::update(store, comments, |held| {
                held.announce.push(Announce::Meta(id.clone()));
            });
        }
    }
}

pub fn run_card_work(
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
    let records: Vec<(AnnotationId, CommentRecord)> = Comments::records(store, comments);
    for (id, record) in records {
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

/// The document observer, WIRED: minted by the family ceremony with
/// the cards' collection in hand, installed SCOPED to the family's
/// documents — it fires only for its own collection and dies with it
/// (docs/entities.md law 4).
pub struct CommentsHook {
    pub comments: imba::store::Id<Comments>,
}

impl crate::DocumentHook for CommentsHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<crate::OpenDocuments>,
        document: DocumentId,
        location: Option<&crate::ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        // Scoped install: this hook fires only for its own family's
        // documents, and the cards' collection is its own record.
        let comments = self.comments;
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
        _documents: imba::store::Id<crate::OpenDocuments>,
        document: DocumentId,
        _location: Option<&crate::ResourceLocation>,
        doc: &crate::Document,
    ) {
        let comments = self.comments;
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
