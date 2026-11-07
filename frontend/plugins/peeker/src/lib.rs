// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#[allow(unused_imports)]
use workbench_shell::app::WorkbenchShell as _;
#[allow(unused_imports)]
use workbench_shell::app_ext::AppExt as _;
#[allow(unused_imports)]
use himark::app_ext::AppExt as _;
#[cfg(any(test, feature = "test-support"))]
#[allow(unused_imports)]
use workbench_shell::test_support::TestSupport as _;

use ahp_locations::FindEffect;
use documents::entity_view::EditorIdView;
use documents::BuildDocumentEffect;
use documents::FetchDocumentEffect;
use editor::document::Document;
use editor::location::ResourceLocation;
use gfx::Size;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use hikit::panel::WidgetOrigin;
use himark::app::Himark;
use imba::layout::{Insets, LayoutExt as _};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    layout::Layout as _,
    leaf::leaf,
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use locations::results::{ResultsCommand, ResultsOutcome, ResultsView};
use locations::{FeedId, LocationLists, LocationsAsk};
use std::sync::Arc;
use workbench::workbench_node::EditorPane;

mod sheet;
use workbench::workbench_node::PaneCommand;

#[derive(Clone)]
pub struct Peeker {
    // ------------------------------------------------------- wiring
    /// The peeker's own query input: the overlay owns its text, typed
    /// directly — no omnibox in between.
    input: editor::editor_view::EditorView,

    /// The documents collection this peeker fronts — its id is wired
    /// at open from the window.s session bundle (docs/entities.md law 3).
    documents: imba::store::Id<documents::OpenDocuments>,

    /// The session's location-lists collection — the hits lanes are
    /// FACES over a peeker-owned feed here, the same model the
    /// Search dock tab fronts (docs/ui/location-list.md).
    lists: imba::store::Id<LocationLists>,

    // ------------------------------------------------------ sources
    /// Frozen at open — Arc'd, so the modal's per-update clone
    /// (docs/entities.md: the window entity is a store VALUE) is a
    /// refcount bump, not a copy.
    recents: Arc<[ResourceLocation]>,

    /// The workspace folders — the finds' scope and every row's
    /// shown place. Frozen at open, Arc'd like `recents`.
    workspace: Arc<[ResourceLocation]>,

    /// The widget handles the files lane lists and previews
    /// (docs/ui/peeker.md); row titles read off these, not a cache.
    widgets: Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)>,

    // ------------------------------------------------- the master
    /// The ONE standing master, named by the query's sigil: a lane's
    /// state lives exactly as long as the query names it — there is
    /// never a hidden second lane to drift.
    master: Master,

    /// The find effect's bookkeeping — PEEKER-level like the fetch
    /// tokens, so a stale answer from a dropped lane still fails the
    /// serial gate.
    find_serial: u64,

    find_token: Option<imba::effect::CancellationToken>,

    // ------------------------------------------------- the preview
    temp_docs: std::collections::HashMap<ResourceLocation, documents::DocumentId>,

    pending_fetch: std::collections::HashSet<ResourceLocation>,

    preview: Option<PreviewSlot>,

    preview_width: f32,

    /// The selected hit's wash on the previewed document — removed
    /// when the aim moves on.
    aimed: Option<Aim>,

    // -------------------------------------------------- the chrome
    /// The windowed modal stands on its own SHEET, centered in the
    /// window; the mounted GLASS road (`unfloored`) rides the host's
    /// island backdrop and the dialog fills the island.
    floored: bool,

    request: hikit::modal::RequestSlot<ModalRequest>,
}

/// The master column's state — exactly one lane stands at a time.
#[derive(Clone)]
enum Master {
    /// The flat buffer switcher (a plain query).
    Files(FilesLane),

    /// The hits tree over a peeker-owned feed (`#` symbols, `%` text).
    Hits(HitsLane),
}

/// The flat buffer-switcher master: the filter's product is `rows` —
/// INDICES into the sources, never copies; a row's name and place
/// derive on demand, and the one realized copy of the visible rows is
/// the list view's own slice (the framework's shape).
#[derive(Clone)]
struct FilesLane {
    /// Workspace finds, as the capped find effect answered them —
    /// replaced whole per answer, never edited: Arc'd, so the lane's
    /// clone is a refcount bump.
    found: Arc<[ResourceLocation]>,

    filter_query: String,

    /// Which source each shown row names, capped at `PEEKER_SHOWN` —
    /// rebuilt whole per filter, never edited: Arc'd like `found`.
    rows: Arc<[PeekerRow]>,

    /// Matches past the cap — the tail note's count.
    hidden: usize,

    list: Rows,
}

/// The files lane's sources, read off the peeker per call — the lane
/// never copies them.
#[derive(Clone, Copy)]
struct Sources<'a> {
    recents: &'a [ResourceLocation],
    widgets: &'a [(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)],
    workspace: &'a [ResourceLocation],
}

/// The sigil lanes' master: the shared Find results tree over a
/// peeker-owned feed — the feed row in `LocationLists` is the one
/// holder of the hits (docs/ui/location-list.md); this face holds
/// ids and a dedup.
#[derive(Clone)]
struct HitsLane {
    /// `#` or `%` — which stream the feed rides; the caption's word.
    kind: HitsKind,

    /// The Find dock's results tree (`locations::results`),
    /// keys-only: the peeker's input owns typing.
    results: ResultsView,

    /// The feed the tree fronts; disposed with the overlay and on
    /// every re-query.
    feed: Option<FeedId>,

    /// The last (kind, term) streamed — the keystroke dedup; a caret
    /// move must not re-mint a feed.
    asked: Option<(HitsKind, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HitsKind {
    Symbols,
    Text,
}

pub(crate) type PeekerEffects<'a> = imba::effect::Effects<'a, PeekerCommand>;

#[derive(Clone)]
pub enum PeekerCommand {
    /// The peeker's OWN input editor — the query lives here, not in
    /// any toolbar.
    Input(editor::editor_view::EditorCommand),

    Preview(PaneCommand),

    Pick(usize),

    Close,

    Found {
        serial: u64,
        locations: Vec<ResourceLocation>,
    },

    FetchedPreview {
        location: ResourceLocation,
        text: Option<String>,
    },

    BuiltPreview {
        location: ResourceLocation,
        document: Document,
    },

    Widget(imba::dyn_view::DynCommand),

    Rows(RowsCommand),

    /// The hits master (the shared Find results tree) — symbol and
    /// text modes.
    Results(ResultsCommand),

    /// The hits master noticed its feed moved (paint-driven, the
    /// Search dock's refresh-probe recipe).
    RefreshResults,
}

impl std::fmt::Display for PeekerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeekerCommand::Input(command) => command.fmt(out),
            PeekerCommand::Preview(command) => command.fmt(out),
            PeekerCommand::Widget(command) => command.fmt(out),
            PeekerCommand::Rows(command) => command.fmt(out),
            PeekerCommand::Results(command) => command.fmt(out),
            PeekerCommand::RefreshResults => out.write_str("peeker refresh results"),
            PeekerCommand::Pick(_) => out.write_str("peeker pick"),
            PeekerCommand::Close => out.write_str("peeker close"),
            PeekerCommand::Found { .. } => out.write_str("peeker found"),
            PeekerCommand::FetchedPreview { .. } => out.write_str("peeker fetched preview"),
            PeekerCommand::BuiltPreview { .. } => out.write_str("peeker built preview"),
        }
    }
}

