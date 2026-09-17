// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;
use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    list::{ListCommand, ListView, SeparatorStyle},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, LayoutExt as _, UiCtx, View,
};
use skia_safe::{Paint, Size};

use crate::{
    BuildDocumentEffect, Document, DocumentId, EditorCommand, EditorIdView, FetchDocumentEffect,
    ResourceLocation,
};
use editor::EditorId;

/// The group band's OWN height — the header `ListRow` laid once.
/// Groups and notes declare list heights from this measurement, not
/// from a themed constant.
fn header_band_height(store: &Store, ui: &UiCtx) -> f32 {
    let arena = Arena::default();
    let band: crate::ui::ListRow<'_, GroupCommand> =
        crate::ui::ListRow::new(&arena, crate::ui::RowStyle::header(store, ui)).label("");
    let thunk = imba::Layout::layout(
        band,
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(1_000.0, f32::MAX),
        },
    );
    let height = imba::Thunk::size(&thunk).height;
    drop(thunk);
    height
}

fn trace_repair() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_REPAIR").is_some())
}

fn occurrence_separator(chrome: &crate::theme::SearchChrome) -> SeparatorStyle {
    SeparatorStyle {
        color: chrome.row_separator.0,
        inset: chrome.row_separator_inset,
        thickness: chrome.row_separator_thickness,
    }
}

fn document_separator(chrome: &crate::theme::SearchChrome) -> SeparatorStyle {
    SeparatorStyle {
        color: chrome.group_separator.0,
        inset: 0.0,
        thickness: chrome.group_separator_thickness,
    }
}

pub type ResultRows = ListView<EditorIdView>;

pub type ResultGroups = ListView<ResultGroup, DocumentId>;
pub type ResultsCommand = ListCommand<GroupCommand>;

pub enum GroupCommand {
    Rows(ListCommand<EditorCommand>),

    Open,
}

#[derive(Clone)]
pub struct ResultGroup {
    name: String,

    document: Option<DocumentId>,

    first_match: Option<u32>,
    rows: ResultRows,
}

impl View for ResultGroup {
    type Command = GroupCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, GroupCommand> {
        let own = match self.document.is_some() {
            true => imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
                "workbench.open-in-full",
                "Open File in Full",
                GroupCommand::Open,
            )]),
            false => imba::focus::FocusData::default(),
        };
        self.rows
            .focus_data(store, ui)
            .map(GroupCommand::Rows)
            .merge_under(own)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            GroupCommand::Rows(command) => fx.scope(GroupCommand::Rows, |fx| {
                self.rows.perform(store, ui, command, fx)
            }),

            GroupCommand::Open => {
                let Some(document_id) = self.document else {
                    return;
                };

                let target =
                    crate::OpenDocuments::document_ref(store, document_id).map(|document| {
                        let byte = self
                            .rows
                            .rows()
                            .find(|view| document.focus(view.editor()) == crate::EditorFocus::Text)
                            .map(|view| document.caret_byte(view.editor()))
                            .or(self.first_match)
                            .unwrap_or(0);
                        let mut view = document.text().view();
                        let at = crate::line_col_at(&mut view, byte as usize);
                        at..at
                    });
                crate::AppRequests::push(
                    store,
                    Arc::new(OpenResultDocument {
                        document: document_id,
                        target,
                    }),
                );
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        GroupFrame {
            group: self,
            store,
            ui,
        }
    }
}

struct OpenResultDocument {
    document: DocumentId,

    target: Option<Range<crate::LineCol>>,
}

impl crate::DynamicCommand for OpenResultDocument {
    fn id(&self) -> &'static str {
        "results.apply-open"
    }

    fn name(&self) -> String {
        "Open Search Result".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };

        entity.show_document(store, window, self.document, self.target.clone(), fx);
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub struct GroupSpans {
    pub ranges: Vec<Range<u32>>,

    pub marks: Vec<Range<u32>>,
}

pub struct InstallGroup {
    pub document: DocumentId,
    pub spans: GroupSpans,

    pub prebuilt: Option<PrebuiltRows>,
}

impl InstallGroup {
    pub fn open(document: DocumentId, spans: GroupSpans) -> Self {
        Self {
            document,
            spans,
            prebuilt: None,
        }
    }

