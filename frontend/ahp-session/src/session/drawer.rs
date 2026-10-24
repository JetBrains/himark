// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_types::state::SessionSummary;
use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;
use hikit::list_keyboard::ListKeyCommand;
use hikit::list_keyboard::ListKeyboardController;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
type DrawerRowsCommand =
    imba::scroll::ScrollCommand<imba::list::ListCommand<std::convert::Infallible>>;
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
use skia_safe::{Rect, Size};

use super::agents::Agents;
use super::drawer_row::{AgeInk, DrawerLine, Lead, LineKind, TitleForm};
use super::state::HostStatus;
use super::summary::{self, Recency, SessionActivity};
use ::editor::{editor_view::EditorCommand, editor_view::EditorView};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AgentKey {
    /// A host that is not connected — picking it nudges a connect.
    Server(HostId),
    /// A recency group's head. Keyed like every line so the keyboard
    /// walk counts lines one for one.
    Group(Recency),
    Session(HostId, SessionUri),

    NewSession(HostId),

    Note(HostId),

    AddHost,
}

type Lines = ScrollView<ListView<DrawerLine, AgentKey>>;

/// Speed-search over the drawer's session titles — group heads, hosts,
/// notes and the "+ …" affordances stay out of the match set. Keys
/// ascend with the lines, so zipping pairs each key with its line.
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