#[derive(Clone)]
struct Preview {
    document: documents::DocumentId,
    width: f32,
    pane: EditorPane,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PeekerRow {
    Recent(usize),

    Widget(usize),

    Found(usize),
}

/// What the query's first character names (docs/ui/location-list.md):
/// a plain query jumps to files, `#` to symbols, `%` to text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QueryMode {
    Files,
    Symbols,
    Text,
}

/// The hit wash standing on a previewed document: one `Match`-styled
/// span, shown on the preview's editor only (the peek card's recipe —
/// the document may be genuinely open elsewhere and its panes must
/// not light up).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Aim {
    document: documents::DocumentId,
    markup: editor::markup::MarkupId,
    span: (u32, u32),
}

/// How far from the preview's top the aimed hit rests.
const AIM_HEADROOM: f32 = 160.0;

/// The query with its sigil stripped: `#name` → symbols, `%words` →
/// text, anything else → the file lane.
fn sniff(query: &str) -> (QueryMode, &str) {
    match query.strip_prefix('#') {
        Some(term) => (QueryMode::Symbols, term.trim()),
        None => match query.strip_prefix('%') {
            Some(term) => (QueryMode::Text, term.trim()),
            None => (QueryMode::Files, query),
        },
    }
}

#[derive(Clone)]
enum PreviewSlot {
    Editor(Preview),

    Widget(usize),
}

const PEEKER_SHOWN: usize = 200;

/// Keys-only controller over the file rows — the peeker's own
/// input filters; the table does the movement, the list's cursor IS
/// the selection (docs/ui/list-keyboard.md).
type Rows = hikit::list_keyboard::ListKeyboardController<
    imba::scroll::ScrollView<imba::list::ListView<FileRow, usize>>,
>;

/// One match: the file's name with the typed letters marked, its
/// directory at the right in grey. The SELECTED row grows — a
/// taller row, a larger and heavier name, a brighter directory — so
/// the row knows whether it is the one. The tail note ("… 12 more")
/// is a dim, unkeyed row.
#[derive(Clone)]
struct FileRow {
    name: String,
    marks: Vec<std::ops::Range<usize>>,
    dir: String,
    selected: bool,
    note: bool,
}

impl View for FileRow {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let themes = ::editor::env::Themes::of(store);
        let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
        let chrome = &peeker.jump;
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let (role, color, dir_color, height) = match (self.selected, self.note) {
            (true, _) => (
                &chrome.selected_name,
                ink.ink_bright.0,
                ink.ink_2.0,
                chrome.selected_row_height,
            ),
            (false, false) => (
                &chrome.name,
                chrome.name_color.0,
                ink.ink_3.0,
                chrome.row_height,
            ),
            (false, true) => (&chrome.name, ink.ink_3.0, ink.ink_3.0, chrome.row_height),
        };
        let plain = sheet::styled(ui, role, color);
        let mark = sheet::MatchMark::of(
            ui,
            role,
            chrome.mark_weight,
            ink.ink_bright.0,
            ink.live.0,
            chrome.mark_thickness,
            chrome.mark_offset,
        );
        // The flexible gap pushes the directory to the right edge
        // (main axis only: a bare `Fill` would drag the baseline
        // group to the row's top).
        let mut row = imba::layout::Row::new(arena)
            .child_by_baseline(
                sheet::marked_label(arena, &self.name, &self.marks, &plain, &mark).pad_insets(
                    Insets {
                        left: chrome.pad_x,
                        ..Default::default()
                    },
                ),
            )
            .weighted(1.0, imba::layout::Fill::new().height(0.0));
        if !self.dir.is_empty() {
            // Measured after the name, against what it leaves: a
            // long directory ellipsizes, the name never does.
            row = row.child_by_baseline(
                hikit::ui::text(
                    &hikit::ui::brief(store, ui).colored(dir_color),
                    self.dir.clone(),
                )
                .pad_insets(Insets {
                    left: chrome.dir_gap,
                    right: chrome.pad_x,
                    ..Default::default()
                }),
            );
        }
        // A press on the row is the LIST's: `Select`, then
        // `Activate(Click)` (docs/ui/list-keyboard.md §2).
        row.align(imba::layout::Alignment::CenterStart)
            .height(height)
            .on_event(
                move |_arena: &Arena, event: &Event<'_>, _size| match event {
                    Event::MouseDown { .. } => EventResult::handled(),
                    _ => EventResult::Ignored,
                },
            )
    }
}
type RowsCommand = hikit::list_keyboard::ListKeyCommand<
    imba::scroll::ScrollCommand<imba::list::ListCommand<std::convert::Infallible>>,
>;

fn rows_list() -> Rows {
    hikit::list_keyboard::ListKeyboardController::new(imba::scroll::ScrollView::new(
        imba::list::ListView::empty(),
    ))
}

impl FilesLane {
    fn new() -> Self {
        Self {
            found: Vec::new().into(),
            filter_query: String::new(),
            rows: Vec::new().into(),
            hidden: 0,
            list: rows_list(),
        }
    }

    fn selected(&self) -> usize {
        use imba::list::ListOps;
        self.list.cursor_index().unwrap_or(0)
    }

    fn location_at<'a>(&'a self, sources: Sources<'a>, row: usize) -> Option<&'a ResourceLocation> {
        match self.rows.get(row) {
            Some(PeekerRow::Recent(index)) => sources.recents.get(*index),
            Some(PeekerRow::Found(index)) => self.found.get(*index),
            _ => None,
        }
    }

    fn widget_at(&self, row: usize) -> Option<usize> {
        match self.rows.get(row) {
            Some(PeekerRow::Widget(index)) => Some(*index),
            _ => None,
        }
    }