    pub fn prebuilt(document: DocumentId, spans: GroupSpans, prebuilt: PrebuiltRows) -> Self {
        Self {
            document,
            spans,
            prebuilt: Some(prebuilt),
        }
    }
}

pub struct PrebuiltRows {
    pub revision: u64,
    pub markup_generation: u64,
    pub rows: Vec<::editor::DocumentLayout>,
}

pub fn snap_ranges(document: &Document, ranges: &[Range<u32>]) -> Vec<Range<u32>> {
    let mut snapped: Vec<Range<u32>> = Vec::new();
    for range in ranges {
        let range = snap_over_instead(document, range.clone());
        match snapped.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => snapped.push(range),
        }
    }
    snapped
}

pub type SpanSource = Arc<dyn Fn(&ResourceLocation, &Document) -> GroupSpans + Send + Sync>;

pub enum LocationListCommand {
    Results(ResultsCommand),

    Fetched {
        generation: u64,
        location: ResourceLocation,
        text: Option<String>,
    },

    Built {
        generation: u64,
        location: ResourceLocation,
        built: crate::BuiltDocument,
    },

    Reshape {
        document: DocumentId,
        row: usize,
        command: EditorCommand,
    },
}

#[derive(Clone)]
struct Installed {
    document: DocumentId,

    order: GroupOrder,

    fragments: editor::FragmentSetId,
    editors: Vec<EditorId>,

    markup: crate::MarkupId,

    matches: Vec<Range<u32>>,
}

#[derive(Clone)]
enum OverflowGroup {
    Open {
        document: DocumentId,
        name: String,
        location: Option<ResourceLocation>,
        ranges: Vec<Range<u32>>,
        matches: Vec<Range<u32>>,
    },

