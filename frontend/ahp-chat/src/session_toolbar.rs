// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use gfx::Size;
use imba::{
    constraints::Constraints, container::Container, store::Store, thunk_ext::ThunkExt, ui::UiCtx,
    View,
};

use ahp_wire::client::SessionChannel;
use hikit::combo::{Combo, ComboCommand, ComboOption};

/// The dir combo's ACTION row: not a folder, a verb.
const ADD_FOLDER: &str = "\u{0}add-folder";
/// The dirless placeholder: display only.
const NO_FOLDER: &str = "\u{0}no-folder";

/// What the composer may offer for the session's FOLDER: whether the
/// session is still newborn (no prompt yet — the one time the combo
/// shows; afterwards the folder changes through a command), and the
/// folders this host's sessions recently worked in, most recent
/// first. Answered by the catalog through `Catalog::folder_picks`.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct FolderPicks {
    pub fresh: bool,
    pub recent: Vec<String>,
}

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

    /// The session's folder — its first working directory — for the
    /// pane's subtitle, whether or not the combo is showing.
    folder: Option<String>,

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

    /// A folder picked from the host's recent ones: it becomes THE
    /// session folder — the previous one goes.
    SetFolder(String),
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
            folder: None,
            synced: 0,
            cell_spans: Arc::new((0..5).map(|_| AtomicU64::new(0)).collect()),
            strip_origin: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl SessionToolbar {
    pub fn fingerprint(
        agents: &[ahp_types::state::AgentInfo],
        session: &SessionChannel,
        picks: &FolderPicks,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        agents.len().hash(&mut hasher);
        session.provider.hash(&mut hasher);
        if let Some(config) = &session.config {
            (Arc::as_ptr(config) as usize).hash(&mut hasher);
        }
        for folder in session.working_directories.iter() {
            folder.hash(&mut hasher);
        }
        picks.hash(&mut hasher);
        hasher.finish()
    }

    pub fn sync(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        agents: &[ahp_types::state::AgentInfo],
        session: &SessionChannel,
        picks: &FolderPicks,
    ) {
        self.synced = Self::fingerprint(agents, session, picks);
        let values = session
            .config
            .as_ref()
            .map(|config| config.values.clone())
            .unwrap_or_default();

        let provider = (!session.provider.is_empty()).then_some(session.provider.as_str());
        let models = agent_models(agents);
        let fresh = self.model.is_empty();
        self.model.set_options(store, ui, model_tree(agents));
        if fresh {
            // The config's word first; a bare session falls to its own
            // harness's first model, never another branch of the tree.
            let seeded = values
                .get("model")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    agents
                        .iter()
                        .find(|agent| provider == Some(agent.provider.as_str()))
                        .and_then(|agent| agent.models.first())
                        .map(|model| model.id.clone())
                });
            if let Some(id) = seeded {
                self.model.pick_id(&id);
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

        // The session's folder is the first working directory; the
        // combo offers it only while the session is newborn — once
        // the first prompt is in, the folder changes through the
        // add-folder command, and the strip keeps its three words.
        self.folder = session.working_directories.first().cloned();
        if !picks.fresh {
            self.dir.set_options(store, ui, Vec::new());
            return;
        }
        let mut folders: Vec<ComboOption> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for dir in self.folder.iter().chain(picks.recent.iter()) {
            if seen.insert(dir.clone()) {
                folders.push(ComboOption::plain(dir.clone(), folder_label(dir)));
            }
        }
        if folders.is_empty() {
            folders.push(ComboOption::plain(NO_FOLDER, "No folder"));
        }
        folders.push(ComboOption::plain(ADD_FOLDER, "Add folder…"));
        self.dir.set_options(store, ui, folders);
        self.dir
            .pick_id(self.folder.as_deref().unwrap_or(NO_FOLDER));
    }

    /// WHERE the session works: its first folder's name — the chat
    /// header's subtitle. None while the session has no folder.
    pub(crate) fn folder_name(&self) -> Option<String> {
        self.folder.as_deref().map(folder_label)
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
                let current = self.folder.as_deref().unwrap_or(NO_FOLDER);
                match picked {
                    Some(option) if option.id == ADD_FOLDER => {
                        // A verb, not a folder: the face snaps back.
                        self.dir.pick_id(current);
                        ToolbarAsk::AddFolder
                    }
                    Some(option) if option.id != NO_FOLDER && option.id != current => {
                        // The pick stands optimistically; the wire's
                        // answer re-syncs the strip.
                        ToolbarAsk::SetFolder(option.id)
                    }
                    _ => ToolbarAsk::None,
                }
            }
        }
    }

    /// Lay the combo strip left to right within `budget`: the picks
    /// as bare WORDS ("Fable · High · Auto"), a dim dot between two.
    /// A combo with nothing to offer stands out of the strip. The
    /// strip degrades gracefully: a word that would overflow the
    /// budget is dropped along with everything after it — never
    /// clipped, and never under whatever the caller anchors to the
    /// right (SEND).
    pub fn place<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        root: &mut Container<'a, super::chat::ChatPanelCommand>,
        store: &'a Store,
        ui: &'a UiCtx,
        left: f32,
        y: f32,
        height: f32,
        budget: f32,
    ) -> f32 {
        use imba::layout::{Layout as _, LayoutExt as _};
        let themes = editor::env::Themes::of(store);
        let chrome = themes.part(::hikit::chrome::ComboChrome::KEY);
        let gap = themes.part(::hikit::chrome::ChatChrome::KEY).options_gap;
        let dot = hikit::ui::brief(store, ui)
            .sized(chrome.value_size)
            .colored(themes.part(editor::theme::InkChrome::KEY).ink_4.0);
        let dot_ascent = -dot.font.metrics().1.ascent;
        let dot_top = (height * 0.5 + chrome.value_size * 0.35 - dot_ascent).max(0.0);
        let dot_w = imba::layout::text_advance(ui, &dot.font, "\u{00B7}");
        // The combo cell keeps a trailing gap (and a chevron slot the
        // theme zeroes) after its word; the dot measures from the word.
        let tail = chrome.gap + chrome.chevron + chrome.pad;
        let mut x = left;
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
        let mut first = true;
        let mut right = left;
        for (index, (combo, wrap)) in combos.into_iter().enumerate() {
            if combo.is_empty() {
                span(index, x, 0.0);
                continue;
            }
            let width = combo.cell_width(ui, &chrome);
            let lead = match first {
                true => 0.0,
                false => gap + dot_w + gap,
            };
            if dropped || x + lead + width - tail > budget {
                dropped = true;
                span(index, x, 0.0);
                continue;
            }
            if !first {
                root.place_boxed(
                    x + gap,
                    y,
                    hikit::ui::text(&dot, "\u{00B7}")
                        .pad_insets(imba::layout::Insets {
                            top: dot_top,
                            ..Default::default()
                        })
                        .layout(arena, Constraints::tight(Size::new(dot_w, height)).loosen()),
                );
            }
            x += lead;
            span(index, x, width);
            root.place(
                x,
                y,
                combo
                    .cell(arena, store, ui, height)
                    .map(move |command| super::chat::ChatPanelCommand::Toolbar(wrap(command))),
            );
            right = x + width - tail;
            x = right;
            first = false;
        }
        span(4, x, 0.0);
        right
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
) -> Vec<ahp_types::state::SessionModelInfo> {
    agents
        .iter()
        .flat_map(|agent| agent.models.iter().cloned())
        .collect()
}

