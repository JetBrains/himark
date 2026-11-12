// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Sessions drawer (the Sessions mock): the header names the
//! column, counts the hosts and offers "Add host"; each host is a
//! heading with its own "New session"; under it the sessions that are
//! live right now or open in this client, then the "Older · N" fold
//! over the rest. One list, one keyboard walk, speed search over the
//! titles. The catalog it dresses is kept true by the resident
//! subscription (`driver`); the drawer only reads it.

use std::collections::HashSet;
use std::sync::Arc;

use ahp_types::state::SessionSummary;
use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;
use hikit::combo::ComboOption;
use hikit::list_keyboard::ListKeyCommand;
use hikit::list_keyboard::ListKeyboardController;
use hikit::menu::{MenuCommand, MenuView};
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
type DrawerRowsCommand =
    imba::scroll::ScrollCommand<imba::list::ListCommand<std::convert::Infallible>>;
use gfx::{Rect, Size};
use imba::list::ActivateTrigger;
use imba::list::ListOps;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult, Key as InputKey},
    layout::LayoutExt as _,
    leaf::leaf,
    list::{ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View, Widget,
};

use super::agents::Agents;
use super::drawer_row::{AgeInk, DrawerLine, HostState, Lead, LineKind, Mark, TitleForm};
use super::state::HostStatus;
use super::summary::{self, SessionActivity};
use crate::tailnet::{TailnetFaceState, TailnetFlows, TailnetState};
use ::editor::{editor_view::EditorCommand, editor_view::EditorView};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AgentKey {
    /// A host's heading. Activating it starts a session on a
    /// connected host and nudges a connect on one that is not.
    Host(HostId),
    /// The host's error line.
    Note(HostId),
    Session(HostId, SessionUri),
    /// The host's "Older · N" fold; activating it toggles.
    Fold(HostId),
    /// A folders heading under the fold, keyed by its label.
    Folders(HostId, String),
}

type Lines = ScrollView<ListView<DrawerLine, AgentKey>>;

/// The add-host menu's rows.
const ADD_TAILNET: &str = "tailnet";
const ADD_URL: &str = "url";

/// Speed-search over the drawer's session titles — hosts, notes and
/// folds stay out of the match set. Keys ascend with the lines, so
/// zipping pairs each key with its line.
#[derive(Clone)]
struct SessionSearcher;

impl hikit::list_keyboard::Searcher for SessionSearcher {
    type View = Lines;
    type Key = AgentKey;

    fn capture(&self, view: &Self::View) -> hikit::list_keyboard::ItemSource<AgentKey> {
        let keys = view.content().structure_keys().ordered_keys();
        let titles: Vec<String> = view
            .content()
            .rows()
            .map(|line| line.text().to_owned())
            .collect();
        let items: Vec<(String, AgentKey)> = keys
            .into_iter()
            .zip(titles)
            .filter(|(key, _)| matches!(key, AgentKey::Session(..)))
            .map(|(key, title)| (title, key))
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
    Rows(ListKeyCommand<DrawerRowsCommand>),

    /// The paint-side ask: the panel's rows lag the catalog — rebuild
    /// them. The catalog itself is kept true by the resident
    /// subscription (`ahp_session::session::driver`); the drawer only
    /// dresses it.
    Boot,

    Dismiss,

    /// ⌘N from the drawer: a fresh session where the open one lives
    /// (else the first connected host).
    NewHere,

    /// The header's "Add host": the two-way menu — join the tailnet,
    /// or type a URL.
    AddHost,

    AddHostMenu(MenuCommand),

    /// The menu's first way: the embedder's tailnet connect flow
    /// (the login opens in the browser; discovery lists the hosts).
    ConnectTailnet,

    /// The menu's second way: the URL field.
    EnterHostUrl,

    AddHostInput(EditorCommand),

    SubmitAddHost,

    CancelAddHost,
}

impl std::fmt::Display for AgentsCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentsCommand::Rows(command) => command.fmt(out),
            AgentsCommand::AddHostInput(command) => command.fmt(out),
            AgentsCommand::AddHostMenu(command) => command.fmt(out),
            AgentsCommand::Boot => out.write_str("agents boot"),
            AgentsCommand::Dismiss => out.write_str("agents dismiss"),
            AgentsCommand::NewHere => out.write_str("agents new here"),
            AgentsCommand::AddHost => out.write_str("add host"),
            AgentsCommand::ConnectTailnet => out.write_str("connect tailnet"),
            AgentsCommand::EnterHostUrl => out.write_str("enter host url"),
            AgentsCommand::SubmitAddHost => out.write_str("submit add host"),
            AgentsCommand::CancelAddHost => out.write_str("cancel add host"),
        }
    }
}

