// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use imba::{container::Container, store::Store, thunk_ext::ThunkExt, ui::UiCtx, View};

use ahp_wire::client::SessionChannel;
use hikit::combo::{Combo, ComboCommand, ComboOption};

/// The dir combo's ACTION row: not a folder, a verb.
const ADD_FOLDER: &str = "\u{0}add-folder";
/// The dirless placeholder: display only.
const NO_FOLDER: &str = "\u{0}no-folder";

/// Installed by the SHELL: turns the toolbar's add-folder ask into
/// the native-picker road — the picker needs a WINDOW, a concept
/// this crate does not know. The returned command rides
/// `imba::command::Requests`.
#[derive(Clone)]
pub struct AddFolderRoad(
    pub  Arc<
        dyn Fn(
                ahp_wire::client::HostId,
                ahp_wire::client::SessionUri,
            ) -> Arc<dyn imba::command::DynamicCommand>
            + Send
            + Sync,
    >,
);

#[derive(Clone)]
pub struct SessionToolbar {
    pub model: Combo,
    pub effort: Combo,
    pub edits: Combo,
    pub dir: Combo,

    /// What the dir combo DISPLAYS: the first folder. The combo is a
    /// menu of the session's folders plus the add action, never a
    /// selection — any pick snaps the face back to this anchor.
    dir_anchor: Option<String>,

    pub synced: u64,

    pub cell_spans: Arc<Vec<AtomicU64>>,

    pub strip_origin: Arc<AtomicU64>,
}

#[derive(Clone)]
pub enum ToolbarCommand {
    Model(ComboCommand),
    Effort(ComboCommand),
    Edits(ComboCommand),
    Dir(ComboCommand),
}

impl std::fmt::Display for ToolbarCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolbarCommand::Model(command) => command.fmt(out),
            ToolbarCommand::Effort(command) => command.fmt(out),
            ToolbarCommand::Edits(command) => command.fmt(out),
            ToolbarCommand::Dir(command) => command.fmt(out),
        }
    }
}

pub enum ToolbarAsk {
    None,

    Edits(String),

    /// The dir combo's action row: open the native folder picker.
    AddFolder,
}