    /// The `row`th row's name, read off its source.
    fn row_label(&self, store: &Store, sources: Sources<'_>, row: usize) -> String {
        match self.rows.get(row) {
            Some(PeekerRow::Widget(index)) => sources
                .widgets
                .get(*index)
                .map(|(_, widget)| widget.title(store))
                .unwrap_or_default(),
            Some(_) => self
                .location_at(sources, row)
                .map(|location| location.name().to_owned())
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    /// The `row`th match as a row, selected or not — derived from its
    /// source; the list slice is the one realized copy.
    fn file_row(&self, store: &Store, sources: Sources<'_>, row: usize, selected: bool) -> FileRow {
        let name = self.row_label(store, sources, row);
        let dir = self
            .location_at(sources, row)
            .map(|location| place_of(sources.workspace, location).1)
            .unwrap_or_default();
        FileRow {
            marks: sheet::match_runs(&name, &self.filter_query),
            name,
            dir,
            selected,
            note: false,
        }
    }

    fn refilter(&mut self, store: &Store, ui: &UiCtx, sources: Sources<'_>) {
        let query = self.filter_query.to_lowercase();
        let mut rows = Vec::new();
        let mut hidden = 0usize;
        let mut push = |row: PeekerRow, matched: bool| {
            if !matched {
                return;
            }
            if rows.len() < PEEKER_SHOWN {
                rows.push(row);
            } else {
                hidden += 1;
            }
        };
        for (index, location) in sources.recents.iter().enumerate() {
            push(
                PeekerRow::Recent(index),
                subsequence_match(&location.name().to_lowercase(), &query),
            );
        }
        for (index, (_, widget)) in sources.widgets.iter().enumerate() {
            push(
                PeekerRow::Widget(index),
                subsequence_match(&widget.title(store).to_lowercase(), &query),
            );
        }
        for index in 0..self.found.len() {
            push(PeekerRow::Found(index), true);
        }
        self.rows = rows.into();
        self.hidden = hidden;
        let selected = self.selected().min(self.rows.len().saturating_sub(1));
        let mut slice: imba::list::ListSlice<FileRow, usize> = imba::list::ListSlice::new();
        for row in 0..self.rows.len() {
            slice.push_keyed(
                row,
                self.file_row(store, sources, row, row == selected),
                store,
                ui,
            );
        }
        if self.hidden > 0 {
            let note = FileRow {
                name: format!("… {} more — narrow the filter", self.hidden),
                marks: Vec::new(),
                dir: String::new(),
                selected: false,
                note: true,
            };
            slice.push(note, store, ui);
        }
        let scroll_y = self.list.inner().scroll_y();
        let mut list =
            imba::list::ListView::from_slice(slice).with_selection(selection_style(store));
        if !self.rows.is_empty() {
            list.select_only(selected);
        }
        *self.list.inner_mut() = imba::scroll::ScrollView::new(list);
        self.list.inner_mut().set_scroll_y(scroll_y);
    }

    /// The selection moved: the row it left shrinks back and the row
    /// it reached grows — two rows re-laid, in place.
    fn restyle(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        sources: Sources<'_>,
        left: Option<usize>,
        reached: Option<usize>,
    ) {
        for (row, selected) in [(left, false), (reached, true)] {
            let Some(row) = row.filter(|&row| row < self.rows.len()) else {
                continue;
            };
            let mut slice: imba::list::ListSlice<FileRow, usize> = imba::list::ListSlice::new();
            slice.push_keyed(row, self.file_row(store, sources, row, selected), store, ui);
            self.list
                .inner_mut()
                .content_mut()
                .splice_slice(row..row + 1, slice);
        }
    }
}

impl HitsLane {
    fn new(store: &Store, lists: imba::store::Id<LocationLists>, kind: HitsKind) -> Self {
        Self {
            kind,
            results: ResultsView::keys_only(store, lists),
            feed: None,
            asked: None,
        }
    }
}

/// Where a file sits: its workspace folder shown from home
/// ("~/Projects/himark-jb/", empty outside every folder) and its
/// directory under that folder ("tools/grammar-pack/src").
fn place_of(workspace: &[ResourceLocation], location: &ResourceLocation) -> (String, String) {
    let path = location.path();
    let parent = &path[..path.len().saturating_sub(1)];
    match workspace
        .iter()
        .find(|folder| parent.starts_with(folder.path()))
    {
        Some(folder) => (
            format!("{}/", shown_path(folder.path())),
            parent[folder.path().len()..].join("/"),
        ),
        None => (String::new(), parent.join("/")),
    }
}

impl Peeker {
    pub fn open(
        store: &mut Store,
        ui: &UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        lists: imba::store::Id<LocationLists>,
        viewport: Size,
        recents: Vec<ResourceLocation>,
        widgets: Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)>,
        folders: Vec<editor::location::ResourceLocation>,
        fx: &mut PeekerEffects<'_>,
    ) -> Self {
        let workspace: Arc<[ResourceLocation]> = folders.into();
        // The symbol and text lanes search the model's own folders —
        // the same stamp the Search dock mount makes.
        LocationLists::adopt_folders(store, lists, &workspace);
        let chrome = ::editor::env::Themes::of(store).part(::hikit::chrome::PeekerChrome::KEY);
        let mut input = editor::editor_view::EditorView::input_styled(
            600.0,
            store,
            ui,
            editor::theme::StyleId::Query,
        );
        input.focus_text();

        let mut peeker = Self {
            input,
            documents,
            recents: recents.into(),
            workspace,
            lists,
            master: Master::Files(FilesLane::new()),
            find_serial: 0,
            find_token: None,
            aimed: None,
            temp_docs: std::collections::HashMap::new(),
            pending_fetch: std::collections::HashSet::new(),
            widgets,
            preview: None,
            // The windowed sheet's preview: the dialog's width in
            // this viewport, less the matches column.
            preview_width: preview_width(
                sheet::Sheet(chrome.jump.sheet.clone()).width_in(viewport),
                &chrome.jump,
            ),
            floored: true,
            request: Default::default(),
        };
        peeker.filter(store, ui, "");
        peeker.ensure_preview(store, ui, fx);
        peeker
    }

    /// The GLASS face: no sheet of its own — the mounting host
    /// paints the island floor behind the whole peeker.
    pub fn unfloored(mut self) -> Self {
        self.floored = false;
        self
    }

    fn query(&self) -> String {
        let mut view = self.input.document.text().view();
        let byte_count = view.byte_count();
        view.byte_string(0, byte_count)
    }

    fn apply_query(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut PeekerEffects<'_>) {
        let query = self.query();
        let query = query.trim().to_owned();
        match sniff(&query) {
            (QueryMode::Files, term) => self.enter_files(store, ui, term, fx),
            (QueryMode::Symbols, term) => self.enter_hits(store, ui, HitsKind::Symbols, term),
            (QueryMode::Text, term) => self.enter_hits(store, ui, HitsKind::Text, term),
        }
        self.ensure_preview(store, ui, fx);
        self.aim_preview(store, ui, fx);
    }

    /// The plain query's lane: a standing hits lane dies with its
    /// feed, then the filter and the find run as ever.
    fn enter_files(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        term: &str,
        fx: &mut PeekerEffects<'_>,
    ) {
        if let Master::Hits(hits) = &mut self.master {
            if let Some(feed) = hits.feed.take() {
                LocationLists::ask(store, self.lists, LocationsAsk::Dispose(feed));
            }
            self.master = Master::Files(FilesLane::new());
        }
        self.filter(store, ui, term);
        self.launch_find(store, ui, term, fx);
    }

    /// A sigil lane: swap the master in when the files lane stood
    /// (its in-flight find resolves into the serial gate), then
    /// re-query — dispose the standing feed, open a new one and NOTE
    /// the ask; the wire lane launches it (the Search dock tab's
    /// requery, over a peeker-owned feed).
    fn enter_hits(&mut self, store: &mut Store, ui: &UiCtx, kind: HitsKind, term: &str) {
        if matches!(self.master, Master::Files(_)) {
            self.master = Master::Hits(HitsLane::new(store, self.lists, kind));
        }
        let lists = self.lists;
        let Master::Hits(hits) = &mut self.master else {
            unreachable!("just ensured");
        };
        hits.kind = kind;
        if hits
            .asked
            .as_ref()
            .is_some_and(|(asked, held)| *asked == kind && held == term)
        {
            return;
        }
        hits.asked = Some((kind, term.to_owned()));
        if let Some(previous) = hits.feed.take() {
            LocationLists::ask(store, lists, LocationsAsk::Dispose(previous));
        }
        if term.len() < 2 {
            hits.results.reset(store, ui, None);
            return;
        }
        let feed = FeedId::mint();
        let title = match kind {
            HitsKind::Symbols => format!("Symbols: {term}"),
            HitsKind::Text => format!("Search: {term}"),
        };
        locations::open_feed(store, lists, feed, title, term.to_owned());
        let query = term.to_owned();
        LocationLists::ask(
            store,
            lists,
            match kind {
                HitsKind::Symbols => LocationsAsk::Symbols { feed, query },
                HitsKind::Text => LocationsAsk::Search { feed, query },
            },
        );
        let Master::Hits(hits) = &mut self.master else {
            unreachable!("just ensured");
        };
        hits.feed = Some(feed);
        hits.results.reset(store, ui, Some(feed));
    }

    fn sources(&self) -> Sources<'_> {
        Sources {
            recents: &self.recents,
            widgets: &self.widgets,
            workspace: &self.workspace,
        }
    }

