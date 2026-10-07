// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diagnostics WIRE driver (docs/ahp/ahp-lsp.md §6): language
//! servers PUSH diagnostics, so they are state and ride a session
//! channel — one server, any number of protocol clients. The driver
//! owns the channel (dial, snapshot, poll, replacement reducer) and
//! lands per-resource squiggle markups into the session's open
//! documents through the wholesale `replace_markup` door. Dirty
//! resources drain at the batch tail (`sync`) — the comments-lane
//! pattern: a clean wire costs a map read.

use std::sync::Arc;

use ahp_wire::client::SessionUri;
use documents::text_ext::LineCol;
use documents::OpenDocuments;
use editor::location::ResourceLocation;
use editor::markup::{Markup, MarkupId};
use editor::theme::StyleId;
use himark_ahp_ext_types::lsp::PublishedDiagnostics;
use imba::command::{Fx, Verb};
use imba::effect::AnyEffect;
use imba::store::Store;

/// The one feature-markup id diagnostics own on every document —
/// Document-scoped: squiggles show on every editor of the document.
pub fn diagnostics_markup() -> MarkupId {
    static ID: std::sync::OnceLock<MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(MarkupId::mint)
}

/// `HIMARK_TRACE_LSP=1`: the driver narrates its road on stderr —
/// the `HIMARK_TRACE_DIFF` precedent.
fn trace(line: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HIMARK_TRACE_LSP").is_some()) {
        eprintln!("[lsp] {}", line());
    }
}

/// The one channel a session's diagnostics ride.
#[derive(Clone)]
struct ChannelWire {
    server: ahp_wire::client::HostId,
    session: SessionUri,
    client: ahp_wire::client::Client,

    /// The minted diagnostics channel, once `lsp/diagnostics`
    /// answered; `live` once the snapshot landed.
    channel: Option<ahp_wire::client::ChannelUri>,
    live: bool,

    /// The location the dial was asked from — a failed dial re-arms
    /// `pending` with it, so the lane re-dials after the backoff.
    origin: ResourceLocation,
}

/// How long a failed dial (the host down, the session not yet
/// materialized, no `lsp/diagnostics` served) rests before the lane
/// re-asks. Per batch tail otherwise: a dead host would be dialed on
/// every keystroke.
const REDIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone)]
pub struct DiagnosticsWire {
    documents: imba::store::Id<OpenDocuments>,
    uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
    channel: Option<ChannelWire>,

    /// The channel state, replacement-reduced: resource URI → last
    /// published diagnostics. Resources without diagnostics are
    /// absent, mirroring LSP publishDiagnostics.
    items: rpds::HashTrieMapSync<String, PublishedDiagnostics>,

    /// Resources whose entry changed (or whose document just opened)
    /// since the last apply — drained by the batch-tail lane.
    dirty: rpds::HashTrieSetSync<String>,

    /// A dial that could not complete yet (the local host not yet
    /// designated, a dial that failed): the batch-tail lane re-asks
    /// from here until the channel stands. Documents restoring at
    /// startup open BEFORE the host connects — without the retry the
    /// session never dials.
    pending: Option<ResourceLocation>,

    /// The earliest instant the lane re-dials a failed ask.
    retry_at: Option<std::time::Instant>,
}

impl DiagnosticsWire {
    pub fn wired(
        documents: imba::store::Id<OpenDocuments>,
        uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
    ) -> Self {
        Self {
            documents,
            uris,
            channel: None,
            items: rpds::HashTrieMapSync::new_sync(),
            dirty: rpds::HashTrieSetSync::new_sync(),
            pending: None,
            retry_at: None,
        }
    }

    pub fn stamp_uris(
        store: &mut Store,
        wire: imba::store::Id<DiagnosticsWire>,
        uris: &Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) {
        update(store, wire, |row| row.uris = Some(Arc::clone(uris)));
    }

    pub fn is_empty(&self) -> bool {
        self.channel.is_none() && self.items.is_empty()
    }
}

fn of(store: &Store, wire: imba::store::Id<DiagnosticsWire>) -> Option<&DiagnosticsWire> {
    store.entity(wire)
}

