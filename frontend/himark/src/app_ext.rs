// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::app::AppCommand;
use crate::app::Application;
use crate::commands::WindowedCommand;
use hikit::modal::ModalView;
use crate::app::OpenedDocument;
use editor::document::Document;

pub trait AppExt {
    fn perform_command(&mut self, command: AppCommand) -> bool;

    fn perform_registered(&mut self, window: ::workbench::window::WindowId, id: &str) -> bool;

    fn add_document(
        &mut self,
        window: ::workbench::window::WindowId,
        document: Document,
        name: String,
        primary: bool,
    ) -> bool;

    fn new_scratch(&mut self, window: ::workbench::window::WindowId) -> bool;

    fn open_async(
        &mut self,
        window: ::workbench::window::WindowId,
        name: String,
        primary: bool,
        location: Option<editor::location::ResourceLocation>,
        build: impl FnOnce(
                &imba::store::Store,
                &imba::ui::UiCtx,
                &skia_safe::textlayout::FontCollection,
                &::editor::theme::Theme,
            ) -> Document
            + Send
            + Sync
            + 'static,
    ) -> bool;

    fn open_panel(&mut self, window: ::workbench::window::WindowId, panel: Box<dyn hikit::panel::DynPanelView>) -> bool;

    fn open_modal(&mut self, window: ::workbench::window::WindowId, modal: Box<dyn ModalView>) -> bool;

    fn close_modal(&mut self, window: ::workbench::window::WindowId) -> bool;

    fn register_command(&mut self, command: Arc<dyn WindowedCommand>) -> bool;

    fn register_syntax_languages(&mut self, languages: ::editor::reparse::SyntaxLanguages) -> bool;

    fn register_enrichers(&mut self, enrichers: ::editor::enrich::Enrichers) -> bool;

    fn register_diff_policy(
        &mut self,
        policy: std::sync::Arc<dyn ::editor::diff::DiffPolicy>,
    ) -> bool;
}

impl AppExt for Application {
    fn perform_command(&mut self, command: AppCommand) -> bool {
        self.perform_batch(vec![command])
    }

    fn perform_registered(&mut self, window: ::workbench::window::WindowId, id: &str) -> bool {
        let store = self.window_store(window);
        let ui = self.ui_handle();
        let Some(command) = crate::commands::palette_commands(&store, &ui, window)
            .into_iter()
            .find(|presentable| presentable.id == id)
            .map(|presentable| presentable.command)
        else {
            return false;
        };
        self.perform_command(command)
    }

    fn add_document(
        &mut self,
        window: ::workbench::window::WindowId,
        document: Document,
        name: String,
        primary: bool,
    ) -> bool {
        // Bind the window's session HERE, at the gesture — the landing
        // consumes the id and never re-asks the window.
        let documents = {
            let store = self.window_store(window);
            let Some(state) = ::workbench::window::Windows::session_state(&store, window) else {
                return false;
            };
            state.documents()
        };
        self.perform_command(AppCommand::Opened(
            window,
            OpenedDocument {
                documents,
                name,
                document,
                location: None,
                primary,
                target: None,
                focus: false,
            },
        ))
    }

    fn new_scratch(&mut self, window: ::workbench::window::WindowId) -> bool {
        self.add_document(
            window,
            crate::app::markdown_scratch(),
            "scratch".to_owned(),
            true,
        )
    }

    fn open_async(
        &mut self,
        window: ::workbench::window::WindowId,
        name: String,
        primary: bool,
        location: Option<editor::location::ResourceLocation>,
        build: impl FnOnce(
                &imba::store::Store,
                &imba::ui::UiCtx,
                &skia_safe::textlayout::FontCollection,
                &::editor::theme::Theme,
            ) -> Document
            + Send
            + Sync
            + 'static,
    ) -> bool {
        self.perform_command(AppCommand::OpenAsync {
            window,
            name,
            primary,
            location,
            build: Box::new(build),
        })
    }

    fn open_panel(&mut self, window: ::workbench::window::WindowId, panel: Box<dyn hikit::panel::DynPanelView>) -> bool {
        self.perform_command(AppCommand::OpenPanel(window, panel))
    }

    fn open_modal(&mut self, window: ::workbench::window::WindowId, modal: Box<dyn ModalView>) -> bool {
        self.perform_command(AppCommand::OpenModal(window, modal))
    }

    fn close_modal(&mut self, window: ::workbench::window::WindowId) -> bool {
        self.perform_command(AppCommand::CloseModal(window))
    }

    fn register_command(&mut self, command: Arc<dyn WindowedCommand>) -> bool {
        self.perform_command(AppCommand::Register(command))
    }

    fn register_syntax_languages(&mut self, languages: ::editor::reparse::SyntaxLanguages) -> bool {
        self.perform_command(AppCommand::RegisterLanguages(languages))
    }

    fn register_enrichers(&mut self, enrichers: ::editor::enrich::Enrichers) -> bool {
        self.perform_command(AppCommand::RegisterEnrichers(enrichers))
    }

    fn register_diff_policy(
        &mut self,
        policy: std::sync::Arc<dyn ::editor::diff::DiffPolicy>,
    ) -> bool {
        self.perform_command(AppCommand::RegisterDiffPolicy(policy))
    }
}