    /// What the master's selection names: the flat file row, or the
    /// hits tree's cursor (a directory names nothing).
    fn current_location(&self, store: &Store) -> Option<ResourceLocation> {
        match &self.master {
            Master::Files(files) => files.location_at(self.sources(), files.selected()).cloned(),
            Master::Hits(hits) => {
                let key = hits.results.cursor()?;
                Some(hits.results.found(store, hits.feed, &key)?.location)
            }
        }
    }

    /// The sheet's caps label: what it does, and in which repo.
    fn caption(&self) -> String {
        let what = match &self.master {
            Master::Files(_) => "JUMP TO FILE",
            Master::Hits(hits) => match hits.kind {
                HitsKind::Symbols => "JUMP TO SYMBOL",
                HitsKind::Text => "JUMP TO TEXT",
            },
        };
        match self.workspace.first() {
            Some(folder) => format!("{what} · {}", folder.name().to_uppercase()),
            None => what.to_owned(),
        }
    }

    /// The shown rows' names, derived off their sources (the files
    /// lane's; a hits master lists through its tree instead).
    pub fn labels(&self, store: &Store) -> Vec<String> {
        match &self.master {
            Master::Files(files) => (0..files.rows.len())
                .map(|row| files.row_label(store, self.sources(), row))
                .collect(),
            Master::Hits(_) => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn hidden_count(&self) -> usize {
        match &self.master {
            Master::Files(files) => files.hidden,
            Master::Hits(_) => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn hits_lane(&self) -> Option<&HitsLane> {
        match &self.master {
            Master::Hits(hits) => Some(hits),
            Master::Files(_) => None,
        }
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn preview_height(&self, store: &Store) -> Option<f32> {
        match self.preview.as_ref()? {
            PreviewSlot::Editor(preview) => {
                Some(preview.pane.content().gathered(store)?.content_height())
            }
            PreviewSlot::Widget(_) => None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn previewed_widget_title(&self, store: &Store) -> Option<String> {
        match self.preview.as_ref()? {
            PreviewSlot::Widget(index) => self
                .widgets
                .get(*index)
                .map(|(_, widget)| widget.title(store)),
            PreviewSlot::Editor(_) => None,
        }
    }

    fn filter(&mut self, store: &Store, ui: &UiCtx, query: &str) {
        let sources = Sources {
            recents: &self.recents,
            widgets: &self.widgets,
            workspace: &self.workspace,
        };
        let Master::Files(files) = &mut self.master else {
            return;
        };
        files.filter_query = query.to_owned();
        files.refilter(store, ui, sources);
    }

    fn refilter(&mut self, store: &Store, ui: &UiCtx) {
        let sources = Sources {
            recents: &self.recents,
            widgets: &self.widgets,
            workspace: &self.workspace,
        };
        let Master::Files(files) = &mut self.master else {
            return;
        };
        files.refilter(store, ui, sources);
    }

    fn launch_find(&mut self, store: &Store, ui: &UiCtx, query: &str, fx: &mut PeekerEffects<'_>) {
        self.find_serial += 1;
        if self.workspace.is_empty() || query.len() < 2 {
            if let Master::Files(files) = &mut self.master {
                files.found = Vec::new().into();
            }
            self.refilter(store, ui);
            return;
        }
        let serial = self.find_serial;

        let effect = imba::effect::AnyEffect::new(FindEffect {
            folders: self.workspace.to_vec(),
            term: query.to_owned(),
        })
        .map(move |locations| PeekerCommand::Found { serial, locations });
        fx.relaunch_erased(&mut self.find_token, effect);
    }

    /// In the hits lanes, aim the standing preview at the selected
    /// hit: a one-range editor wash and the scroll to the hit's line
    /// (the peek card's recipe, over the pane's shared document). A
    /// move to another hit of the same file re-aims in place; another
    /// file's hit lands here after `ensure_preview` swapped the pane.
    fn aim_preview(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut PeekerEffects<'_>) {
        let found = match &self.master {
            Master::Files(_) => return self.unaim(store, ui, fx),
            Master::Hits(hits) => hits
                .results
                .cursor()
                .and_then(|key| hits.results.found(store, hits.feed, &key)),
        };
        let Some(found) = found else {
            return;
        };
        let Some(PreviewSlot::Editor(preview)) = &self.preview else {
            return;
        };
        let entity = *preview.pane.content();
        let documents = entity.documents();
        if documents::OpenDocuments::location(store, documents, entity.document()).as_ref()
            != Some(&found.location)
        {
            return; // the pane still shows another file; its swap re-aims
        }
        // A wash standing on ANOTHER document leaves through its own
        // handle first.
        if self
            .aimed
            .as_ref()
            .is_some_and(|aim| aim.document != entity.document())
        {
            self.unaim(store, ui, fx);
        }
        let Some(mut document) =
            documents::OpenDocuments::document(store, documents, entity.document())
        else {
            return;
        };
        let (hit, span) = {
            let mut view = document.text().view();
            let hit = documents::text_ext::offset_at(&mut view, found.target().start) as u32;
            (hit, (hit, hit + found.length.max(1)))
        };
        if self
            .aimed
            .as_ref()
            .is_some_and(|aim| aim.document == entity.document() && aim.span == span)
        {
            documents::OpenDocuments::put_document(store, documents, entity.document(), document);
            return;
        }
        let theme = ::editor::env::Themes::of(store);
        let scoped = |command| PeekerCommand::Preview(PaneCommand::Content(command));
        let mut tints = editor::markup::Markup::new();
        tints.push_styled(
            span.0..span.1,
            editor::theme::StyleId::Match,
            editor::markup::StyleWeight::OVERLAY,
        );
        let markup = match self.aimed.take() {
            // The same document's wash moves: old ∪ new is the change
            // set — no re-add, no re-show.
            Some(aim) => {
                let changed = vec![span.0..span.1, aim.span.0..aim.span.1];
                fx.scope(scoped, |fx| {
                    document.replace_markup(aim.markup, tints, &changed, store, ui, &theme, fx)
                });
                aim.markup
            }
            None => {
                let markup = document.add_markup();
                fx.scope(scoped, |fx| {
                    document.replace_markup(markup, tints, &[span.0..span.1], store, ui, &theme, fx)
                });
                document.show_markup(entity.editor(), markup);
                markup
            }
        };
        fx.scope(scoped, |fx| {
            document.reveal_at_instant(entity.editor(), hit, store, ui, &theme, fx)
        });
        let reveal = document
            .caret_content_rect(entity.editor(), hit, store, ui, &theme)
            .map(|(_, y, _, _)| (y - AIM_HEADROOM).max(0.0));
        documents::OpenDocuments::put_document(store, documents, entity.document(), document);
        self.aimed = Some(Aim {
            document: entity.document(),
            markup,
            span,
        });
        if let Some(reveal) = reveal {
            if let Some(PreviewSlot::Editor(preview)) = &mut self.preview {
                preview.pane.set_scroll_y(reveal);
            }
        }
    }

    /// The standing hit wash leaves its document (when it still
    /// stands) — aim teardown on mode exits, pane swaps and the
    /// overlay's end.
    fn unaim(&mut self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut PeekerEffects<'_>) {
        let Some(aim) = self.aimed.take() else {
            return;
        };
        let Some(mut document) =
            documents::OpenDocuments::document(store, self.documents, aim.document)
        else {
            return;
        };
        let theme = ::editor::env::Themes::of(store);
        let changed = vec![aim.span.0..aim.span.1];
        fx.scope(
            |command| PeekerCommand::Preview(PaneCommand::Content(command)),
            |fx| document.remove_markup(aim.markup, &changed, store, ui, &theme, fx),
        );
        documents::OpenDocuments::put_document(store, self.documents, aim.document, document);
    }

    /// A hit picked: the deliberate jump — the overlay closes and the
    /// shell opens the file at the hit's span.
    fn open_hit(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: locations::LocationKey,
        fx: &mut PeekerEffects<'_>,
    ) {
        let Master::Hits(hits) = &self.master else {
            return;
        };
        let Some(found) = hits.results.found(store, hits.feed, &key) else {
            return;
        };
        let target = found.target();
        self.cleanup_temps(store, ui, None, fx);
        self.request.file(ModalRequest::OpenAt {
            location: found.location,
            target: Some(target),
            focus: true,
        });
    }

    fn cleanup_temps(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        keep: Option<documents::DocumentId>,
        fx: &mut PeekerEffects<'_>,
    ) {
        self.unaim(store, ui, fx);
        let lists = self.lists;
        if let Master::Hits(hits) = &mut self.master {
            hits.asked = None;
            if let Some(feed) = hits.feed.take() {
                LocationLists::ask(store, lists, LocationsAsk::Dispose(feed));
            }
        }
        let documents = self.documents;
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            documents::lifecycle::close_editor(
                store,
                entity.documents(),
                entity.document(),
                entity.editor(),
            );
        }

        for (_, document) in self.temp_docs.drain() {
            if Some(document) != keep {
                documents::OpenDocuments::remove_if_editorless(store, documents, ui, document, fx);
            }
        }
        self.preview = None;
    }

    fn drop_preview(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut PeekerEffects<'_>,
    ) {
        self.unaim(store, ui, fx);
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            documents::lifecycle::close_editor(
                store,
                entity.documents(),
                entity.document(),
                entity.editor(),
            );

            if self.temp_docs.values().any(|id| *id == entity.document()) {
                documents::OpenDocuments::remove_if_editorless(
                    store,
                    entity.documents(),
                    ui,
                    entity.document(),
                    fx,
                );
                self.temp_docs.retain(|_, id| {
                    documents::OpenDocuments::contains(store, entity.documents(), *id)
                });
            }
        }
        self.preview = None;
    }

    fn ensure_preview(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut PeekerEffects<'_>,
    ) {
        let documents = self.documents;
        let width = EditorIdView::editor_width(
            self.preview_width,
            &::editor::env::Themes::of(store).part(::editor::theme::WindowChrome::KEY),
        );

        if let Master::Files(files) = &self.master {
            if let Some(index) = files.widget_at(files.selected()) {
                self.preview = Some(PreviewSlot::Widget(index));
                return;
            }
        }

        let document_id = match self.current_location(store) {
            Some(location) => {
                match documents::OpenDocuments::by_location(store, documents, &location) {
                    Some(id) => Some(id),
                    None => match self.temp_docs.get(&location) {
                        Some(&id) => Some(id),
                        None => {
                            self.drop_preview(store, ui, fx);
                            if self.pending_fetch.insert(location.clone()) {
                                let landing = location.clone();
                                let _ = fx.push(
                                    imba::effect::AnyEffect::new(FetchDocumentEffect { location })
                                        .map(move |text| PeekerCommand::FetchedPreview {
                                            location: landing,
                                            text,
                                        }),
                                );
                            }
                            return;
                        }
                    },
                }
            }
            None => None,
        };
        let Some(document_id) = document_id else {
            self.drop_preview(store, ui, fx);
            return;
        };
        if self.preview.as_ref().is_some_and(|slot| match slot {
            PreviewSlot::Editor(preview) => {
                preview.document == document_id && preview.width == width
            }
            PreviewSlot::Widget(_) => false,
        }) {
            return;
        }
        let Some(mut document) = documents::OpenDocuments::document(store, documents, document_id)
        else {
            self.drop_preview(store, ui, fx);
            return;
        };

        let previous = match &self.preview {
            Some(PreviewSlot::Editor(preview)) => Some(*preview.pane.content()),
            _ => None,
        };
        if let Some(previous) = previous {
            documents::lifecycle::close_editor(
                store,
                previous.documents(),
                previous.document(),
                previous.editor(),
            );
        }
        let editor = fx.scope(
            |command| PeekerCommand::Preview(PaneCommand::Content(command)),
            |fx| documents::lifecycle::mount_editor(store, ui, &mut document, width, None, fx),
        );
        documents::OpenDocuments::put_document(store, documents, document_id, document);
        if let Some(previous) = previous.filter(|previous| previous.document() != document_id) {
            if self.temp_docs.values().any(|id| *id == previous.document()) {
                documents::OpenDocuments::remove_if_editorless(
                    store,
                    previous.documents(),
                    ui,
                    previous.document(),
                    fx,
                );
                self.temp_docs
                    .retain(|_, id| documents::OpenDocuments::contains(store, documents, *id));
            }
        }

        self.preview = Some(PreviewSlot::Editor(Preview {
            document: document_id,
            width,
            pane: ScrollView::new(EditorIdView::new(documents, document_id, editor).blurred()),
        }));
    }
}

impl View for Peeker {
    type Command = PeekerCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, PeekerCommand> {
        use imba::event::EventResult;
        // Movement and Enter are the controller's table; the peeker
        // keeps its own close (and Enter-with-nothing closes too).
        let empty = match &self.master {
            Master::Files(files) => files.rows.is_empty(),
            Master::Hits(hits) => hits.results.is_empty(),
        };
        let own = imba::focus::FocusData {
            commands: vec![imba::PresentableCommand::new(
                "peeker.close",
                "Close Peeker",
                PeekerCommand::Close,
            )],
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Enter if empty => EventResult::command(PeekerCommand::Close),
                Key::Escape => EventResult::command(PeekerCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        };
        let master = match &self.master {
            Master::Files(files) => files.list.focus_data(_store, _ui).map(PeekerCommand::Rows),
            Master::Hits(hits) => hits
                .results
                .focus_data(_store, _ui)
                .map(PeekerCommand::Results),
        };
        own.merge_under(master)
            .merge_under(self.input.focus_data(_store, _ui).map(PeekerCommand::Input))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(token) = self.find_token.take() {
            fx.cancel(token);
        }
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::ui::UiCtx::dont_use_too_slow();
        self.cleanup_temps(store, ui, None, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut PeekerEffects<'_>,
    ) {
        match command {
            PeekerCommand::Input(command) => {
                fx.scope(PeekerCommand::Input, |fx| {
                    self.input.perform(store, ui, command, fx)
                });
                self.apply_query(store, ui, fx);
            }
            PeekerCommand::Preview(command) => {
                if let Some(PreviewSlot::Editor(preview)) = &mut self.preview {
                    fx.scope(PeekerCommand::Preview, |fx| {
                        preview.pane.perform(store, ui, command, fx)
                    });
                }
            }
            PeekerCommand::Widget(command) => {
                if let Some(PreviewSlot::Widget(index)) = &self.preview {
                    let index = *index;
                    if let Some((_, widget)) = self.widgets.get_mut(index) {
                        fx.scope(PeekerCommand::Widget, |fx| {
                            widget.perform_dyn(store, ui, command, fx)
                        });
                    }
                }
            }
            PeekerCommand::Results(command) => {
                let outcome = {
                    let Master::Hits(hits) = &mut self.master else {
                        return; // a stale tree command after the lane left
                    };
                    fx.scope(PeekerCommand::Results, |fx| {
                        hits.results.perform(store, ui, command, fx)
                    })
                };
                match outcome {
                    // Moving the selection returns the preview, aimed
                    // at the hit.
                    ResultsOutcome::Selected => {
                        self.ensure_preview(store, ui, fx);
                        self.aim_preview(store, ui, fx);
                    }
                    // Enter and click both open: the overlay is modal —
                    // there is no browse-behind-it, unlike the dock.
                    ResultsOutcome::Picked { key, .. } => self.open_hit(store, ui, key, fx),
                    ResultsOutcome::Quiet => {}
                }
            }
            PeekerCommand::RefreshResults => {
                {
                    let Master::Hits(hits) = &mut self.master else {
                        return;
                    };
                    let feed = hits.feed;
                    hits.results.rebuild(store, ui, feed);
                }
                self.ensure_preview(store, ui, fx);
                self.aim_preview(store, ui, fx);
            }
            PeekerCommand::Rows(command) => {
                use imba::list::ListOps;
                let selected = {
                    let sources = Sources {
                        recents: &self.recents,
                        widgets: &self.widgets,
                        workspace: &self.workspace,
                    };
                    let Master::Files(files) = &mut self.master else {
                        return; // a stale list command after the lane left
                    };
                    if let Some((row, _trigger)) = Rows::activated(&command) {
                        // Enter and click both pick; the note row is
                        // unkeyed and never answers.
                        if files.list.inner().content().key_at(row).is_some() {
                            return self.perform(store, ui, PeekerCommand::Pick(row), fx);
                        }
                    }
                    let selected = Rows::selected_index(&command).is_some();
                    let left = ListOps::cursor_index(&files.list);
                    fx.scope(PeekerCommand::Rows, |fx| {
                        imba::View::perform(&mut files.list, store, ui, command, fx)
                    });
                    let reached = ListOps::cursor_index(&files.list);
                    if reached != left {
                        // The row it left shrinks back, the row it
                        // reached grows — two rows re-laid, in place.
                        files.restyle(store, ui, sources, left, reached);
                    }
                    selected
                };
                // Moving the selection returns the preview.
                if selected {
                    self.ensure_preview(store, ui, fx);
                }
            }
            PeekerCommand::Pick(row) => {
                let (widget, location) = match &self.master {
                    Master::Files(files) => (
                        files.widget_at(row),
                        files.location_at(self.sources(), row).cloned(),
                    ),
                    // Hits picks ride the Results outcome, never this.
                    Master::Hits(_) => return,
                };
                if let Some(index) = widget {
                    let (_, widget) = self.widgets.remove(index);
                    self.cleanup_temps(store, ui, None, fx);
                    self.request.file(ModalRequest::SelectWidget(widget));
                    return;
                }
                let request = if let Some(location) = location {
                    let documents = self.documents;
                    if let Some(document) =
                        documents::OpenDocuments::by_location(store, documents, &location)
                    {
                        self.cleanup_temps(store, ui, Some(document), fx);
                        ModalRequest::ShowDocument(document)
                    } else {
                        match self.temp_docs.remove(&location) {
                            Some(document) => {
                                self.cleanup_temps(store, ui, Some(document), fx);
                                ModalRequest::ShowDocument(document)
                            }

                            None => {
                                self.cleanup_temps(store, ui, None, fx);
                                ModalRequest::OpenLocations(vec![location])
                            }
                        }
                    }
                } else {
                    self.cleanup_temps(store, ui, None, fx);
                    ModalRequest::Close
                };
                self.request.file(request);
            }
            PeekerCommand::Close => {
                self.cleanup_temps(store, ui, None, fx);
                self.request.file(ModalRequest::Close);
            }
            PeekerCommand::Found { serial, locations } => {
                if serial != self.find_serial {
                    return;
                }
                {
                    let Master::Files(files) = &mut self.master else {
                        return; // the lane left while the find flew
                    };
                    files.found = locations
                        .into_iter()
                        .filter(|location| !self.recents.contains(location))
                        .collect();
                }
                self.refilter(store, ui);
                self.ensure_preview(store, ui, fx)
            }
            PeekerCommand::FetchedPreview { location, text } => {
                let Some(text) = text else {
                    self.pending_fetch.remove(&location);
                    return;
                };

                let landing = location.clone();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(BuildDocumentEffect { location, text }).map(
                        move |built| PeekerCommand::BuiltPreview {
                            location: landing,
                            document: built.document,
                        },
                    ),
                );
            }
            PeekerCommand::BuiltPreview { location, document } => {
                self.pending_fetch.remove(&location);

                let revision = document.revision();
                let id = {
                    let documents = self.documents;
                    documents::OpenDocuments::register(
                        store,
                        documents,
                        document,
                        Some(location.clone()),
                        location.name().to_owned(),
                        revision,
                    )
                };
                self.temp_docs.insert(location.clone(), id);
                if self.current_location(store).as_ref() == Some(&location) {
                    self.ensure_preview(store, ui, fx);
                    self.aim_preview(store, ui, fx);
                }
            }
        }
    }

    /// The dialog: the matches column (the query band, the rows, the
    /// key footer) and, past one hairline, the selected file's
    /// preview under its header. Windowed, the dialog stands on its
    /// SHEET centered in the window and a press beside it closes;
    /// on the glass road the host's island is the sheet and the
    /// dialog fills it.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{fixed, Column};
            use imba::Thunk as _;
            let size = constraints.max;
            let themes = ::editor::env::Themes::of(store);
            let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
            let chrome = &peeker.jump;
            let ink = themes.part(::editor::theme::InkChrome::KEY);
            let sheet = sheet::Sheet(chrome.sheet.clone());
            let dialog = match self.floored {
                true => sheet.rect_in(
                    size,
                    Size::new(sheet.width_in(size), sheet.max_height_in(size)),
                ),
                false => gfx::Rect::from_size(size),
            };
            let list_w = list_width(dialog.width(), chrome);
            // Live, not the open-time snapshot: the mounted host
            // resizes with its window and the preview fills to it.
            let preview_w = preview_width(dialog.width(), chrome);
            let loose = |width: f32| Constraints {
                min: Size::default(),
                max: Size::new(width, dialog.height()),
            };

            // The matches column.
            let input_width = (list_w - 2.0 * chrome.pad_x).max(1.0);
            let input = imba::layout::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(input_width, self.input.content_height().max(1.0))),
            )
            .map(PeekerCommand::Input);
            let count = match &self.master {
                Master::Files(files) => {
                    format!("{} matched", files.rows.len() + files.hidden)
                }
                // The hits lane reads its feed: landing batches move
                // the number, the stream's end settles the words.
                Master::Hits(hits) => {
                    let row = hits
                        .feed
                        .and_then(|feed| LocationLists::row_ref(store, self.lists, feed));
                    let (hits, done, truncated) = row
                        .map(|row| (row.locations.len(), row.done, row.truncated))
                        .unwrap_or((0, true, false));
                    let mut count = format!("{hits} matched");
                    if !done {
                        count.push_str(" — searching…");
                    } else if truncated {
                        count.push_str(" (cut off)");
                    }
                    count
                }
            };
            let head = Column::new(arena)
                .child(hikit::ui::text(&hikit::ui::caps(store, ui), self.caption()))
                .child(fixed(input).pad_insets(Insets {
                    top: chrome.label_gap,
                    ..Default::default()
                }))
                .child(
                    hikit::ui::text(&hikit::ui::brief(store, ui), count).pad_insets(Insets {
                        top: chrome.count_gap,
                        ..Default::default()
                    }),
                )
                .pad_insets(Insets {
                    left: chrome.pad_x,
                    right: chrome.pad_x,
                    top: chrome.pad_top,
                    bottom: chrome.pad_bottom,
                })
                .layout(arena, loose(list_w));
            let foot = sheet::key_hints(
                arena,
                &[("↑↓", "move"), ("⏎", "open"), ("esc", "close")],
                &sheet::mono(ui, chrome.footer_size, ink.ink_2.0),
                &hikit::ui::brief(store, ui).sized(chrome.footer_size),
                chrome.footer_gap,
            )
            .pad_insets(Insets {
                left: chrome.pad_x,
                right: chrome.pad_x,
                top: chrome.footer_pad_y,
                bottom: chrome.footer_pad_y,
            })
            .layout(arena, loose(list_w));
            let rows_h = (dialog.height()
                - head.size().height
                - foot.size().height
                - 2.0
                - 2.0 * chrome.list_pad_y)
                .max(chrome.row_height);
            // The master band: the flat file rows, or the shared Find
            // results tree (symbol and text lanes).
            let rows: imba::ThunkBox<'a, PeekerCommand> = match &self.master {
                Master::Files(files) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        files.list.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(list_w, rows_h)),
                    )
                    .map(PeekerCommand::Rows),
                ),
                Master::Hits(hits) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        hits.results.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(list_w, rows_h)),
                    )
                    .map(PeekerCommand::Results)
                    // The sticky bands plant on the tree's own scroll
                    // (the Search dock's parity law).
                    .overlay_host(imba::list::STICKY_HOST),
                ),
            };
            let matches = Column::new(arena)
                .child(fixed(head))
                .child(hikit::ui::rule_h(ink.rule.0, list_w))
                .child(fixed(rows).pad_insets(Insets {
                    top: chrome.list_pad_y,
                    bottom: chrome.list_pad_y,
                    ..Default::default()
                }))
                .child(hikit::ui::rule_h(ink.rule.0, list_w))
                .child(fixed(foot))
                .sized(list_w, dialog.height())
                .layout(arena, loose(list_w));

            // The preview: its header, then the file itself.
            let preview_head = self.preview_header(arena, store, ui, preview_w);
            let body_top = preview_head.size().height + chrome.preview_top_gap;
            let body_h = (dialog.height() - body_top).max(1.0);
            let body: imba::ThunkBox<'a, PeekerCommand> = match &self.preview {
                Some(PreviewSlot::Editor(preview)) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        preview.pane.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(preview_w, body_h)),
                    )
                    .map(PeekerCommand::Preview)
                    .focus_scope(false),
                ),
                Some(PreviewSlot::Widget(index)) => match self.widgets.get(*index) {
                    Some((_, widget)) => imba::ThunkBox::new(
                        arena,
                        widget
                            .layout_dyn(
                                arena,
                                store,
                                ui,
                                Constraints::tight(Size::new(preview_w, body_h)),
                            )
                            .map(PeekerCommand::Widget)
                            .focus_scope(false),
                    ),
                    None => imba::ThunkBox::new(arena, leaf(preview_w, body_h)),
                },
                None => hikit::ui::text(&hikit::ui::brief(store, ui), "no preview")
                    .pad_insets(Insets {
                        left: chrome.preview_pad_left,
                        ..Default::default()
                    })
                    .layout(arena, loose(preview_w)),
            };

            let mut sheet_box = imba::container::container(arena, dialog.size());
            sheet_box.place(
                0.0,
                0.0,
                hikit::ui::Hairline::new(ink.rule.0).right().wrap(matches),
            );
            sheet_box.place_boxed(list_w, 0.0, preview_head);
            sheet_box.place_boxed(list_w, body_top, body);
            // A press on the sheet's own ground stays on the sheet.
            let sheet_box = sheet_box.event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::handled(),
                _ => EventResult::Ignored,
            });

            let mut container = imba::container::container(arena, size);
            // The click-away catcher behind the dialog; windowed, it
            // also casts the sheet's shadow.
            let catcher =
                leaf::<PeekerCommand>(size.width, size.height).event(|_arena, event, _size| {
                    match event {
                        Event::MouseDown { .. } => EventResult::command(PeekerCommand::Close),
                        _ => EventResult::Ignored,
                    }
                });
            match self.floored {
                true => {
                    container.place(0.0, 0.0, catcher.paint_below(sheet.shadow_painter(dialog)));
                    container.place(dialog.left, dialog.top, sheet.wrap(sheet_box));
                }
                false => {
                    container.place(0.0, 0.0, catcher);
                    container.place(dialog.left, dialog.top, sheet_box);
                }
            }

            // Movement and Enter live in the controller's overlay.
            let empty = match &self.master {
                Master::Files(files) => files.rows.is_empty(),
                Master::Hits(hits) => hits.results.is_empty(),
            };
            let keymap = leaf::<PeekerCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } if empty => EventResult::command(PeekerCommand::Close),
                    Event::KeyDown {
                        key: Key::Escape, ..
                    } => EventResult::command(PeekerCommand::Close),
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, 0.0, keymap);

            // The refresh probe: a 1px leaf whose own paint files
            // RefreshResults when the hits feed moved — the Search
            // dock's recipe; the overlay keeps painting.
            let stale = match &self.master {
                Master::Files(_) => false,
                Master::Hits(hits) => hits.results.stale(store, hits.feed),
            };
            if stale {
                container.place(
                    0.0,
                    0.0,
                    leaf::<PeekerCommand>(1.0, 1.0).event(
                        move |_arena, event, _size| match event {
                            Event::Paint { .. } => {
                                EventResult::command(PeekerCommand::RefreshResults)
                            }
                            _ => EventResult::Ignored,
                        },
                    ),
                );
            }

            container
        })
    }
}

