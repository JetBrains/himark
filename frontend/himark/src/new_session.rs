// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::{AnyEffect, CancellationToken, Effects},
    event::{Event, EventResult, Key},
    leaf::leaf,
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, LayoutExt as _, UiCtx, View, Widget,
};
use skia_safe::{Paint, Rect, Size};

use crate::combo::{Combo, ComboCommand, ComboItem, ComboOption};
use crate::higent::{
    ConnectServerEffect, HostId, HostStatus, Hosts, ListSessionsEffect, ResolveSessionConfigEffect,
    RootInfo, Servers, SessionOptions, SessionsPage,
};
use crate::{EditorCommand, EditorView};

const PICK_FOLDER: &str = "\u{1}pick-folder";

const NO_FOLDER: &str = "\u{1}no-folder";

#[derive(Clone)]
struct ModelOption {
    key: String,
    provider: String,
    provider_label: String,
    label: String,
    model: Option<ahp_types::state::SessionModelInfo>,
}

impl ModelOption {
    fn heading(provider: &str, label: &str) -> Self {
        Self {
            key: format!("\u{1}provider:{provider}"),
            provider: provider.to_owned(),
            provider_label: label.to_owned(),
            label: label.to_owned(),
            model: None,
        }
    }

    fn model(
        provider: &str,
        provider_label: &str,
        model: ahp_types::state::SessionModelInfo,
    ) -> Self {
        Self {
            key: format!("\u{1}model:{provider}:{}", model.id),
            provider: provider.to_owned(),
            provider_label: provider_label.to_owned(),
            label: model.name.clone(),
            model: Some(model),
        }
    }
}

impl View for ModelOption {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        ModelOptionRow {
            option: self,
            store,
            ui,
        }
    }
}

impl ComboItem for ModelOption {
    fn id(&self) -> &str {
        &self.key
    }

    fn cell_label(&self) -> String {
        self.label.clone()
    }

    fn search_label(&self) -> String {
        format!("{} {}", self.provider_label, self.label)
    }

    fn selectable(&self) -> bool {
        self.model.is_some()
    }
}

pub struct PickFoldersEffect {
    pub window: crate::WindowId,
}

impl imba::effect::Effect for PickFoldersEffect {
    type Result = Vec<crate::ResourceLocation>;
}

#[derive(Clone, Default)]
pub struct PendingFolderPick(pub Arc<Vec<crate::ResourceLocation>>);

/// Values carried over from the session that was current when the composer
/// opened. Each field is applied once its combo lists the value, then
/// cleared; an explicit user pick also cancels the corresponding seed so
/// late-arriving host data never overrides it.
#[derive(Clone, Default)]
struct Prefill {
    dir: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    edits: Option<String>,
    worktree: Option<bool>,
}

impl Prefill {
    fn of_session(store: &Store, session: &crate::SessionId) -> Self {
        let mut prefill = Self::default();
        if let Some(folder) = crate::higent::session_folders(store, session).first() {
            if let Some(uris) = Hosts::uris(store, session.host) {
                prefill.dir = Some(uris.uri_of(folder).as_str().to_owned());
            }
        }
        let Some(channel) = crate::higent::Agents::channel(store, session) else {
            return prefill;
        };
        if !channel.provider.is_empty() {
            prefill.provider = Some(channel.provider.clone());
        }
        if let Some(config) = &channel.config {
            let value = |key: &str| {
                config
                    .values
                    .get(key)
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            };
            prefill.model = value("model");
            prefill.effort = value("thinkingLevel");
            prefill.mode = value("mode");
            prefill.edits = value("permissionMode");
            prefill.worktree = config
                .values
                .get("worktree")
                .and_then(|value| value.as_bool());
        }
        prefill
    }
}

/// Per-window: two windows can compose concurrently, and a shared slot
/// would let one consume or clobber the schema resolved for the other.
#[derive(Clone, Default)]
pub struct ComposerFeed {
    pub resolving: rpds::HashTrieMapSync<crate::WindowId, CancellationToken>,
    pub resolved: rpds::HashTrieMapSync<
        crate::WindowId,
        (
            HostId,
            Result<ahp_types::commands::ResolveSessionConfigResult, String>,
        ),
    >,

    pub generation: u64,
}

fn store_fingerprint(store: &Store) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    crate::higent::Hosts::generation(store).hash(&mut hasher);
    store
        .get::<ComposerFeed>()
        .map(|feed| feed.generation)
        .unwrap_or(0)
        .hash(&mut hasher);
    store
        .get::<PendingFolderPick>()
        .map(|pending| pending.0.len())
        .unwrap_or(0)
        .hash(&mut hasher);
    hasher.finish()
}

#[doc(hidden)]
pub struct ComboProbe {
    pub labels: Vec<String>,
    pub picked: Option<String>,
    pub open: bool,
}

#[doc(hidden)]
pub struct NewSessionProbe {
    pub host: ComboProbe,
    pub dir: ComboProbe,
    pub mode: ComboProbe,
    pub model: ComboProbe,
    pub effort: ComboProbe,
    pub edits: ComboProbe,
    pub worktree: bool,
    pub ready: bool,
    pub prompt: String,

    pub cells: Vec<(f32, f32)>,
}

pub enum NewSessionCommand {
    Editor(ScrollCommand<EditorCommand>),
    Host(ComboCommand),
    Dir(ComboCommand),
    Mode(ComboCommand),
    Model(ComboCommand),
    Effort(ComboCommand),
    Edits(ComboCommand),
    ToggleWorktree,

    Sync,

    Rewrap(f32),
    Start,
}

#[derive(Clone)]
pub struct NewSessionView {
    window: crate::WindowId,
    input: ScrollView<EditorView>,
    host: Combo,

    hosts: Arc<Vec<HostId>>,
    dir: Combo,
    mode: Combo,
    model: Combo<ModelOption>,
    effort: Combo,
    edits: Combo,
    worktree: bool,

    host_hint: Option<HostId>,
    prefill: Prefill,

    synced: u64,

    asked: Option<String>,
    request: Option<crate::PanelRequest>,

    cell_spans: Arc<Vec<std::sync::atomic::AtomicU64>>,
}

fn fresh_input(store: &imba::store::Store, ui: &imba::UiCtx) -> ScrollView<EditorView> {
    let document = crate::Document::new(crate::Text::from_string_exact(""), crate::Markup::new())
        .with_syntax(
            crate::Syntax::new("markdown", None, crate::Markup::new()),
            &[],
        );
    let fonts = crate::fonts::source()();
    let theme = crate::Theme::embedded();
    let mut view = EditorView::of_document(document, 600.0, store, ui, &fonts, &theme);
    view.set_placeholder("What are you building?", &fonts, &theme);
    view.gutter_width = theme.ui().editor_gutter.width;
    let mut input = ScrollView::new(view);
    input.content_mut().focus_text();
    input
}

impl NewSessionView {
    pub fn new(store: &imba::store::Store, ui: &imba::UiCtx, window: crate::WindowId) -> Self {
        Self::for_host(store, ui, window, None)
    }

    pub fn for_host(
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        host: Option<HostId>,
    ) -> Self {
        Self::seeded(store, ui, window, host, Prefill::default())
    }

    fn seeded(
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        host: Option<HostId>,
        prefill: Prefill,
    ) -> Self {
        Self {
            window,
            input: fresh_input(store, ui),
            host: Combo::new(store, ui, "HOST"),
            hosts: Arc::new(Vec::new()),
            dir: Combo::new(store, ui, "DIR"),
            mode: Combo::new(store, ui, "MODE"),
            model: Combo::new(store, ui, "MODEL"),
            effort: Combo::new(store, ui, "EFFORT"),
            edits: Combo::new(store, ui, "EDITS"),
            worktree: prefill.worktree.unwrap_or(false),
            host_hint: host,
            prefill,
            synced: u64::MAX,
            asked: None,
            request: None,
            cell_spans: Arc::new(
                (0..7)
                    .map(|_| std::sync::atomic::AtomicU64::new(0))
                    .collect(),
            ),
        }
    }

