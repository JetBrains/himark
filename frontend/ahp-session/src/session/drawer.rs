// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_types::state::SessionSummary;
use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;
use hikit::forest::TreeRow;
use hikit::list_keyboard::ListKeyCommand;
use hikit::list_keyboard::ListKeyboardController;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use hikit::tree_item::TreeLabel;
use hikit::tree_item::TreeListCommand;
use imba::list::ActivateTrigger;
use imba::list::ListOps;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    list::{ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View, Widget,
};
use skia_safe::{Rect, Size};

use super::agents::Agents;
use super::state::HostStatus;
use super::summary::{self, SessionActivity};
use ::editor::{editor_view::EditorCommand, editor_view::EditorView};

const PANEL_WIDTH: f32 = hikit::rows::DRAWER_WIDTH;
const PANEL_PAD: f32 = 6.0;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AgentKey {
    Server(HostId),
    Folder(HostId, Vec<String>),
    Session(HostId, SessionUri),

    NewSession(HostId),

    Note(HostId),

    AddHost,
}

type TreeList = ScrollView<ListView<TreeRow, AgentKey>>;

/// Speed-search over the drawer's folder and session rows — chrome
/// rows (hosts, notes, the "+ …" affordances) stay out of the match
/// set. Keys ascend with the rows, so zipping pairs each key with its
/// row's visible label.
#[derive(Clone)]
struct SessionSearcher;

impl hikit::list_keyboard::Searcher for SessionSearcher {
    type View = TreeList;
    type Key = AgentKey;

    fn capture(&self, view: &Self::View) -> hikit::list_keyboard::ItemSource<AgentKey> {
        let keys = view.content().structure_keys().ordered_keys();
        let labels: Vec<String> = view
            .content()
            .rows()
            .map(|row| row.inner().text().to_owned())
            .collect();
        let items: Vec<(String, AgentKey)> = keys
            .into_iter()
            .zip(labels)
            .filter(|(key, _)| matches!(key, AgentKey::Session(..) | AgentKey::Folder(..)))
            .map(|(key, label)| (label, key))
            .collect();
        Box::new(move || items)
    }

    fn generation(&self, view: &Self::View) -> u64 {
        view.content().generation()
    }
}

/// TEST SUPPORT: no production caller outside this crate.
#[derive(Clone)]
#[doc(hidden)]
pub enum AgentsCommand {
    Rows(ListKeyCommand<TreeListCommand>),

    /// The paint-side ask: the panel's rows lag the catalog — rebuild
    /// them. The catalog itself is kept true by the resident
    /// subscription (`ahp_session::session::driver`); the drawer only
    /// dresses it.
    Boot,

    Dismiss,

    AddHostInput(EditorCommand),

    SubmitAddHost,

    CancelAddHost,
}

impl std::fmt::Display for AgentsCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentsCommand::Rows(command) => command.fmt(out),
            AgentsCommand::AddHostInput(command) => command.fmt(out),
            AgentsCommand::Boot => out.write_str("agents boot"),
            AgentsCommand::Dismiss => out.write_str("agents dismiss"),
            AgentsCommand::SubmitAddHost => out.write_str("submit add host"),
            AgentsCommand::CancelAddHost => out.write_str("cancel add host"),
        }
    }
}

pub struct AgentsPanel {
    list: ListKeyboardController<TreeList, SessionSearcher>,
    asks: Arc<DrawerAsks>,
    /// The catalog generation the rows were last built from — the
    /// paint-side ask fires while this lags `Hosts::generation`.
    seen: Option<u64>,

    collapsed: rpds::HashTrieSetSync<HostId>,
    folded: rpds::HashTrieSetSync<(HostId, Vec<String>)>,

    adding: Option<EditorView>,
    request: Option<ModalRequest>,
}

impl Clone for AgentsPanel {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            asks: Arc::clone(&self.asks),
            seen: self.seen,
            collapsed: self.collapsed.clone(),
            folded: self.folded.clone(),
            adding: self.adding.clone(),

