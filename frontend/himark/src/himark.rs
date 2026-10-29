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

use crate::bridge::ViewBridgeId;
use crate::stats::Stats;

/// One registry entry: the mounted view and, beside it, the last
/// products its client derived through the pipeline — plain data,
/// tagged with what they were derived FROM. Clients compose the
/// stages themselves (`crate::app::Himark::ensure_frame` is the
/// window client); nothing here decides when anything is stale.
pub(crate) struct Mounted {
    pub(crate) bridge: crate::bridge::ViewBridge,

    /// The overlays this bridge's last frame bubbled — kinds and
    /// anchors in content coordinates, an epoch that moves when the
    /// set changes. The shell reconciles native layers against
    /// this; the bridge composites none of them.
    pub(crate) overlay_anchors: Vec<(
        imba::overlay::Attachment,
        imba::overlay::OverlayHost,
        skia_safe::Rect,
    )>,
    pub(crate) overlay_epoch: u64,

    /// Set on a POPOVER's slot: the bridge whose overlay this view
    /// stands. The popover window is content-sized, but the
    /// re-derivation inside must run at the PARENT'S size — both
    /// layout roads substitute the parent slot's standing size for
    /// the shell's constraints.
    pub(crate) overlay_parent: Option<ViewBridgeId>,

    /// The last realized widget tree, derived at `revision` ×
    /// `size` — the standing frame events replay against between
    /// store batches.
    pub(crate) frame: Option<crate::bridge::Frame>,
    pub(crate) revision: u64,
    pub(crate) size: skia_safe::Size,

    /// The STAGE road's shadow: the band the host last realized.
    /// A stage client never fills `frame`, but the IME fold needs
    /// one — `mounted_frame` rebuilds at `size` and realizes this
    /// band, so the folded geometry matches what is on screen.
    pub(crate) stage_viewport: Option<skia_safe::Rect>,

    /// One warm arena from the last recycled frame.
    pub(crate) spare_arena: Option<Box<imba::arena::Arena>>,
}

impl Mounted {
    pub(crate) fn new(view: std::sync::Arc<dyn crate::bridge::BridgeView>) -> Mounted {
        Mounted {
            bridge: crate::bridge::ViewBridge::new(view),
            overlay_anchors: Vec::new(),
            overlay_epoch: 0,
            overlay_parent: None,
            frame: None,
            revision: 0,
            size: skia_safe::Size::new(-1.0, -1.0),
            stage_viewport: None,
            spare_arena: None,
        }
    }
}

/// THE shell: the callback API himark uses to control whoever
/// owns the frame (docs/himark-core.md). Every window-shaped
/// command is app logic FUSED to a placement choice; this trait is
/// the knife between them — the logic stays in Rust forever, only
/// the placement crosses. Handed to the engine at construction
/// and SEATED in the store, where every command road reaches it.
pub trait Shell: Send + Sync {
    /// The user asked to see `place` (the Navigator currency),
    /// from the view mounted as `origin`. A workbench shell acts
    /// inline — panes, docks, splits — with the store and fx in
    /// hand; a native shell records and returns (its answer is a
    /// screen push plus the minting door, with the same place).
    /// `false` DECLINES: a native shell decorates the workbench
    /// one and takes places over one kind at a time.
    fn navigate(
        &self,
        origin: ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool;
}

/// The seated shell — a boot resident like every road.
#[derive(Clone)]
pub struct ShellSeat(pub Arc<dyn Shell>);

pub struct Himark {
    /// THE store — one, global, always live. Hosts, Windows and
    /// Servers are residents like everything else; a batch borrows
    /// it, nothing is projected in or filed back.
    pub(crate) store: Store,

    pub(crate) ui: std::rc::Rc<UiCtx>,

    pub(crate) poster: crate::effects::Poster,

    pub(crate) stats: Stats,

    /// The mounted views, one each — a workbench window is just
    /// one kind of mounted view (`crate::app::WorkbenchView`).
    pub(crate) bridges: std::collections::HashMap<ViewBridgeId, Mounted>,

    /// The store's frame revision, bumped by every `&mut store`
    /// door — a bridge's frame is a memo over (revision × size),
    /// rebuilt lazily when next asked.
    pub(crate) revision: u64,

    /// Standing frames BUILT by this app — the one-tree-per-frame
    /// law's probe: a pure-scroll tick builds at most ONE (the
    /// settle pulse's; the quiet commit lets the clock and the
    /// paint reuse it). Per instance, so parallel test engines
    /// never count into each other.
    pub(crate) frames_built: u64,

    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    effects: crate::effects::EffectLauncher,

    handlers: Arc<crate::effects::Handlers>,