    #[doc(hidden)]
    pub fn probe(&self) -> NewSessionProbe {
        let combo = |combo: &Combo| ComboProbe {
            labels: combo
                .options()
                .iter()
                .map(|option| option.label.clone())
                .collect(),
            picked: combo.value().map(|option| option.label.clone()),
            open: combo.open,
        };
        NewSessionProbe {
            host: combo(&self.host),
            dir: combo(&self.dir),
            mode: combo(&self.mode),
            model: ComboProbe {
                labels: self
                    .model
                    .options()
                    .iter()
                    .map(|option| option.label.clone())
                    .collect(),
                picked: self.model.value().map(|option| option.label),
                open: self.model.open,
            },
            effort: combo(&self.effort),
            edits: combo(&self.edits),
            worktree: self.worktree,
            ready: self.ready(),
            prompt: self.prompt(),
            cells: self
                .cell_spans
                .iter()
                .map(|span| {
                    let bits = span.load(std::sync::atomic::Ordering::Relaxed);
                    (
                        f32::from_bits((bits >> 32) as u32),
                        f32::from_bits(bits as u32),
                    )
                })
                .collect(),
        }
    }

    fn prompt(&self) -> String {
        let document = &self.input.content().document;
        let end = document.text().byte_count().min(u32::MAX as usize) as u32;
        document.text().view().substring(0..end)
    }

    fn picked_host(&self) -> Option<HostId> {
        self.hosts.get(self.host.picked).copied()
    }

    fn picked_provider(&self) -> Option<String> {
        self.model.value().map(|model| model.provider)
    }

    fn ready(&self) -> bool {
        let host_ready = self
            .picked_host()
            .is_some_and(|_| matches!(host_status(&self.host), Some(HostStatus::Connected)));

        host_ready && self.picked_provider().is_some() && !self.prompt().trim().is_empty()
    }

    fn refresh_hosts(&mut self, store: &Store, ui: &UiCtx) {
        let listed = Hosts::list(store);
        let mut ids = Vec::new();
        let mut options = Vec::new();
        for (id, host) in listed {
            let trail = match &host.status {
                HostStatus::Connected => None,
                HostStatus::Connecting => Some("connecting…".to_owned()),
                HostStatus::Idle => Some("idle".to_owned()),
                HostStatus::Failed(_) => Some("failed".to_owned()),
            };
            ids.push(id);
            options.push(ComboOption {
                id: format!("{id:?}"),
                label: host.name.clone(),
                trail,
            });
        }
        self.hosts = Arc::new(ids);
        self.host.set_options(store, ui, options);

        let preferred = self.host_hint.take().or_else(|| {
            store
                .get::<crate::higent::LocalHost>()
                .and_then(|local| local.0)
        });
        if let Some(preferred) = preferred {
            if let Some(index) = self.hosts.iter().position(|id| *id == preferred) {
                self.host.picked = index;
            }
        }
        self.refresh_dirs(store, ui);
        self.refresh_models(store, ui);
    }

    fn refresh_dirs(&mut self, store: &Store, ui: &UiCtx) {
        let picked = self
            .dir
            .value()
            .filter(|option| option.id != NO_FOLDER && option.id != PICK_FOLDER);
        let mut options = vec![ComboOption::plain(NO_FOLDER, "No folder")];
        if let Some(host) = self.picked_host() {
            let uris = Hosts::uris(store, host);
            for folder in host_folders(store, host) {
                let Some(uris) = &uris else { continue };
                let id = uris.uri_of(&folder).as_str().to_owned();

                if options.iter().any(|option| option.id == id) {
                    continue;
                }
                options.push(ComboOption {
                    id,
                    label: folder.name().to_owned(),
                    trail: None,
                });
            }
        }
        if let Some(picked) = picked {
            if !options.iter().any(|option| option.id == picked.id) {
                options.push(picked);
            }
        }
        options.push(ComboOption::plain(PICK_FOLDER, "Choose folder…"));
        self.dir.set_options(store, ui, options);
        if let Some(dir) = self.prefill.dir.clone() {
            self.dir.pick_id(&dir);
            if self.dir.value().is_some_and(|option| option.id == dir) {
                self.prefill.dir = None;
            }
        }
    }

    fn refresh_models(&mut self, store: &Store, ui: &UiCtx) {
        let options = self
            .picked_host()
            .and_then(|id| crate::higent::Hosts::host_ref(store, id))
            .map(|host| {
                let mut options = Vec::new();
                for agent in &host.agents {
                    if agent.models.is_empty() {
                        continue;
                    }
                    options.push(ModelOption::heading(&agent.provider, &agent.display_name));
                    options.extend(agent.models.iter().cloned().map(|model| {
                        ModelOption::model(&agent.provider, &agent.display_name, model)
                    }));
                }
                options
            })
            .unwrap_or_default();
        self.model.set_options(store, ui, options);
        self.apply_model_prefill();
        self.refresh_effort(store, ui);
    }

    fn apply_model_prefill(&mut self) {
        let Some(provider) = self.prefill.provider.clone() else {
            return;
        };
        let options = self.model.options();
        // The session's exact model may no longer be advertised; once the
        // provider is listed, settle for its first model so the agent still
        // carries over instead of the combo keeping the global default.
        let key = self
            .prefill
            .model
            .as_ref()
            .map(|model| format!("\u{1}model:{provider}:{model}"))
            .filter(|key| options.iter().any(|option| &option.key == key))
            .or_else(|| {
                options
                    .iter()
                    .find(|option| option.provider == provider && option.model.is_some())
                    .map(|option| option.key.clone())
            });
        let Some(key) = key else { return };
        self.model.pick_id(&key);
        if self.model.value().is_some_and(|option| option.key == key) {
            self.prefill.provider = None;
            self.prefill.model = None;
        }
    }

    fn refresh_effort(&mut self, store: &Store, ui: &UiCtx) {
        // Effort ids such as `medium` recur across models, so the seed waits
        // for the model prefill to resolve: applied to a stand-in model it
        // would be consumed there and lost for the intended one.
        let seed = self
            .prefill
            .provider
            .is_none()
            .then(|| self.prefill.effort.clone())
            .flatten();
        crate::higent::sync_effort_for_model(
            store,
            ui,
            &mut self.effort,
            self.model.value().and_then(|option| option.model),
            seed.as_deref(),
        );
        if let Some(seed) = seed {
            self.effort.pick_id(&seed);
            if self.effort.value().is_some_and(|picked| picked.id == seed) {
                self.prefill.effort = None;
            }
        }
    }

    fn config_values(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut values = serde_json::Map::new();
        if let Some(mode) = self.mode.value() {
            values.insert("mode".to_owned(), serde_json::json!(mode.id));
        }
        if let Some(edits) = self.edits.value() {
            values.insert("permissionMode".to_owned(), serde_json::json!(edits.id));
        }
        values.insert("worktree".to_owned(), serde_json::json!(self.worktree));
        values
    }