            request: None,
        }
    }
}

/// How the drawer ASKS the shell. Each gesture becomes a Verb the
/// shell prepared at open — the panel never names a window or the
/// shell's command stream; it only hands these verbs to the modal
/// drain.
pub struct DrawerAsks {
    /// The session the panel's window has open, for the
    /// highlight-and-land walk.
    pub open_session: Arc<dyn Fn(&Store) -> Option<ahp_wire::SessionId> + Send + Sync>,
    /// Open an existing session row.
    pub open_row: Arc<dyn Fn(HostId, SessionUri) -> imba::command::Verb + Send + Sync>,
    /// Start a NEW session on a host.
    pub new_session: Arc<dyn Fn(&Store, HostId) -> imba::command::Verb + Send + Sync>,
    /// Submit an add-host URL.
    pub add_host: Arc<dyn Fn(String) -> imba::command::Verb + Send + Sync>,
}

impl AgentsPanel {
    pub fn open(store: &Store, ui: &UiCtx, asks: Arc<DrawerAsks>) -> Self {
        let panel = Self {
            list: ListKeyboardController::searchable(
                ScrollView::new(
                    ListView::empty().with_selection(hikit::rows::selection_style(store)),
                ),
                SessionSearcher,
                store,
                ui,
                ::editor::env::Fonts::of(store),
            )
            .with_folds(),
            asks,
            seen: None,
            collapsed: rpds::HashTrieSetSync::new_sync(),
            folded: rpds::HashTrieSetSync::new_sync(),
            adding: None,
            request: None,
        };
        // The first fill happens on Boot (perform has the UiCtx the
        // row measurement needs).
        panel
    }

    pub fn rows(&self) -> Vec<(String, usize)> {
        self.list
            .inner()
            .content()
            .rows()
            .map(|row| {
                let label = row.inner();
                let text = match label.badge() {
                    Some(glyph) => format!("{glyph} {}", label.text()),
                    None => label.text().to_owned(),
                };
                (text, usize::from(row.depth()))
            })
            .collect()
    }

    #[doc(hidden)]
    pub fn match_count(&self) -> usize {
        self.list.inner().match_count()
    }

    /// TEST SUPPORT: the command the key table's Down emits while a
    /// query stands.
    #[doc(hidden)]
    pub fn matched_step_rows(&self, delta: isize) -> Option<AgentsCommand> {
        let index = self.list.matched_step_index(delta)?;
        Some(AgentsCommand::Rows(self.list.select_command(index)))
    }

    #[doc(hidden)]
    pub fn selected_row(&self) -> Option<usize> {
        let key = self.list.inner().content().cursor()?;
        let range = self.list.inner().content().row_range(key)?;
        Some(range.start)
    }

    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let cursor = self.list.inner().content().cursor().cloned();
        let mut slice: ListSlice<TreeRow, AgentKey> = ListSlice::new();

