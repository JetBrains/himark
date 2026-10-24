// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The contents drawer: the TOC (a dir/file forest over a location
//! set) and the document outline (the syntax items, derived
//! off-thread per revision stamp). Windowless — a pick files a
//! ModalRequest; the outline's jump rides the injected place
//! builder; the shell's toggle and toolbar button live in himark.

use std::sync::Arc;

mod chrome;
mod entry;

pub use entry::{ContentsEntry, ContentsRow, Place};

use imba::{
    arena::Arena,
    event::{Event, EventResult, Key as InputKey},
    layout::LayoutExt as _,
    store::Store,
    ui::UiCtx,
    View,
};
use skia_safe::Size;

use hikit::{
    forest::ForestList, forest::ForestNode, forest::ForestSearcher, list_keyboard::ListKeyCommand,
    list_keyboard::ListKeyboardController, modal::ModalRequest, modal::ModalView,
    navigation::EditorPlace, tree_item::tree_toggle, tree_item::TreeListCommand,
};
use imba::list::{ActivateTrigger, ListOps};

pub(crate) const OUTLINE_CAP: usize = 2_000;

pub(crate) type SearchListCommand = ListKeyCommand<TreeListCommand>;

#[derive(Clone)]
pub enum TocCommand {
    List(SearchListCommand),
    Dismiss,
}

impl std::fmt::Display for TocCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TocCommand::List(command) => command.fmt(out),
            TocCommand::Dismiss => out.write_str("toc dismiss"),
        }
    }
}

pub struct TocView {
    context: String,
    search: ListKeyboardController<ForestList<u64>, ForestSearcher<u64>>,

    targets: rpds::HashTrieMapSync<u64, editor::location::ResourceLocation>,
    request: Option<ModalRequest>,
}

impl Clone for TocView {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            search: self.search.clone(),
            targets: self.targets.clone(),

            request: None,
        }
    }
}

#[derive(Default)]
struct DirTrie {
    dirs: std::collections::BTreeMap<String, DirTrie>,
    files: Vec<editor::location::ResourceLocation>,
}