    Built {
        name: String,
        location: ResourceLocation,
        document: Document,
        ranges: Vec<Range<u32>>,
        matches: Vec<Range<u32>>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ListId(u64);

impl ListId {
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct ListEntry {
    pub title: String,
    pub list: imba::scroll::ScrollView<LocationList>,
}

#[derive(Clone, Default)]
pub struct LocationLists(rpds::HashTrieMapSync<u64, ListEntry>);

impl LocationLists {
    pub fn put(store: &mut Store, id: ListId, entry: ListEntry) {
        store.update::<LocationLists>(|lists| {
            lists.0.insert_mut(id.0, entry);
        });
    }

    pub fn take(store: &mut Store, id: ListId) -> Option<ListEntry> {
        let entry = store
            .get::<LocationLists>()
            .and_then(|lists| lists.0.get(&id.0).cloned());
        if entry.is_some() {
            store.update::<LocationLists>(|lists| {
                lists.0.remove_mut(&id.0);
            });
        }
        entry
    }

    pub fn entry_ref(store: &Store, id: ListId) -> Option<&ListEntry> {
        store
            .get::<LocationLists>()
            .and_then(|lists| lists.0.get(&id.0))
    }

    pub fn remove(store: &mut Store, id: ListId) {
        store.update::<LocationLists>(|lists| {
            lists.0.remove_mut(&id.0);
        });
    }

    pub fn titles(store: &Store) -> Vec<(ListId, String)> {
        store
            .get::<LocationLists>()
            .map(|lists| {
                lists
                    .0
                    .iter()
                    .map(|(id, entry)| (ListId(*id), entry.title.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub struct LocationList {
    row_width: std::sync::atomic::AtomicU32,

    results: ResultGroups,
    installed: Vec<Installed>,

    set_locations: rpds::HashTrieSetSync<ResourceLocation>,

    set_generation: u64,

    generation: u64,

    spans: Option<SpanSource>,

    shown: usize,

    display_limit: usize,
    overflow: Vec<OverflowGroup>,

    noted: bool,

    note: fn(usize) -> String,
}

impl Clone for LocationList {
    fn clone(&self) -> Self {
        Self {
            row_width: std::sync::atomic::AtomicU32::new(
                self.row_width.load(std::sync::atomic::Ordering::Relaxed),
            ),
            results: self.results.clone(),
            installed: self.installed.clone(),
            set_locations: self.set_locations.clone(),
            set_generation: self.set_generation,
            generation: self.generation,
            spans: self.spans.clone(),
            shown: self.shown,
            display_limit: self.display_limit,
            overflow: self.overflow.clone(),
            noted: self.noted,
            note: self.note,
        }
    }
}

fn no_note(_shown: usize) -> String {
    String::new()
}

impl Default for LocationList {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationList {
    pub fn new() -> Self {
        Self {
            row_width: std::sync::atomic::AtomicU32::new(600.0f32.to_bits()),
            results: ListView::empty().with_separators(SeparatorStyle::default()),
            installed: Vec::new(),
            set_locations: rpds::HashTrieSetSync::new_sync(),
            set_generation: 0,
            generation: 0,
            spans: None,
            shown: 0,
            display_limit: usize::MAX,
            overflow: Vec::new(),
            noted: false,
            note: no_note,
        }
    }

    pub fn with_budget(limit: usize, note: fn(usize) -> String) -> Self {
        Self {
            display_limit: limit,
            note,
            ..Self::new()
        }
    }

    pub fn result_locations(&self) -> Vec<crate::ResourceLocation> {
        self.set_locations.iter().cloned().collect()
    }

    pub fn set_generation(&self) -> u64 {
        self.set_generation
    }

    pub fn note_locations(&mut self, locations: Vec<crate::ResourceLocation>) {
        for location in locations {
            self.note_location(&location);
        }
    }

    fn note_location(&mut self, location: &crate::ResourceLocation) {
        if !self.set_locations.contains(location) {
            self.set_locations.insert_mut(location.clone());
            self.set_generation += 1;
        }
    }

    pub fn group_names(&self) -> Vec<String> {
        self.results
            .rows()
            .map(|group| group.name.clone())
            .collect()
    }

    pub fn group_heights(&self) -> Vec<f32> {
        self.results
            .rows()
            .map(|group| group.rows.total_height())
            .collect()
    }

    pub fn group_sizes(&self) -> Vec<usize> {
        self.results.rows().map(|group| group.rows.len()).collect()
    }

    pub fn focused_group(&self) -> Option<usize> {
        self.results.focused()
    }

    pub fn group_row_editors(&self, group: usize) -> Vec<(DocumentId, EditorId)> {
        self.installed
            .get(group)
            .map(|install| {
                install
                    .editors
                    .iter()
                    .map(|editor| (install.document, *editor))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn installed_markups(&self) -> Vec<crate::MarkupId> {
        self.installed
            .iter()
            .map(|install| install.markup)
            .collect()
    }

    pub fn install(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        groups: Vec<InstallGroup>,
        spans: Option<SpanSource>,
        fx: &mut imba::effect::Effects<'_, LocationListCommand>,
    ) {
        self.generation += 1;
        self.spans = spans;
        let theme = crate::env::Themes::of(store);

        let retracted = self.retract_installed(store, fonts);

        self.shown = 0;
        self.overflow.clear();
        self.noted = false;
        if !self.set_locations.is_empty() {
            self.set_locations = rpds::HashTrieSetSync::new_sync();
            self.set_generation += 1;
        }

        let mut resolved: Vec<(InstallGroup, String, Option<ResourceLocation>)> = groups
            .into_iter()
            .map(|group| {
                let name = crate::OpenDocuments::name(store, group.document)
                    .unwrap_or_else(|| "untitled".to_owned());
                let location = crate::OpenDocuments::location(store, group.document);
                (group, name, location)
            })
            .collect();
        resolved
            .sort_by(|a, b| group_order(a.2.as_ref(), &a.1).cmp(&group_order(b.2.as_ref(), &b.1)));
        let mut installed: imba::list::ListSlice<ResultGroup, DocumentId> =
            imba::list::ListSlice::new();
        for (group, name, location) in resolved {
            let InstallGroup {
                document: document_id,
                spans: GroupSpans { ranges, marks },
                prebuilt,
            } = group;
            if let Some(location) = &location {
                self.note_location(location);
            }

            if self.shown >= self.display_limit {
                self.overflow.push(OverflowGroup::Open {
                    document: document_id,
                    name,
                    location,
                    ranges,
                    matches: marks,
                });
                continue;
            }
            if let Some((install, group, height)) = self.install_document(
                store,
                ui,
                fonts,
                &theme,
                document_id,
                ranges,
                marks,
                location,
                name,
                prebuilt,
                fx,
            ) {
                self.shown += group.rows.len();
                self.installed.push(install);
                installed.push_keyed_sized(document_id, group, height);
            }
        }
        let panel_width = f32::from_bits(self.row_width.load(std::sync::atomic::Ordering::Relaxed));
        self.results = ListView::from_slice_at(panel_width, installed)
            .with_separators(document_separator(&theme.ui().search));
        if !self.overflow.is_empty() {
            self.push_note(store, ui);
        }

        for document in retracted {
            crate::OpenDocuments::remove_if_editorless(store, document, fx);
        }
    }

    fn retract_installed(
        &mut self,
        store: &mut Store,
        fonts: &skia_safe::textlayout::FontCollection,
    ) -> Vec<DocumentId> {
        let theme = crate::env::Themes::of(store);
        let mut retracted: Vec<DocumentId> = Vec::new();
        for previous in self.installed.drain(..) {
            if let Some(mut document) = crate::OpenDocuments::document(store, previous.document) {
                for editor in &previous.editors {
                    document.remove_editor(*editor);
                }
                document.remove_fragment_set(previous.fragments);
                document.remove_markup(
                    previous.markup,
                    &previous.matches,
                    fonts,
                    &theme,
                    &mut imba::effect::Batch::new().effects(),
                );
                crate::OpenDocuments::put_document(store, previous.document, document);
            }
            retracted.push(previous.document);
        }
        retracted
    }

    pub fn fetch(
        &mut self,
        locations: Vec<ResourceLocation>,
        fx: &mut imba::effect::Effects<'_, LocationListCommand>,
    ) {
        let generation = self.generation;
        for location in locations {
            let landing = location.clone();
            let _ = fx.push(
                imba::effect::AnyEffect::new(FetchDocumentEffect { location }).map(move |text| {
                    LocationListCommand::Fetched {
                        generation,
                        location: landing,
                        text,
                    }
                }),
            );
        }
    }

    pub fn lift_budget(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        fx: &mut imba::effect::Effects<'_, LocationListCommand>,
    ) {
        self.display_limit = usize::MAX;
        self.drop_note();
        let theme = crate::env::Themes::of(store);
        for group in std::mem::take(&mut self.overflow) {
            let (document_id, name, location, ranges, matches) = match group {
                OverflowGroup::Open {
                    document,
                    name,
                    location,
                    ranges,
                    matches,
                } => (document, name, location, ranges, matches),

                OverflowGroup::Built {
                    name,
                    location,
                    document,
                    ranges,
                    matches,
                } => {
                    let revision = document.revision();
                    let id = crate::OpenDocuments::register(
                        store,
                        document,
                        Some(location.clone()),
                        location.name().to_owned(),
                        revision,
                    );
                    (id, name, Some(location), ranges, matches)
                }
            };
            if let Some((install, group, height)) = self.install_document(
                store,
                ui,
                fonts,
                &theme,
                document_id,
                ranges,
                matches,
                location,
                name,
                None,
                fx,
            ) {
                self.shown += group.rows.len();
                let group_index = self.sorted_index(&install.order);
                self.installed.insert(group_index, install);

                let mut lifted: imba::list::ListSlice<ResultGroup, DocumentId> =
                    imba::list::ListSlice::new();
                lifted.push_keyed_sized(document_id, group, height);
                self.results.splice_slice(group_index..group_index, lifted);
            } else {
                crate::OpenDocuments::remove_if_editorless(store, document_id, fx);
            }
        }
    }

    fn sorted_index(&self, order: &GroupOrder) -> usize {
        self.installed.partition_point(|held| held.order <= *order)
    }

    pub fn uninstall(&mut self, store: &mut Store) {
        let fonts = crate::env::Fonts::of(store)();
        self.generation += 1;
        self.spans = None;
        self.shown = 0;
        self.overflow.clear();
        self.noted = false;
        let retracted = self.retract_installed(store, &fonts);
        self.results = ListView::empty();
        let mut batch: imba::effect::Batch<LocationListCommand> = imba::effect::Batch::new();
        for document in retracted {
            crate::OpenDocuments::remove_if_editorless(store, document, &mut batch.effects());
        }
    }

    fn push_note(&mut self, store: &Store, ui: &UiCtx) {
        if self.noted {
            return;
        }
        let note = ResultGroup {
            name: (self.note)(self.shown),
            document: None,
            first_match: None,
            rows: ListView::empty(),
        };
        let index = self.results.len();
        let height = header_band_height(store, ui);
        self.results.splice(index..index, [(note, height)]);
        self.noted = true;
    }

    fn drop_note(&mut self) {
        if !self.noted {
            return;
        }
        let index = self.results.len().saturating_sub(1);
        self.results.splice(index..index + 1, []);
        self.noted = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn install_document(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::Theme,
        document_id: DocumentId,
        ranges: Vec<Range<u32>>,
        matches: Vec<Range<u32>>,
        location: Option<ResourceLocation>,
        name: String,
        prebuilt: Option<PrebuiltRows>,
        fx: &mut imba::effect::Effects<'_, LocationListCommand>,
    ) -> Option<(Installed, ResultGroup, f32)> {
        let mut document = crate::OpenDocuments::document(store, document_id)?;
        let width = self.installed_row_width(store);

        let prebuilt = prebuilt.filter(|prebuilt| {
            prebuilt.revision == document.revision()
                && prebuilt.markup_generation == document.markup_generation()
        });
        let (markup_id, fragment_set, matches, row_editors) = prepare_rows(
            &mut document,
            document_id,
            fonts,
            theme,
            width,
            ranges,
            matches,
            prebuilt,
            fx,
        );
        let mut installed = Installed {
            document: document_id,
            order: group_order(location.as_ref(), &name),
            fragments: fragment_set,
            editors: Vec::new(),
            markup: markup_id,
            matches,
        };
        let mut rows: Vec<(EditorIdView, f32)> = Vec::new();
        for (editor, height) in row_editors {
            installed.editors.push(editor);
            rows.push((EditorIdView::new(document_id, editor), height));
        }
        crate::OpenDocuments::put_document(store, document_id, document);
        if rows.is_empty() {
            return None;
        }
        let installed_matches_first = installed.matches.first().map(|range| range.start);
        let theme = crate::env::Themes::of(store);
        let chrome = &theme.ui().search;
        let list = ListView::from_rope_at(width, imba::list::measured(rows))
            .with_separators(occurrence_separator(chrome));
        let height = header_band_height(store, ui) + list.total_height();
        Some((
            installed,
            ResultGroup {
                name,
                document: Some(document_id),
                first_match: installed_matches_first,
                rows: list,
            },
            height,
        ))
    }

    pub fn install_width(&self, store: &Store) -> f32 {
        self.installed_row_width(store)
    }

    pub fn note_panel_width(&self, store: &Store, width: f32) {
        let inset = crate::env::Themes::of(store).ui().search.group_text_x;
        self.row_width.store(
            (width - inset).max(1.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    fn installed_row_width(&self, store: &Store) -> f32 {
        EditorIdView::editor_width(
            f32::from_bits(self.row_width.load(std::sync::atomic::Ordering::Relaxed)),
            &crate::env::Themes::of(store).ui().window,
        )
    }
}

fn lift_reshape(document: DocumentId, command: ListCommand<GroupCommand>) -> LocationListCommand {
    match command {
        ListCommand::Child(_, GroupCommand::Rows(ListCommand::Child(row, command))) => {
            LocationListCommand::Reshape {
                document,
                row,
                command,
            }
        }
        other => LocationListCommand::Results(other),
    }
}

pub fn sort_locations(locations: &mut [crate::ResourceLocation]) {
    locations.sort_by_cached_key(|location| group_order(Some(location), location.name()));
}

type GroupOrder = (u8, Vec<(u8, String)>);

fn group_order(location: Option<&ResourceLocation>, name: &str) -> GroupOrder {
    match location {
        Some(location) => {
            let path = location.path();
            let key = path
                .iter()
                .enumerate()
                .map(|(index, segment)| {
                    let terminal = index + 1 == path.len();
                    (terminal as u8, segment.clone())
                })
                .collect();
            (0, key)
        }
        None => (1, vec![(1, name.to_owned())]),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_rows(
    document: &mut Document,
    document_id: DocumentId,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::Theme,
    width: f32,
    ranges: Vec<Range<u32>>,
    matches: Vec<Range<u32>>,
    prebuilt: Option<PrebuiltRows>,
    fx: &mut imba::effect::Effects<'_, LocationListCommand>,
) -> (
    crate::MarkupId,
    editor::FragmentSetId,
    Vec<Range<u32>>,
    Vec<(EditorId, f32)>,
) {
    let markup_id = document.add_markup();
    let mut tints = crate::Markup::new();
    for range in &matches {
        tints.push_styled(range.clone(), crate::StyleId::Match);
    }

    fx.scope(
        move |command| LocationListCommand::Reshape {
            document: document_id,
            row: 0,
            command,
        },
        |fx| document.replace_markup(markup_id, tints, &matches, fonts, theme, fx),
    );
    let snapped = snap_ranges(document, &ranges);

    let mut prebuilt_rows = prebuilt
        .filter(|prebuilt| prebuilt.rows.len() == snapped.len())
        .map(|prebuilt| prebuilt.rows.into_iter());
    let fragment_set = document.add_fragment_set();
    let mut rows = Vec::new();
    for range in snapped {
        let fragment = document.add_fragment(fragment_set, range);
        let row_index = rows.len();

        let build = match prebuilt_rows.as_mut().and_then(Iterator::next) {
            Some(layout) => ::editor::EditorBuild::Prebuilt(layout),
            None => ::editor::EditorBuild::Bounded,
        };
        let editor = fx.scope(
            move |command| LocationListCommand::Reshape {
                document: document_id,
                row: row_index,
                command,
            },
            |fx| document.add_editor(width, Some(fragment), build, &[], fonts, theme, fx),
        );

        document.show_markup(editor, markup_id);

        let height = document.content_height(editor).max(24.0);
        rows.push((editor, height));
    }
    (markup_id, fragment_set, matches, rows)
}

impl View for LocationList {
    type Command = LocationListCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, LocationListCommand> {
        self.results
            .focus_data(store, ui)
            .map(LocationListCommand::Results)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        let fonts = crate::env::Fonts::of(store)();
        self.generation += 1;
        self.spans = None;
        self.shown = 0;
        self.overflow.clear();
        self.noted = false;
        let retracted = self.retract_installed(store, &fonts);
        self.results = ListView::empty();
        for document in retracted {
            crate::OpenDocuments::remove_if_editorless(store, document, fx);
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            LocationListCommand::Results(command) => fx.scope(LocationListCommand::Results, |fx| {
                self.results.perform(store, ui, command, fx)
            }),

            LocationListCommand::Reshape {
                document,
                row,
                command,
            } => {
                if trace_repair() {
                    eprintln!("[repair] RESHAPE ARRIVED {document:?} row {row}");
                }

                if let Some(group) = self.results.row_range(&document).map(|range| range.start) {
                    fx.scope(
                        move |command| lift_reshape(document, command),
                        |fx| {
                            self.results.perform(
                                store,
                                ui,
                                ListCommand::Child(
                                    group,
                                    GroupCommand::Rows(ListCommand::Child(row, command)),
                                ),
                                fx,
                            )
                        },
                    );
                } else if trace_repair() {
                    eprintln!("[repair] RESHAPE DROP {document:?} (group left)");
                }
            }
            LocationListCommand::Fetched {
                generation,
                location,
                text,
            } => {
                if generation != self.generation {
                    return;
                }
                let Some(text) = text else {
                    return;
                };

                let landing = location.clone();
                let prep = self.spans.clone().map(|spans| crate::RowPrep {
                    spans,
                    width: self.installed_row_width(store),
                });
                let _ = fx.push(
                    imba::effect::AnyEffect::new(BuildDocumentEffect {
                        location,
                        text,
                        prep,
                    })
                    .map(move |built| LocationListCommand::Built {
                        generation,
                        location: landing,
                        built,
                    }),
                );
            }
            LocationListCommand::Built {
                generation,
                location,
                built,
            } => {
                if generation != self.generation {
                    return;
                }
                let crate::BuiltDocument {
                    document,
                    spans,
                    prebuilt,
                } = built;

                let Some(GroupSpans { ranges, marks }) = spans else {
                    return;
                };
                if ranges.is_empty() {
                    return;
                }
                self.note_location(&location);
                let name = location.name().to_owned();

                if self.shown >= self.display_limit {
                    self.overflow.push(OverflowGroup::Built {
                        name,
                        location,
                        document,
                        ranges,
                        matches: marks,
                    });
                    self.push_note(store, ui);
                    return;
                }

                let revision = document.revision();
                let document_id = crate::OpenDocuments::register(
                    store,
                    document,
                    Some(location.clone()),
                    location.name().to_owned(),
                    revision,
                );

                let fonts = crate::env::ui_collection(store, ui);
                let theme = crate::env::Themes::of(store);
                match self.install_document(
                    store,
                    ui,
                    &fonts,
                    &theme,
                    document_id,
                    ranges,
                    marks,
                    Some(location),
                    name,
                    prebuilt,
                    fx,
                ) {
                    Some((install, group, height)) => {
                        self.shown += group.rows.len();

                        let group_index = self.sorted_index(&install.order);
                        self.installed.insert(group_index, install);
                        self.results
                            .splice(group_index..group_index, [(group, height)]);
                    }
                    None => {
                        crate::OpenDocuments::remove_if_editorless(store, document_id, fx);
                    }
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let inset = crate::env::Themes::of(store).ui().search.group_text_x;
            self.row_width.store(
                (constraints.max.width - inset).max(1.0).to_bits(),
                std::sync::atomic::Ordering::Relaxed,
            );
            imba::Layout::layout(self.results.display(arena, store, ui), arena, constraints)
                .map(LocationListCommand::Results)
        })
    }
}

fn snap_over_instead(document: &Document, mut range: Range<u32>) -> Range<u32> {
    loop {
        let mut widened = range.clone();
        for interval in document.markup().all_inlays_in(range.clone()) {
            if matches!(interval.inlay.mode(), crate::InlayMode::Instead(_)) {
                widened.start = widened.start.min(interval.range.start);
                widened.end = widened.end.max(interval.range.end);
            }
        }
        if widened == range {
            return range;
        }
        range = widened;
    }
}

pub enum ListPanelCommand {
    List(imba::scroll::ScrollCommand<LocationListCommand>),
}

#[derive(Clone, Copy)]
pub struct ListPanel {
    id: ListId,
}

impl ListPanel {
    pub fn new(id: ListId) -> Self {
        Self { id }
    }

    #[doc(hidden)]
    pub fn list_id(&self) -> ListId {
        self.id
    }
}

impl View for ListPanel {
    type Command = ListPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ListPanelCommand> {
        match LocationLists::entry_ref(store, self.id) {
            Some(entry) => entry.list.focus_data(store, ui).map(ListPanelCommand::List),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            ListPanelCommand::List(command) => {
                let Some(mut entry) = LocationLists::take(store, self.id) else {
                    return;
                };
                fx.scope(ListPanelCommand::List, |fx| {
                    entry.list.perform(store, ui, command, fx)
                });
                LocationLists::put(store, self.id, entry);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let chrome = crate::env::Themes::of(store).ui().search.clone();
            let mut panel = imba::container::container(arena, size);
            let title = LocationLists::entry_ref(store, self.id)
                .map(|entry| entry.title.clone())
                .unwrap_or_else(|| "Results".to_owned());
            // The band sizes ITSELF; the rows start where it ends.
            let band = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
                .label(title)
                .backdrop(
                    crate::ui::Surface::fill(chrome.group_fill.0)
                        .radius(0.0)
                        .painter(),
                )
                .layout(
                    arena,
                    Constraints {
                        min: Size::new(size.width, 0.0),
                        max: Size::new(size.width, f32::MAX),
                    },
                );
            let header = imba::Thunk::size(&band).height;
            panel.place_boxed(0.0, 0.0, band);
            match LocationLists::entry_ref(store, self.id) {
                Some(entry) => panel.place(
                    0.0,
                    header,
                    imba::Layout::layout(
                        entry.list.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(size.width, (size.height - header).max(1.0))),
                    )
                    .map(ListPanelCommand::List),
                ),
                None => panel.place(
                    0.0,
                    header,
                    imba::leaf::leaf::<ListPanelCommand>(
                        size.width,
                        (size.height - header).max(1.0),
                    ),
                ),
            }
            panel.wrap_realized(move |panel| ListPanelWidget { panel, size })
        })
    }
}

struct ListPanelWidget<'a> {
    panel: imba::container::RealizedContainer<'a, ListPanelCommand>,
    size: Size,
}

impl<'a> imba::Widget<'a, ListPanelCommand> for ListPanelWidget<'a> {
    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, ListPanelCommand>> {
        self.panel.overlays()
    }

    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<ListPanelCommand> {
        use imba::event::Event;
        match event {
            Event::Paint { .. }
            | Event::AnimationClock { .. }
            | Event::ThemeChanged
            | Event::MouseDown { .. }
            | Event::Scroll { .. } => self.panel.handle_event(arena, event, viewport),

            _ => self.panel.route_to(1, arena, event, viewport),
        }
    }

    fn layout_data<'w>(&'w mut self) -> imba::focus::LayoutData<'w, ListPanelCommand>
    where
        'a: 'w,
    {
        self.panel.layout_data()
    }
}

impl crate::PanelView for ListPanel {
    type Place = crate::NoPlace;

    fn title(&self, store: &Store) -> String {
        LocationLists::entry_ref(store, self.id)
            .map(|entry| entry.title.clone())
            .unwrap_or_else(|| "Results".to_owned())
    }

    fn dismantle(&mut self, store: &mut Store) {
        let Some(mut entry) = LocationLists::take(store, self.id) else {
            return;
        };
        entry.list.content_mut().uninstall(store);
    }

    fn family_row(&self) -> Option<crate::FamilyRow> {
        Some(crate::FamilyRow::List(self.id))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn drawer_view(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
    ) -> Option<Box<dyn crate::ModalView>> {
        let entry = LocationLists::entry_ref(store, self.id)?;
        let locations = entry.list.content().result_locations();
        crate::TocView::for_locations(store, ui, window, &locations)
            .map(|view| Box::new(view) as Box<dyn crate::ModalView>)
    }
}

/// One search-result GROUP, reified (docs/UI.md stage 2): the header
/// band — fill, hairlines, name, and the OPEN action — over the
/// occurrence rows, stacked in a `Column`.
struct GroupFrame<'a> {
    group: &'a ResultGroup,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for GroupFrame<'_> {}

impl<'a> imba::Layout<'a, GroupCommand> for GroupFrame<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, GroupCommand> {
        use imba::thunk_ext::ThunkExt;
        use imba::LayoutExt as _;
        let GroupFrame { group, store, ui } = self;
        let width = constraints.max.width;
        let chrome = crate::env::Themes::of(store).ui().search.clone();
        let rows = imba::Layout::layout(
            group.rows.display(arena, store, ui),
            arena,
            Constraints {
                min: Size::default(),
                max: Size::new((width - chrome.group_text_x).max(120.0), f32::MAX),
            },
        );

        let mut band = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
            .label(group.name.clone());
        if group.document.is_some() {
            band = band.action("OPEN", || GroupCommand::Open);
        }
        let fill = chrome.group_fill.0;
        let separator = chrome.group_separator.0;
        let header = band.backdrop(
            move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(fill);
                canvas.draw_rect(rect, &paint);
                paint.set_color(separator);
                for y in [rect.top, rect.bottom - 1.0] {
                    canvas.draw_rect(
                        skia_safe::Rect::from_xywh(rect.left, y, rect.width(), 1.0),
                        &paint,
                    );
                }
            },
        );

        let column = imba::Column::new(arena).child(header).child(
            imba::fixed(rows.map(GroupCommand::Rows)).pad_insets(imba::Insets {
                left: chrome.group_text_x,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            }),
        );
        column.layout(arena, constraints)
    }
}