/// Mutate in place; a gone driver takes no write.
fn update(
    store: &mut Store,
    wire: imba::store::Id<DiagnosticsWire>,
    mutate: impl FnOnce(&mut DiagnosticsWire),
) {
    let Some(mut row) = store.entity::<DiagnosticsWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// Dial the diagnostics channel of the wire serving a location's
/// authority — idempotent while a channel for the session stands.
pub fn ensure(
    store: &mut Store,
    wire: imba::store::Id<DiagnosticsWire>,
    location: &ResourceLocation,
    fx: &mut Fx<'_>,
) {
    let authority = location.authority().as_str();
    let Some((server, client, session)) = ahp_wire::client::route_client(store, authority) else {
        // No wire serves the authority yet. Only `local` is worth
        // holding — it routes once the local host is designated; a
        // scratch or foreign authority never will, and holding it
        // would re-ask on every batch tail for good.
        if authority == "local" {
            trace(|| format!("ensure: no route for {authority:?} yet; pending"));
            let location = location.clone();
            update(store, wire, |row| row.pending = Some(location));
        }
        return;
    };
    let known = of(store, wire)
        .and_then(|row| row.channel.as_ref())
        .is_some_and(|held| held.session == session);
    if known {
        return;
    }
    trace(|| format!("ensure: dialing lsp/diagnostics for {}", session.as_str()));
    let origin = location.clone();
    update(store, wire, |row| {
        row.pending = None;
        row.retry_at = None;
        row.channel = Some(ChannelWire {
            server,
            session: session.clone(),
            client: client.clone(),
            channel: None,
            live: false,
            origin,
        });
    });
    fx.push(
        AnyEffect::new(ahp_wire::effects::LspDiagnosticsChannelEffect {
            client: client.lsp.clone(),
            session: session.clone(),
        })
        .map(move |result| {
            Verb::Dynamic(Arc::new(ChannelLanded {
                wire,
                session: session.clone(),
                result,
            }))
        }),
    );
}

/// The document-open half: a document opening under diagnostics the
/// wire already holds gets dressed by the next lane run; a session
/// with no channel yet dials from the document's own authority.
pub struct DiagnosticsHook {
    pub wire: imba::store::Id<DiagnosticsWire>,
}

impl documents::DocumentHook for DiagnosticsHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<OpenDocuments>,
        _document: documents::DocumentId,
        location: Option<&ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        let wire = self.wire;
        let Some(row) = of(store, wire) else {
            return;
        };
        if row.channel.is_none() {
            let location = location.clone();
            imba::command::Requests::push(store, Arc::new(EnsureDiagnostics { wire, location }));
            return;
        }
        let Some(uris) = row.uris.clone() else {
            return;
        };
        let uri = uris.uri_of(location);
        if row.items.contains_key(uri.as_str()) {
            let uri = uri.as_str().to_owned();
            update(store, wire, |row| {
                row.dirty.insert_mut(uri);
            });
        }
    }

    fn closing(
        &self,
        _store: &mut Store,
        _documents: imba::store::Id<OpenDocuments>,
        _document: documents::DocumentId,
        _location: Option<&ResourceLocation>,
        _doc: &editor::document::Document,
    ) {
        // The markup rides the document; it leaves with it.
    }
}

struct EnsureDiagnostics {
    wire: imba::store::Id<DiagnosticsWire>,
    location: ResourceLocation,
}

impl imba::command::DynamicCommand for EnsureDiagnostics {
    fn id(&self) -> &'static str {
        "lsp.diagnostics.ensure"
    }
    fn name(&self) -> String {
        "Ensure Diagnostics Channel".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        ensure(store, self.wire, &self.location, fx);
    }
}

struct ChannelLanded {
    wire: imba::store::Id<DiagnosticsWire>,
    session: SessionUri,
    result: Result<ahp_wire::client::ChannelUri, String>,
}