impl Peeker {
    /// The preview's header: the file's name, its full path (the
    /// workspace folder quiet, the name bright), and at the far
    /// right how long the file is — one baseline, a hairline under.
    fn preview_header<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> imba::ThunkBox<'a, PeekerCommand> {
        let themes = ::editor::env::Themes::of(store);
        let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
        let chrome = &peeker.jump;
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let title = sheet::styled(ui, &chrome.preview_title, ink.ink_bright.0);
        let path = |color| sheet::mono(ui, chrome.path_size, color);
        let mut row = imba::layout::Row::new(arena);
        if let Some(location) = self.current_location(store) {
            let (folder, dir) = place_of(&self.workspace, &location);
            let dir = match dir.is_empty() {
                true => dir,
                false => format!("{dir}/"),
            };
            row = row
                .child_by_baseline(hikit::ui::text(&title, location.name().to_owned()))
                .child_by_baseline(
                    hikit::ui::text(&path(ink.ink_4.0), folder).pad_insets(Insets {
                        left: chrome.preview_gap,
                        ..Default::default()
                    }),
                )
                .child_by_baseline(hikit::ui::text(&path(ink.ink_3.0), dir))
                .child_by_baseline(hikit::ui::text(
                    &path(ink.ink.0),
                    location.name().to_owned(),
                ));
        } else if let Master::Files(files) = &self.master {
            if let Some(index) = files.widget_at(files.selected()) {
                let name = self
                    .widgets
                    .get(index)
                    .map(|(_, widget)| widget.title(store))
                    .unwrap_or_default();
                row = row.child_by_baseline(hikit::ui::text(&title, name));
            }
        }
        row = row.weighted(1.0, imba::layout::Fill::new().height(0.0));
        let lines = match &self.preview {
            Some(PreviewSlot::Editor(preview)) => {
                documents::OpenDocuments::document(store, self.documents, preview.document)
                    .map(|document| document.text().line_count().0)
            }
            _ => None,
        };
        if let Some(lines) = lines {
            let noun = match lines {
                1 => "line",
                _ => "lines",
            };
            row = row.child_by_baseline(
                hikit::ui::text(
                    &hikit::ui::brief(store, ui).sized(chrome.meta_size),
                    format!("{lines} {noun}"),
                )
                .pad_insets(Insets {
                    left: chrome.preview_gap,
                    ..Default::default()
                }),
            );
        }
        let laid = row
            .pad_insets(Insets {
                left: chrome.preview_pad_left,
                right: chrome.preview_pad_right,
                top: chrome.preview_pad_top,
                bottom: chrome.preview_pad_bottom,
            })
            .width(width)
            .layout(
                arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(width, f32::MAX),
                },
            );
        imba::ThunkBox::new(
            arena,
            hikit::ui::Hairline::new(ink.rule.0).bottom().wrap(laid),
        )
    }
}

