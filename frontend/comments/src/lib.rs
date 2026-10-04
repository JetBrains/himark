// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The comments COLLECTION (docs/entities.md): a session's review
//! annotations, addressed by `(Id<Comments>, AnnotationId)`. Pure
//! model — records fold MIRRORED seeds and deltas (the wire driver
//! digests the protocol shape), mutations leave ANNOUNCE and
//! card-work notes for the driver's lane, and the cards' inlay work
//! itself lives with the workbench.

use std::ops::Range;

use documents::{DocumentId, LineCol, OpenDocuments};
use editor::{InlayKey, ResourceLocation};
use imba::store::Store;
use text::Text;

pub mod cards;
pub mod panel;
pub mod view;

/// The feature gate — app configuration, owned by the feature:
/// hosts that serve no annotations never install it. Live state, so
/// an install after a session minted still takes effect.
#[derive(Clone, Default)]
struct Gate(bool);

pub fn install(store: &mut Store) {
    store.put(Gate(true));
}

pub fn installed(store: &Store) -> bool {
    store.get::<Gate>().is_some_and(|gate| gate.0)
}

pub type AnnotationId = String;

#[derive(Clone)]
pub struct EntryRecord {
    pub id: String,

    pub text: Text,

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

    pub fn foreign_texts(&self) -> Vec<Text> {
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
        text: Text,
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
    /// The documents the cards live in — wired at the session mint
    /// (docs/entities.md law 4).
    documents: imba::store::Id<OpenDocuments>,

    records: rpds::HashTrieMapSync<AnnotationId, CommentRecord>,

    /// The session folders the dock groups by — stamped by the wire
    /// driver as it attaches them; the panel never consults a
    /// session.
    folders: rpds::VectorSync<ResourceLocation>,

    /// Send-to-agent intents the faces NOTED — the wire lane drains
    /// them; no face carries a wire or a window.
    send_asks: Vec<Vec<AnnotationId>>,

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
/// the batch-tail lane; the entity holds no document-addressed
/// effects of its own.
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
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.cards.is_empty()
    }

    /// A collection wired to the documents its cards live in — minted
    /// by the session ceremony.
    pub fn wired(documents: imba::store::Id<OpenDocuments>) -> Self {
        Self {
            documents,
            records: rpds::HashTrieMapSync::new_sync(),
            folders: rpds::VectorSync::new_sync(),
            send_asks: Vec::new(),
            cards: rpds::HashTrieMapSync::new_sync(),
            work: CardWork::default(),
            announce: Vec::new(),
            generation: 0,
            minted: 0,
        }
    }

    pub fn documents(&self) -> imba::store::Id<OpenDocuments> {
        self.documents
    }

    /// The collection by its id — `None` is gone.
    fn of(store: &Store, comments: imba::store::Id<Comments>) -> Option<&Comments> {
        store.entity(comments)
    }