impl imba::command::DynamicCommand for ChannelLanded {
    fn id(&self) -> &'static str {
        "lsp.diagnostics.channel"
    }
    fn name(&self) -> String {
        "Diagnostics Channel".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let held = of(store, self.wire)
            .and_then(|row| row.channel.clone())
            .filter(|held| held.session == self.session);
        let Some(held) = held else {
            trace(|| {
                format!(
                    "channel landed for {} but no wire holds it",
                    self.session.as_str()
                )
            });
            return;
        };
        trace(|| format!("channel landed: {:?}", self.result));
        match &self.result {
            // The host serves no diagnostics, or the dial failed (the
            // connection down, the session not materialized yet):
            // drop the channel and re-arm the ask — the lane re-dials
            // after the backoff.
            Err(_) => redial_later(store, self.wire),
            Ok(channel) => {
                let channel = channel.clone();
                update(store, self.wire, |row| {
                    if let Some(held) = &mut row.channel {
                        held.channel = Some(channel.clone());
                    }
                });
                let wire = self.wire;
                let session = self.session.clone();
                fx.push(
                    AnyEffect::new(ahp_wire::effects::SubscribeLspDiagnosticsEffect {
                        client: held.client.lsp.clone(),
                        channel,
                    })
                    .map(move |result| {
                        Verb::Dynamic(Arc::new(SnapshotLanded {
                            wire,
                            session: session.clone(),
                            result,
                        }))
                    }),
                );
            }
        }
    }
}

struct SnapshotLanded {
    wire: imba::store::Id<DiagnosticsWire>,
    session: SessionUri,
    result: Result<himark_ahp_ext_types::lsp::DiagnosticsState, String>,
}

impl imba::command::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "lsp.diagnostics.snapshot"
    }
    fn name(&self) -> String {
        "Diagnostics Snapshot".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let held = of(store, self.wire)
            .and_then(|row| row.channel.clone())
            .filter(|held| held.session == self.session);
        if held.is_none() {
            trace(|| "snapshot landed but no wire holds the channel".to_owned());
            return;
        }
        trace(|| match &self.result {
            Ok(state) => format!("snapshot landed: {} resources", state.items.len()),
            Err(error) => format!("snapshot failed: {error}"),
        });
        match &self.result {
            Err(_) => redial_later(store, self.wire),
            Ok(state) => {
                let fresh = state.items.clone();
                update(store, self.wire, |row| {
                    // Every resource the old or new state names is
                    // dirty: entries the snapshot dropped must sweep
                    // their standing squiggles too.
                    for uri in row.items.keys() {
                        row.dirty.insert_mut(uri.clone());
                    }
                    let mut items = rpds::HashTrieMapSync::new_sync();
                    for (uri, published) in &fresh {
                        row.dirty.insert_mut(uri.clone());
                        items.insert_mut(uri.clone(), published.clone());
                    }
                    row.items = items;
                    if let Some(held) = &mut row.channel {
                        held.live = true;
                    }
                });
                relaunch_poll(store, self.wire, fx);
            }
        }
    }
}

/// Drop a failed channel and hold its origin for a re-dial once
/// `REDIAL_BACKOFF` has passed.
fn redial_later(store: &mut Store, wire: imba::store::Id<DiagnosticsWire>) {
    update(store, wire, |row| {
        if let Some(held) = row.channel.take() {
            row.pending = Some(held.origin);
            row.retry_at = Some(std::time::Instant::now() + REDIAL_BACKOFF);
        }
    });
}

struct Polled {
    wire: imba::store::Id<DiagnosticsWire>,
    session: SessionUri,
    published: Vec<himark_ahp_ext_types::lsp::DiagnosticsPublished>,
}

impl imba::command::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "lsp.diagnostics.polled"
    }
    fn name(&self) -> String {
        "Diagnostics Update".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let standing = of(store, self.wire)
            .and_then(|row| row.channel.as_ref())
            .is_some_and(|held| held.session == self.session);
        if !standing {
            trace(|| "poll landed but no wire holds the channel".to_owned());
            return;
        }
        trace(|| {
            let uris: Vec<String> = self
                .published
                .iter()
                .map(|action| format!("{} ({})", action.uri, action.diagnostics.len()))
                .collect();
            format!("polled {} publishes: {uris:?}", self.published.len())
        });
        update(store, self.wire, |row| {
            // The replacement reducer (ahp-lsp.md §6.3): the entry
            // swaps whole; empty diagnostics remove it.
            for action in &self.published {
                if action.diagnostics.is_empty() {
                    row.items.remove_mut(&action.uri);
                } else {
                    row.items.insert_mut(
                        action.uri.clone(),
                        PublishedDiagnostics {
                            version: action.version.clone(),
                            diagnostics: action.diagnostics.clone(),
                        },
                    );
                }
                row.dirty.insert_mut(action.uri.clone());
            }
        });
        relaunch_poll(store, self.wire, fx);
    }
}