fn subsequence_match(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate.chars();
    query
        .chars()
        .all(|wanted| candidate.by_ref().any(|ch| ch == wanted))
}

/// The selected row's flat wash, across the matches column.
fn selection_style(store: &Store) -> imba::list::SelectionStyle {
    imba::list::SelectionStyle {
        fill: ::editor::env::Themes::of(store)
            .part(::hikit::chrome::PeekerChrome::KEY)
            .jump
            .selected_fill
            .0,
        ..hikit::rows::selection_style(store)
    }
}

/// An absolute path as the user reads it: from home as "~".
fn shown_path(segments: &[String]) -> String {
    let path = format!("/{}", segments.join("/"));
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path,
    }
}

/// The matches column's width in a dialog `width` wide — the
/// chrome's, short of crowding the preview out.
fn list_width(width: f32, chrome: &::hikit::chrome::JumpSheetChrome) -> f32 {
    chrome.list_width.min(width * 0.5).max(1.0)
}

/// The preview's width in a dialog `width` wide: what the matches
/// column and its hairline leave.
fn preview_width(width: f32, chrome: &::hikit::chrome::JumpSheetChrome) -> f32 {
    (width - list_width(width, chrome) - 1.0).max(160.0)
}

impl ModalView for Peeker {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn release_widgets(&mut self) -> Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)> {
        if matches!(self.preview, Some(PreviewSlot::Widget(_))) {
            self.preview = None;
        }
        std::mem::take(&mut self.widgets)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Build the goto-file overlay: a plain z-stacked modal layer that
/// OWNS its input — no toolbar session involved.
pub fn build(
    store: &mut Store,
    ui: &UiCtx,
    window: workbench::window::WindowId,
    fx: &mut himark::app::AppFx<'_>,
) -> Box<dyn hikit::modal::ModalView> {
    {
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");
        let viewport = entity.viewport_size();

        let recents = recents::RecentLocations::list(
            store,
            workbench_shell::workspace::entity_state(&entity).recents(),
        );

        let mut widgets = entity.unmount_all_widgets();
        let fronted: Vec<hikit::pane_row::PaneRow> = widgets
            .iter()
            .filter_map(|(_, widget)| widget.pane_row())
            .collect();

        let state = workbench_shell::workspace::entity_state(&entity).clone();
        widgets.extend(
            workbench_shell::pane_rows::mint_unfronted(store, &state, &fronted)
                .into_iter()
                .map(|widget| (WidgetOrigin::Row, widget)),
        );
        let folders = ahp_session::session::folders::session_folders(
            store,
            &workbench_shell::workspace::entity_session(&entity),
        );

        let state = workbench_shell::workspace::entity_state(&entity);
        let documents = state.documents();
        let lists = state.lists();
        let peeker = fx.scope(workbench_shell::modal::modal_scope(window), |fx| {
            fx.scope(imba::dyn_view::DynCommand::new::<PeekerCommand>, |fx| {
                Peeker::open(
                    store, ui, documents, lists, viewport, recents, widgets, folders, fx,
                )
            })
        });
        workbench::window::Windows::put(store, window, entity);
        Box::new(peeker)
    }
}

/// The peeker as a plugin: the toggle command and the go-to-file
/// face on the MOUNTED road — the peeker whole (query, results,
/// preview, the FSP search behind them), answered as an external
/// place arm. The peeker crate sits above himark, so himark cannot
/// name it itself.
pub struct PeekerPlugin;

impl himark::plugin::Plugin for PeekerPlugin {
    fn install(&self, app: &mut Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(TogglePeeker));
        app.register_bridge_arm(std::sync::Arc::new(|app, place| {
            let place = place.place::<hiahp::places::PeekerPlace>()?;
            let key = place.0.clone();
            app.mount(move |store, ui, id, fx| {
                let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
                let documents = state.documents();
                let lists = state.lists();
                let recents = recents::RecentLocations::list(store, state.recents());
                let folders = ahp_session::session::folders::session_folders(store, &key);
                let view = fx.scope(
                    move |command: PeekerCommand| {
                        himark::app::AppCommand::ToView(id, imba::dyn_view::DynCommand::new(command))
                    },
                    |fx| {
                        Peeker::open(
                            store,
                            ui,
                            documents,
                            lists,
                            // The desktop go-to sheet's viewport (engine
                            // pixels) — near-full-window glass; display
                            // relays out live, this seeds the preview's
                            // wrap width.
                            gfx::Size::new(3_100.0, 1_850.0),
                            recents,
                            Vec::new(),
                            folders,
                            fx,
                        )
                        // The island's glass is the sheet — the peeker
                        // keeps only its hairlines.
                        .unfloored()
                    },
                );
                    Some(himark::mounts::dyn_modal_mount(id, Box::new(view)))
            })
        }));
    }
}

