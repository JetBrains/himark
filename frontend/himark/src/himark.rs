// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Himark ENGINE (docs/himark-core.md): the one global store,
//! the mounted-view bridges, the batch loop's state, and the
//! effects road. Nothing here knows a window or a workbench — the
//! engine stores bridges, and a workbench window is just one kind
//! of mounted view, adapted by the shell (`crate::app`). Every
//! window-shaped method is an EXTENSION over this struct (an
//! `impl Himark` in the shell's own file), never a member here.

use std::sync::Arc;

use imba::{store::Store, ui::UiCtx};

use crate::bridge::{ViewBridge, ViewBridgeId};
use crate::stats::Stats;

pub struct Himark {
    /// THE store — one, global, always live. Hosts, Windows and
    /// Servers are residents like everything else; a batch borrows
    /// it, nothing is projected in or filed back.
    pub(crate) store: Store,

    pub(crate) ui: std::rc::Rc<UiCtx>,

    pub(crate) stats: Stats,

    /// The mounted views, one bridge each — a workbench window is
    /// just one kind of mounted view (`crate::app::WorkbenchView`).
    pub(crate) bridges: std::collections::HashMap<ViewBridgeId, ViewBridge>,

    /// The store's frame revision, bumped by every `&mut store`
    /// door — a bridge's frame is a memo over (revision × size),
    /// rebuilt lazily when next asked.
    pub(crate) revision: u64,

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    effects: crate::effects::EffectLauncher,

    handlers: Arc<crate::effects::Handlers>,

    pub(crate) workshop: Arc<::editor::env::Workshop>,

    pub(crate) pending_file_events: Vec<documents::watch::Subscription>,

    /// A perform raised the settle bit (`Effects::settle`): run the
    /// synchronous `Event::Settle` pulse before the next paint so
    /// viewport corrections land in the SAME frame
    /// (docs/editor/viewport-preservation.md §3.2).
    pub(crate) settle_requested: bool,
    pub(crate) settling: bool,
}

pub struct AppFonts {
    source: ::editor::FontSource,
}

impl AppFonts {
    pub fn new(source: ::editor::FontSource) -> Self {
        Self { source }
    }

    pub fn platform() -> Self {
        Self::new(hikit::fonts::source())
    }

    pub fn embedded() -> Self {
        Self::new(::editor::embedded_fonts::source())
    }

    pub fn source(&self) -> ::editor::FontSource {
        self.source.clone()
    }
}

impl Himark {
    /// The BARE engine: store, ui, effects, the editor environment
    /// — and not one workbench wire. Shells add their own dressing
    /// over this (`crate::app`'s `Himark::new` is the workbench
    /// shell's boot).
    pub fn bare(fonts: AppFonts) -> Self {
        let total_started = crate::startup_profile::start();
        let mut store = Store::new();

        let theme = ::editor::theme::Theme::embedded();
        store.put(::editor::env::Fonts(fonts.source()));
        store.put(::editor::env::Themes(theme.clone()));

        let ui = std::rc::Rc::new(UiCtx::dont_use_too_slow());
        ui.set(imba::ui::UiFonts((fonts.source())()));
        ui.set(theme.ui().scrollbar.style());

        let overlay_font = hikit::fonts::ui_text_font(&ui, theme.ui().stats.font_size);

        // The baseline diff policy; outer edges override via
        // `register_diff_policy` (docs/editor/structural-diff.md).
        store.put(::editor::env::Differ(std::sync::Arc::new(myersdiff::Myers)));
        // What `@` completes against in a located document: the
        // session the document lives in.
        ::editor::completion::Mentions::install(&mut store, |store, location| {
            let state = ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .find(|state| {
                    documents::OpenDocuments::by_location(store, state.documents(), location)
                        .is_some()
                })?;
            let (session, state) =
                ahp_session::session::state::Hosts::home_of_documents(store, state.documents())?;
            Some(::editor::completion::MentionContext {
                folders: Arc::new(ahp_session::session::folders::session_folders(
                    store, &session,
                )),
                recents: recents::RecentLocations::list(store, state.recents()),
            })
        });

        let workshop = Arc::new(::editor::env::Workshop::new(
            ::editor::env::Fonts::of(&store),
            ::editor::env::Themes::of(&store),
        ));
        let handlers = Arc::new(crate::effects::Handlers::default());
        crate::effects::register_builtins(&handlers, &workshop);
        store.put(ahp_session::session::state::Hosts::default());
        store.put(ahp_wire::client::Servers::default());
        let engine = Self {
            store,
            ui,
            stats: Stats::new(overlay_font),
            #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
            effects: crate::effects::EffectLauncher::new(),
            handlers,
            workshop,
            bridges: std::collections::HashMap::new(),
            revision: 0,
            pending_file_events: Vec::new(),
            settle_requested: false,
            settling: false,
        };
        crate::startup_profile::log("Himark::bare", total_started);
        engine
    }

