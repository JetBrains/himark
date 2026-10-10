// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's command vocabulary and interpreter. `AppCommand`
//! is window-free: the addressed road (`At`), the windowless verb
//! lane, the file-event border, the editor-environment
//! registrations — and ONE erased arm for the shell's own
//! vocabulary, interpreted by whatever shell the boot installed
//! (`crate::shell` for the workbench). The engine routes; it never
//! learns what a window is (docs/himark-core.md, commands
//! section).

use imba::command::{Addressed, Verb};

use std::sync::Arc;

use imba::{effect::Effects, store::Store, ui::UiCtx};

use ::workbench::window::WindowId;

#[derive(Clone)]
pub enum AppCommand {
    /// The app-level erased vocabulary (imba::command) — windowless:
    /// addressed entity commands, dynamic commands and one-shot
    /// landings that know collections by id and never a window.
    Verb(imba::command::Verb),

    /// THE one command road (docs/entities.md law 5): any collection,
    /// addressed by its id, stamped at launch, carried through the
    /// whole landing chain — nothing re-derives a scope that was never
    /// lost. Built by `AppCommand::at`; the erasure is what keeps the
    /// enum from growing an arm per collection.
    At(Addressed),

    FileChanged(documents::watch::Subscription),

    RegisterLanguages(::editor::reparse::SyntaxLanguages),

    RegisterDiffPolicy(std::sync::Arc<dyn ::editor::diff::DiffPolicy>),

    RegisterEnrichers(::editor::enrich::Enrichers),
}

impl AppCommand {
    /// The one addressed-command constructor: every launch stamp and
    /// every landing re-wrap goes through here.
    pub fn at<T: imba::store::Entity>(id: imba::store::Id<T>, command: T::Command) -> AppCommand {
        AppCommand::At(Addressed::at(id, command))
    }
}

impl std::fmt::Display for AppCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&command_label(self))
    }
}

pub(crate) type AppEffects = imba::effect::Batch<AppCommand>;
pub type AppFx<'a> = Effects<'a, AppCommand>;

/// Wrap a shell command for the verb lane — the opaque escape a kit
/// request rides when it must name the application (a window-coupled
/// gesture). Interpreted by the drains below, never by `Verb::run`.
pub fn shell_verb(command: AppCommand) -> Verb {
    Verb::shell(Box::new(command))
}

/// A drained verb back into the app stream: shell payloads unwrap
/// (an `AppCommand`, or a deferred dynamic ask that takes the
/// draining window); everything else rides the Verb arm.
#[doc(hidden)]
pub fn verb_command(window: WindowId, verb: Verb) -> Option<AppCommand> {
    match verb {
        Verb::Shell(payload) => {
            let Some(payload) = payload.take() else {
                eprintln!("[app] a spent shell verb was drained again — dropped");
                return None;
            };
            match payload.downcast::<AppCommand>() {
                Ok(command) => Some(*command),
                Err(payload) => {
                    match payload
                        .downcast::<std::sync::Arc<dyn crate::commands::WindowedCommand>>()
                    {
                        Ok(command) => Some(crate::shell::windowed(window, *command)),
                        Err(_) => {
                            eprintln!("[app] an unknown shell verb payload was dropped");
                            None
                        }
                    }
                }
            }
        }
        verb => Some(AppCommand::Verb(verb)),
    }
}

pub(crate) fn command_label(command: &AppCommand) -> std::borrow::Cow<'static, str> {
    // The addressed and shell commands print themselves (`Display`
    // through the erasure) — the type is gone by the time the trace
    // reads one.
    match command {
        AppCommand::At(addressed) => addressed.to_string().into(),
        AppCommand::Verb(..) => std::borrow::Cow::Borrowed("verb"),
        AppCommand::FileChanged(..) => std::borrow::Cow::Borrowed("file changed"),
        AppCommand::RegisterLanguages(_) => std::borrow::Cow::Borrowed("register languages"),
        AppCommand::RegisterDiffPolicy(_) => std::borrow::Cow::Borrowed("register diff policy"),
        AppCommand::RegisterEnrichers(_) => std::borrow::Cow::Borrowed("register enrichers"),
    }
}

/// The perform CLOSURE: what the command interpreter touches besides
/// the store — borrowed from the application for one batch, so a
/// perform never holds the application itself.
pub(crate) struct Performer<'app> {
    pub(crate) pending_file_events: &'app mut Vec<documents::watch::Subscription>,
    pub(crate) workshop: &'app Arc<::editor::env::Workshop>,
}

impl Performer<'_> {
    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: AppCommand,
        fx: &mut AppFx<'_>,
    ) {
        match command {
            AppCommand::At(addressed) => {
                // The one command road (docs/entities.md law 5): lease
                // the row, perform under its own address, put it back.
                fx.scope(AppCommand::Verb, |fx| addressed.run(store, ui, fx));
            }
            AppCommand::Verb(Verb::Shell(payload)) => {
                // A shell escape that landed WINDOWLESS (an effect
                // mapped through `shell_verb`): the payload is an
                // AppCommand — it rides the FOLLOW-UP lane so the loop
                // re-dispatches it with its own scope (a window-scoped
                // command performed inside this verb's scopeless gather
                // would scatter its window writes away). A deferred
                // window-taking ask has no window here and drops loudly.
                let Some(payload) = payload.take() else {
                    eprintln!("[app] a spent shell verb landed again — dropped");
                    return;
                };
                match payload.downcast::<AppCommand>() {
                    Ok(command) => fx.follow_up(*command),
                    Err(_) => {
                        eprintln!("[app] a window-taking shell verb landed windowless — dropped")
                    }
                }
            }
            AppCommand::Verb(verb) => {
                fx.scope(AppCommand::Verb, |fx| verb.run(store, ui, fx));
            }
            AppCommand::FileChanged(subscription) => {
                // The border road: the event names only a subscription;
                // its documents collection is found once, by content.
                if let Some(documents) =
                    ahp_session::session::state::Hosts::documents_of_watch(store, subscription)
                {
                    fx.scope(AppCommand::Verb, |fx| {
                        documents::lanes::refetch_watched(store, documents, subscription, fx)
                    });
                }

                self.pending_file_events.push(subscription);
            }
            AppCommand::RegisterLanguages(languages) => {
                store.put(::editor::env::Parsers(std::sync::Arc::new(languages)));
            }
            AppCommand::RegisterDiffPolicy(policy) => {
                store.put(::editor::env::Differ(policy));
            }
            AppCommand::RegisterEnrichers(enrichers) => {
                let enrichers = std::sync::Arc::new(enrichers);
                self.workshop.install_enrichers(enrichers.clone());
                store.put(::editor::env::Enrichers(enrichers));
            }
        }
    }
}