    fn file_ask(&mut self) {
        let Some(host) = self.picked_host() else {
            return;
        };
        let working_directory = self
            .dir
            .value()
            .filter(|option| option.id != PICK_FOLDER && option.id != NO_FOLDER)
            .map(|option| option.id.clone());
        let config = self.config_values();
        let provider = self.picked_provider();
        let inputs = format!(
            "{host:?}|{}|{}|{}",
            provider.as_deref().unwrap_or(""),
            working_directory.as_deref().unwrap_or(""),
            serde_json::Value::Object(config.clone())
        );
        if self.asked.as_deref() == Some(inputs.as_str()) {
            return;
        }
        self.asked = Some(inputs);
        self.request = Some(crate::PanelRequest::Perform(Arc::new(ComposerAsk {
            host,
            provider,
            working_directory,
            config,
        })));
    }

    fn apply_schema(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        result: ahp_types::commands::ResolveSessionConfigResult,
    ) {
        let fresh_mode = self.mode.is_empty();
        let fresh_edits = self.edits.is_empty();
        if let Some(property) = result.schema.properties.get("mode") {
            self.mode.set_options(
                store,
                ui,
                enum_options(property.r#enum.as_deref(), property.enum_labels.as_deref()),
            );
        }
        if let Some(property) = result.schema.properties.get("permissionMode") {
            self.edits.set_options(
                store,
                ui,
                enum_options(property.r#enum.as_deref(), property.enum_labels.as_deref()),
            );
        }

        if fresh_mode {
            if let Some(value) = result.values.get("mode").and_then(|value| value.as_str()) {
                self.mode.pick_id(value);
            }
        }
        // An early resolve may carry a schema without the session's mode or
        // edits value; each seed survives until an option matches, so a
        // later provider-specific schema can still restore it.
        if let Some(value) = self.prefill.mode.clone() {
            self.mode.pick_id(&value);
            if self.mode.value().is_some_and(|option| option.id == value) {
                self.prefill.mode = None;
            }
        }
        if fresh_edits {
            if let Some(value) = result
                .values
                .get("permissionMode")
                .and_then(|value| value.as_str())
            {
                self.edits.pick_id(value);
            }
        }
        if let Some(value) = self.prefill.edits.clone() {
            self.edits.pick_id(&value);
            if self.edits.value().is_some_and(|option| option.id == value) {
                self.prefill.edits = None;
            }
        }
    }

    fn start(&mut self, _store: &Store) {
        if !self.ready() {
            return;
        }
        let Some(server) = self.picked_host() else {
            return;
        };
        let Some(provider) = self.picked_provider() else {
            return;
        };

        let working_directories = self
            .dir
            .value()
            .map(|option| option.id.clone())
            .filter(|id| id != PICK_FOLDER && id != NO_FOLDER)
            .map(|dir| vec![dir])
            .unwrap_or_default();
        let model = self
            .model
            .value()
            .and_then(|option| option.model)
            .map(|model| ahp_types::state::ModelSelection {
                id: model.id,
                config: self.effort.value().map(|effort| {
                    let mut config = std::collections::HashMap::new();
                    config.insert("thinkingLevel".to_owned(), serde_json::json!(effort.id));
                    config
                }),
            });
        self.request = Some(crate::PanelRequest::Perform(Arc::new(
            StartComposedSession {
                server,
                working_directories,
                options: SessionOptions {
                    provider: Some(provider),
                    config: Some(self.config_values()),
                    model,
                },
                prompt: self.prompt().trim().to_owned(),
            },
        )));
    }
}

pub(crate) fn enum_options(
    values: Option<&[serde_json::Value]>,
    labels: Option<&[String]>,
) -> Vec<ComboOption> {
    let Some(values) = values else {
        return Vec::new();
    };
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let id = value.as_str()?;
            let label = labels
                .and_then(|labels| labels.get(index).cloned())
                .unwrap_or_else(|| id.to_owned());
            Some(ComboOption::plain(id, label))
        })
        .collect()
}

fn host_status(combo: &Combo) -> Option<HostStatus> {
    let value = combo.value()?;
    match value.trail.as_deref() {
        None => Some(HostStatus::Connected),
        Some("connecting…") => Some(HostStatus::Connecting),
        Some("idle") => Some(HostStatus::Idle),
        Some(_) => Some(HostStatus::Failed(String::new())),
    }
}

fn host_folders(store: &Store, host: HostId) -> Vec<crate::ResourceLocation> {
    let mut seen = std::collections::HashSet::new();
    let mut folders = Vec::new();
    for (id, entry) in Hosts::list(store) {
        if id != host {
            continue;
        }
        let mut sessions: Vec<String> = entry.sessions.iter().map(|s| s.resource.clone()).collect();
        for (session, _) in entry.states.iter() {
            sessions.push(session.clone());
        }
        for session in sessions {
            let key = crate::SessionId { host: id, session };
            for folder in crate::higent::session_folders(store, &key) {
                if seen.insert(folder.clone()) {
                    folders.push(folder);
                }
            }
        }
    }
    folders
}

