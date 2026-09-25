// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

use crate::app::{AppCommand, Application};

pub fn palette_commands(
    store: &Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
) -> Vec<imba::PresentableCommand<AppCommand>> {
    // A STATE WALK over the views — no tree is built for the palette.
    let mut commands: Vec<imba::PresentableCommand<AppCommand>> =
        crate::focus::window_focus_data(store, ui, window)
            .map(|data| data.commands)
            .unwrap_or_default();
    commands.extend(Commands::of(store).iter().map(|command| {
        imba::PresentableCommand::new(
            command.id(),
            command.name(),
            AppCommand::Dynamic(window, command.clone()),
        )
    }));
    commands
}

pub trait LandingCommand: Send + Sync {
    fn perform(
        self: Box<Self>,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    );
}

pub trait DynamicCommand: Send + Sync {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    );
}

#[derive(Clone, Default)]
pub struct Commands(Vec<Arc<dyn DynamicCommand>>);

impl Commands {
    pub fn of(store: &Store) -> Commands {
        store.get::<Commands>().cloned().unwrap_or_default()
    }

    pub(crate) fn register(store: &mut Store, command: Arc<dyn DynamicCommand>) {
        store.update::<Commands>(|commands| commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn DynamicCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn DynamicCommand>> {
        self.0.iter()
    }
}

#[derive(Clone, Default)]
pub struct AppRequests(Vec<Arc<dyn DynamicCommand>>);

#[derive(Clone, Default)]
pub struct BatchRequests(Vec<(crate::WindowId, Arc<dyn DynamicCommand>)>);

impl BatchRequests {
    pub fn push(store: &mut Store, window: crate::WindowId, request: Arc<dyn DynamicCommand>) {
        store.update::<BatchRequests>(|requests| requests.0.push((window, request)));
    }

    pub(crate) fn drain(store: &mut Store) -> Vec<(crate::WindowId, Arc<dyn DynamicCommand>)> {
        store
            .take::<BatchRequests>()
            .map(|requests| requests.0)
            .unwrap_or_default()
    }
}

impl AppRequests {
    pub fn push(store: &mut Store, request: Arc<dyn DynamicCommand>) {
        store.update::<AppRequests>(|requests| requests.0.push(request));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn drain(store: &mut Store) -> Vec<Arc<dyn DynamicCommand>> {
        let Some(requests) = store.get::<AppRequests>() else {
            return Vec::new();
        };
        let drained = requests.0.clone();
        if !drained.is_empty() {
            store.put(AppRequests(Vec::new()));
        }
        drained
    }
}

pub(crate) struct NewScratch;

impl DynamicCommand for NewScratch {
    fn id(&self) -> &'static str {
        "workbench.new-document"
    }
    fn name(&self) -> String {
        "New Document".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let location = crate::next_scratch_location(store);
        fx.push(crate::app::open_effect(
            window,
            location.name().to_owned(),
            true,
            Some(location),
            Box::new(|_, _, _, _| crate::app::markdown_scratch()),
        ));
    }
}

pub(crate) struct SplitPane;

impl DynamicCommand for SplitPane {
    fn id(&self) -> &'static str {
        "workbench.split-pane"
    }
    fn name(&self) -> String {
        "Split Pane".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        entity.split_current(store, ui, fx);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct CloseFocused;

impl DynamicCommand for CloseFocused {
    fn id(&self) -> &'static str {
        "workbench.close"
    }
    fn name(&self) -> String {
        "Close".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        let _ = entity.close_focused_widget(store, ui, window, fx);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct ClosePane;

impl DynamicCommand for ClosePane {
    fn id(&self) -> &'static str {
        "workbench.close-pane"
    }
    fn name(&self) -> String {
        "Close Pane".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        let _ = entity.close_current(store);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct NavigateBack;

impl DynamicCommand for NavigateBack {
    fn id(&self) -> &'static str {
        "navigation.back"
    }
    fn name(&self) -> String {
        "Go Back".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        let _ = entity.navigate_back(store, ui, window, fx);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct NavigateForward;

impl DynamicCommand for NavigateForward {
    fn id(&self) -> &'static str {
        "navigation.forward"
    }
    fn name(&self) -> String {
        "Go Forward".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        let _ = entity.navigate_forward(store, ui, window, fx);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct ToggleTheme;

impl DynamicCommand for ToggleTheme {
    fn id(&self) -> &'static str {
        "theme.toggle"
    }
    fn name(&self) -> String {
        "Toggle Light/Dark Theme".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        let next = match ::editor::env::Themes::of(store).name() {
            "light" => ::editor::theme::Theme::embedded(),
            _ => ::editor::theme::Theme::light(),
        };
        ::editor::env::Themes::set(store, next);
    }
}

/// Selects an explicit theme, unlike [ToggleTheme] — the host calls this to
/// follow the OS appearance (at startup and when the system theme changes).
pub(crate) struct SetTheme {
    pub(crate) dark: bool,
}

impl DynamicCommand for SetTheme {
    fn id(&self) -> &'static str {
        if self.dark {
            "theme.dark"
        } else {
            "theme.light"
        }
    }
    fn name(&self) -> String {
        if self.dark {
            "Use Dark Theme".to_owned()
        } else {
            "Use Light Theme".to_owned()
        }
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        let theme = if self.dark {
            ::editor::theme::Theme::embedded()
        } else {
            ::editor::theme::Theme::light()
        };
        ::editor::env::Themes::set(store, theme);
    }
}

pub(crate) struct CompletionTrigger;

impl DynamicCommand for CompletionTrigger {
    fn id(&self) -> &'static str {
        "completion.trigger"
    }
    fn name(&self) -> String {
        "Trigger Completion".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            let Some((id, editor)) = slot.find_target() else {
                crate::Windows::put(store, window, entity);
                return;
            };
            let location = crate::OpenDocuments::location(store, id)
                .filter(|location| !location.is_synthetic());
            let Some(location) = location else {
                crate::Windows::put(store, window, entity);
                return;
            };
            let Some(mut document) = crate::OpenDocuments::document(store, id) else {
                crate::Windows::put(store, window, entity);
                return;
            };
            let markdown =
                document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
            if !markdown {
                let ui = app.ui_ctx();
                slot.completion.sync_lsp(
                    store,
                    &ui,
                    &mut document,
                    editor,
                    None,
                    true,
                    &location,
                    Some((id, editor)),
                    fx,
                    move |found| {
                        crate::AppCommand::Dynamic(window, Arc::new(CompletionLanded(found)))
                    },
                    move |command| AppCommand::Entity(id, command),
                );
            }
            crate::OpenDocuments::put_document(store, id, document);
        }
        crate::Windows::put(store, window, entity);
    }
}

struct CompletionLanded(crate::completion::CompletionFound);

impl DynamicCommand for CompletionLanded {
    fn id(&self) -> &'static str {
        "completion.landed"
    }
    fn name(&self) -> String {
        "Completion Landed".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let _ = fx;
        let ui = app.ui_ctx();
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        entity.workbench_mut().root.for_each_slot_mut(&mut |slot| {
            let mut discarded = imba::effect::Batch::new();
            slot.land_completion(store, &ui, self.0.clone(), &mut discarded.effects());
        });
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct FindOpen;

impl DynamicCommand for FindOpen {
    fn id(&self) -> &'static str {
        "find.open"
    }
    fn name(&self) -> String {
        "Find in Document".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            if slot.panel.editor().is_some() {
                let seed = slot.find_target().and_then(|(document_id, editor)| {
                    let document = crate::OpenDocuments::document_ref(store, document_id)?;
                    let caret = document.carets(editor).primary();
                    if !caret.has_selection() {
                        return None;
                    }
                    let selection = caret.selection();
                    if selection.end - selection.start > 200 {
                        return None;
                    }
                    let text = document.text().view().substring(selection);
                    (!text.contains('\n')).then_some(text)
                });
                let ui = &app.ui_ctx();
                match (&mut slot.find, seed) {
                    (Some(find), Some(seed)) => find.seed(store, ui, &seed),
                    (Some(find), None) => find.refocus(),
                    (None, seed) => {
                        let mut find = crate::find::FindBar::new(store, ui);
                        if let Some(seed) = &seed {
                            find.seed(store, ui, seed);
                        }
                        slot.find = Some(find);
                    }
                }
                find_sync_slot(slot, app, store, window, fx);
            }
        }
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct FindStep(pub bool);

impl DynamicCommand for FindStep {
    fn id(&self) -> &'static str {
        match self.0 {
            true => "find.next",
            false => "find.previous",
        }
    }
    fn name(&self) -> String {
        match self.0 {
            true => "Find Next",
            false => "Find Previous",
        }
        .to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = app.ui_ctx();
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        {
            let slot = entity.workbench_mut().root.focused_slot_mut();
            let target = slot.find_target();
            if let (Some(find), Some((document, _))) = (&mut slot.find, target) {
                let fonts = ::editor::env::ui_collection(store, &ui);
                let theme = ::editor::env::Themes::of(store);
                let forward = self.0;
                crate::app::entity_scope(document, fx, |fx| {
                    find.sync(store, target, &ui, &fonts, &theme, fx);
                    find.step(store, forward, &ui, &fonts, &theme, fx);
                });
            }
        }
        crate::Windows::put(store, window, entity);
    }
}

fn find_sync_slot(
    slot: &mut crate::workbench_node::PaneSlot,
    app: &Application,
    store: &mut Store,
    window: crate::WindowId,
    fx: &mut crate::AppFx<'_>,
) {
    let target = slot.find_target();
    let Some(find) = &mut slot.find else {
        return;
    };
    let Some((document, _)) = target else {
        return;
    };
    let ui = app.ui_ctx();
    let fonts = ::editor::env::ui_collection(store, &ui);
    let theme = ::editor::env::Themes::of(store);
    crate::app::entity_scope(document, fx, |fx| {
        find.sync(store, target, &ui, &fonts, &theme, fx)
    });

    find.launch(store, target, fx, move |scan| {
        crate::AppCommand::Dynamic(window, Arc::new(FindScanLanded(scan)))
    });
}

struct FindScanLanded(crate::find::Scan);

impl DynamicCommand for FindScanLanded {
    fn id(&self) -> &'static str {
        "find.scan-landed"
    }
    fn name(&self) -> String {
        "Find Scan Landed".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let ui = app.ui_ctx();
        let fonts = ::editor::env::ui_collection(store, &ui);
        let theme = ::editor::env::Themes::of(store);
        entity.workbench_mut().root.for_each_slot_mut(&mut |slot| {
            let target = slot.find_target();
            let Some(find) = &mut slot.find else {
                return;
            };
            let Some((document, _)) = target else {
                return;
            };
            crate::app::entity_scope(document, fx, |fx| {
                find.adopt(store, target, &self.0, &ui, &fonts, &theme, fx)
            });
        });
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) struct ChatComposer;

impl DynamicCommand for ChatComposer {
    fn id(&self) -> &'static str {
        "chat.composer"
    }
    fn name(&self) -> String {
        "Chat Input".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = std::rc::Rc::clone(&app.ui);
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        entity.front_chat(store, &ui, fx);
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) fn register_builtins(store: &mut Store) {
    Commands::register(store, Arc::new(FindOpen));
    Commands::register(store, Arc::new(CompletionTrigger));
    Commands::register(store, Arc::new(FindStep(true)));
    Commands::register(store, Arc::new(FindStep(false)));
    Commands::register(store, Arc::new(NewScratch));
    Commands::register(store, Arc::new(SplitPane));
    Commands::register(store, Arc::new(CloseFocused));
    Commands::register(store, Arc::new(ClosePane));
    Commands::register(store, Arc::new(NavigateBack));
    Commands::register(store, Arc::new(NavigateForward));
    Commands::register(store, Arc::new(crate::toc::ToggleToc));
    Commands::register(store, Arc::new(ToggleTheme));
    Commands::register(store, Arc::new(SetTheme { dark: true }));
    Commands::register(store, Arc::new(SetTheme { dark: false }));
    Commands::register(store, Arc::new(ChatComposer));

    Commands::register(
        store,
        Arc::new(crate::new_session::OpenNewSession { host: None }),
    );

    crate::toolbar::ToolbarButtons::register(store, crate::toc::toolbar_button());
}