fn relaunch_poll(store: &Store, wire: imba::store::Id<DiagnosticsWire>, fx: &mut Fx<'_>) {
    let Some(held) = of(store, wire).and_then(|row| row.channel.clone()) else {
        return;
    };
    let Some(channel) = held.channel else {
        return;
    };
    let session = held.session.clone();
    fx.push(
        AnyEffect::new(ahp_wire::effects::PollLspDiagnosticsEffect {
            client: held.client.lsp.clone(),
            channel,
        })
        .map(move |published| {
            Verb::Dynamic(Arc::new(Polled {
                wire,
                session: session.clone(),
                published,
            }))
        }),
    );
}

/// The batch-tail diagnostics lane: drain the dirty resources into
/// their open documents' squiggle markups. A clean wire costs a map
/// read.
pub fn sync(
    store: &mut Store,
    wire: imba::store::Id<DiagnosticsWire>,
    ui: &imba::ui::UiCtx,
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    // The held-back dial: a document asked before its wire could
    // serve it, or a dial that failed; retry until the channel stands,
    // a failed one only once its backoff has passed.
    if row.channel.is_none() {
        let due = row
            .retry_at
            .map_or(true, |at| std::time::Instant::now() >= at);
        if let Some(location) = row.pending.clone().filter(|_| due) {
            ensure(store, wire, &location, fx);
        }
    }
    let Some(row) = of(store, wire) else {
        return;
    };
    if row.dirty.is_empty() {
        return;
    }
    let row = row.clone();
    trace(|| format!("sync: {} dirty resources", row.dirty.size()));
    update(store, wire, |row| {
        row.dirty = rpds::HashTrieSetSync::new_sync();
    });
    for uri in row.dirty.iter() {
        apply(store, ui, &row, uri, fx);
    }
}

/// Land one resource's diagnostics into its open document: positions
/// convert against the CURRENT text (utf-8 line/character, ahp-lsp.md
/// §4), the markup swaps wholesale with the producer-brought change
/// set. A publish computed against older text lands slightly off for
/// one publish cycle; the standing markup meanwhile shifts correctly
/// at the edit door.
fn apply(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    row: &DiagnosticsWire,
    uri: &str,
    fx: &mut Fx<'_>,
) {
    let Some(held) = &row.channel else {
        trace(|| format!("apply {uri}: no channel"));
        return;
    };
    let Some(uris) = &row.uris else {
        trace(|| format!("apply {uri}: no uri map"));
        return;
    };
    let authority = ahp_wire::client::route_authority(held.server, &held.session);
    let Some(location) = uris.location_of(
        &ahp_wire::client::ResourceUri::new(uri.to_owned()),
        editor::location::ResourceType::document(),
        &authority,
    ) else {
        trace(|| format!("apply {uri}: no location"));
        return;
    };
    let Some(document_id) = OpenDocuments::by_location(store, row.documents, &location) else {
        trace(|| {
            let open: Vec<String> = OpenDocuments::list(store, row.documents)
                .iter()
                .filter_map(|(_, entry)| entry.location())
                .map(|location| format!("{location:?}"))
                .collect();
            format!("apply {uri}: not open as {location:?}; open: {open:?}")
        });
        return;
    };
    let Some(mut document) = OpenDocuments::document(store, row.documents, document_id) else {
        trace(|| format!("apply {uri}: document {document_id:?} gone"));
        return;
    };

    let replacement = squiggle_markup(&document, row.items.get(uri));
    trace(|| {
        format!(
            "apply {uri}: {} diagnostics -> markup empty={}",
            row.items
                .get(uri)
                .map_or(0, |published| published.diagnostics.len()),
            replacement.is_empty()
        )
    });
    let markup = diagnostics_markup();
    document.ensure_document_markup(markup);
    let changed = editor::markup::set_diff(document.feature_markup(markup), &replacement);

    let fonts = editor::env::ui_collection(store, ui);
    let theme = editor::env::Themes::of(store);
    let documents_id = row.documents;
    fx.scope(
        move |command| {
            Verb::at(
                documents_id,
                documents::DocumentsCommand::Editor(document_id, command),
            )
        },
        |fx| {
            document.replace_markup(markup, replacement, &changed, store, ui, &fonts, &theme, fx);
        },
    );
    OpenDocuments::put_document(store, row.documents, document_id, document);
}