impl View for NewSessionView {
    type Command = NewSessionCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, NewSessionCommand> {
        use imba::focus::FocusData;
        // A STANDING combo menu owns the keyboard before the prompt.
        let open = if self.host.open {
            Some(self.host.focus_data(store, ui).map(NewSessionCommand::Host))
        } else if self.dir.open {
            Some(self.dir.focus_data(store, ui).map(NewSessionCommand::Dir))
        } else if self.mode.open {
            Some(self.mode.focus_data(store, ui).map(NewSessionCommand::Mode))
        } else if self.model.open {
            Some(
                self.model
                    .focus_data(store, ui)
                    .map(NewSessionCommand::Model),
            )
        } else if self.effort.open {
            Some(
                self.effort
                    .focus_data(store, ui)
                    .map(NewSessionCommand::Effort),
            )
        } else if self.edits.open {
            Some(
                self.edits
                    .focus_data(store, ui)
                    .map(NewSessionCommand::Edits),
            )
        } else {
            None
        };
        let own = FocusData {
            on_key: Some(Box::new(|key, mods| match key {
                Key::Enter if mods.command => EventResult::Command(NewSessionCommand::Start),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        let inner = match open {
            Some(combo) => combo.merge_over(
                self.input
                    .focus_data(store, ui)
                    .map(NewSessionCommand::Editor),
            ),
            None => self
                .input
                .focus_data(store, ui)
                .map(NewSessionCommand::Editor),
        };
        own.merge_under(inner)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            NewSessionCommand::Editor(command) => {
                fx.scope(NewSessionCommand::Editor, |fx| {
                    self.input.perform(store, ui, command, fx)
                });
            }
            NewSessionCommand::Host(command) => {
                let picked = command.picks();
                fx.scope(NewSessionCommand::Host, |fx| {
                    self.host.perform(store, ui, command, fx)
                });
                if picked {
                    // The seeds describe a session on the original host;
                    // none of them survive an explicit host change.
                    self.prefill = Prefill::default();
                    self.refresh_dirs(store, ui);
                    self.refresh_models(store, ui);
                    self.file_ask();
                }
            }
            NewSessionCommand::Dir(command) => {
                let picked = command.picks();
                fx.scope(NewSessionCommand::Dir, |fx| {
                    self.dir.perform(store, ui, command, fx)
                });
                if picked {
                    self.prefill.dir = None;
                    if self.dir.value().map(|option| option.id) == Some(PICK_FOLDER.to_owned()) {
                        self.request =
                            Some(crate::PanelRequest::Perform(Arc::new(PickSessionFolder)));
                    } else {
                        self.file_ask();
                    }
                }
            }
            NewSessionCommand::Mode(command) => {
                let picked = command.picks();
                fx.scope(NewSessionCommand::Mode, |fx| {
                    self.mode.perform(store, ui, command, fx)
                });
                if picked {
                    self.prefill.mode = None;
                    self.file_ask();
                }
            }
            NewSessionCommand::Model(command) => {
                let picked = command.picks();
                let before = self.model.value().map(|option| option.key);
                fx.scope(NewSessionCommand::Model, |fx| {
                    self.model.perform(store, ui, command, fx)
                });
                if picked {
                    // The effort seed follows the session's model, so an
                    // explicit model pick retires it along with the model.
                    self.prefill.provider = None;
                    self.prefill.model = None;
                    self.prefill.effort = None;
                }
                let after = self.model.value().map(|option| option.key);
                if before != after {
                    self.effort.set_options(store, ui, Vec::new());
                    self.refresh_effort(store, ui);
                    self.file_ask();
                }
            }
            NewSessionCommand::Effort(command) => {
                let picked = command.picks();
                fx.scope(NewSessionCommand::Effort, |fx| {
                    self.effort.perform(store, ui, command, fx)
                });
                if picked {
                    self.prefill.effort = None;
                }
            }
            NewSessionCommand::Edits(command) => {
                let picked = command.picks();
                fx.scope(NewSessionCommand::Edits, |fx| {
                    self.edits.perform(store, ui, command, fx)
                });
                if picked {
                    self.prefill.edits = None;
                    self.file_ask();
                }
            }
            NewSessionCommand::ToggleWorktree => {
                self.worktree = !self.worktree;
                self.file_ask();
            }
            NewSessionCommand::Sync => {
                self.refresh_hosts(store, ui);

                let picked = store
                    .get::<PendingFolderPick>()
                    .filter(|pending| !pending.0.is_empty())
                    .map(|pending| Arc::clone(&pending.0));
                if let Some(picked) = picked {
                    store.put(PendingFolderPick::default());
                    if let Some(host) = self.picked_host() {
                        if let Some(uris) = Hosts::uris(store, host) {
                            let mut options = self.dir.options();
                            let mut last = None;
                            for location in picked.iter().filter(|l| l.kind().is_directory()) {
                                let id = uris.uri_of(location).as_str().to_owned();
                                if !options.iter().any(|option| option.id == id) {
                                    options.insert(
                                        options.len().saturating_sub(1),
                                        ComboOption {
                                            id: id.clone(),
                                            label: location.name().to_owned(),
                                            trail: None,
                                        },
                                    );
                                }
                                last = Some(id);
                            }
                            self.dir.set_options(store, ui, options);
                            if let Some(id) = last {
                                self.dir.pick_id(&id);
                            }
                        }
                    }
                }
                let resolved = store
                    .get::<ComposerFeed>()
                    .and_then(|feed| feed.resolved.get(&self.window).cloned());
                if let Some((host, result)) = resolved {
                    store.update::<ComposerFeed>(|feed| {
                        feed.resolved.remove_mut(&self.window);
                    });
                    if Some(host) == self.picked_host() {
                        match result {
                            Ok(result) => self.apply_schema(store, ui, result),
                            Err(error) => {
                                eprintln!("[new-session] resolveSessionConfig: {error}")
                            }
                        }
                    }
                }

                self.file_ask();

                self.synced = store_fingerprint(store);
            }
            NewSessionCommand::Rewrap(width) => {
                let fonts = crate::env::ui_collection(store, ui);
                let theme = crate::env::Themes::of(store);
                let editor = self.input.content().editor;
                fx.scope(NewSessionCommand::Editor, |fx| {
                    fx.scope(ScrollCommand::Content, |fx| {
                        self.input
                            .content_mut()
                            .document
                            .resize(editor, width, 0, store, ui, &fonts, &theme, fx);
                    })
                });
            }
            NewSessionCommand::Start => self.start(store),
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
            let themes = crate::env::Themes::of(store);
            let theme = themes.ui();
            let controls_h = theme.toolbar.height;
            let editor_h = (size.height - controls_h).max(1.0);
            let gutter = theme.editor_gutter.width;
            let top_pad = theme.chat.pad * 2.0;

            let mut root = imba::container::container(arena, size);

            let background = theme.window.background.0;
            let rule = theme.toolbar.rule.0;
            root.place(
                0.0,
                0.0,
                leaf::<NewSessionCommand>(size.width, size.height).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_color(background);
                        canvas.draw_rect(rect, &paint);
                        paint.set_color(rule);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.bottom - controls_h, rect.width(), 1.0),
                            &paint,
                        );
                    },
                ),
            );

            root.place(
                0.0,
                top_pad,
                imba::Layout::layout(
                    self.input.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(size.width, (editor_h - top_pad).max(1.0))),
                )
                .map(NewSessionCommand::Editor)
                .focus_scope(true),
            );

            let row_top = size.height - controls_h + 1.0;
            let row_h = controls_h - 1.0;

            let key_font = crate::fonts::ui_text_font(ui, theme.peeker.hint_size * 0.95);
            let caps_font = crate::fonts::ui_font(ui, theme.combo.label_size * 1.1);
            let pad = theme.combo.pad;
            let start_label = "START SESSION";
            let start_width = start_label
                .chars()
                .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                .sum::<f32>()
                + key_font.measure_str("⌘⏎", None).0
                + theme.combo.gap
                + pad * 2.0;
            let start_x = size.width - start_width;