    pub fn ui_ctx(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn ui_handle(&self) -> std::rc::Rc<UiCtx> {
        self.ui.clone()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn store_mut(&mut self) -> StoreMut<'_> {
        StoreMut { app: self }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn stats_mut(&mut self) -> &mut Stats {
        &mut self.stats
    }

    pub fn workshop(&self) -> &Arc<::editor::env::Workshop> {
        &self.workshop
    }

    /// The empty-session housekeeping, run after every batch — the
    /// one thing the old gather/scatter rite still did.
    pub(crate) fn sweep(&mut self) {
        ahp_session::session::state::Hosts::sweep_empty(&mut self.store);
        self.invalidate_frames();
    }

    /// The one `&mut store` door outside the batch loop: mutate,
    /// then outdate every standing frame.
    pub(crate) fn setup(&mut self, mutate: impl FnOnce(&mut Store)) {
        mutate(&mut self.store);
        self.invalidate_frames();
    }

    /// The store moved on: every standing frame is now stale. The
    /// bump is the whole invalidation — each bridge rebuilds
    /// lazily, against the store as it stands when next asked.
    pub(crate) fn invalidate_frames(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// THE batch loop (docs/himark-core.md): run the commands with
    /// their follow-ups against the one store, run the batch-end
    /// lanes, commit, launch the effects — then hand the shell its
    /// tail (`crate::app`'s `batch_tail`: the window-shaped
    /// reactions, to become each bridge's own reaction to the
    /// outcome as the lanes and tails get registered instead of
    /// named).
    pub fn perform_batch(&mut self, commands: Vec<crate::perform::AppCommand>) -> bool {
        use crate::perform::AppCommand;

        if commands.is_empty() {
            return false;
        }
        let probe = std::time::Instant::now();
        let label = crate::perform::command_label(&commands[0]);
        let theme_before = ::editor::env::Themes::of(&self.store);
        let ui = self.ui.clone();
        let mut batch = crate::perform::AppEffects::new();

        let mut store = std::mem::take(&mut self.store);
        let mut performer = crate::perform::Performer {
            pending_file_events: &mut self.pending_file_events,
            workshop: &self.workshop,
        };
        let mut queue: std::collections::VecDeque<AppCommand> = commands.into();
        while let Some(command) = queue.pop_front() {
            let mut fx = batch.effects();
            performer.perform(&mut store, &ui, command, &mut fx);

            // The performed command's follow-ups run next, in push
            // order — the loop's own lane, not a store note.
            for (index, follow_up) in batch.take_follow_ups().into_iter().enumerate() {
                queue.insert(index, follow_up);
            }
        }

        {
            let mut fx = batch.effects();
            // The resident sessions subscription: hosts connect, list
            // and drain whether or not any view shows them — the
            // catalog stays true at all times (views only dress it).
            fx.scope(AppCommand::Verb, |fx| {
                ahp_session::session::driver::sync_sessions_lane(&mut store, fx)
            });
            // The lanes run over EVERY session: each drains its own
            // pending queue, so a clean session costs map reads — and a
            // landing's session gets its sweep THIS batch whatever the
            // batch's scope (the old tail served only the LAST scope's
            // session, so a cross-session batch starved the others).
            let probe_lanes = std::env::var_os("HIMARK_TRACE_DIFF").is_some();
            for state in ahp_session::session::state::Hosts::states(&store) {
                let documents = state.documents();
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_lanes(&mut store, documents, fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_diff_lanes: {:?}", started.elapsed());
                }
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_scroll_stripe_lanes(&mut store, documents, fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_scroll_stripe_lanes: {:?}", started.elapsed());
                }
                // The DRESSING sweep: any view whose basis lags its
                // pair resyncs NOW, id-routed — a normalize landing
                // and its re-dress share a batch, and no face waits
                // for paint.
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_dressing(&mut store, documents, &self.ui_ctx(), fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_diff_dressing: {:?}", started.elapsed());
                }
                // The dock tree views ride the push road too: a
                // changes / history feed landing refreshes a mounted
                // stale view in the SAME batch — no paint probe.
                changesview::changes_view::sync_changes_views(
                    &mut store,
                    state.changes(),
                    &self.ui_ctx(),
                );
                // The canvases sync against the fresh document/diff/
                // changeset state — the SAME batch a feed landed in, a
                // direct lane over the sets that own them. The
                // dressed-views queue drains here: the session's own
                // note (id-routed landings appended mid-batch), taken
                // by the one lane that reads it.
                let dressed = documents::OpenDocuments::take_dressed(&mut store, documents);
                fx.scope(AppCommand::Verb, |fx| {
                    canvas::canvas::sync_canvases(
                        &mut store,
                        state.canvas_router(),
                        &self.ui_ctx(),
                        &dressed,
                        fx,
                    )
                });
                // The comments lane: the model's announce and card
                // notes drain onto the wire — a clean collection
                // costs a map read.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_comments::sync(&mut store, state.comments_wire(), &self.ui_ctx(), fx)
                });
                // The diagnostics lane: dirty resources land their
                // squiggle markups into the session's open documents.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_lsp::diagnostics::sync(
                        &mut store,
                        state.diagnostics_wire(),
                        &self.ui_ctx(),
                        fx,
                    )
                });
                // The language-services pull lane: edited and newly
                // opened documents re-ask their semantic tokens and
                // inlay hints.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_lsp::enrich::sync(&mut store, state.enrich_wire(), fx)
                });
                // The gesture-ask lanes: the views noted onto their
                // MODELS (grow, commit fetches, refetches); the
                // drivers drain the notes onto the wires here — no
                // view carries a wire or a window for these.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_changes::history::sync(&mut store, state.history_wire(), fx);
                    ahp_changes::changes::sync(&mut store, state.changes_wire(), fx);
                    ahp_locations::driver::sync(
                        &mut store,
                        &self.ui_ctx(),
                        state.locations_wire(),
                        fx,
                    );
                });
            }
        }
        // Every session's lane claimed its own edit notes above; what
        // is left names documents open nowhere.
        ahp_lsp::enrich::drop_unclaimed(&store);
        // The safety net for a tail lane's follow-up: the queue loop
        // is over, so perform them here — late but never lost.
        let mut performer = crate::perform::Performer {
            pending_file_events: &mut self.pending_file_events,
            workshop: &self.workshop,
        };
        loop {
            let late = batch.take_follow_ups();
            if late.is_empty() {
                break;
            }
            for command in late {
                let mut fx = batch.effects();
                performer.perform(&mut store, &ui, command, &mut fx);
            }
        }
        let probe_perform = probe.elapsed();
        self.store = store;
        self.sweep();
        let probe_commit = probe.elapsed().saturating_sub(probe_perform);
        if batch.take_settle() {
            self.settle_requested = true;
        }
        self.launch(batch);
        let probe_launch = probe
            .elapsed()
            .saturating_sub(probe_perform)
            .saturating_sub(probe_commit);

        let theme_changed =
            theme_before.name() != ::editor::env::Themes::of(&self.store).name();
        self.batch_tail(&label, theme_changed);

        if crate::app::trace_stalls_enabled() && probe.elapsed().as_millis() >= 10 {
            eprintln!(
                "[stall] command {label}: perform={probe_perform:?} \
                 commit={probe_commit:?} launch={probe_launch:?} tail={:?}",
                probe
                    .elapsed()
                    .saturating_sub(probe_perform)
                    .saturating_sub(probe_commit)
                    .saturating_sub(probe_launch)
            );
        }
        true
    }

    pub fn register_client(
        &mut self,
        client: ahp_wire::client::Client,
    ) -> ahp_wire::client::HostId {
        let mut minted = ahp_wire::client::HostId::LOCAL;
        self.store
            .update::<ahp_wire::client::Servers>(|servers| minted = servers.mint(client));
        self.invalidate_frames();
        minted
    }

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    pub fn attach_host(
        &mut self,
        dispatcher: crate::effects::EffectDispatcher,
        scheduler: Arc<dyn Fn() + Send + Sync>,
    ) -> crate::effects::BackgroundRunner {
        self.effects
            .attach(dispatcher, scheduler, Arc::clone(&self.handlers))
    }

    pub fn register_handler<E: imba::effect::Effect>(
        &mut self,
        handler: impl imba::effect::EffectHandler<E>,
    ) where
        E::Result: Send + Sync,
    {
        self.handlers.register::<E>(handler);
    }

    pub fn effect_caller(&self) -> imba::effect::EffectCaller {
        let handlers = Arc::downgrade(&self.handlers);
        imba::effect::EffectCaller::new(Arc::new(move |type_id, payload| {
            handlers.upgrade()?.call_future(type_id, payload)
        }))
    }

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    pub(crate) fn launch(&mut self, effects: crate::perform::AppEffects) {
        self.effects.launch(effects);
    }

    #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
    pub(crate) fn launch(&mut self, batch: crate::perform::AppEffects) {
        use imba::effect::Message;

        let messages = batch.drain();
        let cancelled: std::collections::HashSet<_> = messages
            .iter()
            .filter_map(|message| match message {
                Message::Cancel(token) => Some(*token),

                Message::Relaunch(previous, _, _) => Some(*previous),
                Message::Launch(..) => None,
            })
            .collect();
        for message in messages {
            let (token, effect) = match message {
                Message::Launch(token, effect) => (token, effect),
                Message::Relaunch(_, token, effect) => (token, effect),
                Message::Cancel(_) => continue,
            };
            if cancelled.contains(&token) {
                continue;
            }
            let payload = effect.into_payload();
            let type_id = payload.type_id();
            match self.handlers.dispatch(payload) {
                Err(_) => crate::effects::orphan(type_id),
                Ok((future, lift)) => {
                    if let Some(command) = lift(imba::effect::block_on(future)) {
                        self.perform_batch(vec![command]);
                    }
                }
            }
        }
    }

    pub fn observe_file_changes(&mut self) {
        self.setup(documents::watch::Watching::install);
    }

    /// Install the edge's BASE RESOLVER: working location → base ref,
    /// answered synchronously from store truth at ask time.
    pub fn observe_stripe_bases(
        &mut self,
        resolve: std::sync::Arc<dyn documents::diffs::StripeBaseResolver>,
    ) {
        self.setup(move |store| documents::diffs::StripeBases::install(store, resolve.clone()));
    }

    pub fn register_editor_command(
        &mut self,
        command: Arc<dyn editor::dynamic::DynamicEditorCommand>,
    ) {
        self.setup(|store| ::editor::dynamic::EditorCommands::register(store, command));
    }

    /// Commands that act on the COLLECTION — handed the pane's ids at
    /// dispatch (docs/entities.md law 3), never resolving an owner.
    pub fn register_document_command(
        &mut self,
        command: Arc<dyn documents::dynamic::DocumentCommand>,
    ) {
        self.setup(|store| documents::dynamic::DocumentCommands::register(store, command));
    }

    pub(crate) fn refresh_chrome(&mut self, theme: &::editor::theme::Theme) {
        self.ui.set(theme.ui().scrollbar.style());
        self.stats.set_font(hikit::fonts::ui_text_font(
            &self.ui,
            theme.ui().stats.font_size,
        ));
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct StoreMut<'a> {
    app: &'a mut Himark,
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::Deref for StoreMut<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.app.store
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::ops::DerefMut for StoreMut<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.app.store
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for StoreMut<'_> {
    fn drop(&mut self) {
        self.app.sweep();
    }
}