impl DirTrie {
    fn emit(
        mut self,
        mut label: String,
        next: &mut u64,
        targets: &mut rpds::HashTrieMapSync<u64, editor::location::ResourceLocation>,
    ) -> ForestNode<u64> {
        while self.files.is_empty() && self.dirs.len() == 1 {
            let (segment, child) = self.dirs.pop_first().expect("one child");
            label.push('/');
            label.push_str(&segment);
            self = child;
        }
        let key = *next;
        *next += 1;
        let mut children = Vec::new();
        for (segment, child) in std::mem::take(&mut self.dirs) {
            children.push(child.emit(segment, next, targets));
        }
        for file in self.files {
            let key = *next;
            *next += 1;
            children.push(ForestNode {
                key,
                label: file.name().to_owned(),
                pick: true,
                dim: false,
                trail: Vec::new(),
                tint: hikit::tree_item::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
            targets.insert_mut(key, file);
        }
        ForestNode {
            key,
            label,
            pick: false,
            dim: true,
            trail: Vec::new(),
            tint: hikit::tree_item::TreeTint::Label,
            action: None,
            children,
        }
    }
}

impl TocView {
    pub fn for_locations(
        store: &Store,
        ui: &UiCtx,
        locations: &[editor::location::ResourceLocation],
    ) -> Option<Self> {
        if locations.is_empty() {
            return None;
        }
        let mut sorted: Vec<&editor::location::ResourceLocation> = locations.iter().collect();
        sorted.sort_by_key(|location| location.path().to_vec());
        sorted.dedup_by(|a, b| a == b);

        let total = sorted.len();
        sorted.truncate(OUTLINE_CAP);
        let capped = total > sorted.len();

        let mut root = DirTrie::default();
        for location in sorted {
            let path = location.path();
            let mut level = &mut root;
            for segment in &path[..path.len().saturating_sub(1)] {
                level = level.dirs.entry(segment.clone()).or_default();
            }
            level.files.push(location.clone());
        }

        let mut next = 0u64;
        let mut targets = rpds::HashTrieMapSync::new_sync();
        let mut nodes = Vec::new();
        for (segment, child) in root.dirs {
            nodes.push(child.emit(segment, &mut next, &mut targets));
        }
        for file in root.files {
            let key = next;
            next += 1;
            nodes.push(ForestNode {
                key,
                label: file.name().to_owned(),
                pick: true,
                dim: false,
                trail: Vec::new(),
                tint: hikit::tree_item::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
            targets.insert_mut(key, file);
        }

        let mut forest = ForestList::new(store);
        forest.set(&nodes, store, ui);
        Some(Self {
            context: match (capped, total) {
                (true, n) => format!("first {} of {n} files", OUTLINE_CAP),
                (false, 1) => "1 file".to_owned(),
                (false, n) => format!("{n} files"),
            },
            search: ListKeyboardController::searchable(
                forest,
                ForestSearcher::default(),
                store,
                ui,
            )
            .with_folds(),
            targets,
            request: None,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.search.inner().forest.rows()
    }

    /// The search controller — the shell's tests drive the key
    /// table through it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn search(&self) -> &ListKeyboardController<ForestList<u64>, ForestSearcher<u64>> {
        &self.search
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn visible_rows(&self) -> usize {
        self.search.inner().list().len()
    }

    fn pick(&mut self, key: u64, store: &Store, ui: &UiCtx) {
        match self.targets.get(&key) {
            Some(location) => {
                self.request = Some(ModalRequest::OpenLocations(vec![location.clone()]));
            }

            None => self.search.inner_mut().toggle(&key, store, ui),
        }
    }
}

impl View for TocView {
    type Command = TocCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, TocCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.search.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::command(TocCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.search.focus_data(store, ui).map(TocCommand::List))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: TocCommand,
        fx: &mut imba::effect::Effects<'_, TocCommand>,
    ) {
        match command {
            TocCommand::List(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.search.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.search.inner().list().key_at(index).copied()
                            else {
                                return;
                            };
                            self.search.inner_mut().list_mut().select_only(key);
                            return self.search.inner_mut().toggle(&key, store, ui);
                        }
                    }
                    _ => {}
                }
                type Search = ListKeyboardController<ForestList<u64>, ForestSearcher<u64>>;
                if let Some((index, trigger)) = Search::activated(&command) {
                    if let Some(key) = self.search.inner().list().key_at(index).copied() {
                        let searching = self.search.searching();
                        self.pick(key, store, ui);
                        match trigger {
                            // The deliberate pick ends the search in
                            // the same stroke.
                            ActivateTrigger::Enter if searching => {
                                return self.perform(
                                    store,
                                    ui,
                                    TocCommand::List(ListKeyCommand::Clear),
                                    fx,
                                );
                            }
                            ActivateTrigger::Enter | ActivateTrigger::Click => {}
                        }
                    }
                }
                fx.scope(TocCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
            }
            TocCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let searching = self.search.searching();
        contents_column(
            arena,
            store,
            ui,
            self.context.clone(),
            imba::layout::LayoutBox::new(
                arena,
                self.search
                    .display(arena, store, ui)
                    .map_layout(TocCommand::List),
            ),
            move |_arena: &Arena, event: &Event<'_>, _size: Size| match event {
                Event::KeyDown {
                    key: InputKey::Escape,
                    ..
                } if !searching => EventResult::command(TocCommand::Dismiss),
                _ => EventResult::Ignored,
            },
        )
    }
}

impl ModalView for TocView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub type OutlineKey = (::editor::markup::SyntaxId, ::editor::markup::IntervalId);

#[derive(Clone, Debug)]
pub struct OutlineRow {
    pub syntax: ::editor::markup::SyntaxId,
    pub key: ::editor::markup::IntervalId,
    pub depth: u8,
    pub title: String,

    pub line: u32,
}

pub struct OutlineEffect {
    document: ::editor::document::Document,
    stamp: (u64, u64),
    /// Where the caret stood at launch — the entry holding it is the
    /// drawer's CURRENT one.
    caret: Option<u32>,
}

impl std::fmt::Display for OutlineEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "outline revision {}", self.stamp.0)
    }
}