            let mut row = imba::container::container(
                arena,
                Size::new((start_x - theme.combo.gap).max(0.0), row_h),
            );
            let mut x = 0.0f32;
            let context: [(&Combo, fn(ComboCommand) -> NewSessionCommand); 3] = [
                (&self.host, NewSessionCommand::Host),
                (&self.dir, NewSessionCommand::Dir),
                (&self.mode, NewSessionCommand::Mode),
            ];
            for (index, (combo, wrap)) in context.into_iter().enumerate() {
                let width = combo.cell_width(ui, &theme.combo);
                if let Some(span) = self.cell_spans.get(index) {
                    span.store(
                        ((x.to_bits() as u64) << 32) | width.to_bits() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                row.place(x, 0.0, combo.cell(arena, store, ui, row_h).map(wrap));
                x += width;
            }
            let width = self.model.cell_width(ui, &theme.combo);
            if let Some(span) = self.cell_spans.get(3) {
                span.store(
                    ((x.to_bits() as u64) << 32) | width.to_bits() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            row.place(
                x,
                0.0,
                self.model
                    .cell(arena, store, ui, row_h)
                    .map(NewSessionCommand::Model),
            );
            x += width;

            let choices: [(&Combo, fn(ComboCommand) -> NewSessionCommand); 2] = [
                (&self.effort, NewSessionCommand::Effort),
                (&self.edits, NewSessionCommand::Edits),
            ];
            for (offset, (combo, wrap)) in choices.into_iter().enumerate() {
                let width = combo.cell_width(ui, &theme.combo);
                if let Some(span) = self.cell_spans.get(offset + 4) {
                    span.store(
                        ((x.to_bits() as u64) << 32) | width.to_bits() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                row.place(x, 0.0, combo.cell(arena, store, ui, row_h).map(wrap));
                x += width;
            }
            root.place(0.0, row_top, row);

            let hint_font = crate::fonts::ui_text_font(ui, theme.peeker.hint_size);

            let ready = self.ready();
            let accent = theme.chat.accent.0;
            let on_accent = theme.chat.on_accent.0;
            let accent_soft = theme.peeker.dim_text.0;
            // The START cell as a Row of `Text`s at exact baseline
            // parity (the chat SEND cell pattern): the old per-char
            // loop advanced by glyph width + 1.5 (= `.tracking(1.5)`)
            // from x = left + pad, and drew the key hint `theme_gap()`
            // after the label's last advance (= the Row's `.gap`).
            // Each text pads down so its baseline lands on the old
            // mid + font.size() * 0.35 line. The accent fill and left
            // rule stay a backdrop painter; the press is `.on_click`,
            // minting Start like the old event closure (readiness
            // gates inside `start()`, as before).
            let mid = row_h * 0.5;
            let caps_ascent = -caps_font.metrics().1.ascent;
            let key_ascent = -key_font.metrics().1.ascent;
            let start_cell = imba::Row::new(arena)
                .gap(theme_gap())
                .child(
                    imba::text(start_label, caps_font.clone(), on_accent)
                        .tracking(1.5)
                        .pad_insets(imba::Insets {
                            left: 0.0,
                            top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                            right: 0.0,
                            bottom: 0.0,
                        }),
                )
                .child(
                    imba::text("⌘⏎", key_font.clone(), accent_soft).pad_insets(imba::Insets {
                        left: 0.0,
                        top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                        right: 0.0,
                        bottom: 0.0,
                    }),
                )
                .pad_insets(imba::Insets {
                    left: pad,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                })
                .sized(start_width, row_h)
                .backdrop(
                    move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                        let mut paint = Paint::default();
                        let mut fill = accent;
                        if !ready {
                            fill = fill.with_a(0x50);
                        }
                        paint.set_color(fill.with_a(fill.a() / 3));
                        canvas.draw_rect(rect, &paint);
                        paint.set_anti_alias(false);
                        paint.set_color(fill);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                    },
                )
                .on_click(|| NewSessionCommand::Start);
            root.place_boxed(
                start_x,
                row_top,
                start_cell.layout(arena, Constraints::tight(Size::new(start_width, row_h))),
            );

            let hints = [("⌘⏎", "send"), ("⏎", "newline")];
            let hint_gap = theme.combo.gap;
            let hints_width: f32 = hints
                .iter()
                .map(|(key, label)| {
                    key_font.measure_str(key, None).0 + 5.0 + hint_font.measure_str(label, None).0
                })
                .sum::<f32>()
                + hint_gap
                + pad * 2.0;
            let hints_x = start_x - hints_width;
            let key_color = theme.peeker.dim_text.0;
            let label_color = theme.combo.label_color.0;
            // The hint pairs as a Row of `Text`s at exact baseline
            // parity: the old x bookkeeping advanced by the key's
            // measured width + 5.0 before its label and by the
            // label's width + the combo gap before the next key —
            // reproduced as trailing pads (none after the last
            // label, so nothing clips against the cell width). Keys
            // and labels pad down so their baselines land on the old
            // mid + font.size() * 0.35 lines; the left rule stays a
            // backdrop painter, and the cell keeps ignoring presses
            // like the old leaf.
            let hint_ascent = -hint_font.metrics().1.ascent;
            let mut hint_row = imba::Row::new(arena);
            let pairs = hints.len();
            for (index, (key, label)) in hints.into_iter().enumerate() {
                hint_row = hint_row
                    .child(
                        imba::text(key, key_font.clone(), key_color).pad_insets(imba::Insets {
                            left: 0.0,
                            top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                            right: 5.0,
                            bottom: 0.0,
                        }),
                    )
                    .child(
                        imba::text(label, hint_font.clone(), label_color).pad_insets(
                            imba::Insets {
                                left: 0.0,
                                top: (mid + hint_font.size() * 0.35 - hint_ascent).max(0.0),
                                right: match index + 1 == pairs {
                                    true => 0.0,
                                    false => hint_gap,
                                },
                                bottom: 0.0,
                            },
                        ),
                    );
            }
            let hint_cell = hint_row
                .pad_insets(imba::Insets {
                    left: pad,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                })
                .sized(hints_width, row_h)
                .backdrop(
                    move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(false);
                        paint.set_color(rule);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                    },
                );
            root.place_boxed(
                hints_x,
                row_top,
                hint_cell.layout(arena, Constraints::tight(Size::new(hints_width, row_h))),
            );

            let worktree_label = "New worktree";
            let check = theme.checkbox.size;
            let worktree_width =
                pad * 2.0 + check + 10.0 + hint_font.measure_str(worktree_label, None).0;
            let worktree_x = hints_x - worktree_width;
            if let Some(span) = self.cell_spans.get(6) {
                span.store(
                    ((worktree_x.to_bits() as u64) << 32) | worktree_width.to_bits() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            let checked = self.worktree;
            let box_color = theme.combo.label_color.0;
            let text_dim = theme.peeker.dim_text.0;
            // The worktree cell's LABEL is a `Text` at exact baseline
            // parity (top pad = old mid + font.size() * 0.35 baseline
            // − ascent, left pad = the old pad + check + 10.0 x); the
            // left rule and the checkbox GLYPH stay a backdrop
            // painter (form geometry), and the press is `.on_click`,
            // minting ToggleWorktree like the old event closure.
            let worktree_cell = imba::text(worktree_label, hint_font.clone(), text_dim)
                .pad_insets(imba::Insets {
                    left: pad + check + 10.0,
                    top: (mid + hint_font.size() * 0.35 - hint_ascent).max(0.0),
                    right: 0.0,
                    bottom: 0.0,
                })
                .sized(worktree_width, row_h)
                .backdrop(
                    move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(false);
                        paint.set_color(rule);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                        paint.set_anti_alias(true);
                        let mid = rect.top + rect.height() * 0.5;
                        let box_rect =
                            Rect::from_xywh(rect.left + pad, mid - check * 0.5, check, check);
                        paint.set_stroke(true);
                        paint.set_stroke_width(1.0);
                        paint.set_color(box_color);
                        canvas.draw_rect(box_rect, &paint);
                        if checked {
                            paint.set_stroke(false);
                            canvas.draw_rect(box_rect.with_inset((3.0, 3.0)), &paint);
                        }
                    },
                )
                .on_click(|| NewSessionCommand::ToggleWorktree);
            root.place_boxed(
                worktree_x,
                row_top,
                worktree_cell.layout(arena, Constraints::tight(Size::new(worktree_width, row_h))),
            );

            let stale = self.synced != store_fingerprint(store);
            let want = (size.width - gutter - theme.chat.pad * 2.0).max(120.0);
            let rewrap = ((self.input.content().layout_width() - want).abs() > 1.0).then_some(want);
            root.wrap(move |inner| NewSessionShell {
                inner,
                stale,
                rewrap,
            })
        })
    }
}

fn theme_gap() -> f32 {
    16.0
}

struct NewSessionShell<Inner> {
    inner: Inner,
    stale: bool,
    rewrap: Option<f32>,
}

impl<'a, Inner: Widget<'a, NewSessionCommand>> Widget<'a, NewSessionCommand>
    for NewSessionShell<Inner>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, NewSessionCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<NewSessionCommand> {
        match event {
            Event::Paint { .. } => {
                let mut result = self.inner.handle_event(arena, event, viewport);
                if self.stale {
                    result = result.merge(EventResult::Command(NewSessionCommand::Sync));
                }
                if let Some(width) = self.rewrap {
                    result = result.merge(EventResult::Command(NewSessionCommand::Rewrap(width)));
                }
                result
            }
            _ => self.inner.handle_event(arena, event, viewport),
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, NewSessionCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

#[derive(Clone, Default)]
pub struct Composers(rpds::HashTrieMapSync<crate::WindowId, NewSessionView>);

impl Composers {
    pub fn put(store: &mut Store, window: crate::WindowId, composer: NewSessionView) {
        store.update::<Composers>(|composers| {
            composers.0.insert_mut(window, composer);
        });
    }

    pub fn take(store: &mut Store, window: crate::WindowId) -> Option<NewSessionView> {
        let composer = store
            .get::<Composers>()
            .and_then(|composers| composers.0.get(&window).cloned());
        if composer.is_some() {
            store.update::<Composers>(|composers| {
                composers.0.remove_mut(&window);
            });
        }
        composer
    }

    pub fn composer_ref(store: &Store, window: crate::WindowId) -> Option<&NewSessionView> {
        store
            .get::<Composers>()
            .and_then(|composers| composers.0.get(&window))
    }

    pub fn remove(store: &mut Store, window: crate::WindowId) {
        store.update::<Composers>(|composers| {
            composers.0.remove_mut(&window);
        });
    }
}

pub struct ComposerPane {
    window: crate::WindowId,

    request: Option<crate::PanelRequest>,
}

impl ComposerPane {
    pub fn new(window: crate::WindowId) -> Self {
        Self {
            window,
            request: None,
        }
    }

    #[doc(hidden)]
    pub fn window(&self) -> crate::WindowId {
        self.window
    }
}

impl Clone for ComposerPane {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            request: None,
        }
    }
}

impl View for ComposerPane {
    type Command = NewSessionCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, NewSessionCommand> {
        match Composers::composer_ref(store, self.window) {
            Some(composer) => composer.focus_data(store, ui),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        let Some(mut composer) = Composers::take(store, self.window) else {
            return;
        };
        composer.perform(store, ui, command, fx);
        if let Some(request) = composer.request.take() {
            self.request = Some(request);
        }
        Composers::put(store, self.window, composer);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let Some(composer) = Composers::composer_ref(store, self.window) else {
                let blank = imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf(constraints.max.width, constraints.max.height),
                );
                return blank;
            };
            imba::ThunkBox::new(
                arena,
                imba::Layout::layout(composer.display(arena, store, ui), arena, constraints),
            )
        })
    }
}