impl SessionToolbar {
    pub fn new(store: &imba::store::Store, ui: &imba::ui::UiCtx) -> Self {
        let compact = |mut combo: Combo| {
            combo.compact = true;
            combo
        };
        Self {
            model: compact(Combo::new(store, ui, "MODEL")),
            effort: compact(Combo::new(store, ui, "EFFORT")),
            edits: compact(Combo::new(store, ui, "EDITS")),
            dir: compact(Combo::new(store, ui, "DIR")),
            dir_anchor: None,
            synced: 0,
            cell_spans: Arc::new((0..5).map(|_| AtomicU64::new(0)).collect()),
            strip_origin: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl SessionToolbar {
    pub fn fingerprint(agents: &[ahp_types::state::AgentInfo], session: &SessionChannel) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        agents.len().hash(&mut hasher);
        session.provider.hash(&mut hasher);
        if let Some(config) = &session.config {
            (Arc::as_ptr(config) as usize).hash(&mut hasher);
        }
        session.working_directories.len().hash(&mut hasher);
        hasher.finish()
    }

    pub fn sync(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        agents: &[ahp_types::state::AgentInfo],
        session: &SessionChannel,
    ) {
        self.synced = Self::fingerprint(agents, session);
        let values = session
            .config
            .as_ref()
            .map(|config| config.values.clone())
            .unwrap_or_default();

        let provider = (!session.provider.is_empty()).then_some(session.provider.as_str());
        let models = agent_models(agents, provider);
        let fresh = self.model.is_empty();
        self.model.set_options(
            store,
            ui,
            models
                .iter()
                .map(|model| ComboOption::plain(model.id.clone(), model.name.clone()))
                .collect(),
        );
        if fresh {
            if let Some(current) = values.get("model").and_then(|value| value.as_str()) {
                self.model.pick_id(current);
            }
        }
        let seed = values
            .get("thinkingLevel")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        sync_effort(
            store,
            ui,
            &mut self.effort,
            &self.model,
            &models,
            seed.as_deref(),
        );

        let fresh = self.edits.is_empty();
        if let Some(config) = &session.config {
            if let Some(property) = config.schema.properties.get("permissionMode") {
                self.edits.set_options(
                    store,
                    ui,
                    enum_options(property.r#enum.as_deref(), property.enum_labels.as_deref()),
                );
            }
        }
        if fresh {
            if let Some(current) = values
                .get("permissionMode")
                .and_then(|value| value.as_str())
            {
                self.edits.pick_id(current);
            }
        }

        let mut folders: Vec<ComboOption> = session
            .working_directories
            .iter()
            .map(|dir| {
                let label = dir
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(dir.as_str())
                    .to_owned();
                ComboOption::plain(dir.clone(), label)
            })
            .collect();
        if folders.is_empty() {
            folders.push(ComboOption::plain(NO_FOLDER, "No folder"));
        }
        folders.push(ComboOption::plain(ADD_FOLDER, "Add folder…"));
        self.dir.set_options(store, ui, folders);
        self.dir_anchor = Some(
            session
                .working_directories
                .first()
                .cloned()
                .unwrap_or_else(|| NO_FOLDER.to_owned()),
        );
        if let Some(anchor) = &self.dir_anchor {
            self.dir.pick_id(anchor);
        }
    }

    pub(crate) fn model_selection(&self) -> Option<ahp_types::state::ModelSelection> {
        let model = self.model.value()?;
        Some(ahp_types::state::ModelSelection {
            id: model.id.clone(),
            config: self.effort.value().map(|effort| {
                let mut config = std::collections::HashMap::new();
                config.insert("thinkingLevel".to_owned(), serde_json::json!(effort.id));
                config
            }),
        })
    }

    pub fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ToolbarCommand,
        fx: &mut imba::effect::Effects<'_, ToolbarCommand>,
    ) -> ToolbarAsk {
        match command {
            ToolbarCommand::Model(command) => {
                let moved = command.picks();
                fx.scope(ToolbarCommand::Model, |fx| {
                    self.model.perform(store, ui, command, fx)
                });
                if moved {
                    self.effort.set_options(store, ui, Vec::new());
                }
                ToolbarAsk::None
            }
            ToolbarCommand::Effort(command) => {
                fx.scope(ToolbarCommand::Effort, |fx| {
                    self.effort.perform(store, ui, command, fx)
                });
                ToolbarAsk::None
            }
            ToolbarCommand::Edits(command) => {
                let moved = command.picks();
                fx.scope(ToolbarCommand::Edits, |fx| {
                    self.edits.perform(store, ui, command, fx)
                });
                match moved.then(|| self.edits.value()).flatten() {
                    Some(option) => ToolbarAsk::Edits(option.id),
                    None => ToolbarAsk::None,
                }
            }
            ToolbarCommand::Dir(command) => {
                let moved = command.picks();
                fx.scope(ToolbarCommand::Dir, |fx| {
                    self.dir.perform(store, ui, command, fx)
                });
                let picked = moved.then(|| self.dir.value()).flatten();
                // A menu, not a selection: whatever was chosen, the
                // face snaps back to the anchor.
                if let Some(anchor) = self.dir_anchor.clone() {
                    self.dir.pick_id(&anchor);
                }
                match picked {
                    Some(option) if option.id == ADD_FOLDER => ToolbarAsk::AddFolder,
                    _ => ToolbarAsk::None,
                }
            }
        }
    }

    /// Lay the combo strip left to right within `budget`. The strip
    /// degrades gracefully: a cell that would overflow the budget is
    /// dropped along with everything after it — never clipped, and
    /// never under whatever the caller anchors to the right (SEND).
    pub fn place<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        root: &mut Container<'a, super::chat::ChatPanelCommand>,
        store: &'a Store,
        ui: &'a UiCtx,
        y: f32,
        height: f32,
        budget: f32,
    ) -> f32 {
        let themes = editor::env::Themes::of(store);
        let theme = themes.ui();
        let mut x = 0.0f32;
        let combos: [(&Combo, fn(ComboCommand) -> ToolbarCommand); 4] = [
            (&self.model, ToolbarCommand::Model),
            (&self.effort, ToolbarCommand::Effort),
            (&self.edits, ToolbarCommand::Edits),
            // LAST on purpose: the probe's cell indices predate it.
            (&self.dir, ToolbarCommand::Dir),
        ];
        let span = |index: usize, x: f32, width: f32| {
            if let Some(span) = self.cell_spans.get(index) {
                span.store(
                    ((x.to_bits() as u64) << 32) | width.to_bits() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        };
        let mut dropped = false;
        for (index, (combo, wrap)) in combos.into_iter().enumerate() {
            let width = combo.cell_width(ui, &theme.combo);
            if dropped || x + width > budget {
                dropped = true;
                span(index, x, 0.0);
                continue;
            }
            span(index, x, width);
            root.place(
                x,
                y,
                combo
                    .cell(arena, store, ui, height)
                    .map(move |command| super::chat::ChatPanelCommand::Toolbar(wrap(command))),
            );
            x += width;
        }
        span(4, x, 0.0);
        x
    }
}

#[doc(hidden)]
pub struct ToolbarProbe {
    pub model: hikit::combo::ComboProbe,
    pub effort: hikit::combo::ComboProbe,
    pub edits: hikit::combo::ComboProbe,
    pub dir: hikit::combo::ComboProbe,

    pub cells: Vec<(f32, f32)>,

    pub origin: (f32, f32),
}

impl SessionToolbar {
    pub fn probe(&self) -> ToolbarProbe {
        let combo = |combo: &Combo| hikit::combo::ComboProbe {
            labels: combo
                .options()
                .iter()
                .map(|option| option.label.clone())
                .collect(),
            picked: combo.value().map(|option| option.label.clone()),
            open: combo.open,
        };
        let origin = self.strip_origin.load(std::sync::atomic::Ordering::Relaxed);
        ToolbarProbe {
            model: combo(&self.model),
            effort: combo(&self.effort),
            edits: combo(&self.edits),
            dir: combo(&self.dir),
            origin: (
                f32::from_bits((origin >> 32) as u32),
                f32::from_bits(origin as u32),
            ),
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
}

pub(crate) fn agent_models(
    agents: &[ahp_types::state::AgentInfo],
    provider: Option<&str>,
) -> Vec<ahp_types::state::SessionModelInfo> {
    agents
        .iter()
        .filter(|agent| provider.is_none_or(|provider| agent.provider == provider))
        .flat_map(|agent| agent.models.iter().cloned())
        .collect()
}

pub(crate) fn sync_effort(
    store: &imba::store::Store,
    ui: &UiCtx,
    effort: &mut Combo,
    model: &Combo,
    models: &[ahp_types::state::SessionModelInfo],
    seed: Option<&str>,
) {
    let picked = model
        .value()
        .and_then(|option| models.iter().find(|held| held.id == option.id))
        .cloned();
    sync_effort_for_model(store, ui, effort, picked, seed);
}

pub fn sync_effort_for_model(
    store: &imba::store::Store,
    ui: &UiCtx,
    effort: &mut Combo,
    picked: Option<ahp_types::state::SessionModelInfo>,
    seed: Option<&str>,
) {
    let mut options = Vec::new();
    let mut default = None;
    if let Some(schema) = picked
        .as_ref()
        .and_then(|model| model.config_schema.as_ref())
    {
        if let Some(property) = schema.properties.get("thinkingLevel") {
            options = enum_options(property.r#enum.as_deref(), property.enum_labels.as_deref());
            default = property
                .default
                .as_ref()
                .and_then(|value| value.as_str())
                .map(str::to_owned);
        }
    }
    let fresh = effort.is_empty();
    effort.set_options(store, ui, options);
    if fresh {
        if let Some(id) = seed.map(str::to_owned).or(default) {
            effort.pick_id(&id);
        }
    }
}

pub fn enum_options(
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