/// The add-host gesture in flight: the two-way menu, or the URL
/// field it opened.
#[derive(Clone)]
enum Adding {
    Menu(MenuView),
    Url(EditorView),
}

pub struct AgentsPanel {
    list: ListKeyboardController<Lines, SessionSearcher>,
    asks: Arc<DrawerAsks>,
    /// The catalog generation the rows were last built from — the
    /// paint-side ask fires while this lags `Hosts::generation`.
    seen: Option<u64>,

    /// The hosts whose Older fold is open.
    expanded: HashSet<HostId>,

    adding: Option<Adding>,

    /// The user asked for the tailnet from this drawer: while the
    /// node has not yet reported, the status line says so.
    tailnet_asked: bool,

    request: Option<ModalRequest>,
}

impl Clone for AgentsPanel {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            asks: Arc::clone(&self.asks),
            seen: self.seen,
            expanded: self.expanded.clone(),
            adding: self.adding.clone(),
            tailnet_asked: self.tailnet_asked,

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
        // The first fill happens on Boot (perform has the UiCtx the
        // row measurement needs).
        Self {
            list: ListKeyboardController::searchable(
                ScrollView::new(
                    ListView::empty().with_selection(hikit::rows::selection_style(store)),
                ),
                SessionSearcher,
                store,
                ui,
            ),
            asks,
            seen: None,
            expanded: HashSet::new(),
            adding: None,
            tailnet_asked: false,
            request: None,
        }
    }

    /// TEST SUPPORT: each line's word and depth — hosts at 0, what
    /// they head at 1. A host reads "name — kind" (its OS, or its
    /// state while it is not connected).
    pub fn rows(&self) -> Vec<(String, usize)> {
        self.list
            .inner()
            .content()
            .rows()
            .map(|line| {
                let text = match &line.kind {
                    LineKind::Host { name, kind, .. } => format!("{name} — {kind}"),
                    _ => line.text().to_owned(),
                };
                (text, line.depth())
            })
            .collect()
    }

    /// TEST SUPPORT: each line's three words — (detail, word, trail):
    /// a session's (folders, title, age), a host's (kind, name,
    /// action), a fold's (count, "Older", toggle).
    #[doc(hidden)]
    pub fn lines(&self) -> Vec<(String, String, String)> {
        self.list
            .inner()
            .content()
            .rows()
            .map(|line| {
                (
                    line.detail(),
                    line.text().to_owned(),
                    line.trail().to_owned(),
                )
            })
            .collect()
    }

    /// TEST SUPPORT: the line a host's heading stands on — the row
    /// whose activation starts a new session there.
    #[doc(hidden)]
    pub fn new_session_row(&self, host: HostId) -> Option<usize> {
        let range = self
            .list
            .inner()
            .content()
            .row_range(&AgentKey::Host(host))?;
        Some(range.start)
    }

    /// TEST SUPPORT: the line a host's Older fold stands on.
    #[doc(hidden)]
    pub fn fold_row(&self, host: HostId) -> Option<usize> {
        let range = self
            .list
            .inner()
            .content()
            .row_range(&AgentKey::Fold(host))?;
        Some(range.start)
    }

    /// TEST SUPPORT: the line a session stands on.
    #[doc(hidden)]
    pub fn session_row(&self, session: &ahp_wire::SessionId) -> Option<usize> {
        let key = AgentKey::Session(session.host, session.session.clone());
        Some(self.list.inner().content().row_range(&key)?.start)
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

    /// TEST SUPPORT: the add-host menu's rows while it is up.
    #[doc(hidden)]
    pub fn add_host_menu(&self) -> Option<Vec<String>> {
        match &self.adding {
            Some(Adding::Menu(_)) => Some(vec![
                "Connect to tailnet".to_owned(),
                "Enter URL…".to_owned(),
            ]),
            _ => None,
        }
    }

    /// Rebuild the lines from the catalog: host by host, the heading,
    /// its error, the live rows (running, or open in this client),
    /// then the Older fold over the rest — and under it, while
    /// folded, a short preview. Placeholders (a session created and
    /// never prompted) stay out, except the one open in front of the
    /// user.
    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let cursor = self.list.inner().content().cursor().cloned();
        let here = self.open_session_key(store);
        let now = std::time::SystemTime::now();
        let hosts = Agents::list(store);
        let preview = ::editor::env::Themes::of(store)
            .part(crate::chrome::SessionsChrome::KEY)
            .fold_preview;

        let mut slice: ListSlice<DrawerLine, AgentKey> = ListSlice::new();
        let push = |slice: &mut ListSlice<DrawerLine, AgentKey>, key, kind, lead| {
            slice.push_keyed(key, DrawerLine::new(kind, lead), store, ui);
        };
        let mut lead = Lead::Top;
        for (server, record) in hosts.iter() {
            let (state, kind, action, key) = match &record.status {
                HostStatus::Connected => (
                    HostState::Connected,
                    record.os.clone().unwrap_or_default(),
                    "New session",
                    Some("⌘N".to_owned()),
                ),
                HostStatus::Idle | HostStatus::Connecting => {
                    (HostState::Connecting, "connecting…".to_owned(), "", None)
                }
                HostStatus::Failed(_) => {
                    (HostState::Failed, "offline".to_owned(), "Reconnect", None)
                }
            };
            push(
                &mut slice,
                AgentKey::Host(*server),
                LineKind::Host {
                    name: record.display_name().to_owned(),
                    kind,
                    state,
                    action: action.to_owned(),
                    key,
                },
                lead,
            );
            lead = Lead::Host;
            if let HostStatus::Failed(error) = &record.status {
                push(
                    &mut slice,
                    AgentKey::Note(*server),
                    LineKind::Error(error.clone()),
                    Lead::None,
                );
            }

            let mut sessions: Vec<&SessionSummary> = record
                .sessions
                .iter()
                .filter(|summary| {
                    let key = AgentKey::Session(*server, SessionUri::new(summary.resource.clone()));
                    !summary::is_placeholder(summary) || here.as_ref() == Some(&key)
                })
                .collect();
            sessions.sort_by_key(|summary| std::cmp::Reverse(summary::modified_stamp(summary)));
            let (live, older): (Vec<_>, Vec<_>) = sessions.into_iter().partition(|summary| {
                let uri = SessionUri::new(summary.resource.clone());
                here.as_ref() == Some(&AgentKey::Session(*server, uri.clone()))
                    || summary::is_running(summary)
                    || record.states.contains_key(&uri)
            });
            for summary in live {
                let key = AgentKey::Session(*server, SessionUri::new(summary.resource.clone()));
                let line = session_line(now, summary, here.as_ref() == Some(&key), false, 0);
                push(&mut slice, key, line, Lead::None);
            }
            let expanded = self.expanded.contains(server);
            let shown: &[&SessionSummary] = match older.len() > preview {
                true => {
                    push(
                        &mut slice,
                        AgentKey::Fold(*server),
                        LineKind::Fold {
                            count: older.len(),
                            expanded,
                        },
                        Lead::None,
                    );
                    match expanded {
                        true => &older,
                        false => &older[..preview],
                    }
                }
                false => &older,
            };
            // Grouped by folders, groups in order of their newest row
            // (the rows are already newest-first).
            let mut groups: Vec<(String, Vec<&SessionSummary>)> = Vec::new();
            for summary in shown {
                let label = folders_label(summary);
                match groups.iter_mut().find(|(held, _)| *held == label) {
                    Some((_, rows)) => rows.push(summary),
                    None => groups.push((label, vec![summary])),
                }
            }
            for (label, rows) in groups {
                push(
                    &mut slice,
                    AgentKey::Folders(*server, label.clone()),
                    LineKind::Folders(label),
                    Lead::None,
                );
                for summary in rows {
                    let key = AgentKey::Session(*server, SessionUri::new(summary.resource.clone()));
                    let line = session_line(now, summary, false, true, 2);
                    push(&mut slice, key, line, Lead::None);
                }
            }
        }

        let len = self.list.inner().content().len();
        self.list
            .inner_mut()
            .content_mut()
            .splice_slice(0..len, slice);
        // The cursor survives a refresh; a fresh list lands on the
        // session the window currently shows.
        let target = cursor.or(here);
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
            AgentKey::Host(server) => {
                match Agents::record(store, *server).map(|record| record.status) {
                    Some(HostStatus::Connected) => {
                        self.request = Some(ModalRequest::Perform((self.asks.new_session)(
                            store, *server,
                        )));
                    }
                    Some(HostStatus::Failed(_)) | Some(HostStatus::Idle) => {
                        // The nudge rides the verb drain: connecting is the
                        // resident subscription's job, not the drawer's.
                        self.request = Some(ModalRequest::Perform(imba::command::Verb::Dynamic(
                            std::sync::Arc::new(super::driver::ConnectHost(*server)),
                        )));
                        self.refresh(store, ui);
                    }
                    Some(HostStatus::Connecting) | None => {}
                }
            }
            AgentKey::Session(server, session) => {
                self.list.inner_mut().content_mut().select_only(key.clone());

                self.request = Some(ModalRequest::Perform((self.asks.open_row)(
                    *server,
                    session.clone(),
                )));
            }
            AgentKey::Fold(server) => {
                // The toggle re-lays the rows under it; the cursor
                // stays on the fold line through the refresh.
                self.list.inner_mut().content_mut().select_only(key.clone());
                if !self.expanded.remove(server) {
                    self.expanded.insert(*server);
                }
                self.refresh(store, ui);
            }
            AgentKey::Note(_) | AgentKey::Folders(..) => {}
        }
    }

    #[doc(hidden)]
    pub fn add_host_text(&self) -> Option<String> {
        let Some(Adding::Url(input)) = self.adding.as_ref() else {
            return None;
        };
        let text = input.document.text();
        let end = text.byte_count().min(u32::MAX as usize) as u32;
        Some(text.view().substring(0..end))
    }

    /// What the add-host field takes off the lines' height.
    fn adding_height(&self, store: &Store) -> f32 {
        let chrome = ::editor::env::Themes::of(store).part(crate::chrome::SessionsChrome::KEY);
        match &self.adding {
            Some(Adding::Url(input)) => input.content_height() + 2.0 * chrome.row_pad,
            Some(Adding::Menu(_)) | None => 0.0,
        }
    }

    /// The tailnet status line's word, while the node is on its way.
    fn tailnet_word(&self, store: &Store) -> Option<&'static str> {
        let state = TailnetFaceState::of(store);
        match (&state, self.tailnet_asked) {
            (TailnetState::Off, true) => Some("Tailnet: connecting…"),
            _ => state.progress_word(),
        }
    }
}