impl crate::PanelView for ComposerPane {
    type Place = crate::NoPlace;

    fn title(&self, _store: &Store) -> String {
        "New session".to_owned()
    }

    fn dismantle(&mut self, store: &mut Store) {
        Composers::remove(store, self.window);
        store.update::<ComposerFeed>(|feed| {
            feed.resolving.remove_mut(&self.window);
            feed.resolved.remove_mut(&self.window);
        });
    }

    fn take_request(&mut self) -> Option<crate::PanelRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn full_bleed(&self) -> bool {
        true
    }
}

struct PickSessionFolder;

impl crate::DynamicCommand for PickSessionFolder {
    fn id(&self) -> &'static str {
        "session.pick-folder"
    }

    fn name(&self) -> String {
        "Choose Session Folder…".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        fx.push(
            AnyEffect::new(PickFoldersEffect { window }).map(move |locations| {
                crate::app::AppCommand::Dynamic(window, Arc::new(FoldersPicked { locations }))
            }),
        );
    }
}

struct FoldersPicked {
    locations: Vec<crate::ResourceLocation>,
}

impl crate::DynamicCommand for FoldersPicked {
    fn id(&self) -> &'static str {
        "session.folders-picked"
    }

    fn name(&self) -> String {
        "Session Folder Picked".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        store.put(PendingFolderPick(Arc::new(self.locations.clone())));
    }
}

struct StartComposedSession {
    server: HostId,
    working_directories: Vec<String>,
    options: SessionOptions,
    prompt: String,
}

impl crate::DynamicCommand for StartComposedSession {
    fn id(&self) -> &'static str {
        "session.start-composed"
    }

    fn name(&self) -> String {
        "Start Session".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(seat) = Servers::seat(store, self.server) else {
            eprintln!("[new-session] start: unregistered host {:?}", self.server);
            return;
        };

        if let Some(mut entity) = crate::Windows::window(store, window) {
            let _ = entity.close_focused_widget(store, ui, window, fx);
            crate::Windows::put(store, window, entity);
        }
        let server = self.server;
        let prompt = self.prompt.clone();

        if let Some((_, _, session)) =
            Placeholders::session_of(store, window).filter(|(host, provider, _)| {
                *host == server && self.options.provider.as_deref() == Some(provider.as_str())
            })
        {
            let applied: Vec<String> = store
                .get::<Placeholders>()
                .and_then(|rows| rows.0.get(&window).cloned())
                .map(|row| row.applied.iter().cloned().collect())
                .unwrap_or_default();
            store.update::<Placeholders>(|rows| {
                rows.0.remove_mut(&window);
            });
            for directory in &self.working_directories {
                if !applied.contains(directory) {
                    fx.push(
                        AnyEffect::new(crate::higent::DispatchChatActionEffect {
                            seat: Arc::clone(&seat),
                            channel: session.clone(),
                            action: crate::higent::ahp_types::actions::StateAction::SessionWorkingDirectorySet(
                                crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                                    directory: directory.clone(),
                                },
                            ),
                        })
                        .map(move |result| {
                            crate::app::AppCommand::Dynamic(
                                window,
                                Arc::new(PlaceholderDispatched {
                                    label: "workingDirectorySet",
                                    result,
                                }),
                            )
                        }),
                    );
                }
            }

            let mut config = self.options.config.clone().unwrap_or_default();
            if let Some(model) = &self.options.model {
                config.insert("model".to_owned(), serde_json::json!(model.id));
                if let Some(thinking) = model
                    .config
                    .as_ref()
                    .and_then(|values| values.get("thinkingLevel"))
                {
                    config.insert("thinkingLevel".to_owned(), thinking.clone());
                }
            }
            if !config.is_empty() {
                fx.push(
                    AnyEffect::new(crate::higent::DispatchChatActionEffect {
                        seat,
                        channel: session.clone(),
                        action:
                            crate::higent::ahp_types::actions::StateAction::SessionConfigChanged(
                                crate::higent::ahp_types::actions::SessionConfigChangedAction {
                                    config,
                                    replace: None,
                                },
                            ),
                    })
                    .map(move |result| {
                        crate::app::AppCommand::Dynamic(
                            window,
                            Arc::new(PlaceholderDispatched {
                                label: "configChanged",
                                result,
                            }),
                        )
                    }),
                );
            }
            crate::higent::open_session_with(
                store,
                window,
                server,
                session,
                true,
                Some(prompt),
                fx,
            );
            return;
        }
        fx.push(
            AnyEffect::new(crate::higent::CreateSessionEffect {
                seat,
                working_directories: self.working_directories.clone(),
                options: self.options.clone(),
            })
            .map(move |result| {
                crate::app::AppCommand::Dynamic(
                    window,
                    Arc::new(crate::higent::OpenCreatedSession {
                        server,
                        open_chat: true,
                        initial_prompt: Some(prompt.clone()),
                        result,
                    }),
                )
            }),
        );
    }
}

struct ComposerAsk {
    host: HostId,
    provider: Option<String>,
    working_directory: Option<String>,
    config: serde_json::Map<String, serde_json::Value>,
}

impl crate::DynamicCommand for ComposerAsk {
    fn id(&self) -> &'static str {
        "session.compose-ask"
    }

    fn name(&self) -> String {
        "Resolve Session Options".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let host = self.host;
        let Some(seat) = Servers::seat(store, host) else {
            return;
        };

        let settled = Hosts::host_ref(store, host).is_some_and(|entry| {
            matches!(entry.status, HostStatus::Connected | HostStatus::Connecting)
        });
        if !settled {
            crate::higent::Agents::set_status(store, host, HostStatus::Connecting);
            fx.push(
                AnyEffect::new(ConnectServerEffect {
                    seat: Arc::clone(&seat),
                })
                .map(move |result| {
                    crate::app::AppCommand::Dynamic(window, Arc::new(HostReady { host, result }))
                }),
            );
        }

        if let Some(token) = store
            .get::<ComposerFeed>()
            .and_then(|feed| feed.resolving.get(&window).cloned())
        {
            fx.cancel(token);
        }
        let token = fx.push(
            AnyEffect::new(ResolveSessionConfigEffect {
                seat,
                working_directory: self.working_directory.clone(),
                config: Some(self.config.clone()),
            })
            .map(move |result| {
                crate::app::AppCommand::Dynamic(window, Arc::new(ConfigResolved { host, result }))
            }),
        );
        store.update::<ComposerFeed>(|feed| {
            feed.resolving.insert_mut(window, token);
        });

        ensure_placeholder(
            store,
            window,
            host,
            self.provider.clone(),
            self.working_directory.clone(),
            fx,
        );
    }
}

