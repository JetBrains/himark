// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's convenience extension over [`Himark`]: one-command
//! batches and the environment registrations. The window-shaped
//! openers live in the workbench shell's own extension.

use std::sync::Arc;

use crate::app::{AppCommand, Himark};
use crate::commands::HimarkCommand;

pub trait AppExt {
    fn perform_command(&mut self, command: AppCommand) -> bool;

    fn register_command(&mut self, command: Arc<dyn HimarkCommand>) -> bool;

    fn register_syntax_languages(&mut self, languages: ::editor::reparse::SyntaxLanguages) -> bool;

    fn register_enrichers(&mut self, enrichers: ::editor::enrich::Enrichers) -> bool;

    fn register_diff_policy(
        &mut self,
        policy: std::sync::Arc<dyn ::editor::diff::DiffPolicy>,
    ) -> bool;
}

impl AppExt for Himark {
    fn perform_command(&mut self, command: AppCommand) -> bool {
        self.perform_batch(vec![command])
    }

    fn register_command(&mut self, command: Arc<dyn HimarkCommand>) -> bool {
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