/// The MODEL menu is a TREE: a dim harness heading (Claude, Codex),
/// its models indented beneath — the one combo picks both, and the
/// host re-homes the session on the model's harness. With a lone
/// harness the heading would be noise, so the list stays flat.
pub(crate) fn model_tree(agents: &[ahp_types::state::AgentInfo]) -> Vec<ComboOption> {
    let staffed = || agents.iter().filter(|agent| !agent.models.is_empty());
    let lone = staffed().count() < 2;
    let mut options = Vec::new();
    for agent in staffed() {
        if !lone {
            options.push(ComboOption::heading(agent.display_name.clone()));
        }
        for model in &agent.models {
            let option = ComboOption::plain(model.id.clone(), model.name.clone());
            options.push(match lone {
                true => option,
                false => option.indented(),
            });
        }
    }
    options
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

/// A folder's short name — the last path segment, which is what the
/// strip has room for. Trailing slashes are not a segment.
fn folder_label(dir: &str) -> String {
    let trimmed = dir.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(dirs: &[&str]) -> SessionChannel {
        SessionChannel {
            working_directories: dirs.iter().map(|dir| (*dir).to_owned()).collect(),
            ..SessionChannel::default()
        }
    }

    /// The folder combo is the NEWBORN session's: it offers the
    /// host's recent folders (the session's own first) and the
    /// picker; a session past its first prompt shows no combo, yet
    /// still names its folder for the subtitle.
    #[test]
    fn the_folder_combo_shows_only_for_a_fresh_session() {
        let store = imba::store::Store::new();
        let ui = imba::ui::UiCtx::dont_use_too_slow();
        let mut toolbar = SessionToolbar::new(&store, &ui);
        let picks = FolderPicks {
            fresh: true,
            recent: vec![
                "file:///dev/hikit".to_owned(),
                "file:///dev/himark".to_owned(),
            ],
        };
        toolbar.sync(&store, &ui, &[], &channel(&["file:///dev/himark"]), &picks);
        let labels: Vec<String> = toolbar
            .dir
            .options()
            .into_iter()
            .map(|option| option.label)
            .collect();
        assert_eq!(labels, vec!["himark", "hikit", "Add folder…"]);
        assert_eq!(
            toolbar.dir.value().expect("picked").id,
            "file:///dev/himark"
        );
        assert_eq!(toolbar.folder_name().as_deref(), Some("himark"));

        let settled = FolderPicks {
            fresh: false,
            ..picks
        };
        toolbar.sync(
            &store,
            &ui,
            &[],
            &channel(&["file:///dev/himark"]),
            &settled,
        );
        assert!(
            toolbar.dir.is_empty(),
            "no combo once the session has spoken"
        );
        assert_eq!(toolbar.folder_name().as_deref(), Some("himark"));
    }

    /// Picking a recent folder asks to SET it — the session keeps one
    /// folder, so the previous one goes with it; the picker row asks
    /// for the picker and leaves the face on the current folder.
    #[test]
    fn a_folder_pick_asks_to_replace_the_session_folder() {
        let mut store = imba::store::Store::new();
        let ui = imba::ui::UiCtx::dont_use_too_slow();
        let mut toolbar = SessionToolbar::new(&store, &ui);
        let picks = FolderPicks {
            fresh: true,
            recent: vec!["file:///dev/hikit".to_owned()],
        };
        toolbar.sync(&store, &ui, &[], &channel(&["file:///dev/himark"]), &picks);
        let index = |toolbar: &SessionToolbar, id: &str| {
            toolbar
                .dir
                .options()
                .iter()
                .position(|option| option.id == id)
                .expect("the option")
        };
        let mut batch = imba::effect::Batch::new();
        let pick = toolbar.perform(
            &mut store,
            &ui,
            ToolbarCommand::Dir(ComboCommand::Pick(index(&toolbar, "file:///dev/hikit"))),
            &mut batch.effects(),
        );
        assert!(matches!(pick, ToolbarAsk::SetFolder(ref uri) if uri == "file:///dev/hikit"));

        let add = toolbar.perform(
            &mut store,
            &ui,
            ToolbarCommand::Dir(ComboCommand::Pick(index(&toolbar, ADD_FOLDER))),
            &mut batch.effects(),
        );
        assert!(matches!(add, ToolbarAsk::AddFolder));
        assert_eq!(
            toolbar.dir.value().expect("picked").id,
            "file:///dev/himark",
            "the verb row never stands as the face"
        );
    }

    fn agent(provider: &str, name: &str, models: &[&str]) -> ahp_types::state::AgentInfo {
        ahp_types::state::AgentInfo {
            provider: provider.to_owned(),
            display_name: name.to_owned(),
            description: String::new(),
            models: models
                .iter()
                .map(|id| ahp_types::state::SessionModelInfo {
                    id: (*id).to_owned(),
                    provider: provider.to_owned(),
                    name: (*id).to_owned(),
                    max_context_window: None,
                    max_output_tokens: None,
                    max_prompt_tokens: None,
                    supports_vision: None,
                    policy_state: None,
                    config_schema: None,
                    meta: None,
                })
                .collect(),
            protected_resources: None,
            customizations: None,
            capabilities: None,
        }
    }

    #[test]
    fn two_harnesses_make_a_tree() {
        let agents = [
            agent("claude", "Claude", &["claude-fable-5", "claude-opus-5"]),
            agent("codex", "Codex", &["gpt-6-astra"]),
        ];
        let options = model_tree(&agents);
        let shape: Vec<(bool, bool, &str)> = options
            .iter()
            .map(|option| (option.heading, option.indent, option.label.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (true, false, "Claude"),
                (false, true, "claude-fable-5"),
                (false, true, "claude-opus-5"),
                (true, false, "Codex"),
                (false, true, "gpt-6-astra"),
            ]
        );
    }

    #[test]
    fn a_lone_harness_stays_flat() {
        // The second card offers no models — a heading over the only
        // real branch would be noise.
        let agents = [
            agent("claude", "Claude", &["claude-fable-5"]),
            agent("codex", "Codex", &[]),
        ];
        let shape: Vec<(bool, bool)> = model_tree(&agents)
            .iter()
            .map(|option| (option.heading, option.indent))
            .collect();
        assert_eq!(shape, vec![(false, false)]);
    }

    #[test]
    fn headings_never_take_the_pick() {
        let store = imba::store::Store::new();
        let ui = imba::ui::UiCtx::dont_use_too_slow();
        let mut combo = Combo::new(&store, &ui, "MODEL");
        let agents = [
            agent("claude", "Claude", &["claude-fable-5"]),
            agent("codex", "Codex", &["gpt-6-astra"]),
        ];
        combo.set_options(&store, &ui, model_tree(&agents));
        // The default pick skips the heading to the first model, and
        // picking into the other branch lands on its model row.
        assert_eq!(combo.value().expect("picked").id, "claude-fable-5");
        combo.pick_id("gpt-6-astra");
        assert_eq!(combo.value().expect("picked").id, "gpt-6-astra");
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