struct HostReady {
    host: HostId,
    result: Result<RootInfo, String>,
}

impl crate::DynamicCommand for HostReady {
    fn id(&self) -> &'static str {
        "session.compose-host-ready"
    }

    fn name(&self) -> String {
        "Session Host Ready".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let host = self.host;
        match &self.result {
            Ok(info) => {
                crate::higent::Agents::set_agents(store, host, info.agents.clone());
                crate::higent::Agents::set_status(store, host, HostStatus::Connected);
                list_sessions(store, window, host, None, fx);
            }
            Err(error) => {
                crate::higent::Agents::set_status(store, host, HostStatus::Failed(error.clone()));
            }
        }
        bump_feed(store);
    }
}

fn list_sessions(
    store: &Store,
    window: crate::WindowId,
    host: HostId,
    cursor: Option<String>,
    fx: &mut crate::app::AppFx<'_>,
) {
    let Some(seat) = Servers::seat(store, host) else {
        return;
    };
    fx.push(
        AnyEffect::new(ListSessionsEffect { seat, cursor }).map(move |result| {
            crate::app::AppCommand::Dynamic(window, Arc::new(SessionsListed { host, result }))
        }),
    );
}

struct SessionsListed {
    host: HostId,
    result: Result<SessionsPage, String>,
}

impl crate::DynamicCommand for SessionsListed {
    fn id(&self) -> &'static str {
        "session.compose-sessions-listed"
    }

    fn name(&self) -> String {
        "Session Catalog Listed".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        match &self.result {
            Ok(page) => {
                crate::higent::Agents::add_sessions(
                    store,
                    self.host,
                    page.sessions.clone(),
                    page.next_cursor.is_none(),
                );
                if page.next_cursor.is_some() {
                    list_sessions(store, window, self.host, page.next_cursor.clone(), fx);
                }
            }
            Err(error) => eprintln!("[new-session] listSessions failed: {error}"),
        }
        bump_feed(store);
    }
}

struct ConfigResolved {
    host: HostId,
    result: Result<ahp_types::commands::ResolveSessionConfigResult, String>,
}

impl crate::DynamicCommand for ConfigResolved {
    fn id(&self) -> &'static str {
        "session.compose-config-resolved"
    }

    fn name(&self) -> String {
        "Session Options Resolved".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let host = self.host;
        let result = self.result.clone();
        store.update::<ComposerFeed>(|feed| {
            feed.resolving.remove_mut(&window);
            feed.resolved.insert_mut(window, (host, result.clone()));
        });
        bump_feed(store);
    }
}

fn bump_feed(store: &mut Store) {
    store.update::<ComposerFeed>(|feed| feed.generation = feed.generation.wrapping_add(1));
}

#[derive(Clone, Default)]
pub struct Placeholders(rpds::HashTrieMapSync<crate::WindowId, Placeholder>);

#[derive(Clone)]
struct Placeholder {
    host: HostId,
    provider: String,

    session: Option<String>,

    applied: rpds::VectorSync<String>,

    pending: Option<String>,
}

impl Placeholders {
    #[doc(hidden)]
    pub fn session_of(store: &Store, window: crate::WindowId) -> Option<(HostId, String, String)> {
        let rows = store.get::<Placeholders>()?;
        let row = rows.0.get(&window)?;
        Some((row.host, row.provider.clone(), row.session.clone()?))
    }
}

fn ensure_placeholder(
    store: &mut Store,
    window: crate::WindowId,
    host: HostId,
    provider: Option<String>,
    working_directory: Option<String>,
    fx: &mut crate::app::AppFx<'_>,
) {
    let Some(provider) = provider else {
        return;
    };
    let connected = Hosts::host_ref(store, host)
        .is_some_and(|entry| matches!(entry.status, HostStatus::Connected));
    let slot = store
        .get::<Placeholders>()
        .and_then(|rows| rows.0.get(&window).cloned());
    match slot {
        Some(row) if row.host == host && row.provider == provider => {
            let Some(directory) = working_directory else {
                return;
            };
            if row.applied.iter().any(|applied| *applied == directory) {
                return;
            }
            match row.session.clone() {
                Some(session) => grant_folder(store, window, host, session, directory),
                None => {
                    store.update::<Placeholders>(|rows| {
                        if let Some(mut row) = rows.0.get(&window).cloned() {
                            row.pending = Some(directory.clone());
                            rows.0.insert_mut(window, row);
                        }
                    });
                }
            }
        }
        Some(row) => {
            if let (Some(session), Some(seat)) = (row.session, Servers::seat(store, row.host)) {
                fx.push(
                    AnyEffect::new(crate::higent::DisposeSessionEffect { seat, session }).map(
                        move |result| {
                            crate::app::AppCommand::Dynamic(
                                window,
                                Arc::new(PlaceholderDispatched {
                                    label: "dispose",
                                    result: result.map(|_| ()),
                                }),
                            )
                        },
                    ),
                );
            }
            store.update::<Placeholders>(|rows| {
                rows.0.remove_mut(&window);
            });
            if connected {
                create_placeholder(store, window, host, provider, working_directory, fx);
            }
        }
        None => {
            if connected {
                create_placeholder(store, window, host, provider, working_directory, fx);
            }
        }
    }
}

fn create_placeholder(
    store: &mut Store,
    window: crate::WindowId,
    host: HostId,
    provider: String,
    working_directory: Option<String>,
    fx: &mut crate::app::AppFx<'_>,
) {
    let Some(seat) = Servers::seat(store, host) else {
        return;
    };
    store.update::<Placeholders>(|rows| {
        rows.0.insert_mut(
            window,
            Placeholder {
                host,
                provider: provider.clone(),
                session: None,
                applied: rpds::VectorSync::new_sync(),
                pending: working_directory.clone(),
            },
        );
    });
    let mut config = serde_json::Map::new();
    config.insert("unlisted".to_owned(), serde_json::json!(true));
    fx.push(
        AnyEffect::new(crate::higent::CreateSessionEffect {
            seat,
            working_directories: Vec::new(),
            options: SessionOptions {
                provider: Some(provider.clone()),
                config: Some(config),
                model: None,
            },
        })
        .map(move |result| {
            crate::app::AppCommand::Dynamic(
                window,
                Arc::new(PlaceholderCreated {
                    host,
                    provider,
                    result,
                }),
            )
        }),
    );
}

fn grant_folder(
    store: &mut Store,
    window: crate::WindowId,
    host: HostId,
    session: String,
    directory: String,
) {
    let Some(seat) = Servers::seat(store, host) else {
        return;
    };
    store.update::<Placeholders>(|rows| {
        if let Some(mut row) = rows.0.get(&window).cloned() {
            row.applied.push_back_mut(directory.clone());
            row.pending = None;
            rows.0.insert_mut(window, row);
        }
    });

    crate::AppRequests::push(
        store,
        Arc::new(GrantPlaceholderFolder {
            host,
            seat,
            session,
            directory,
        }),
    );
}

struct GrantPlaceholderFolder {
    host: HostId,
    seat: Arc<dyn crate::higent::AhpServer>,
    session: String,
    directory: String,
}