impl imba::effect::Effect for OutlineEffect {
    type Result = OutlineRows;
}

/// TEST SUPPORT: no production caller outside this crate.
#[derive(Clone)]
#[doc(hidden)]
pub struct OutlineRows {
    pub rows: Vec<OutlineRow>,
    /// The drawer's reading of each row, parallel to `rows`.
    pub entries: Vec<ContentsEntry>,
    pub stamp: (u64, u64),
    pub caret: Option<u32>,
    /// The innermost row holding the caret.
    pub current: Option<OutlineKey>,
}

pub struct OutlineHandler;

impl imba::effect::EffectHandler<OutlineEffect> for OutlineHandler {
    async fn handle(&self, effect: OutlineEffect) -> OutlineRows {
        let items = effect.document.outline_items();
        let mut view = effect.document.text().view();
        let mut rows = Vec::with_capacity(items.len().min(OUTLINE_CAP));

        let mut enclosing: Vec<u32> = Vec::new();
        let mut current = None;
        for (syntax, key, range, item) in items {
            if rows.len() >= OUTLINE_CAP {
                break;
            }
            while enclosing.last().is_some_and(|end| *end <= range.start) {
                enclosing.pop();
            }
            let depth = enclosing.len().min(u8::MAX as usize) as u8;
            enclosing.push(range.end);
            // Items arrive in start order, so the LAST one holding the
            // caret is the innermost.
            if effect
                .caret
                .is_some_and(|caret| range.start <= caret && caret < range.end)
            {
                current = Some((syntax, key));
            }
            let line = documents::text_ext::line_col_at(&mut view, range.start as usize).line + 1;
            rows.push(OutlineRow {
                syntax,
                key,
                depth,
                title: item.title,
                line,
            });
        }
        let prose = effect
            .document
            .syntax()
            .is_some_and(|syntax| syntax.language == "markdown");
        let entries = read_entries(&rows, prose, current);
        OutlineRows {
            rows,
            entries,
            stamp: effect.stamp,
            caret: effect.caret,
            current,
        }
    }
}

/// How the drawer READS the outline: each title split into its kind
/// and name, top-level containers turned group heads, nesting into
/// member indent. Prose outlines (or titles that never name a kind)
/// are all name.
fn read_entries(rows: &[OutlineRow], prose: bool, current: Option<OutlineKey>) -> Vec<ContentsEntry> {
    let split: Vec<(Option<&str>, &str)> = rows
        .iter()
        .map(|row| match prose {
            true => (None, row.title.trim()),
            false => entry::split_kind(&row.title),
        })
        .collect();
    let prose = prose || split.iter().all(|(kind, _)| kind.is_none());
    let mut entries = Vec::with_capacity(rows.len());
    let mut grouped = false;
    for (index, row) in rows.iter().enumerate() {
        let (kind, name) = match prose {
            true => (None, row.title.trim()),
            false => split[index],
        };
        let holds = rows
            .get(index + 1)
            .is_some_and(|next| next.depth > row.depth);
        if row.depth == 0 {
            grouped = holds && kind.map_or(prose, entry::is_group_kind);
        }
        let place = match row.depth == 0 && grouped {
            true => Place::Group { first: index == 0 },
            false => Place::Entry {
                strong: kind.is_some_and(entry::is_type_kind),
                indent: row.depth.saturating_sub(u8::from(grouped)),
            },
        };
        entries.push(ContentsEntry {
            kind: kind.map(str::to_owned),
            name: name.to_owned(),
            line: row.line,
            place,
            current: current == Some((row.syntax, row.key)),
            prose,
        });
    }
    entries
}

