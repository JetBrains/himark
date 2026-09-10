use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use ::editor::theme::ComboChrome;
use ahp_types::actions::{SessionWorkingDirectorySetAction, StateAction};
use ahp_types::common::Uri;
use imba::{
    container::Container,
    effect::AnyEffect,
    event::{Event, EventResult, MouseButton},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};

use super::session::SessionChannel;
use super::{HostId, Hosts};
use crate::combo::{Combo, ComboCommand, ComboOption};

#[derive(Clone)]
pub struct SessionToolbar {
    pub model: Combo,
    pub effort: Combo,
    pub edits: Combo,

    pub synced: u64,

    pub cell_spans: Arc<Vec<AtomicU64>>,

    pub strip_origin: Arc<AtomicU64>,
}

pub enum ToolbarCommand {
    Model(ComboCommand),
    Effort(ComboCommand),
    Edits(ComboCommand),
    AddFolder,
}

pub enum ToolbarAsk {
    None,

    Edits(String),

    AddFolder,
}

impl Default for SessionToolbar {
    fn default() -> Self {
        Self {
            model: Combo::new("MODEL"),
            effort: Combo::new("EFFORT"),
            edits: Combo::new("EDITS"),
            synced: 0,
            cell_spans: Arc::new((0..4).map(|_| AtomicU64::new(0)).collect()),
            strip_origin: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl SessionToolbar {
    pub fn fingerprint(store: &Store, server: HostId, session: &SessionChannel) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(host) = Hosts::host_ref(store, server) {
            host.agents.len().hash(&mut hasher);
        }
        session.provider.hash(&mut hasher);
        if let Some(config) = &session.config {
            (Arc::as_ptr(config) as usize).hash(&mut hasher);
        }
        session.working_directories.len().hash(&mut hasher);
        hasher.finish()
    }

    pub fn sync(&mut self, store: &Store, ui: &UiCtx, server: HostId, session: &SessionChannel) {
        self.synced = Self::fingerprint(store, server, session);
        let values = session
            .config
            .as_ref()
            .map(|config| config.values.clone())
            .unwrap_or_default();

        let provider = (!session.provider.is_empty()).then_some(session.provider.as_str());
        let models = agent_models(store, server, provider);
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
                    crate::new_session::enum_options(
                        property.r#enum.as_deref(),
                        property.enum_labels.as_deref(),
                    ),
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
    }

    pub fn model_selection(&self) -> Option<ahp_types::state::ModelSelection> {
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
            ToolbarCommand::AddFolder => ToolbarAsk::AddFolder,
        }
    }

    pub fn place<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        root: &mut Container<'a, super::chat::ChatPanelCommand>,
        store: &'a Store,
        ui: &'a UiCtx,
        y: f32,
        height: f32,
    ) -> f32 {
        let themes = crate::env::Themes::of(store);
        let theme = themes.ui();
        let mut x = 0.0f32;
        let combos: [(&Combo, fn(ComboCommand) -> ToolbarCommand); 3] = [
            (&self.model, ToolbarCommand::Model),
            (&self.effort, ToolbarCommand::Effort),
            (&self.edits, ToolbarCommand::Edits),
        ];
        let span = |index: usize, x: f32, width: f32| {
            if let Some(span) = self.cell_spans.get(index) {
                span.store(
                    ((x.to_bits() as u64) << 32) | width.to_bits() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        };
        for (index, (combo, wrap)) in combos.into_iter().enumerate() {
            let width = combo.cell_width(ui, &theme.combo);
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
        let width = action_cell_width(ui, &theme.combo, ADD_FOLDER_LABEL);
        span(3, x, width);
        root.place(x, y, action_cell(store, ui, width, height));
        x + width
    }
}

#[doc(hidden)]
pub struct ToolbarProbe {
    pub model: crate::new_session::ComboProbe,
    pub effort: crate::new_session::ComboProbe,
    pub edits: crate::new_session::ComboProbe,

    pub cells: Vec<(f32, f32)>,

    pub origin: (f32, f32),
}

impl SessionToolbar {
    pub fn probe(&self) -> ToolbarProbe {
        let combo = |combo: &Combo| crate::new_session::ComboProbe {
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

const ADD_FOLDER_LABEL: &str = "＋ FOLDER";

fn action_cell_width(ui: &UiCtx, chrome: &ComboChrome, label: &str) -> f32 {
    let font = crate::fonts::ui_font(ui, chrome.label_size);
    chrome.pad + crate::combo::tracked_width(&font, label) + chrome.pad
}

fn action_cell<'a>(
    store: &'a Store,
    ui: &'a UiCtx,
    width: f32,
    height: f32,
) -> impl Thunk<'a, super::chat::ChatPanelCommand> + 'a {
    let themes = crate::env::Themes::of(store);
    let theme = themes.ui();
    let chrome = theme.combo.clone();
    let rule = theme.toolbar.rule.0;
    let font = crate::fonts::ui_font(ui, chrome.label_size);
    leaf::<super::chat::ChatPanelCommand>(width, height)
        .paint_below(move |_arena, canvas, rect| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(false);
            paint.set_color(rule);
            canvas.draw_rect(
                skia_safe::Rect::from_xywh(rect.right - 1.0, rect.top, 1.0, rect.height()),
                &paint,
            );
            paint.set_anti_alias(true);
            paint.set_color(chrome.label_color.0);
            let mid = rect.top + rect.height() * 0.5;
            crate::combo::draw_tracked(
                canvas,
                &font,
                &paint,
                ADD_FOLDER_LABEL,
                rect.left + chrome.pad,
                mid + chrome.label_size * 0.35,
            );
        })
        .event(|_arena, event, _size| match event {
            Event::MouseDown {
                button: MouseButton::Left,
                ..
            } => EventResult::Command(super::chat::ChatPanelCommand::Toolbar(
                ToolbarCommand::AddFolder,
            )),
            _ => EventResult::Ignored,
        })
}

pub(crate) fn agent_models(
    store: &Store,
    server: HostId,
    provider: Option<&str>,
) -> Vec<ahp_types::state::SessionModelInfo> {
    Hosts::host_ref(store, server)
        .map(|host| {
            host.agents
                .iter()
                .filter(|agent| provider.is_none_or(|provider| agent.provider == provider))
                .flat_map(|agent| agent.models.iter().cloned())
                .collect()
        })
        .unwrap_or_default()
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

pub(crate) fn sync_effort_for_model(
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
            options = crate::new_session::enum_options(
                property.r#enum.as_deref(),
                property.enum_labels.as_deref(),
            );
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

pub struct AddSessionFolders {
    pub server: HostId,
    pub session: Uri,
}

impl crate::DynamicCommand for AddSessionFolders {
    fn id(&self) -> &'static str {
        "session.add-folder"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let server = self.server;
        let session = self.session.clone();
        fx.push(
            AnyEffect::new(crate::new_session::PickFoldersEffect { window }).map(
                move |locations| {
                    crate::app::AppCommand::Dynamic(
                        window,
                        Arc::new(SessionFoldersPicked {
                            server,
                            session: session.clone(),
                            locations,
                        }),
                    )
                },
            ),
        );
    }
}

struct SessionFoldersPicked {
    server: HostId,
    session: Uri,
    locations: Vec<crate::ResourceLocation>,
}

impl crate::DynamicCommand for SessionFoldersPicked {
    fn id(&self) -> &'static str {
        "session.folder-granted"
    }

    fn name(&self) -> String {
        "Session Folder Granted".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(seat) = crate::higent::Servers::seat(store, self.server) else {
            return;
        };
        let Some(uris) = Hosts::uris(store, self.server) else {
            return;
        };
        for location in &self.locations {
            let directory = uris.uri_of(location).as_str().to_owned();
            fx.push(
                AnyEffect::new(crate::higent::DispatchChatActionEffect {
                    seat: seat.clone(),
                    channel: self.session.clone(),
                    action: StateAction::SessionWorkingDirectorySet(
                        SessionWorkingDirectorySetAction { directory },
                    ),
                })
                .map(move |_result| crate::app::AppCommand::Dynamic(window, Arc::new(GrantAck))),
            );
        }
    }
}

struct GrantAck;

impl crate::DynamicCommand for GrantAck {
    fn id(&self) -> &'static str {
        "session.folder-grant-ack"
    }

    fn name(&self) -> String {
        "Session Folder Grant Acknowledged".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
    }
}