        let now = std::time::SystemTime::now();
        let theme = ::editor::env::Themes::of(store);
        let dim = theme.ui().peeker.dim_text.0;
        let (accent, stop) = (theme.ui().chat.accent.0, theme.ui().chat.stop_color.0);
        for (server, record) in Agents::list(store) {
            let expanded = !self.collapsed.contains(&server);
            let label = match &record.status {
                HostStatus::Failed(_) => format!("{} — offline", record.name),
                _ => record.name.clone(),
            };
            slice.push_keyed(
                AgentKey::Server(server),
                hikit::tree_item::TreeItemView::branch(
                    TreeLabel::new(label, false, false),
                    0,
                    expanded,
                )
                .toggling_on_body(),
                store,
                ui,
            );
            if !expanded {
                continue;
            }
            match &record.status {
                HostStatus::Idle | HostStatus::Connecting => {
                    slice.push_keyed(
                        AgentKey::Note(server),
                        hikit::tree_item::TreeItemView::leaf(
                            TreeLabel::new("connecting…".to_owned(), false, true),
                            1,
                        ),
                        store,
                        ui,
                    );
                }
                HostStatus::Failed(error) => {
                    slice.push_keyed(
                        AgentKey::Note(server),
                        hikit::tree_item::TreeItemView::leaf(
                            TreeLabel::new(format!("failed: {error}"), false, true),
                            1,
                        ),
                        store,
                        ui,
                    );
                }
                HostStatus::Connected => {
                    // Sessions gather under their full folder set (order
                    // and duplicates ignored); groups and the folder-less
                    // strays stand most-recent first, recency being the
                    // last message's modified_at.
                    let mut groups: Vec<(Option<Vec<String>>, Vec<&SessionSummary>)> = Vec::new();
                    for summary in record.sessions.iter() {
                        let folder = summary
                            .working_directories
                            .as_ref()
                            .filter(|folders| !folders.is_empty())
                            .map(|folders| {
                                let mut set = folders.clone();
                                set.sort();
                                set.dedup();
                                set
                            });
                        match groups.iter_mut().find(|(held, _)| *held == folder) {
                            Some((_, sessions)) => sessions.push(summary),
                            None => groups.push((folder, vec![summary])),
                        }
                    }
                    for (_, sessions) in groups.iter_mut() {
                        sessions.sort_by_key(|summary| {
                            std::cmp::Reverse(summary::modified_stamp(summary))
                        });
                    }
                    groups.sort_by_key(|(_, sessions)| {
                        std::cmp::Reverse(
                            sessions.first().map(|first| summary::modified_stamp(first)),
                        )
                    });
                    for (folder, sessions) in groups {
                        let depth = match &folder {
                            Some(folder) => {
                                let key = (server, folder.clone());
                                let expanded = !self.folded.contains(&key);
                                slice.push_keyed(
                                    AgentKey::Folder(server, folder.clone()),
                                    hikit::tree_item::TreeItemView::branch(
                                        TreeLabel::new(
                                            super::folders::folders_label(folder),
                                            false,
                                            false,
                                        ),
                                        1,
                                        expanded,
                                    )
                                    .toggling_on_body(),
                                    store,
                                    ui,
                                );
                                if !expanded {
                                    continue;
                                }
                                2
                            }
                            None => 1,
                        };
                        for summary in sessions {
                            slice.push_keyed(
                                AgentKey::Session(
                                    server,
                                    SessionUri::new(summary.resource.clone()),
                                ),
                                hikit::tree_item::TreeItemView::leaf(
                                    TreeLabel::new(summary::label(summary), true, false)
                                        .with_badge(session_badge(summary, accent, stop, dim))
                                        .with_trail(age_trail(now, dim, summary)),
                                    depth,
                                ),
                                store,
                                ui,
                            );
                        }
                    }
                    {
                        slice.push_keyed(
                            AgentKey::NewSession(server),
                            hikit::tree_item::TreeItemView::leaf(
                                TreeLabel::new("+ New Session…".to_owned(), true, false),
                                1,
                            ),
                            store,
                            ui,
                        );
                    }
                }
            }
        }
        slice.push_keyed(
            AgentKey::AddHost,
            hikit::tree_item::TreeItemView::leaf(
                TreeLabel::new("+ Add Host…".to_owned(), true, false),
                0,
            ),
            store,
            ui,
        );
        let len = self.list.inner().content().len();
        self.list
            .inner_mut()
            .content_mut()
            .splice_slice(0..len, slice);
        // The cursor survives a refresh; a fresh list lands on the
        // session the window currently shows.
        let target = cursor.or_else(|| self.open_session_key(store));
        if let Some(key) = target {
            if self.list.inner().content().row_range(&key).is_some() {
                self.list.inner_mut().content_mut().select_only(key);
            }
        }
    }

    /// The row key of the session the panel's window has open, if the
    /// window shows a session at all.
    fn open_session_key(&self, store: &Store) -> Option<AgentKey> {
        let open = (self.asks.open_session)(store)?;
        open.names_session()
            .then(|| AgentKey::Session(open.host, open.session))
    }

    pub fn activate(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        index: usize,
        fx: &mut Effects<'_, AgentsCommand>,
    ) {
        let Some(key) = self.list.inner().content().key_at(index).cloned() else {
            return;
        };
        self.activate_key(store, ui, &key, fx);
    }

    fn activate_key(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: &AgentKey,
        _fx: &mut Effects<'_, AgentsCommand>,
    ) {
        match key {
            AgentKey::Server(server) => {
                match self.collapsed.contains(server) {
                    true => {
                        self.collapsed.remove_mut(server);
                    }
                    false => {
                        self.collapsed.insert_mut(*server);
                    }
                }
                if matches!(
                    Agents::record(store, *server).map(|record| record.status),
                    Some(HostStatus::Failed(_)) | Some(HostStatus::Idle)
                ) && !self.collapsed.contains(server)
                {
                    // The nudge rides the verb drain: connecting is the
                    // resident subscription's job, not the drawer's.
                    self.request = Some(ModalRequest::Perform(imba::command::Verb::Dynamic(
                        std::sync::Arc::new(super::driver::ConnectHost(*server)),
                    )));
                }
                self.refresh(store, ui);
            }
            AgentKey::Folder(server, folder) => {
                let key = (*server, folder.clone());
                match self.folded.contains(&key) {
                    true => {
                        self.folded.remove_mut(&key);
                    }
                    false => {
                        self.folded.insert_mut(key);
                    }
                }
                self.refresh(store, ui);
            }
            AgentKey::Session(server, session) => {
                self.list.inner_mut().content_mut().select_only(key.clone());

                self.request = Some(ModalRequest::Perform((self.asks.open_row)(
                    *server,
                    session.clone(),
                )));
            }
            AgentKey::NewSession(server) => {
                self.request = Some(ModalRequest::Perform((self.asks.new_session)(
                    store, *server,
                )));
            }
            AgentKey::Note(_) => {}
            AgentKey::AddHost => {
                let mut input = EditorView::input(600.0, store, ui, hikit::fonts::source());
                input.focus_text();
                self.adding = Some(input);
            }
        }
    }

    #[doc(hidden)]
    pub fn add_host_text(&self) -> Option<String> {
        let input = self.adding.as_ref()?;
        let text = input.document.text();
        let end = text.byte_count().min(u32::MAX as usize) as u32;
        Some(text.view().substring(0..end))
    }
}