/// A session's line: its title — or an italic stand-in — its folders,
/// the gutter mark, and the age column, which leads with a word when
/// the session wants a look: "open" for the window's own, the turn's
/// state otherwise. What the states ARE is the session's to say
/// (`summary::activity`); this is their dress.
fn session_line(
    now: std::time::SystemTime,
    summary: &SessionSummary,
    here: bool,
    old: bool,
    indent: u8,
) -> LineKind {
    let roots = folders_label(summary);
    let (title, form) = match summary.title.trim() {
        "" => ("untitled".to_owned(), TitleForm::Untitled),
        title if title.starts_with('/') => (summary::label(summary), TitleForm::Command),
        _ => (summary::label(summary), TitleForm::Plain),
    };
    let activity = summary::activity(summary);
    let mark = match activity {
        SessionActivity::Working => Mark::Running,
        SessionActivity::Blocked | SessionActivity::Unviewed => Mark::Attention,
        SessionActivity::Failed | SessionActivity::Quiet => Mark::None,
    };
    let (word, ink) = match (here, activity) {
        (true, _) => (Some("open"), AgeInk::Live),
        (false, SessionActivity::Blocked) => (Some("needs you"), AgeInk::Live),
        (false, SessionActivity::Working) => (Some("working"), AgeInk::Quiet),
        (false, SessionActivity::Failed) => (Some("failed"), AgeInk::Failed),
        (false, SessionActivity::Unviewed | SessionActivity::Quiet) => (None, AgeInk::Quiet),
    };
    let age = word
        .into_iter()
        .map(str::to_owned)
        .chain(summary::age(now, summary))
        .collect::<Vec<_>>()
        .join(" · ");
    LineKind::Session {
        title,
        form,
        mark,
        attention: matches!(
            activity,
            SessionActivity::Unviewed | SessionActivity::Blocked
        ),
        age,
        ink,
        roots,
        here,
        old,
        indent,
    }
}