    pub(crate) workshop: Arc<::editor::env::Workshop>,

    /// The stage tables (docs/ui/imba-ffi.md): layouts and frames
    /// a host holds by handle. Engine-owned so the `ToView` road
    /// can quiesce a view's stages before mutating it.
    pub(crate) stage_layouts: crate::bridge::Handles<(ViewBridgeId, crate::bridge::BridgeLayout)>,
    pub(crate) stage_frames: crate::bridge::Handles<(ViewBridgeId, crate::bridge::Frame)>,

    /// Mount ids for non-window views, minted from the high half —
    /// window bridges keep their window's raw id in the low half.
    next_mount: u64,

    pub(crate) pending_file_events: Vec<documents::watch::Subscription>,

    /// The last REVEAL a mounted road answered and nobody scrolled:
    /// the engine owns no viewport for a mounted view, so the ask
    /// parks here and the HOST drains it (`take_reveal`) into its
    /// own scroller — the native half of the reveal mechanism.
    /// Parked reveals, LATEST rect per bridge — one slot starved
    /// honest asks: a caret-follow flooding every tick overwrote a
    /// canvas's row reveal before the host drained it.
    pub(crate) pending_reveals: Vec<(ViewBridgeId, imba::event::Reveal)>,

    /// External place arms (plugins) — `mount_place` consults them
    /// after its own.
    /// The palette plugin's view factory (`register_palette`).
    pub(crate) palette: Option<crate::mounts::PaletteFactory>,

    pub(crate) modal_place_arms: Vec<crate::mounts::ModalPlaceArm>,

    /// A perform raised the settle bit (`Effects::settle`): run the
    /// synchronous `Event::Settle` pulse before the next paint so
    /// viewport corrections land in the SAME frame
    /// (docs/editor/viewport-preservation.md §3.2).
    pub(crate) settle_requested: bool,
    pub(crate) settling: bool,
}

/// One catalog snapshot — plain rows, cloned out, immutable.
pub struct Catalog {
    pub hosts: Vec<HostRow>,
    pub sessions: Vec<SessionRow>,
}

pub struct HostRow {
    pub host: ahp_wire::client::HostId,
    pub name: String,
    /// 0 idle · 1 connecting · 2 connected · 3 failed.
    pub status: u32,
}

pub struct SessionRow {
    pub host: ahp_wire::client::HostId,
    pub uri: String,
    pub provider: String,
    pub title: String,
    pub status: i64,
    pub modified_at: String,
}

/// THE FONT WORLD the app shapes with, built once at boot. Every
/// `UiCtx` the engine mints is handed this one — the UI thread's
/// and the workshop's alike — so the system scan happens once and
/// the typeface bridge is answered once, for everyone.
pub struct AppFonts {
    world: ::editor::parley_line::FontWorld,
}

impl AppFonts {
    pub fn platform() -> Self {
        Self {
            world: ::editor::parley_line::world(),
        }
    }

    /// The embedded faces ARE the world's — a test and a shell boot
    /// the same way.
    pub fn embedded() -> Self {
        Self::platform()
    }

    /// A world the edge built and seeded itself — the web's road,
    /// where the face arrives over the wire before the app exists.
    pub fn over(world: ::editor::parley_line::FontWorld) -> Self {
        Self { world }
    }