/// The age trail a row shows, dressed: the session's own words for
/// how long ago it moved (`summary::age`), in the dim ink.
fn age_trail(
    now: std::time::SystemTime,
    dim: skia_safe::Color,
    summary: &SessionSummary,
) -> Vec<(String, skia_safe::Color)> {
    summary::age(now, summary)
        .map(|age| vec![(age, dim)])
        .unwrap_or_default()
}

/// The session's activity mark: one glyph in one color, leading the
/// row. Circles tell the session's own pace (○ hollow while a turn is
/// still cooking, ● filled once an answer stands unviewed);
/// punctuation flags the states that want the user (? blocked on an
/// answer, ! the last turn failed). What the states ARE is the
/// session's to say (`summary::activity`) — this is only their dress.
fn session_badge(
    summary: &SessionSummary,
    accent: skia_safe::Color,
    stop: skia_safe::Color,
    dim: skia_safe::Color,
) -> Option<(String, skia_safe::Color)> {
    match summary::activity(summary) {
        SessionActivity::Blocked => Some(("?".to_owned(), accent)),
        SessionActivity::Working => Some(("○".to_owned(), accent)),
        SessionActivity::Failed => Some(("!".to_owned(), stop)),
        SessionActivity::Unviewed => Some(("●".to_owned(), dim)),
        SessionActivity::Quiet => None,
    }
}