impl crate::DynamicCommand for GrantPlaceholderFolder {
    fn id(&self) -> &'static str {
        "session.placeholder-grant"
    }
    fn name(&self) -> String {
        "Grant Placeholder Folder".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let _ = self.host;
        fx.push(
            AnyEffect::new(crate::higent::DispatchChatActionEffect {
                seat: Arc::clone(&self.seat),
                channel: self.session.clone(),
                action: crate::higent::ahp_types::actions::StateAction::SessionWorkingDirectorySet(
                    crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                        directory: self.directory.clone(),
                    },
                ),
            })
            .map(move |result| {
                crate::app::AppCommand::Dynamic(
                    window,
                    Arc::new(PlaceholderDispatched {
                        label: "workingDirectorySet",
                        result,
                    }),
                )
            }),
        );
    }
}

struct PlaceholderCreated {
    host: HostId,
    provider: String,
    result: Result<String, String>,
}

impl crate::DynamicCommand for PlaceholderCreated {
    fn id(&self) -> &'static str {
        "session.placeholder-created"
    }
    fn name(&self) -> String {
        "Placeholder Session Created".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let session = match &self.result {
            Ok(session) => session.clone(),
            Err(error) => {
                eprintln!("[new-session] placeholder create failed: {error}");
                store.update::<Placeholders>(|rows| {
                    if rows.0.get(&window).is_some_and(|row| {
                        row.host == self.host
                            && row.provider == self.provider
                            && row.session.is_none()
                    }) {
                        rows.0.remove_mut(&window);
                    }
                });
                return;
            }
        };

        let standing = store
            .get::<Placeholders>()
            .and_then(|rows| rows.0.get(&window).cloned());
        let Some(row) = standing.filter(|row| {
            row.host == self.host && row.provider == self.provider && row.session.is_none()
        }) else {
            if let Some(seat) = Servers::seat(store, self.host) {
                fx.push(
                    AnyEffect::new(crate::higent::DisposeSessionEffect {
                        seat,
                        session: session.clone(),
                    })
                    .map(move |result| {
                        crate::app::AppCommand::Dynamic(
                            window,
                            Arc::new(PlaceholderDispatched {
                                label: "dispose",
                                result: result.map(|_| ()),
                            }),
                        )
                    }),
                );
            }
            return;
        };
        store.update::<Placeholders>(|rows| {
            if let Some(mut row) = rows.0.get(&window).cloned() {
                row.session = Some(session.clone());
                rows.0.insert_mut(window, row);
            }
        });
        let key = crate::SessionId {
            host: self.host,
            session: session.clone(),
        };
        if let Some(mut entity) = crate::Windows::window(store, window) {
            if entity.rekey_current(key.clone()) {
                crate::Windows::put(store, window, entity);
            } else {
                crate::Windows::put(store, window, entity);
                crate::switch_session(store, window, key, fx);
            }
        }

        crate::higent::open_session(store, window, self.host, session, false, fx);
        if let Some(directory) = row.pending {
            let host = self.host;
            ensure_placeholder(
                store,
                window,
                host,
                Some(self.provider.clone()),
                Some(directory),
                fx,
            );
        }
    }
}

struct PlaceholderDispatched {
    label: &'static str,
    result: Result<(), String>,
}

impl crate::DynamicCommand for PlaceholderDispatched {
    fn id(&self) -> &'static str {
        "session.placeholder-dispatched"
    }
    fn name(&self) -> String {
        "Placeholder Dispatched".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        if let Err(error) = &self.result {
            eprintln!("[new-session] placeholder {}: {error}", self.label);
        }
    }
}

pub struct OpenNewSession {
    pub host: Option<HostId>,
}

impl crate::DynamicCommand for OpenNewSession {
    fn id(&self) -> &'static str {
        "session.new"
    }

    fn name(&self) -> String {
        "New Session".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(entity) = crate::Windows::window_ref(store, window) else {
            return;
        };
        let current = entity.current_session();

        // Seed the composer from the session the user is looking at, unless
        // it targets a different host than the one explicitly requested.
        let mut host = self.host;
        let mut prefill = Prefill::default();
        if current.names_session() && host.is_none_or(|host| host == current.host) {
            host = Some(current.host);
            prefill = Prefill::of_session(store, &current);
        }

        if let Some((host, _, session)) = Placeholders::session_of(store, window) {
            if let Some(seat) = Servers::seat(store, host) {
                fx.push(
                    AnyEffect::new(crate::higent::DisposeSessionEffect { seat, session }).map(
                        move |result| {
                            crate::app::AppCommand::Dynamic(
                                window,
                                Arc::new(PlaceholderDispatched {
                                    label: "dispose",
                                    result: result.map(|_| ()),
                                }),
                            )
                        },
                    ),
                );
            }
        }
        store.update::<Placeholders>(|rows| {
            rows.0.remove_mut(&window);
        });
        Composers::put(
            store,
            window,
            NewSessionView::seeded(store, ui, window, host, prefill),
        );
        if current.names_session() {
            let scratch = crate::SessionId::mint_scratch(store);
            crate::switch_session(store, window, scratch, fx);
            crate::commands::BatchRequests::push(store, window, Arc::new(MountComposer));
            return;
        }
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let _ = entity.open_panel(store, ui, Box::new(ComposerPane::new(window)), fx);
        crate::Windows::put(store, window, entity);
    }
}

struct MountComposer;

impl crate::DynamicCommand for MountComposer {
    fn id(&self) -> &'static str {
        "session.mount-composer"
    }

    fn name(&self) -> String {
        "Mount Composer".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let _ = entity.open_panel(store, ui, Box::new(ComposerPane::new(window)), fx);
        crate::Windows::put(store, window, entity);
    }
}

/// A model-combo menu row, reified: heading rows are TRACKED caps in
/// the trail color, plain rows are the menu text — one `Text` on the
/// row's baseline, width decided by the incoming constraints.
struct ModelOptionRow<'a> {
    option: &'a ModelOption,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for ModelOptionRow<'_> {}

impl<'a> imba::Layout<'a, std::convert::Infallible> for ModelOptionRow<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, std::convert::Infallible> {
        use imba::LayoutExt as _;
        let ModelOptionRow { option, store, ui } = self;
        let chrome = crate::env::Themes::of(store).ui().combo.clone();
        let heading = option.model.is_none();
        let label = if heading {
            option.label.to_uppercase()
        } else {
            option.label.clone()
        };
        let font = if heading {
            crate::fonts::ui_font(ui, chrome.label_size)
        } else {
            crate::fonts::ui_text_font(ui, chrome.menu_row_size)
        };
        let text_width = if heading {
            crate::combo::tracked_width(&font, &label)
        } else {
            font.measure_str(&label, None).0
        };
        let natural = text_width + chrome.menu_pad * if heading { 2.0 } else { 2.75 };
        let width = if constraints.max.width.is_finite() {
            constraints.max.width
        } else {
            natural.max(constraints.min.width)
        };
        let height = chrome.menu_row_height;
        // The painter's baseline: (height + size * 0.7) * 0.5.
        let baseline = (height + font.size() * 0.7) * 0.5;
        let drop = (baseline + font.metrics().1.ascent).max(0.0);
        let x = chrome.menu_pad * if heading { 1.0 } else { 1.75 };
        let color = if heading {
            chrome.menu_trail.0
        } else {
            chrome.menu_text.0
        };
        let mut label = imba::text(label, font, color);
        if heading {
            label = label.tracking(1.5);
        }
        imba::ZBox::new(arena)
            .child(imba::spacer(width, height))
            .child(label.pad_insets(imba::Insets {
                left: x,
                top: drop,
                right: 0.0,
                bottom: 0.0,
            }))
            .layout(arena, constraints)
    }
}