/// TEST SUPPORT: no production caller outside this crate.
#[derive(Clone)]
#[doc(hidden)]
pub enum OutlineCommand {
    List(SearchListCommand),
    Dismiss,

    Refresh,

    Landed(OutlineRows),
}

impl std::fmt::Display for OutlineCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutlineCommand::List(command) => command.fmt(out),
            OutlineCommand::Dismiss => out.write_str("outline dismiss"),
            OutlineCommand::Refresh => out.write_str("outline refresh"),
            OutlineCommand::Landed(_) => out.write_str("outline landed"),
        }
    }
}

/// The outline's keyboard list: a forest of `ContentsRow`s.
pub type OutlineSearch = ListKeyboardController<
    ForestList<OutlineKey, ContentsRow>,
    ForestSearcher<OutlineKey, ContentsRow>,
>;

/// What a derivation was taken against: the document's stamp and
/// where the caret stood.
type Derivation = ((u64, u64), Option<u32>);

pub struct OutlineView {
    /// The jump-to-place request builder, injected at mount — the
    /// shell's window rides in the closure; the drawer never holds
    /// one.
    jump: Arc<dyn Fn(EditorPlace) -> ModalRequest + Send + Sync>,
    documents: imba::store::Id<documents::OpenDocuments>,
    document: documents::DocumentId,
    location: editor::location::ResourceLocation,
    rows: Vec<OutlineRow>,

    derived: Option<Derivation>,

    launched: Option<Derivation>,
    lane: Option<imba::effect::CancellationToken>,
    search: OutlineSearch,
    request: Option<ModalRequest>,
}

impl Clone for OutlineView {
    fn clone(&self) -> Self {
        Self {
            jump: self.jump.clone(),
            documents: self.documents,
            document: self.document,
            location: self.location.clone(),
            rows: self.rows.clone(),
            derived: self.derived,
            launched: self.launched,
            lane: self.lane,
            search: self.search.clone(),
            request: None,
        }
    }
}

/// The forest's row dresser for one landing: each key reads its
/// entry; a key the landing never saw (none should) falls back to
/// its bare title.
fn contents_rows(
    entries: Arc<std::collections::HashMap<OutlineKey, ContentsEntry>>,
) -> hikit::forest::Wrap<OutlineKey, ContentsRow> {
    Arc::new(move |row, key| {
        let entry = entries.get(key).cloned().unwrap_or_else(|| ContentsEntry {
            kind: None,
            name: row.inner().text().to_owned(),
            line: 0,
            place: Place::Entry {
                strong: false,
                indent: row.depth().min(u16::from(u8::MAX)) as u8,
            },
            current: false,
            prose: true,
        });
        ContentsRow::new(entry)
    })
}

impl OutlineView {
    pub fn new(
        store: &Store,
        ui: &imba::ui::UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        document: documents::DocumentId,
        location: editor::location::ResourceLocation,
        jump: Arc<dyn Fn(EditorPlace) -> ModalRequest + Send + Sync>,
    ) -> Self {
        Self {
            jump,
            documents,
            document,
            location,
            rows: Vec::new(),
            derived: None,
            launched: None,
            lane: None,
            search: ListKeyboardController::searchable(
                ForestList::wrapping(store, contents_rows(Arc::default())),
                ForestSearcher::default(),
                store,
                ui,
            )
            .with_folds(),
            request: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String)> {
        self.rows
            .iter()
            .map(|row| (row.depth, row.title.clone()))
            .collect()
    }

    /// The search controller — the shell's tests drive the key
    /// table through it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn search(&self) -> &OutlineSearch {
        &self.search
    }