impl AgentsPanel {
    /// What the add-host field takes off the lines' height.
    fn adding_height(&self, store: &Store) -> f32 {
        let chrome = ::editor::env::Themes::of(store).ui().sessions.clone();
        match &self.adding {
            Some(input) => input.content_height() + 2.0 * chrome.row_pad,
            None => 0.0,
        }
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
    list: ListKeyboardController<Lines, SessionSearcher>,
    asks: Arc<DrawerAsks>,
    /// The catalog generation the rows were last built from — the
    /// paint-side ask fires while this lags `Hosts::generation`.
    seen: Option<u64>,

    adding: Option<EditorView>,
    request: Option<ModalRequest>,
}

impl Clone for AgentsPanel {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            asks: Arc::clone(&self.asks),
            seen: self.seen,
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
            ),
            asks,
            seen: None,
            adding: None,
            request: None,
        };
        // The first fill happens on Boot (perform has the UiCtx the
        // row measurement needs).
        panel
    }

    /// TEST SUPPORT: each line's word and depth — heads (groups,
    /// hosts) at 0, what they head at 1. A host that is not connected
    /// reads "name — state".
    pub fn rows(&self) -> Vec<(String, usize)> {
        self.list
            .inner()
            .content()
            .rows()
            .map(|line| {
                let text = match &line.kind {
                    LineKind::Host { name, status, .. } => format!("{name} — {status}"),
                    _ => line.text().to_owned(),
                };
                (text, line.depth())
            })
            .collect()
    }

    /// TEST SUPPORT: each line's grid — (repo, word, age).
    #[doc(hidden)]
    pub fn lines(&self) -> Vec<(String, String, String)> {
        self.list
            .inner()
            .content()
            .rows()
            .map(|line| {
                (
                    line.repo().to_owned(),
                    line.text().to_owned(),
                    line.age().to_owned(),
                )
            })
            .collect()
    }

    /// TEST SUPPORT: the line a host's "+ New Session…" stands on.
    #[doc(hidden)]
    pub fn new_session_row(&self, host: HostId) -> Option<usize> {
        let range = self
            .list
            .inner()
            .content()
            .row_range(&AgentKey::NewSession(host))?;
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

    /// Rebuild the lines from the catalog: every connected host's
    /// sessions in ONE list, most recent first, cut into recency
    /// groups; then the "+ …" affordances; then the foot naming the
    /// hosts that are not connected.
    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let cursor = self.list.inner().content().cursor().cloned();
        let here = self.open_session_key(store);
        let now = std::time::SystemTime::now();
        let hosts = Agents::list(store);

        // Sessions interleave across hosts and projects: the repo
        // column says whose each one is.
        let mut sessions: Vec<(HostId, &str, &SessionSummary)> = hosts
            .iter()
            .filter(|(_, record)| matches!(record.status, HostStatus::Connected))
            .flat_map(|(server, record)| {
                record
                    .sessions
                    .iter()
                    .map(move |summary| (*server, record.name.as_str(), summary))
            })
            .collect();
        sessions.sort_by_key(|(_, _, summary)| std::cmp::Reverse(summary::modified_stamp(summary)));

        let mut slice: ListSlice<DrawerLine, AgentKey> = ListSlice::new();
        let push = |slice: &mut ListSlice<DrawerLine, AgentKey>, key, kind, lead| {
            slice.push_keyed(key, DrawerLine::new(kind, lead), store, ui);
        };
        let mut group = None;
        for (server, host_name, summary) in sessions {
            let recency = summary::recency(now, summary);
            if group != Some(recency) {
                let lead = match group {
                    None => Lead::Top,
                    Some(_) => Lead::Group,
                };
                push(
                    &mut slice,
                    AgentKey::Group(recency),
                    LineKind::Group(recency.caps()),
                    lead,
                );
                group = Some(recency);
            }
            let key = AgentKey::Session(server, SessionUri::new(summary.resource.clone()));
            let line = session_line(now, host_name, summary, here.as_ref() == Some(&key));
            push(&mut slice, key, line, Lead::None);
        }

        let mut lead = match group {
            Some(_) => Lead::Group,
            None => Lead::Top,
        };
        for (server, record) in hosts.iter() {
            if !matches!(record.status, HostStatus::Connected) {
                continue;
            }
            let line = LineKind::Affordance {
                repo: record.name.clone(),
                label: "+ New Session…".to_owned(),
            };
            push(&mut slice, AgentKey::NewSession(*server), line, lead);
            lead = Lead::None;
        }
        let line = LineKind::Affordance {
            repo: String::new(),
            label: "+ Add Host…".to_owned(),
        };
        push(&mut slice, AgentKey::AddHost, line, lead);

        // The foot: hosts that are not connected, set apart by a
        // hairline — their name, their state, their error.
        let mut lead = Lead::Foot;
        for (server, record) in hosts.iter() {
            let (status, failed, error) = match &record.status {
                HostStatus::Connected => continue,
                HostStatus::Idle | HostStatus::Connecting => ("connecting…", false, None),
                HostStatus::Failed(error) => ("offline", true, Some(error.clone())),
            };
            let line = LineKind::Host {
                name: record.name.clone(),
                status: status.to_owned(),
                failed,
            };
            push(&mut slice, AgentKey::Server(*server), line, lead);
            if let Some(error) = error {
                push(
                    &mut slice,
                    AgentKey::Note(*server),
                    LineKind::Error(error),
                    Lead::None,
                );
            }
            lead = Lead::Host;
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
            AgentKey::Server(server) => {
                if matches!(
                    Agents::record(store, *server).map(|record| record.status),
                    Some(HostStatus::Failed(_)) | Some(HostStatus::Idle)
                ) {
                    // The nudge rides the verb drain: connecting is the
                    // resident subscription's job, not the drawer's.
                    self.request = Some(ModalRequest::Perform(imba::command::Verb::Dynamic(
                        std::sync::Arc::new(super::driver::ConnectHost(*server)),
                    )));
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
            AgentKey::Group(_) | AgentKey::Note(_) => {}
            AgentKey::AddHost => {
                let mut input = EditorView::input_styled(
                    600.0,
                    store,
                    ui,
                    ::editor::theme::StyleId::Field,
                );
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

/// A session's line: the repo (its first folder's name, else its
/// host's), its title — or an italic stand-in — and the age column,
/// which leads with a word when the session wants a look: "open" for
/// the window's own, the turn's state otherwise. What the states ARE
/// is the session's to say (`summary::activity`); this is their dress.
fn session_line(
    now: std::time::SystemTime,
    host_name: &str,
    summary: &SessionSummary,
    here: bool,
) -> LineKind {
    let repo = summary
        .working_directories
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| super::folders::folder_label(folder))
        .unwrap_or_else(|| host_name.to_owned());
    let (title, form) = match summary.title.trim() {
        "" => ("untitled".to_owned(), TitleForm::Untitled),
        title if title.starts_with('/') => (summary::label(summary), TitleForm::Command),
        _ => (summary::label(summary), TitleForm::Plain),
    };
    let activity = summary::activity(summary);
    let (word, ink) = match (here, activity) {
        (true, _) => (Some("open"), AgeInk::Live),
        (false, SessionActivity::Blocked) => (Some("needs you"), AgeInk::Live),
        (false, SessionActivity::Working) => (Some("working"), AgeInk::Live),
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
        repo,
        title,
        form,
        unviewed: activity == SessionActivity::Unviewed,
        age,
        ink,
        here,
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
                    InputKey::Enter => EventResult::command(AgentsCommand::SubmitAddHost),
                    InputKey::Escape => EventResult::command(AgentsCommand::CancelAddHost),
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
                InputKey::Escape if !searching => EventResult::command(AgentsCommand::Dismiss),
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

            // The panel's FLOOR, its header and its width belong to
            // the window slot that stands this view up
            // (`hikit::modal::Paneled`) — a mounted host draws its
            // own island. The drawer lays rows into what it is
            // given, and that is all it knows.
            let mut panel = container(arena, size);
            let adding_height = self.adding_height(store);
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    size.width,
                    (size.height - adding_height).max(1.0),
                )),
            )
            .map(AgentsCommand::Rows);
            panel.place(0.0, 0.0, rows);
            if let Some(input) = &self.adding {
                panel.place(
                    0.0,
                    size.height - adding_height,
                    self.add_host_field(arena, store, ui, input, size.width),
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
                            } => EventResult::command(AgentsCommand::SubmitAddHost),
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
        let theme = themes.ui();
        let chrome = theme.sessions.clone();
        let rule = theme.ink.rule_strong.0;
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