/// One resource's squiggles against the CURRENT text: a severity-
/// styled span per diagnostic, positions converted from utf-8
/// line/character (ahp-lsp.md §4) and clamped.
fn squiggle_markup(
    document: &editor::document::Document,
    published: Option<&PublishedDiagnostics>,
) -> Markup {
    let mut builder = Markup::builder();
    if let Some(published) = published {
        let mut view = document.text().view();
        let len = view.byte_count();
        for diagnostic in &published.diagnostics {
            let Some((start, end)) = lsp_range(diagnostic) else {
                continue;
            };
            let start = documents::text_ext::offset_at(&mut view, start).min(len);
            let mut end = documents::text_ext::offset_at(&mut view, end).min(len);
            // An empty range still squiggles SOMETHING: widen to the
            // next character, the LSP clients' convention.
            while end <= start && end < len {
                end += 1;
                while end < len && !view.is_char_boundary(end) {
                    end += 1;
                }
            }
            if start >= end {
                continue;
            }
            builder.push_styled(start as u32..end as u32, severity_style(diagnostic));
        }
    }
    builder.finish()
}

fn lsp_range(diagnostic: &serde_json::Value) -> Option<(LineCol, LineCol)> {
    let position = |value: &serde_json::Value| -> Option<LineCol> {
        Some(LineCol {
            line: value["line"].as_u64()? as u32,
            col: value["character"].as_u64()? as u32,
        })
    };
    let range = &diagnostic["range"];
    Some((position(&range["start"])?, position(&range["end"])?))
}

fn severity_style(diagnostic: &serde_json::Value) -> StyleId {
    match diagnostic["severity"].as_u64() {
        Some(2) => StyleId::DiagnosticWarning,
        Some(3) => StyleId::DiagnosticInfo,
        Some(4) => StyleId::DiagnosticHint,
        // 1 and the LSP default both read as errors.
        _ => StyleId::DiagnosticError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(range: ((u64, u64), (u64, u64)), severity: Option<u64>) -> serde_json::Value {
        let mut value = serde_json::json!({
            "range": {
                "start": { "line": range.0 .0, "character": range.0 .1 },
                "end": { "line": range.1 .0, "character": range.1 .1 },
            },
            "message": "boom",
        });
        if let Some(severity) = severity {
            value["severity"] = severity.into();
        }
        value
    }

    fn spans(markup: &Markup, len: u32) -> Vec<(std::ops::Range<u32>, StyleId)> {
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        markup.marks_inline_hidden_in(0..len, &mut inline, &mut hidden);
        inline
            .into_iter()
            .map(|interval| (interval.range, interval.id))
            .collect()
    }

    #[test]
    fn squiggles_convert_positions_severities_and_clamp() {
        let text = text::text::Text::from_string_exact("fn main() {}\nlet x = 1;\n");
        let document = editor::document::Document::new(text, Markup::new());
        let published = PublishedDiagnostics {
            version: None,
            diagnostics: vec![
                // "x" on line 1, warning.
                diagnostic(((1, 4), (1, 5)), Some(2)),
                // Severity-less defaults to error: "main".
                diagnostic(((0, 3), (0, 7)), None),
                // Empty range widens to one character.
                diagnostic(((1, 8), (1, 8)), Some(4)),
                // Past the end clamps away entirely.
                diagnostic(((9, 0), (9, 3)), Some(1)),
            ],
        };
        let markup = squiggle_markup(&document, Some(&published));
        let line1 = 13u32; // "let x = 1;\n" starts after "fn main() {}\n"
        let mut found = spans(&markup, u32::MAX);
        found.sort_by_key(|(range, _)| range.start);
        assert_eq!(
            found,
            vec![
                (3..7, StyleId::DiagnosticError),
                (line1 + 4..line1 + 5, StyleId::DiagnosticWarning),
                (line1 + 8..line1 + 9, StyleId::DiagnosticHint),
            ]
        );
    }

    #[test]
    fn no_diagnostics_build_an_empty_markup() {
        let text = text::text::Text::from_string_exact("plain\n");
        let document = editor::document::Document::new(text, Markup::new());
        assert!(squiggle_markup(&document, None).is_empty());
    }
}