    /// TEST SUPPORT: the entries the drawer shows, in list order.
    #[doc(hidden)]
    pub fn entries(&self) -> Vec<ContentsEntry> {
        self.search
            .inner()
            .list()
            .rows()
            .map(|row| row.entry().clone())
            .collect()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn visible_rows(&self) -> usize {
        self.search.inner().list().len()
    }

    pub fn match_count(&self) -> usize {
        ListOps::match_count(self.search.inner().list())
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn cursor_title(&self) -> Option<String> {
        let key = self.search.inner().list().cursor()?;
        self.rows
            .iter()
            .find(|row| (row.syntax, row.key) == *key)
            .map(|row| row.title.clone())
    }

    /// The document's stamp and its caret — what a derivation is
    /// taken against. The caret is the first editor's (a document
    /// shown twice has no one "here").
    fn derivation_of(document: &::editor::document::Document) -> Derivation {
        let caret = document
            .editor_ids()
            .next()
            .map(|editor| document.caret_byte(editor));
        (
            (document.revision(), document.markup_generation()),
            caret,
        )
    }

    fn relaunch(&mut self, store: &Store, fx: &mut imba::effect::Effects<'_, OutlineCommand>) {
        let Some(document) =
            documents::OpenDocuments::document_ref(store, self.documents, self.document)
        else {
            return;
        };
        let derivation = Self::derivation_of(document);
        if self.launched == Some(derivation) || self.derived == Some(derivation) {
            return;
        }
        self.launched = Some(derivation);
        let effect = OutlineEffect {
            document: document.substance(),
            stamp: derivation.0,
            caret: derivation.1,
        };
        fx.relaunch_erased(
            &mut self.lane,
            imba::effect::AnyEffect::new(effect).map(OutlineCommand::Landed),
        );
    }

    fn pick(&mut self, store: &Store, key: OutlineKey) {
        let Some(range) =
            documents::OpenDocuments::document_ref(store, self.documents, self.document)
                .and_then(|document| document.resolve_outline(key.0, key.1))
        else {
            return;
        };
        self.request = Some((self.jump)(EditorPlace {
            location: self.location.clone(),
            caret: range.start,
            scroll_y: 0.0,
        }));
    }

    /// Land a derivation: the rows nest into the forest, dressed by
    /// THIS landing's entries — the list (cursor, matches, scroll)
    /// and the folds carry over. The first landing puts the cursor
    /// on the entry the caret is in.
    fn land(&mut self, landed: OutlineRows, store: &Store, ui: &UiCtx) {
        let first = self.derived.is_none();
        self.derived = Some((landed.stamp, landed.caret));
        self.lane = None;
        let entries: std::collections::HashMap<OutlineKey, ContentsEntry> = landed
            .rows
            .iter()
            .zip(landed.entries)
            .map(|(row, entry)| ((row.syntax, row.key), entry))
            .collect();
        self.rows = landed.rows;

        let mut nodes: Vec<ForestNode<OutlineKey>> = Vec::new();
        let mut stack: Vec<(u8, ForestNode<OutlineKey>)> = Vec::new();
        let settle = |stack: &mut Vec<(u8, ForestNode<OutlineKey>)>,
                      nodes: &mut Vec<ForestNode<OutlineKey>>,
                      depth: u8| {
            while stack.last().is_some_and(|(d, _)| *d >= depth) {
                let (_, done) = stack.pop().expect("standing");
                match stack.last_mut() {
                    Some((_, parent)) => parent.children.push(done),
                    None => nodes.push(done),
                }
            }
        };
        for row in &self.rows {
            settle(&mut stack, &mut nodes, row.depth);
            stack.push((
                row.depth,
                ForestNode {
                    key: (row.syntax, row.key),
                    label: row.title.clone(),
                    pick: true,
                    dim: false,
                    trail: Vec::new(),
                    tint: hikit::tree_item::TreeTint::Label,
                    action: None,
                    children: Vec::new(),
                },
            ));
        }
        settle(&mut stack, &mut nodes, 0);

        // The dresser is per landing; the standing list and forest
        // move into the freshly dressed one.
        let standing = self.search.inner_mut();
        let mut dressed = ForestList::wrapping(store, contents_rows(Arc::new(entries)));
        std::mem::swap(dressed.list_mut(), standing.list_mut());
        dressed.forest = standing.forest.clone();
        *standing = dressed;
        standing.set(&nodes, store, ui);
        if let (true, Some(current)) = (first, landed.current) {
            let list = standing.list_mut();
            list.select_only(current);
            list.reveal_row(current, imba::event::Placement::EnsureVisible);
        }
    }
}

impl View for OutlineView {
    type Command = OutlineCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, OutlineCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.search.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::command(OutlineCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.search.focus_data(store, ui).map(OutlineCommand::List))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: OutlineCommand,
        fx: &mut imba::effect::Effects<'_, OutlineCommand>,
    ) {
        match command {
            OutlineCommand::List(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.search.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.search.inner().list().key_at(index).copied()
                            else {
                                return;
                            };
                            self.search.inner_mut().list_mut().select_only(key);
                            return self.search.inner_mut().toggle(&key, store, ui);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = OutlineSearch::activated(&command) {
                    if let Some(key) = self.search.inner().list().key_at(index).copied() {
                        let searching = self.search.searching();
                        self.pick(store, key);
                        match trigger {
                            // The deliberate pick ends the search in
                            // the same stroke.
                            ActivateTrigger::Enter if searching => {
                                return self.perform(
                                    store,
                                    ui,
                                    OutlineCommand::List(ListKeyCommand::Clear),
                                    fx,
                                );
                            }
                            ActivateTrigger::Enter | ActivateTrigger::Click => {}
                        }
                    }
                }
                fx.scope(OutlineCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
            }
            OutlineCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
            OutlineCommand::Refresh => self.relaunch(store, fx),
            OutlineCommand::Landed(landed) => {
                if self.derived == Some((landed.stamp, landed.caret)) {
                    return;
                }
                self.land(landed, store, ui);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let stale = documents::OpenDocuments::document_ref(store, self.documents, self.document)
            .is_some_and(|document| {
                let derivation = Self::derivation_of(document);
                self.derived != Some(derivation) && self.launched != Some(derivation)
            });
        let searching = self.search.searching();
        contents_column(
            arena,
            store,
            ui,
            self.location.name().to_owned(),
            imba::layout::LayoutBox::new(
                arena,
                self.search
                    .display(arena, store, ui)
                    .map_layout(OutlineCommand::List),
            ),
            move |_arena: &Arena, event: &Event<'_>, _size: Size| match event {
                Event::Paint { .. } if stale => EventResult::command(OutlineCommand::Refresh),
                Event::KeyDown {
                    key: InputKey::Escape,
                    ..
                } if !searching => EventResult::command(OutlineCommand::Dismiss),
                _ => EventResult::Ignored,
            },
        )
    }
}

/// The drawer's column around a list: "Contents" over its subject.
fn contents_column<'a, Command: 'a, Keys>(
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    subject: String,
    content: imba::layout::LayoutBox<'a, Command>,
    keys: Keys,
) -> chrome::ContentsColumn<'a, Command, Keys>
where
    Keys: imba::layout::EventHandler<Command> + 'a,
{
    let theme = editor::env::Themes::of(store).ui().clone();
    let header = hikit::header::ColumnHeader::new(arena, store, ui, theme.contents.text_edge)
        .title("Contents")
        .subtitle(subject);
    chrome::ContentsColumn {
        header,
        content,
        width: theme.window.side_width,
        top_gap: theme.contents.top_gap,
        ground: theme.ink.ground.0,
        rule: theme.window.divider.0,
        keys,
    }
}

impl ModalView for OutlineView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
