use std::sync::Arc;

use crate::higent::{
    ConnectServerEffect, HostId, ListSessionsEffect, PollServerEffect, RootInfo, ServerEvent,
    SessionsPage,
};
use crate::{AppCommand, ModalRequest, ModalView, TreeLabel, TreeListCommand, TreeRow};
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
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Rect, Size};

use crate::higent::session::{Agents, HostStatus, OpenSessionRow};
use ::editor::{EditorCommand, EditorView};

const PANEL_WIDTH: f32 = crate::DRAWER_WIDTH;
const PANEL_PAD: f32 = 6.0;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AgentKey {
    Server(HostId),
    Session(HostId, Uri),

    NewSession(HostId),

    Note(HostId),

    AddHost,
}

type TreeList = ScrollView<ListView<TreeRow, AgentKey>>;

pub enum AgentsCommand {
    Rows(TreeListCommand),

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
    list: TreeList,
    row_height: f32,
    window: crate::WindowId,
    booted: bool,

    collapsed: rpds::HashTrieSetSync<HostId>,

    polls: rpds::HashTrieMapSync<HostId, CancellationToken>,

    adding: Option<EditorView>,
    request: Option<ModalRequest>,
}

impl Clone for AgentsPanel {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            row_height: self.row_height,
            window: self.window,
            booted: self.booted,
            collapsed: self.collapsed.clone(),
            polls: self.polls.clone(),
            adding: self.adding.clone(),

            request: None,
        }
    }
}

impl AgentsPanel {
    pub fn open(store: &Store, window: crate::WindowId) -> Self {
        let theme = crate::env::Themes::of(store);
        let tree = theme.ui().tree.clone();
        let mut panel = Self {
            list: ScrollView::new(
                ListView::from_measured([]).with_selection(crate::selection_style(store)),
            ),
            row_height: tree.row_height.max(1.0),
            window,
            booted: false,
            collapsed: rpds::HashTrieSetSync::new_sync(),
            polls: rpds::HashTrieMapSync::new_sync(),
            adding: None,
            request: None,
        };
        panel.refresh(store);
        panel
    }

    pub fn rows(&self) -> Vec<(String, usize)> {
        self.list
            .content()
            .rows()
            .map(|row| (row.inner().text().to_owned(), usize::from(row.depth())))
            .collect()
    }

    fn refresh(&mut self, store: &Store) {
        let cursor = self.list.content().cursor().cloned();
        let mut slice: ListSlice<TreeRow, AgentKey> = ListSlice::new();

        let now = std::time::SystemTime::now();
        let dim = crate::env::Themes::of(store).ui().peeker.dim_text.0;
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
                self.row_height,
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
                        self.row_height,
                    );
                }
                HostStatus::Failed(error) => {
                    slice.push_keyed(
                        AgentKey::Note(server),
                        crate::TreeItemView::leaf(
                            TreeLabel::new(format!("failed: {error}"), false, true),
                            1,
                        ),
                        self.row_height,
                    );
                }
                HostStatus::Connected => {
                    for summary in record.sessions.iter() {
                        slice.push_keyed(
                            AgentKey::Session(server, summary.resource.clone()),
                            crate::TreeItemView::leaf(
                                TreeLabel::new(session_label(summary), true, false)
                                    .with_trail(age_trail(now, dim, summary)),
                                1,
                            ),
                            self.row_height,
                        );
                    }
                    {
                        slice.push_keyed(
                            AgentKey::NewSession(server),
                            crate::TreeItemView::leaf(
                                TreeLabel::new("+ New Session…".to_owned(), true, false),
                                1,
                            ),
                            self.row_height,
                        );
                    }
                }
            }
        }
        slice.push_keyed(
            AgentKey::AddHost,
            crate::TreeItemView::leaf(TreeLabel::new("+ Add Host…".to_owned(), true, false), 0),
            self.row_height,
        );
        let len = self.list.content().len();
        self.list.content_mut().splice_slice(0..len, slice);
        if let Some(key) = cursor {
            if self.list.content().row_range(&key).is_some() {
                self.list.content_mut().select_only(key);
            }
        }
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
        index: usize,
        fx: &mut Effects<'_, AgentsCommand>,
    ) {
        let Some(key) = self.list.content().key_at(index).cloned() else {
            return;
        };
        self.activate_key(store, &key, fx);
    }

    fn activate_key(
        &mut self,
        store: &mut Store,
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
                self.refresh(store);
            }
            AgentKey::Session(server, session) => {
                self.list.content_mut().select_only(key.clone());

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
                let mut input = EditorView::input(600.0, crate::fonts::source());
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

fn session_label(summary: &SessionSummary) -> String {
    let mut label = String::new();
    if summary.status & 8 != 0 {
        label.push_str("● ");
    }
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

    fn destroy(&mut self, _store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        for (_, token) in self.polls.iter() {
            fx.cancel(*token);
        }
        self.polls = rpds::HashTrieMapSync::new_sync();
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
                self.refresh(store);
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
                self.refresh(store);
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
                self.refresh(store);
            }
            AgentsCommand::Events(server, events) => {
                for event in events {
                    Agents::apply_event(store, server, event);
                }
                self.relaunch_poll(store, server, fx);
                self.refresh(store);
            }
            AgentsCommand::Rows(command) => {
                if let Some((index, _)) = crate::tree_interaction(&command) {
                    self.activate(store, index, fx);
                    return;
                }
                fx.scope(AgentsCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            AgentsCommand::Select(delta) => self.list.content_mut().cursor_step(delta),
            AgentsCommand::Fold(expand) => {
                let Some(key) = self.list.content().cursor().cloned() else {
                    return;
                };
                if let AgentKey::Server(server) = key {
                    let expanded = !self.collapsed.contains(&server);
                    if expanded != expand {
                        self.activate_key(store, &AgentKey::Server(server), fx);
                    }
                }
            }
            AgentsCommand::Pick => {
                if let Some(key) = self.list.content().cursor().cloned() {
                    self.activate_key(store, &key, fx);
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

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
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
        let rows = self
            .list
            .layout(
                arena,
                store,
                ui,
                Constraints::tight(Size::new(
                    PANEL_WIDTH - inset * 2.0 - 2.0,
                    (size.height - inset * 2.0 - header - PANEL_PAD).max(1.0),
                )),
            )
            .map(AgentsCommand::Rows);
        panel.place(inset + 1.0, inset + header + PANEL_PAD, rows);
        if let Some(input) = &self.adding {
            let search = theme.ui().search.clone();
            let well_height = self.row_height + 8.0;
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
                input
                    .layout(
                        arena,
                        store,
                        ui,
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
        let keymap =
            leaf::<AgentsCommand>(size.width, size.height).event(move |_arena, event, _size| {
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
            });
        overlay.place(0.0, 0.0, keymap);

        let boot = !self.booted;
        overlay.wrap(move |inner| BootShell { inner, boot })
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

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, AgentsCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
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
        let panel = AgentsPanel::open(store, window);
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