    pub fn world(&self) -> ::editor::parley_line::FontWorld {
        self.world.clone()
    }
}

impl Himark {
    /// The BARE engine: store, ui, effects, the editor environment
    /// — and not one workbench wire. Shells add their own dressing
    /// over this (`crate::app`'s `Himark::new` is the workbench
    /// shell's boot).
    pub fn bare(fonts: AppFonts, shell: Arc<dyn Shell>) -> Self {
        let total_started = crate::startup_profile::start();
        let mut store = Store::new();

        let theme = ::editor::theme::embedded();
        store.put(::editor::env::Themes(theme.clone()));

        let ui = std::rc::Rc::new(UiCtx::dont_use_too_slow());
        // The one world, into the ctx that paints.
        ui.set(fonts.world());
        ui.set(theme.part(::editor::theme::ScrollbarChrome::KEY).style());

        let overlay_font = hikit::fonts::ui_text_font(
            &ui,
            theme.part(crate::stats_chrome::StatsChrome::KEY).font_size,
        );

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
            fonts.world(),
            ::editor::env::Themes::of(&store),
        ));
        let handlers = Arc::new(crate::effects::Handlers::default());
        crate::effects::register_builtins(&handlers, &workshop);
        store.put(ShellSeat(shell));
        store.put(ahp_session::session::state::Hosts::default());
        store.put(ahp_wire::client::Servers::default());
        let engine = Self {
            store,
            ui,
            poster: crate::effects::Poster::default(),
            stats: Stats::new(overlay_font),
            #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
            effects: crate::effects::EffectLauncher::new(),
            handlers,
            workshop,
            bridges: std::collections::HashMap::new(),
            stage_layouts: Default::default(),
            stage_frames: Default::default(),
            next_mount: crate::bridge::MOUNT_BASE,
            revision: 0,
            frames_built: 0,
            pending_file_events: Vec::new(),
            pending_reveals: Vec::new(),
            palette: None,
            modal_place_arms: Vec::new(),
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

    /// The one-tree-per-frame probe (see `frames_built`).
    pub fn frames_built(&self) -> u64 {
        self.frames_built
    }

    /// The empty-session housekeeping, run after every batch — the
    /// one thing the old gather/scatter rite still did.
    pub(crate) fn sweep(&mut self) {
        ahp_session::session::state::Hosts::sweep_empty(&mut self.store);
        self.invalidate_frames();
    }

    /// The one `&mut store` door outside the batch loop: mutate,
    /// then outdate every standing frame.
    pub fn setup(&mut self, mutate: impl FnOnce(&mut Store)) {
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
        if std::env::var_os("HIMARK_TRACE_BATCH").is_some() {
            eprintln!("[batch] {label} (+{} more)", commands.len() - 1);
        }
        let theme_before = ::editor::env::Themes::of(&self.store);
        let ui = self.ui.clone();
        let mut batch = crate::perform::AppEffects::new();

        let mut store = std::mem::take(&mut self.store);
        let mut performer = crate::perform::Performer {
            pending_file_events: &mut self.pending_file_events,
            workshop: &self.workshop,
            bridges: &mut self.bridges,
            stage_layouts: &mut self.stage_layouts,
            stage_frames: &mut self.stage_frames,
        };
        let mut queue: std::collections::VecDeque<AppCommand> = commands.into();
        // EVERY command must declare itself display-neutral
        // (`fx.quiet()`) for the batch to commit quietly — one
        // unmarked arm and the frames invalidate as always.
        let mut all_quiet = true;
        while let Some(command) = queue.pop_front() {
            let slow_probe = std::env::var_os("HIMARK_TRACE_BATCH")
                .map(|_| (crate::perform::command_label(&command), std::time::Instant::now()));
            let mut fx = batch.effects();
            performer.perform(&mut store, &ui, command, &mut fx);
            if let Some((label, started)) =
                slow_probe.filter(|(_, started)| started.elapsed().as_millis() >= 20)
            {
                eprintln!("[batch] SLOW {label}: {:?}", started.elapsed());
            }
            all_quiet &= batch.take_quiet();

            // The performed command's follow-ups run next, in push
            // order — the loop's own lane, not a store note.
            for (index, follow_up) in batch.take_follow_ups().into_iter().enumerate() {
                queue.insert(index, follow_up);
            }

            // Deferred requests are the ENGINE's to run — a store
            // note, not a window note. The window shell also drains
            // them (its own commands see them first); whatever a
            // windowless road pushed lands here, the same batch.
            for request in imba::command::Requests::drain(&mut store) {
                queue.push_back(AppCommand::Verb(imba::command::Verb::Dynamic(request)));
            }
        }

        // A QUIET batch (every arm display-neutral) skips the
        // session lanes whole: they react to landings and edits,
        // and a quiet batch certified it made none — this is most
        // of what a per-frame observation batch used to cost.
        if !all_quiet {
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
                    canvas::canvases::sync_canvases(
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
            bridges: &mut self.bridges,
            stage_layouts: &mut self.stage_layouts,
            stage_frames: &mut self.stage_frames,
        };
        loop {
            let mut late = batch.take_follow_ups();
            late.extend(
                imba::command::Requests::drain(&mut store)
                    .into_iter()
                    .map(|request| AppCommand::Verb(imba::command::Verb::Dynamic(request))),
            );
            if late.is_empty() {
                break;
            }
            for command in late {
                let mut fx = batch.effects();
                performer.perform(&mut store, &ui, command, &mut fx);
                all_quiet &= batch.take_quiet();
            }
        }
        let probe_perform = probe.elapsed();
        self.store = store;
        if all_quiet {
            // The QUIET commit: the standing frames stay TRUE — no
            // revision bump, so the settle pulse's observations do
            // not outdate the tree the pulse itself just built. ONE
            // tree serves the pulse, the clock and the paint.
            if std::env::var_os("HIMARK_TRACE_BATCH").is_some() {
                eprintln!("[batch] {label}: quiet commit");
            }
        } else {
            self.sweep();
        }
        let probe_commit = probe.elapsed().saturating_sub(probe_perform);
        if batch.take_settle() {
            self.settle_requested = true;
        }
        self.launch(batch);
        let probe_launch = probe
            .elapsed()
            .saturating_sub(probe_perform)
            .saturating_sub(probe_commit);

        if !all_quiet {
            let theme_changed =
                theme_before.name() != ::editor::env::Themes::of(&self.store).name();
            self.batch_tail(&label, theme_changed);
        }

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

    /// One empty batch, for a shell with no first paint to
    /// trigger it: the batch-end lanes run — hosts connect, the
    /// catalog lists — and the effect road takes over from there.
    pub fn sync(&mut self) -> bool {
        struct Nudge;
        impl imba::command::DynamicOnceCommand for Nudge {
            fn perform(
                self: Box<Self>,
                _store: &mut Store,
                _ui: &UiCtx,
                _fx: &mut imba::command::Fx<'_>,
            ) {
            }
        }
        self.perform_batch(vec![crate::perform::AppCommand::Verb(
            imba::command::Verb::once(Nudge),
        )])
    }

    /// The catalog epoch: moved means re-snapshot. Today it rides
    /// the store's frame revision — an over-approximation (any
    /// batch bumps it, catalog or not); snapshots are cheap, so a
    /// spurious re-read costs a few clones. Tightens when the fold
    /// gets its own counter.
    pub fn sessions_epoch(&self) -> u64 {
        self.revision
    }

    /// The session catalog, cloned out whole for native chrome —
    /// the QUERY half of the session surface
    /// (docs/ui/himark-ffi.md): a picker, nav titles, badges.
    pub fn catalog(&self) -> Catalog {
        let mut hosts = Vec::new();
        let mut sessions = Vec::new();
        for (host, record) in ahp_session::session::agents::Agents::list(&self.store) {
            hosts.push(HostRow {
                host,
                name: record.name.clone(),
                status: match record.status {
                    ahp_session::session::state::HostStatus::Idle => 0,
                    ahp_session::session::state::HostStatus::Connecting => 1,
                    ahp_session::session::state::HostStatus::Connected => 2,
                    ahp_session::session::state::HostStatus::Failed(_) => 3,
                },
            });
            for summary in record.sessions.iter() {
                sessions.push(SessionRow {
                    host,
                    uri: summary.resource.clone(),
                    provider: summary.provider.clone(),
                    title: summary.title.clone(),
                    status: summary.status as i64,
                    modified_at: summary.modified_at.clone(),
                });
            }
        }
        Catalog { hosts, sessions }
    }

    /// Enter a session HEADLESSLY — subscribe, adopt, ensure the
    /// wires, relaunch the poll; no window switches, no chat
    /// opens. The native shell's selection road: after this, the
    /// session's views are mintable.
    pub fn enter_session(
        &mut self,
        host: ahp_wire::client::HostId,
        session: ahp_wire::client::SessionUri,
    ) -> bool {
        self.perform_batch(vec![crate::perform::AppCommand::Verb(
            imba::command::Verb::once(crate::higent::open_session::EnterHeadless {
                server: host,
                session,
            }),
        )])
    }

    /// Create a session HEADLESSLY on the given host (or the local
    /// default); the landing enters it, and the catalog carries
    /// the rest — the shell watches the epoch.
    pub fn create_session(&mut self, host: Option<ahp_wire::client::HostId>) -> bool {
        self.perform_batch(vec![crate::perform::AppCommand::Verb(
            imba::command::Verb::once(crate::higent::open_session::CreateHeadless { host }),
        )])
    }

    /// A MOUNTED slot's current frame at its size — rebuilt
    /// lazily off the revision, exactly the window client's memo
    /// but for views the host composes directly. `size` sets a
    /// new layout size (the draw road); `None` reuses the last
    /// drawn one (the event road between paints).
    pub(crate) fn mounted_frame(
        &mut self,
        id: ViewBridgeId,
        size: Option<skia_safe::Size>,
    ) -> Option<(&mut crate::bridge::Frame, skia_safe::Size)> {
        self.mounted_frame_inner(id, size, false)
    }

    /// The STANDING frame as it stands — rebuilt only when there is
    /// none at all. The clock and the wheel advance STATE, not
    /// geometry; letting them ride a one-frame-stale tree spares a
    /// full layout+realize walk per tick (the stage road rebuilds
    /// for the paint anyway — that walk is the one that matters).
    pub(crate) fn mounted_frame_standing(
        &mut self,
        id: ViewBridgeId,
    ) -> Option<(&mut crate::bridge::Frame, skia_safe::Size)> {
        self.mounted_frame_inner(id, None, true)
    }

    fn mounted_frame_inner(
        &mut self,
        id: ViewBridgeId,
        size: Option<skia_safe::Size>,
        standing: bool,
    ) -> Option<(&mut crate::bridge::Frame, skia_safe::Size)> {
        let revision = self.revision;
        // A popover slot's layout runs at the PARENT'S standing
        // size, against a freshly pinned parent; its own `size`
        // stays the content-shaped cache key.
        let host_size = self.overlay_host_size(id);
        let pin = self.overlay_parent_pin(id);
        let slot = self.bridges.get_mut(&id)?;
        if let Some((parent, band)) = pin {
            slot.bridge.view().overlay_pin_parent(parent, band);
        }
        let size = size.or_else(|| (slot.size.width > 0.0).then_some(slot.size))?;
        // STANDING tolerates a stale SIZE, never a stale REVISION. A
        // stage-hosted bridge paints through the stage road, which
        // never touches this slot's frame — reusing it across
        // revisions left the clock ticking a BOOT-ERA tree: an empty
        // transcript list whose tail ask (0..1, zero viewport) could
        // never satisfy, re-parking a reveal every tick — the eternal
        // "animating" that kept every view awake at 60fps. Keyed on
        // the revision, an idle engine stands (no batches, no
        // rebuilds) and each batch buys ONE rebuild at the next tick.
        let fresh =
            slot.frame.is_some() && slot.revision == revision && (standing || slot.size == size);
        if !fresh {
            if let Some(frame) = slot.frame.take() {
                slot.spare_arena = Some(frame.recycle());
            }
            let arena = slot.spare_arena.take().unwrap_or_default();
            let layout = slot.bridge.layout(
                &self.store,
                &self.ui,
                imba::constraints::Constraints::tight(host_size.unwrap_or(size)),
                arena,
            );
            // A stage client's shadow realizes the band the HOST
            // shows; the retained road realizes its whole rect.
            let viewport = slot
                .stage_viewport
                .unwrap_or_else(|| skia_safe::Rect::from_size(size));
            let mut frame = layout.realize(viewport);
            let anchors = frame.exported_overlays();
            if anchors != slot.overlay_anchors {
                slot.overlay_anchors = anchors;
                slot.overlay_epoch += 1;
            }
            slot.frame = Some(frame);
            slot.revision = revision;
            slot.size = size;
        }
        slot.frame.as_mut().map(|frame| (frame, size))
    }

    /// One event into a mounted view, against its standing frame.
    pub(crate) fn handle_mounted(
        &mut self,
        id: ViewBridgeId,
        event: &imba::event::Event<'_>,
    ) -> imba::event::EventResult<crate::perform::AppCommand> {
        let trace = std::env::var_os("HIMARK_TRACE_MOUNT").is_some();
        // The HONEST viewport is the host scroller's band (stamped
        // at every stage realize), not the whole content — the
        // views read it: reveal_satisfied must see what's actually
        // on screen or a caret-follow settles without ever asking.
        let band = self.bridges.get(&id).and_then(|slot| slot.stage_viewport);
        // Clock ticks and wheel deltas advance widget STATE; they
        // ride the standing frame as-is. Pointer and key events
        // hit-test geometry — those get the fresh rebuild.
        let stale_ok = matches!(
            event,
            imba::event::Event::AnimationClock { .. } | imba::event::Event::Scroll { .. }
        );
        let frame_and_size = match stale_ok {
            true => self.mounted_frame_standing(id),
            false => self.mounted_frame(id, None),
        };
        let Some((frame, size)) = frame_and_size else {
            if trace {
                eprintln!("[mount] {id:?}: event against no frame");
            }
            return imba::event::EventResult::Ignored;
        };
        let viewport = band.unwrap_or_else(|| skia_safe::Rect::from_size(size));
        let result = frame.handle(event, viewport);
        if trace {
            eprintln!(
                "[mount] {id:?} size {:?}x{:?} -> {}",
                size.width,
                size.height,
                match &result {
                    imba::event::EventResult::Ignored => "ignored",
                    imba::event::EventResult::Handled(handled) => {
                        match (handled.commands.is_empty(), handled.reveal.is_some()) {
                            (true, true) => "reveal",
                            (true, false) => "handled",
                            (false, true) => "commands+reveal",
                            (false, false) => "commands",
                        }
                    }
                }
            );
        }
        result
    }

    /// Paint a mounted view into the host's canvas at the host's
    /// size — the mounted twin of the window draw.
    pub fn draw_mounted(
        &mut self,
        id: ViewBridgeId,
        canvas: &skia_safe::Canvas,
        size: skia_safe::Size,
    ) -> bool {
        // TRANSPARENT floor, like the stage road: the HOST behind
        // the mounted view owns the background (a glass island, the
        // document floor's color).
        canvas.clear(skia_safe::Color::TRANSPARENT);
        let result = {
            let Some((frame, size)) = self.mounted_frame(id, Some(size)) else {
                return false;
            };
            frame.handle(
                &imba::event::Event::Paint {
                    canvas,
                    focused: true,
                },
                skia_safe::Rect::from_size(size),
            )
        };
        let imba::event::EventResult::Handled(handled) = result else {
            return false;
        };
        let parked = match handled.reveal {
            Some(reveal) => {
                self.park_reveal(id, reveal);
                true
            }
            None => false,
        };
        match handled.commands.is_empty() {
            true => parked,
            false => self.perform_batch(handled.commands) || parked,
        }
    }

    /// A bridge's exported overlay PRESENCE: the epoch, and the
    /// kinded anchors the shell reconciles its layers against.
    pub fn bridge_overlays(
        &self,
        id: ViewBridgeId,
    ) -> Option<(
        u64,
        Vec<(
            imba::overlay::Attachment,
            imba::overlay::OverlayHost,
            skia_safe::Rect,
        )>,
    )> {
        let slot = self.bridges.get(&id)?;
        Some((slot.overlay_epoch, slot.overlay_anchors.clone()))
    }

    /// Mount the POPOVER's bridge: a view that re-derives the
    /// parent's frame and stands overlay #index's content alone.
    /// Recursive popovers are normal — this bridge exports its own
    /// bubbled overlays like any other.
    pub fn mint_overlay(&mut self, parent: ViewBridgeId, index: usize) -> Option<ViewBridgeId> {
        // The positional index names an overlay ONLY in this frame's
        // set — translate it to the stable identity (host, nth of
        // that host) now, so the mount keeps re-deriving the SAME
        // overlay while the set reorders around it.
        let anchors = &self.bridges.get(&parent)?.overlay_anchors;
        let (_, host, _) = *anchors.get(index)?;
        let within = anchors[..index]
            .iter()
            .filter(|(_, other, _)| *other == host)
            .count();
        let id = self.mount(move |_store, _ui, _id, _fx| {
            Some(std::sync::Arc::new(crate::mounts::OverlayMount {
                pinned: std::sync::OnceLock::new(),
                band: std::sync::Mutex::new(None),
                host,
                within,
                content: std::sync::Mutex::new(None),
            })
                as std::sync::Arc<dyn crate::bridge::BridgeView>)
        })?;
        if let Some(slot) = self.bridges.get_mut(&id) {
            slot.overlay_parent = Some(parent);
        }
        Some(id)
    }

    /// A POPOVER slot's content slice, in the parent's coordinates
    /// — stashed by the last layout. The shell sizes the popover
    /// to it and paints exactly this viewport.
    pub fn overlay_content(&self, id: ViewBridgeId) -> Option<skia_safe::Rect> {
        self.bridges.get(&id)?.bridge.view().overlay_content_rect()
    }

    /// The parent view a popover slot pins before laying out, with
    /// the parent's standing band — the slice the re-derivation
    /// realizes, never the whole content.
    fn overlay_parent_pin(
        &self,
        id: ViewBridgeId,
    ) -> Option<(
        std::sync::Arc<dyn crate::bridge::BridgeView>,
        Option<skia_safe::Rect>,
    )> {
        let parent = self.bridges.get(&id)?.overlay_parent?;
        let slot = self.bridges.get(&parent)?;
        Some((slot.bridge.view_rc(), slot.stage_viewport))
    }

    /// A popover slot lays out at its PARENT'S standing size — the
    /// popover window is content-sized, and tight constraints at
    /// that size would collapse the re-derivation inside.
    fn overlay_host_size(&self, id: ViewBridgeId) -> Option<skia_safe::Size> {
        let parent = self.bridges.get(&id)?.overlay_parent?;
        let size = self.bridges.get(&parent)?.size;
        (size.width > 0.0).then_some(size)
    }

    /// Drain the parked reveal — the host scrolls it into view.
    pub fn take_reveal(&mut self) -> Option<(ViewBridgeId, imba::event::Reveal)> {
        if self.pending_reveals.is_empty() {
            return None;
        }
        let (id, reveal) = self.pending_reveals.remove(0);
        if std::env::var_os("HIMARK_TRACE_REVEAL").is_some() {
            eprintln!("[reveal] drained: bridge {} {reveal:?}", id.raw());
        }
        Some((id, reveal))
    }

    /// Every command the SESSION can run. A mounted road has no
    /// focused window to walk — it has a WORKSPACE, the session,
    /// and the views mounted into it. Himark holds those views, so
    /// Himark does the walk: ask each view of this session what it
    /// offers and concatenate. The global registry rides on top at
    /// the call site, bound to the asking bridge.
    pub fn session_commands(
        &self,
        session: &ahp_wire::SessionId,
    ) -> Vec<imba::PresentableCommand<crate::app::AppCommand>> {
        let ui = self.ui.clone();
        let mut collected = Vec::new();
        for id in self.mounted_ids() {
            let mine = crate::mounts::session_of(&self.store, id).as_ref() == Some(session);
            if let (true, Some(slot)) = (mine, self.bridges.get(&id)) {
                // The view's own store, and only its COMMANDS out of
                // the walk — the rest of the focus data borrows that
                // store, which dies with this scope.
                let offered = {
                    let view = slot.bridge.view();
                    let store = view.frame_store(&self.store, ui.as_ref());
                    view.focus(&store, ui.as_ref())
                        .map(|data| data.commands)
                        .unwrap_or_default()
                };
                collected.extend(offered);
            }
        }
        collected
    }

    /// The palette's VIEW, from the plugin that owns it — the
    /// engine hands it the session's commands and gets a modal
    /// back. Registered at boot, like a place arm; `palette` sits
    /// above this crate and cannot be named from here.
    pub fn register_palette(&mut self, make: crate::mounts::PaletteFactory) {
        self.palette = Some(make);
    }

    /// A plugin's place arm — consulted by `mount_place` after the
    /// built-in arms.
    pub fn register_place_arm(&mut self, arm: crate::mounts::ModalPlaceArm) {
        self.modal_place_arms.push(arm);
    }

    pub(crate) fn palette_factory(&self) -> Option<crate::mounts::PaletteFactory> {
        self.palette.clone()
    }

    /// Re-seat the shell, wrapping the one seated now — the native
    /// callback DECORATES the chain (claim some place kinds, pass
    /// the rest through), it never replaces it.
    pub fn seat_shell(&mut self, wrap: impl FnOnce(Arc<dyn Shell>) -> Arc<dyn Shell>) {
        let seated = self
            .store
            .get::<ShellSeat>()
            .map(|seat| seat.0.clone())
            .expect("a shell is seated at boot");
        let wrapped = wrap(seated);
        self.setup(|store| store.put(ShellSeat(wrapped)));
    }

    /// Whether this bridge id is a WINDOW — the dispatch roads
    /// fork on it: windows ride the workbench paths, mounts ride
    /// their own frames.
    pub fn is_window(&self, id: ViewBridgeId) -> bool {
        self.windows()
            .entity(::workbench::window::WindowId::from_raw(id.raw()))
            .is_some()
    }

    /// A mounted slot's last drawn size — the event roads'
    /// viewport stand-in where a window would answer with its own
    /// (the mounted dispatch arm routes against the standing frame
    /// and ignores the passed size; this keeps those roads from
    /// treating "no window viewport" as "gone").
    pub fn mounted_viewport(&self, id: ViewBridgeId) -> Option<skia_safe::Size> {
        id.is_mounted()
            .then(|| self.bridges.get(&id).map(|slot| slot.size))
            .flatten()
    }

    /// Every mounted bridge — the shell's animation clock fans out
    /// over these the way it does over windows.
    pub fn mounted_ids(&self) -> Vec<ViewBridgeId> {
        self.bridges
            .keys()
            .copied()
            .filter(|id| id.is_mounted())
            .collect()
    }

    /// Mount a view as a bridge: the build closure gets the store,
    /// the minted id and a real effects scope (the `add_window`
    /// idiom). `None` from the build aborts the mount whole.
    pub fn mount(
        &mut self,
        build: impl FnOnce(
            &mut Store,
            &UiCtx,
            ViewBridgeId,
            &mut crate::perform::AppFx<'_>,
        ) -> Option<std::sync::Arc<dyn crate::bridge::BridgeView>>,
    ) -> Option<ViewBridgeId> {
        let id = ViewBridgeId::from_raw(self.next_mount);
        self.next_mount += 1;
        let ui = self.ui.clone();
        let mut batch = crate::perform::AppEffects::new();
        let view = {
            let mut fx = batch.effects();
            build(&mut self.store, ui.as_ref(), id, &mut fx)?
        };
        self.bridges.insert(id, Mounted::new(view));
        self.invalidate_frames();
        let late = batch.take_follow_ups();
        self.launch(batch);
        if !late.is_empty() {
            self.perform_batch(late);
        }
        Some(id)
    }

    /// Unmount: the slot, its frame and every stage a host still
    /// holds for it go together.
    pub fn unmount(&mut self, id: ViewBridgeId) -> bool {
        let Some(mut slot) = self.bridges.remove(&id) else {
            return false;
        };
        crate::bridge::quiesce(
            id,
            &mut slot,
            &mut self.stage_layouts,
            &mut self.stage_frames,
        );
        crate::mounts::forget_session(&mut self.store, id);
        true
    }

    // ── the stage doors (the C surface delegates here) ──

    pub fn stage_layout(&mut self, id: ViewBridgeId, size: skia_safe::Size) -> u64 {
        // A popover slot ignores the shell's (content-sized) ask
        // and lays out at the parent's standing size, against a
        // freshly pinned parent.
        let size = self.overlay_host_size(id).unwrap_or(size);
        let pin = self.overlay_parent_pin(id);
        let Some(slot) = self.bridges.get_mut(&id) else {
            return 0;
        };
        if let Some((parent, band)) = pin {
            slot.bridge.view().overlay_pin_parent(parent, band);
        }
        let arena = slot.spare_arena.take().unwrap_or_default();
        // An INFINITE height is the native scroller asking to
        // measure: width pinned, height the content's own — the
        // sized stage's `.size()` becomes the scroller's content
        // size. A finite size stays the window-shaped tight fit.
        let constraints = if size.height.is_infinite() {
            imba::constraints::Constraints {
                min: skia_safe::Size::new(size.width, 0.0),
                max: size,
            }
        } else {
            imba::constraints::Constraints::tight(size)
        };
        let stage = slot
            .bridge
            .layout(&self.store, &self.ui, constraints, arena);
        if id.is_mounted() {
            // The stage road's shadow: the measured size becomes
            // the slot's, so the IME fold (`mounted_frame`)
            // rebuilds the same layout the stage client shows.
            slot.size = stage.size();
        }
        self.stage_layouts.mint((id, stage))
    }

    pub fn stage_layout_size(&self, handle: u64) -> Option<skia_safe::Size> {
        self.stage_layouts
            .get(handle)
            .map(|(_, stage)| stage.size())
    }

    pub fn stage_realize(&mut self, handle: u64, viewport: skia_safe::Rect) -> u64 {
        let Some((id, stage)) = self.stage_layouts.take(handle) else {
            return 0;
        };
        let mut frame = stage.realize(viewport);
        if let Some(slot) = self.bridges.get_mut(&id) {
            if id.is_mounted() {
                slot.stage_viewport = Some(viewport);
                // The paint road is the per-frame rebuild now — the
                // bubbled overlay anchors must move with it, or a
                // popover's presence lags the standing frame.
                let anchors = frame.exported_overlays();
                if anchors != slot.overlay_anchors {
                    slot.overlay_anchors = anchors;
                    slot.overlay_epoch += 1;
                }
            }
        }
        self.stage_frames.mint((id, frame))
    }

    pub fn stage_layout_release(&mut self, handle: u64) -> bool {
        self.stage_layouts.take(handle).is_some()
    }

    pub fn stage_frame(&self, handle: u64) -> Option<&crate::bridge::Frame> {
        self.stage_frames.get(handle).map(|(_, frame)| frame)
    }

    pub fn stage_frame_origin(&self, handle: u64) -> Option<ViewBridgeId> {
        self.stage_frames.get(handle).map(|(id, _)| *id)
    }

    pub fn park_reveal(&mut self, id: ViewBridgeId, reveal: imba::event::Reveal) {
        match self
            .pending_reveals
            .iter_mut()
            .find(|(owner, _)| *owner == id)
        {
            Some(slot) => slot.1 = reveal,
            None => self.pending_reveals.push((id, reveal)),
        }
    }

    pub fn stage_frame_release(&mut self, handle: u64) -> bool {
        let Some((id, frame)) = self.stage_frames.take(handle) else {
            return false;
        };
        let arena = frame.recycle();
        if let Some(slot) = self.bridges.get_mut(&id) {
            slot.spare_arena = Some(arena);
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
        self.poster.fill(Arc::clone(&dispatcher));
        self.effects
            .attach(dispatcher, scheduler, Arc::clone(&self.handlers))
    }

    /// The thread-safe command door — see [`crate::effects::Poster`].
    /// Handed out before attach, usable after: the cell fills when
    /// the host attaches.
    pub fn poster(&self) -> crate::effects::Poster {
        self.poster.clone()
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
        self.ui
            .set(theme.part(::editor::theme::ScrollbarChrome::KEY).style());
        self.stats.set_font(hikit::fonts::ui_text_font(
            &self.ui,
            theme.part(crate::stats_chrome::StatsChrome::KEY).font_size,
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
