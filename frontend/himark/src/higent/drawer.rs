// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::{
    ConnectServerEffect, HostId, ListSessionsEffect, PollServerEffect, RootInfo, ServerEvent,
    SessionsPage,
};
use crate::{
    AppCommand, ModalRequest, ModalView, SpeedSearchCommand, SpeedSearchView, TreeLabel,
    TreeListCommand, TreeRow,
};
use ahp_types::common::Uri;
use ahp_types::state::SessionSummary;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, CancellationToken, Effects},
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    list::{ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View, Widget,
};
use skia_safe::{Rect, Size};

use crate::higent::session::{Agents, HostStatus, OpenSessionRow};
use ::editor::{EditorCommand, EditorView};

const PANEL_WIDTH: f32 = crate::DRAWER_WIDTH;
const PANEL_PAD: f32 = 6.0;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AgentKey {
    Server(HostId),
    Folder(HostId, Vec<String>),
    Session(HostId, Uri),

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

impl crate::Searcher for SessionSearcher {
    type View = TreeList;
    type Key = AgentKey;

    fn capture(&self, view: &Self::View) -> crate::ItemSource<AgentKey> {
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

pub enum AgentsCommand {
    Rows(SpeedSearchCommand<TreeListCommand>),

    Boot,
    Connected(HostId, Result<RootInfo, String>),
    Listed {
        server: HostId,
        first: bool,
        result: Result<SessionsPage, String>,
    },

    Events(HostId, Vec<ServerEvent>),

    Select(isize),

    Fold(bool),

    Pick,

    Dismiss,

    AddHostInput(EditorCommand),

    SubmitAddHost,

    CancelAddHost,
}

pub struct AgentsPanel {
    list: SpeedSearchView<TreeList, SessionSearcher>,
    window: crate::WindowId,
    booted: bool,

    collapsed: rpds::HashTrieSetSync<HostId>,
    folded: rpds::HashTrieSetSync<(HostId, Vec<String>)>,

    polls: rpds::HashTrieMapSync<HostId, CancellationToken>,

    adding: Option<EditorView>,
    request: Option<ModalRequest>,
}

impl Clone for AgentsPanel {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            window: self.window,
            booted: self.booted,
            collapsed: self.collapsed.clone(),
            folded: self.folded.clone(),
            polls: self.polls.clone(),
            adding: self.adding.clone(),

            request: None,
        }
    }
}

impl AgentsPanel {
    pub fn open(store: &Store, ui: &UiCtx, window: crate::WindowId) -> Self {
        let panel = Self {
            list: SpeedSearchView::new(
                ScrollView::new(ListView::empty().with_selection(crate::selection_style(store))),
                SessionSearcher,
                store,
                ui,
                crate::env::Fonts::of(store),
            ),
            window,
            booted: false,
            collapsed: rpds::HashTrieSetSync::new_sync(),
            folded: rpds::HashTrieSetSync::new_sync(),
            polls: rpds::HashTrieMapSync::new_sync(),
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
        use imba::list::SearchableList;
        self.list.inner().match_count()
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
        let theme = crate::env::Themes::of(store);
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
                crate::TreeItemView::branch(TreeLabel::new(label, false, false), 0, expanded)
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
                        crate::TreeItemView::leaf(
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
                        crate::TreeItemView::leaf(
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
                        sessions.sort_by_key(|summary| std::cmp::Reverse(modified_stamp(summary)));
                    }
                    groups.sort_by_key(|(_, sessions)| {
                        std::cmp::Reverse(sessions.first().map(|first| modified_stamp(first)))
                    });
                    for (folder, sessions) in groups {
                        let depth = match &folder {
                            Some(folder) => {
                                let key = (server, folder.clone());
                                let expanded = !self.folded.contains(&key);
                                slice.push_keyed(
                                    AgentKey::Folder(server, folder.clone()),
                                    crate::TreeItemView::branch(
                                        TreeLabel::new(folders_label(folder), false, false),
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
                                AgentKey::Session(server, summary.resource.clone()),
                                crate::TreeItemView::leaf(
                                    TreeLabel::new(session_label(summary), true, false)
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
                            crate::TreeItemView::leaf(
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
            crate::TreeItemView::leaf(TreeLabel::new("+ Add Host…".to_owned(), true, false), 0),
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
        let open = crate::Windows::window_ref(store, self.window)?.current_session();
        open.names_session()
            .then(|| AgentKey::Session(open.host, open.session))
    }

    fn connect(&mut self, store: &mut Store, server: HostId, fx: &mut Effects<'_, AgentsCommand>) {
        let Some(seat) = crate::higent::Servers::seat(store, server) else {
            Agents::set_status(store, server, HostStatus::Failed("unregistered".to_owned()));
            return;
        };
        Agents::set_status(store, server, HostStatus::Connecting);
        fx.push(
            AnyEffect::new(ConnectServerEffect { seat })
                .map(move |result| AgentsCommand::Connected(server, result)),
        );
    }

    fn list_sessions(
        &mut self,
        store: &Store,
        server: HostId,
        cursor: Option<String>,
        fx: &mut Effects<'_, AgentsCommand>,
    ) {
        let Some(seat) = crate::higent::Servers::seat(store, server) else {
            return;
        };
        let first = cursor.is_none();
        fx.push(
            AnyEffect::new(ListSessionsEffect { seat, cursor }).map(move |result| {
                AgentsCommand::Listed {
                    server,
                    first,
                    result,
                }
            }),
        );
    }

    fn relaunch_poll(
        &mut self,
        store: &Store,
        server: HostId,
        fx: &mut Effects<'_, AgentsCommand>,
    ) {
        let Some(seat) = crate::higent::Servers::seat(store, server) else {
            return;
        };
        if let Some(token) = self.polls.get(&server).copied() {
            fx.cancel(token);
        }
        let token = fx.push(
            AnyEffect::new(PollServerEffect { seat })
                .map(move |events| AgentsCommand::Events(server, events)),
        );
        self.polls.insert_mut(server, token);
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
        fx: &mut Effects<'_, AgentsCommand>,
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
                    self.connect(store, *server, fx);
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

                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(OpenSessionRow {
                        server: *server,
                        session: session.clone(),
                    }),
                )));
            }
            AgentKey::NewSession(server) => {
                let command: Arc<dyn crate::DynamicCommand> = match Agents::new_session_flow(store)
                {
                    Some(flow) => flow(*server),
                    None => Arc::new(crate::new_session::OpenNewSession {
                        host: Some(*server),
                    }),
                };
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    command,
                )));
            }
            AgentKey::Note(_) => {}
            AgentKey::AddHost => {
                let mut input = EditorView::input(600.0, store, ui, crate::fonts::source());
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

fn age_trail(
    now: std::time::SystemTime,
    dim: skia_safe::Color,
    summary: &SessionSummary,
) -> Vec<(String, skia_safe::Color)> {
    let Ok(stamp) = humantime::parse_rfc3339_weak(&summary.modified_at) else {
        return Vec::new();
    };
    let Ok(elapsed) = now.duration_since(stamp) else {
        return Vec::new();
    };
    let seconds = elapsed.as_secs();
    let age = match seconds {
        0..=59 => "now".to_owned(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    };
    vec![(age, dim)]
}

/// The stamp the recency order runs on — the summary's modified_at
/// moves on every message, ours or the agent's. Unparseable stamps
/// sink to the epoch, so fresh sessions never hide below them.
fn modified_stamp(summary: &SessionSummary) -> std::time::SystemTime {
    humantime::parse_rfc3339_weak(&summary.modified_at).unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

fn folder_label(folder: &str) -> String {
    let trimmed = folder.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().filter(|name| !name.is_empty());
    name.unwrap_or(trimmed).to_owned()
}

fn folders_label(folders: &[String]) -> String {
    folders
        .iter()
        .map(|folder| folder_label(folder))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The session's activity mark: one glyph in one color, leading the
/// row. Circles tell the session's own pace (○ hollow while a turn is
/// still cooking, ● filled once an answer stands unviewed);
/// punctuation flags the states that want the user (? blocked on an
/// answer, ! the last turn failed).
fn session_badge(
    summary: &SessionSummary,
    accent: skia_safe::Color,
    stop: skia_safe::Color,
    dim: skia_safe::Color,
) -> Option<(String, skia_safe::Color)> {
    let status = summary.status;
    // InputNeeded contains the InProgress bit — ask before running.
    if status & 24 == 24 {
        return Some(("?".to_owned(), accent));
    }
    if status & 8 != 0 {
        return Some(("○".to_owned(), accent));
    }
    if status & 2 != 0 {
        return Some(("!".to_owned(), stop));
    }
    if status & 32 == 0 {
        return Some(("●".to_owned(), dim));
    }
    None
}

fn session_label(summary: &SessionSummary) -> String {
    let mut label = String::new();
    label.push_str(&summary.title);
    if let Some(activity) = summary
        .activity
        .as_ref()
        .filter(|_| summary.status & 8 != 0)
    {
        label.push_str(" · ");
        label.push_str(activity);
    }
    if let Some(changes) = &summary.changes {
        if let (Some(additions), Some(deletions)) = (changes.additions, changes.deletions) {
            if additions > 0 || deletions > 0 {
                label.push_str(&format!("  +{additions} -{deletions}"));
            }
        }
    }
    label
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
        let searching = self.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(AgentsCommand::Dismiss),
                InputKey::Up if !searching => EventResult::Command(AgentsCommand::Select(-1)),
                InputKey::Down if !searching => EventResult::Command(AgentsCommand::Select(1)),
                InputKey::Left if !searching => EventResult::Command(AgentsCommand::Fold(false)),
                InputKey::Right if !searching => EventResult::Command(AgentsCommand::Fold(true)),
                InputKey::Enter if searching => EventResult::Commands(vec![
                    AgentsCommand::Pick,
                    AgentsCommand::Rows(SpeedSearchCommand::Clear),
                ]),
                InputKey::Enter => EventResult::Command(AgentsCommand::Pick),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.list.focus_data(store, ui).map(AgentsCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        for (_, token) in self.polls.iter() {
            fx.cancel(*token);
        }
        self.polls = rpds::HashTrieMapSync::new_sync();
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
                if self.booted {
                    return;
                }
                self.booted = true;
                for (server, record) in Agents::list(store) {
                    if matches!(record.status, HostStatus::Connected) {
                        self.list_sessions(store, server, None, fx);
                        self.relaunch_poll(store, server, fx);
                    } else {
                        self.connect(store, server, fx);
                    }
                }
                self.refresh(store, ui);
            }
            AgentsCommand::Connected(server, result) => {
                match result {
                    Ok(info) => {
                        Agents::set_agents(store, server, info.agents);
                        Agents::set_status(store, server, HostStatus::Connected);
                        self.list_sessions(store, server, None, fx);
                        self.relaunch_poll(store, server, fx);
                    }
                    Err(error) => {
                        Agents::set_status(store, server, HostStatus::Failed(error));
                    }
                }
                self.refresh(store, ui);
            }
            AgentsCommand::Listed {
                server,
                first,
                result,
            } => {
                match result {
                    Ok(page) => {
                        let next = page.next_cursor.clone();
                        Agents::add_sessions(store, server, page.sessions, first);
                        if next.is_some() {
                            self.list_sessions(store, server, next, fx);
                        }
                    }
                    Err(error) => eprintln!("[higent] listSessions failed: {error}"),
                }
                self.refresh(store, ui);
            }
            AgentsCommand::Events(server, events) => {
                for event in events {
                    Agents::apply_event(store, server, event);
                }
                self.relaunch_poll(store, server, fx);
                self.refresh(store, ui);
            }
            AgentsCommand::Rows(command) => {
                if let SpeedSearchCommand::Inner(inner) = &command {
                    if let Some((index, _)) = crate::tree_interaction(inner) {
                        self.activate(store, ui, index, fx);
                        return;
                    }
                }
                fx.scope(AgentsCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            AgentsCommand::Select(delta) => self.list.inner_mut().content_mut().cursor_step(delta),
            AgentsCommand::Fold(expand) => {
                let Some(key) = self.list.inner().content().cursor().cloned() else {
                    return;
                };
                if let AgentKey::Server(server) = key {
                    let expanded = !self.collapsed.contains(&server);
                    if expanded != expand {
                        self.activate_key(store, ui, &AgentKey::Server(server), fx);
                    }
                }
            }
            AgentsCommand::Pick => {
                if let Some(key) = self.list.inner().content().cursor().cloned() {
                    self.activate_key(store, ui, &key, fx);
                }
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

                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(AddHost { url }),
                )));
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
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            let theme = crate::env::Themes::of(store);
            let ui_theme = theme.ui().clone();
            let header = ui_theme.panel.header_height;
            let inset = crate::panel_inset(&ui_theme);
            let title_font = crate::fonts::ui_font(ui, ui_theme.panel.title_size);
            let mut panel = container(arena, Size::new(PANEL_WIDTH, size.height));
            let backdrop = leaf::<AgentsCommand>(PANEL_WIDTH, size.height)
                .paint_instead(move |_arena, canvas, rect| {
                    crate::paint_panel_chrome(canvas, rect, &ui_theme, &title_font, "Sessions", "");
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Handled,
                    _ => EventResult::Ignored,
                })
                .hit_opaque();
            panel.place(0.0, 0.0, backdrop);
            let rows = imba::Layout::layout(
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
                let well_height = input.content_height() + 2.0 * crate::ui::space::S;
                let well_width = (PANEL_WIDTH - inset * 2.0).max(1.0);
                let well_y = size.height - inset - well_height;
                let input_fill = search.input_fill;
                let empty = input.document.text().byte_count() == 0;
                let peeker = theme.ui().peeker.clone();
                let placeholder_font = crate::fonts::ui_text_font(ui, peeker.hint_size);
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
                    imba::Layout::layout(
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

            let adding = self.adding.is_some();
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
                        } => EventResult::Command(AgentsCommand::Dismiss),
                        Event::KeyDown {
                            key: InputKey::Up, ..
                        } => EventResult::Command(AgentsCommand::Select(-1)),
                        Event::KeyDown {
                            key: InputKey::Down,
                            ..
                        } => EventResult::Command(AgentsCommand::Select(1)),
                        Event::KeyDown {
                            key: InputKey::Left,
                            ..
                        } => EventResult::Command(AgentsCommand::Fold(false)),
                        Event::KeyDown {
                            key: InputKey::Right,
                            ..
                        } => EventResult::Command(AgentsCommand::Fold(true)),
                        Event::KeyDown {
                            key: InputKey::Enter,
                            ..
                        } => EventResult::Command(AgentsCommand::Pick),
                        _ => EventResult::Ignored,
                    }
                },
            );
            overlay.place(0.0, 0.0, keymap);

            let boot = !self.booted;
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

pub struct AddHost {
    pub url: String,
}

impl crate::DynamicCommand for AddHost {
    fn id(&self) -> &'static str {
        "agent.add-host"
    }

    fn name(&self) -> String {
        "Add Host".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(flow) = Agents::add_host_flow(store) else {
            eprintln!("[higent] no add-host capability installed — url dropped");
            return;
        };
        if flow(app, store, &self.url).is_none() {
            return;
        }
        ToggleAgentsView.perform(app, store, window, fx);
    }
}

pub struct ToggleAgentsView;

impl crate::DynamicCommand for ToggleAgentsView {
    fn id(&self) -> &'static str {
        "agent.toggle-agents"
    }

    fn name(&self) -> String {
        "Agents".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.has_side_panel() {
            entity.roll_away_side_panel();
            crate::Windows::put(store, window, entity);
            return;
        }
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = AgentsPanel::open(store, &_app.ui_ctx(), window);
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_side_panel(store, Box::new(panel), fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "agent.toggle-agents",
        order: 2.0,
        side: crate::ToolbarSide::Left,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let head = Rect::from_xywh(l + w * 0.14, t + h * 0.3, w * 0.72, h * 0.56);
            canvas.draw_round_rect(head, w * 0.14, w * 0.14, &paint);
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l + w * 0.5, t + h * 0.3));
            path.line_to((l + w * 0.5, t + h * 0.12));
            canvas.draw_path(&path.detach(), &paint);
            let mut dot = skia_safe::Paint::default();
            dot.set_anti_alias(true);
            dot.set_color(color);
            canvas.draw_circle((l + w * 0.36, t + h * 0.56), w * 0.055, &dot);
            canvas.draw_circle((l + w * 0.64, t + h * 0.56), w * 0.055, &dot);
        }),
    }
}

pub struct ShareHost;

impl crate::DynamicCommand for ShareHost {
    fn id(&self) -> &'static str {
        "host.share"
    }

    fn name(&self) -> String {
        "Share Host over HTTP".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let seat = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0)
            .and_then(|host| crate::higent::Servers::seat(store, host));
        let Some(seat) = seat else {
            eprintln!("[himark] share: no local host designated");
            return;
        };
        fx.push(
            imba::effect::AnyEffect::new(crate::higent::ShareHostEffect { seat })
                .map(move |result| AppCommand::Dynamic(window, Arc::new(SharedHost { result }))),
        );
    }
}

struct SharedHost {
    result: Result<String, String>,
}

impl crate::DynamicCommand for SharedHost {
    fn id(&self) -> &'static str {
        "host.shared"
    }

    fn name(&self) -> String {
        "Host Shared".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        match &self.result {
            Ok(url) => eprintln!("[himark] sharing at {url} (copied to clipboard)"),
            Err(error) => eprintln!("[himark] share failed: {error}"),
        }
    }
}