pub struct TogglePeeker;

impl himark::commands::HimarkCommand for TogglePeeker {
    fn id(&self) -> &'static str {
        "peeker.toggle"
    }
    fn name(&self) -> String {
        "Go to Document".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = workbench_shell::app::bound_window(self.id(), bound) else {
            return;
        };
        let entity =
            workbench::window::Windows::window_ref(store, window).expect("the window entity");
        if entity.has_modal() {
            // Toggle: any standing modal is dismissed; the peeker
            // opens only over a clear window.
            let mut entity = entity.clone();
            fx.scope(
                move |command| workbench_shell::app::content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            workbench::window::Windows::put(store, window, entity);
            return;
        }
        let ui = ui;
        let modal = build(store, &ui, window, fx);
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(
            move |command| workbench_shell::app::content(window, command),
            |fx| entity.show_modal(store, modal, fx),
        );
        workbench::window::Windows::put(store, window, entity);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn labels(app: &Himark) -> Option<Vec<String>> {
    Some(peeker_of(app)?.labels(app.store()))
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn preview_height(app: &Himark, store: &Store) -> Option<f32> {
    peeker_of(app)?.preview_height(store)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn previewed_widget(app: &Himark) -> Option<String> {
    peeker_of(app)?.previewed_widget_title(app.store())
}

#[cfg(any(test, feature = "test-support"))]
fn peeker_of(app: &Himark) -> Option<&Peeker> {
    { use workbench_shell::test_support::TestSupport as _; app.plugin_modal() }?.as_any().downcast_ref::<Peeker>()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod widget_tests;

#[cfg(test)]
mod workspace_tests;

#[cfg(test)]
mod utf8_field_repro;