impl View for AgentsPanel {
    type Command = AgentsCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, AgentsCommand> {
        use imba::focus::FocusData;
        if let Some(input) = &self.adding {
            let own = FocusData {
                on_key: Some(Box::new(|key, _mods| match key {
                    InputKey::Enter => EventResult::Command(AgentsCommand::SubmitAddHost),
                    InputKey::Escape => EventResult::Command(AgentsCommand::CancelAddHost),
                    _ => EventResult::Ignored,
                })),
                ..FocusData::default()
            };
            return own.merge_under(input.focus_data(store, ui).map(AgentsCommand::AddHostInput));
        }
        // The key table is the controller's; the surface keeps only
        // its own dismissal (and the add-host mode above).
        let searching = self.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(AgentsCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.list.focus_data(store, ui).map(AgentsCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        fx.scope(AgentsCommand::Rows, |fx| self.list.destroy(store, fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            AgentsCommand::Boot => {
                let generation = super::state::Hosts::generation(store);
                if self.seen == Some(generation) {
                    return;
                }
                self.seen = Some(generation);
                self.refresh(store, ui);
            }
            AgentsCommand::Rows(command) => {
                type Rows = ListKeyboardController<TreeList, SessionSearcher>;
                match &command {
                    // The bespoke fold: expansion state lives in the
                    // panel's own collapsed/folded sets.
                    ListKeyCommand::Fold { expand, .. } => {
                        let expand = *expand;
                        let Some(key) = self.list.inner().content().cursor().cloned() else {
                            return;
                        };
                        let expanded = match &key {
                            AgentKey::Server(server) => !self.collapsed.contains(server),
                            AgentKey::Folder(server, folder) => {
                                !self.folded.contains(&(*server, folder.clone()))
                            }
                            _ => return,
                        };
                        if expanded != expand {
                            self.activate_key(store, ui, &key, fx);
                        }
                        return;
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = hikit::tree_item::tree_toggle(inner) {
                            self.activate(store, ui, index, fx);
                            return;
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Rows::activated(&command) {
                    let searching = self.list.searching();
                    self.activate(store, ui, index, fx);
                    match trigger {
                        // The deliberate pick ends the search in the
                        // same stroke.
                        ActivateTrigger::Enter if searching => {
                            return self.perform(
                                store,
                                ui,
                                AgentsCommand::Rows(ListKeyCommand::Clear),
                                fx,
                            );
                        }
                        ActivateTrigger::Enter | ActivateTrigger::Click => {}
                    }
                }
                fx.scope(AgentsCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            AgentsCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
            AgentsCommand::AddHostInput(command) => {
                let Some(input) = self.adding.as_mut() else {
                    return;
                };
                fx.scope(AgentsCommand::AddHostInput, |fx| {
                    input.perform(store, ui, command, fx)
                });
            }
            AgentsCommand::SubmitAddHost => {
                let Some(url) = self.add_host_text() else {
                    return;
                };
                self.adding = None;
                let url = url.trim().to_owned();
                if url.is_empty() {
                    return;
                }

                self.request = Some(ModalRequest::Perform((self.asks.add_host)(url)));
            }
            AgentsCommand::CancelAddHost => {
                self.adding = None;
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            let theme = ::editor::env::Themes::of(store);
            let ui_theme = theme.ui().clone();
            let header = ui_theme.panel.header_height;
            let inset = hikit::rows::panel_inset(&ui_theme);
            let title_font = hikit::fonts::ui_font(ui, ui_theme.panel.title_size);
            let mut panel = container(arena, Size::new(PANEL_WIDTH, size.height));
            let shaper = imba::layout::TextShaper::of(ui);
            let backdrop = leaf::<AgentsCommand>(PANEL_WIDTH, size.height)
                .paint_instead(move |_arena, canvas, rect| {
                    hikit::rows::paint_panel_chrome(
                        &shaper,
                        canvas,
                        rect,
                        &ui_theme,
                        &title_font,
                        "Sessions",
                        "",
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Handled,
                    _ => EventResult::Ignored,
                })
                .hit_opaque();
            panel.place(0.0, 0.0, backdrop);
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    PANEL_WIDTH - inset * 2.0 - 2.0,
                    (size.height - inset * 2.0 - header - PANEL_PAD).max(1.0),
                )),
            )
            .map(AgentsCommand::Rows);
            panel.place(inset + 1.0, inset + header + PANEL_PAD, rows);
            if let Some(input) = &self.adding {
                let search = theme.ui().search.clone();
                let well_height = input.content_height() + 2.0 * hikit::ui::space::S;
                let well_width = (PANEL_WIDTH - inset * 2.0).max(1.0);
                let well_y = size.height - inset - well_height;
                let input_fill = search.input_fill;
                let empty = input.document.text().byte_count() == 0;
                let peeker = theme.ui().peeker.clone();
                let placeholder_font = hikit::fonts::ui_text_font(ui, peeker.hint_size);
                let placeholder_dim = peeker.dim_text.0;
                let hint_size = peeker.hint_size;
                let well = leaf::<AgentsCommand>(well_width, well_height).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_anti_alias(true);
                        paint.set_color(input_fill.0);
                        canvas.draw_round_rect(rect, 6.0, 6.0, &paint);
                        if empty {
                            paint.set_color(placeholder_dim);
                            canvas.draw_str(
                                "ws://host:port/?tkn=…",
                                (
                                    rect.left + 10.0,
                                    rect.top + rect.height() * 0.5 + hint_size * 0.35,
                                ),
                                &placeholder_font,
                                &paint,
                            );
                        }
                    },
                );
                panel.place(inset, well_y, well);
                let inner_height = (well_height - search.input_pad_y * 2.0).max(1.0);
                panel.place(
                    inset + search.input_pad_x,
                    well_y + search.input_pad_y,
                    imba::layout::Layout::layout(
                        input.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(0.0, inner_height),
                            max: Size::new(
                                (well_width - search.input_pad_x * 2.0).max(1.0),
                                inner_height,
                            ),
                        },
                    )
                    .map(AgentsCommand::AddHostInput)
                    .focus_scope(true),
                );
            }

            overlay.place(0.0, 0.0, panel);

            // The key table lives in the controller's own overlay;
            // the surface keeps its add-host mode and dismissal.
            let adding = self.adding.is_some();
            let searching = self.list.searching();
            let keymap = leaf::<AgentsCommand>(size.width, size.height).event(
                move |_arena, event, _size| {
                    if adding {
                        return match event {
                            Event::KeyDown {
                                key: InputKey::Enter,
                                ..
                            } => EventResult::Command(AgentsCommand::SubmitAddHost),
                            Event::KeyDown {
                                key: InputKey::Escape,
                                ..
                            } => EventResult::Command(AgentsCommand::CancelAddHost),
                            _ => EventResult::Ignored,
                        };
                    }
                    match event {
                        Event::KeyDown {
                            key: InputKey::Escape,
                            ..
                        } if !searching => EventResult::Command(AgentsCommand::Dismiss),
                        _ => EventResult::Ignored,
                    }
                },
            );
            overlay.place(0.0, 0.0, keymap);

            let boot = self.seen != Some(super::state::Hosts::generation(store));
            overlay.wrap(move |inner| BootShell { inner, boot })
        })
    }
}

struct BootShell<Inner> {
    inner: Inner,
    boot: bool,
}

impl<'a, Inner: Widget<'a, AgentsCommand>> Widget<'a, AgentsCommand> for BootShell<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, AgentsCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<AgentsCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.boot {
            return result.merge(EventResult::Command(AgentsCommand::Boot));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, AgentsCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

impl ModalView for AgentsPanel {
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