/// A session's folders as one label — each folder's short name,
/// " · "-joined; a session with none reads "no folder".
fn folders_label(summary: &SessionSummary) -> String {
    let label = summary
        .working_directories
        .iter()
        .flatten()
        .map(|folder| super::folders::folder_label(folder))
        .collect::<Vec<_>>()
        .join(" · ");
    match label.is_empty() {
        true => "no folder".to_owned(),
        false => label,
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
        match &self.adding {
            Some(Adding::Url(input)) => {
                let own = FocusData {
                    on_key: Some(Box::new(|key, _mods| match key {
                        InputKey::Enter => EventResult::command(AgentsCommand::SubmitAddHost),
                        InputKey::Escape => EventResult::command(AgentsCommand::CancelAddHost),
                        _ => EventResult::Ignored,
                    })),
                    ..FocusData::default()
                };
                own.merge_under(input.focus_data(store, ui).map(AgentsCommand::AddHostInput))
            }
            Some(Adding::Menu(menu)) => {
                let own = FocusData {
                    on_key: Some(Box::new(|key, _mods| match key {
                        InputKey::Escape => EventResult::command(AgentsCommand::CancelAddHost),
                        _ => EventResult::Ignored,
                    })),
                    ..FocusData::default()
                };
                own.merge_under(menu.focus_data(store, ui).map(AgentsCommand::AddHostMenu))
            }
            None => {
                // The key table is the controller's; the surface keeps
                // only its own dismissal.
                let searching = self.list.searching();
                let own = FocusData {
                    on_key: Some(Box::new(move |key, _mods| match key {
                        InputKey::Escape if !searching => {
                            EventResult::command(AgentsCommand::Dismiss)
                        }
                        _ => EventResult::Ignored,
                    })),
                    ..FocusData::default()
                };
                own.merge_under(self.list.focus_data(store, ui).map(AgentsCommand::Rows))
            }
        }
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
            AgentsCommand::NewHere => {
                let server = (self.asks.open_session)(store)
                    .map(|open| open.host)
                    .or_else(|| {
                        Agents::list(store)
                            .into_iter()
                            .find_map(|(server, record)| {
                                matches!(record.status, HostStatus::Connected).then_some(server)
                            })
                    });
                let Some(server) = server else {
                    return;
                };
                self.request = Some(ModalRequest::Perform((self.asks.new_session)(
                    store, server,
                )));
            }
            AgentsCommand::Rows(command) => {
                type Rows = ListKeyboardController<Lines, SessionSearcher>;
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
            AgentsCommand::AddHost => {
                self.adding = Some(Adding::Menu(MenuView::new(
                    store,
                    ui,
                    vec![
                        ComboOption::plain(ADD_TAILNET, "Connect to tailnet"),
                        ComboOption::plain(ADD_URL, "Enter URL…"),
                    ],
                )));
            }
            AgentsCommand::AddHostMenu(command) => {
                let Some(Adding::Menu(menu)) = self.adding.as_mut() else {
                    return;
                };
                match menu.picked(&command).as_deref() {
                    Some(ADD_TAILNET) => self.perform(store, ui, AgentsCommand::ConnectTailnet, fx),
                    Some(ADD_URL) => self.perform(store, ui, AgentsCommand::EnterHostUrl, fx),
                    Some(_) => self.adding = None,
                    None => fx.scope(AgentsCommand::AddHostMenu, |fx| {
                        menu.perform(store, ui, command, fx)
                    }),
                }
            }
            AgentsCommand::ConnectTailnet => {
                self.adding = None;
                self.tailnet_asked = true;
                if !TailnetFlows::connect(store) {
                    eprintln!("[sessions] no tailnet flow installed — this shell cannot join one");
                }
            }
            AgentsCommand::EnterHostUrl => {
                let mut input =
                    EditorView::input_styled(600.0, store, ui, ::editor::theme::StyleId::Field);
                input.focus_text();
                self.adding = Some(Adding::Url(input));
            }
            AgentsCommand::AddHostInput(command) => {
                let Some(Adding::Url(input)) = self.adding.as_mut() else {
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

            // The panel's FLOOR and its width belong to the window
            // slot that stands this view up (`hikit::modal::Paneled`)
            // — a mounted host draws its own island. The drawer draws
            // its own HEAD: the column header every column wears —
            // the name, the host count, the "Add host" word.
            let themes = ::editor::env::Themes::of(store);
            let chrome = themes.part(crate::chrome::SessionsChrome::KEY);
            let ink = themes.part(::editor::theme::InkChrome::KEY);
            let host_count = Agents::list(store).len();
            let header = hikit::header::ColumnHeader::new(arena, store, ui, chrome.inset)
                .title("Sessions")
                .subtitle(match host_count {
                    1 => "1 host".to_owned(),
                    count => format!("{count} hosts"),
                })
                .button("Add host", || AgentsCommand::AddHost);
            let head_height = header.band_height();
            let head = imba::layout::Layout::layout(
                header,
                arena,
                Constraints::tight(Size::new(size.width, head_height)),
            );

            // The tailnet's progress, while a node is on its way.
            let status = self.tailnet_word(store).map(|word| {
                let style = hikit::ui::brief(store, ui).sized(chrome.status_size);
                imba::layout::Layout::layout(
                    hikit::ui::text(&style, word)
                        .align(imba::layout::Alignment::CenterStart)
                        .sized(
                            (size.width - 2.0 * chrome.inset).max(1.0),
                            chrome.status_line,
                        )
                        .pad_insets(imba::layout::Insets {
                            left: chrome.inset,
                            ..Default::default()
                        }),
                    arena,
                    Constraints::tight(Size::new(size.width, chrome.status_line)).loosen(),
                )
            });
            let status_height = status.as_ref().map_or(0.0, |_| chrome.status_line);
            let rows_top = head_height + status_height;

            let mut panel = container(arena, size);
            let adding_height = self.adding_height(store);
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    size.width,
                    (size.height - rows_top - adding_height).max(1.0),
                )),
            )
            .map(AgentsCommand::Rows);
            panel.place_boxed(0.0, 0.0, head);
            if let Some(status) = status {
                panel.place_boxed(0.0, head_height, status);
            }
            panel.place(0.0, rows_top, rows);
            match &self.adding {
                Some(Adding::Url(input)) => {
                    panel.place(
                        0.0,
                        size.height - adding_height,
                        self.add_host_field(arena, store, ui, input, size.width),
                    );
                }
                Some(Adding::Menu(menu)) => {
                    // A press anywhere else closes the menu; the menu
                    // itself stands under the header's trailing word.
                    let catcher = leaf::<AgentsCommand>(size.width, size.height).event(
                        |_arena, event, _size| match event {
                            Event::MouseDown { .. } => {
                                EventResult::command(AgentsCommand::CancelAddHost)
                            }
                            _ => EventResult::Ignored,
                        },
                    );
                    panel.place(0.0, 0.0, catcher);
                    let combo = themes.part(hikit::chrome::ComboChrome::KEY);
                    let width = menu
                        .widest(store, ui)
                        .max(combo.menu_min_width)
                        .min((size.width - 2.0 * chrome.inset).max(1.0));
                    let height = menu.len() as f32 * combo.menu_row_height + 2.0;
                    let menu = imba::layout::Layout::layout(
                        menu.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(width, height)),
                    )
                    .map(AgentsCommand::AddHostMenu);
                    panel.place(size.width - chrome.inset - width, head_height, menu);
                }
                None => {}
            }
            let _ = ink;

            overlay.place(0.0, 0.0, panel);

            // The key table lives in the controller's own overlay;
            // the surface keeps its add-host modes and dismissal.
            let adding = self.adding.is_some();
            let url_field = matches!(self.adding, Some(Adding::Url(_)));
            let searching = self.list.searching();
            let keymap = leaf::<AgentsCommand>(size.width, size.height).event(
                move |_arena, event, _size| {
                    if adding {
                        return match event {
                            Event::KeyDown {
                                key: InputKey::Enter,
                                ..
                            } if url_field => EventResult::command(AgentsCommand::SubmitAddHost),
                            Event::KeyDown {
                                key: InputKey::Escape,
                                ..
                            } => EventResult::command(AgentsCommand::CancelAddHost),
                            _ => EventResult::Ignored,
                        };
                    }
                    match event {
                        Event::KeyDown {
                            key: InputKey::Escape,
                            ..
                        } if !searching => EventResult::command(AgentsCommand::Dismiss),
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

impl AgentsPanel {
    /// The add-host field: an input reads as an input by a rule under
    /// it, a grey placeholder and the live caret — no well.
    fn add_host_field<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        input: &'a EditorView,
        width: f32,
    ) -> imba::ThunkBox<'a, AgentsCommand> {
        let themes = ::editor::env::Themes::of(store);
        let chrome = themes.part(crate::chrome::SessionsChrome::KEY);
        let rule = themes.part(::editor::theme::InkChrome::KEY).rule_strong.0;
        let field_width = (width - 2.0 * chrome.inset).max(1.0);
        let height = input.content_height();
        let mut field = imba::layout::ZBox::new(arena);
        if input.document.text().byte_count() == 0 {
            field = field.child_aligned(
                imba::layout::Alignment::CenterStart,
                hikit::ui::text(&hikit::ui::brief(store, ui), "ws://host:port/?tkn=…"),
            );
        }
        let field = field
            .child(
                input
                    .display(arena, store, ui)
                    .map_layout(|command| AgentsCommand::AddHostInput(command)),
            )
            .sized(field_width, height);
        let field = imba::layout::Layout::layout(
            field,
            arena,
            Constraints::tight(Size::new(field_width, height)),
        )
        .focus_scope(true);
        imba::layout::Layout::layout(
            imba::layout::fixed(hikit::ui::Hairline::new(rule).bottom().wrap(field))
                .pad_insets(imba::layout::Insets::xy(chrome.inset, chrome.row_pad)),
            arena,
            Constraints {
                min: Size::default(),
                max: Size::new(width, f32::MAX),
            },
        )
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
            return result.merge(EventResult::command(AgentsCommand::Boot));
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