    /// The documents collection this one's cards live in.
    pub fn documents_of(
        store: &Store,
        comments: imba::store::Id<Comments>,
    ) -> Option<imba::store::Id<OpenDocuments>> {
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

    /// Whether any record placed at `location` still lacks its card —
    /// the document hook's materialize cue.
    pub fn owes_cards_at(
        store: &Store,
        comments: imba::store::Id<Comments>,
        location: &ResourceLocation,
    ) -> bool {
        Self::of(store, comments).is_some_and(|comments| {
            comments.records.iter().any(|(id, record)| {
                record.location == *location && !comments.cards.contains_key(id)
            })
        })
    }

    /// Every card living in one document — the closing hook's sweep.
    pub fn cards_in(
        store: &Store,
        comments: imba::store::Id<Comments>,
        document: DocumentId,
    ) -> Vec<(AnnotationId, InlayKey)> {
        Self::of(store, comments)
            .map(|comments| {
                comments
                    .cards
                    .iter()
                    .filter(|(_, (held, _))| *held == document)
                    .map(|(id, (_, key))| (id.clone(), *key))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The card's document CLOSED: forget the card (the inlay died
    /// with the document); the record stays.
    pub fn card_closed(store: &mut Store, comments: imba::store::Id<Comments>, id: &AnnotationId) {
        let id = id.clone();
        Self::update(store, comments, |comments| {
            comments.cards.remove_mut(&id);
        });
    }

    pub fn update_record(
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

    /// Land a snapshot's seeds — the driver digested the wire; this
    /// is value folding (existing records keep their location, their
    /// ours-marks and their send flags).
    pub fn land_seeds(
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
    pub fn fold_deltas(
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
    /// waits for the batch-tail lane.
    fn note_card_work(&mut self, dead: Vec<(DocumentId, InlayKey)>) {
        self.work.dead.extend(dead);
        self.work.settle = true;
    }

    /// A fresh comment record — unsynced; the wire lane announces it
    /// onto a live channel. `turn_id` is the provenance stamp the
    /// caller resolved (the driver knows the session's latest turn).
    pub fn created(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        location: &ResourceLocation,
        range: Range<LineCol>,
        turn_id: String,
    ) -> AnnotationId {
        let id = Self::mint(store, comments);
        let entry_id = format!("{id}-e1");
        let record = CommentRecord {
            location: location.clone(),
            range: Some(range),
            turn_id,
            resolved: false,
            entries: rpds::VectorSync::new_sync().push_back(EntryRecord {
                id: entry_id,
                text: Text::from_string_exact(""),
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
        id
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

    /// The session folders the dock groups by.
    pub fn folders(store: &Store, comments: imba::store::Id<Comments>) -> Vec<ResourceLocation> {
        Self::of(store, comments)
            .map(|held| held.folders.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The driver's stamp: adopt a folder not yet held, keeping
    /// attach order.
    pub fn adopt_folder(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        folder: &ResourceLocation,
    ) {
        let known = Self::of(store, comments)
            .is_some_and(|held| held.folders.iter().any(|known| known == folder));
        if known {
            return;
        }
        Self::update(store, comments, |held| {
            held.folders.push_back_mut(folder.clone());
            held.generation += 1;
        });
    }

    /// Note a send-to-agent ask for the wire lane.
    pub fn ask_send(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
        ids: Vec<AnnotationId>,
    ) {
        if ids.is_empty() {
            return;
        }
        Self::update(store, comments, |held| held.send_asks.push(ids));
    }

    pub fn take_send_asks(
        store: &mut Store,
        comments: imba::store::Id<Comments>,
    ) -> Vec<Vec<AnnotationId>> {
        let Some(mut held) = store.entity::<Comments>(comments).cloned() else {
            return Vec::new();
        };
        let asks = std::mem::take(&mut held.send_asks);
        store.put_entity(comments, held);
        asks
    }

    /// The wire lane's gates and drains: the model notes, the driver
    /// moves.
    pub fn owes_sync(store: &Store, comments: imba::store::Id<Comments>) -> bool {
        Self::of(store, comments).is_some_and(|held| {
            !held.announce.is_empty() || !held.work.is_empty() || !held.send_asks.is_empty()
        })
    }

    /// Any record's location — the lane's self-ensure resolves the
    /// channel's seat from it when announces wait with no channel.
    pub fn any_location(
        store: &Store,
        comments: imba::store::Id<Comments>,
    ) -> Option<ResourceLocation> {
        Self::of(store, comments)?
            .records
            .iter()
            .next()
            .map(|(_, record)| record.location.clone())
    }

    pub fn take_announces(store: &mut Store, comments: imba::store::Id<Comments>) -> Vec<Announce> {
        let Some(mut held) = store.entity::<Comments>(comments).cloned() else {
            return Vec::new();
        };
        let announces = std::mem::take(&mut held.announce);
        store.put_entity(comments, held);
        announces
    }

    pub fn take_card_work(store: &mut Store, comments: imba::store::Id<Comments>) -> CardWork {
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
        text: Text,
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
