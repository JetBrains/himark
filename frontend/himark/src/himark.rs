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
        gfx::Rect,
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
    pub(crate) size: gfx::Size,

    /// The STAGE road's shadow: the band the host last realized.
    /// A stage client never fills `frame`, but the IME fold needs
    /// one — `mounted_frame` rebuilds at `size` and realizes this
    /// band, so the folded geometry matches what is on screen.
    pub(crate) stage_viewport: Option<gfx::Rect>,

    /// One warm arena from the last recycled frame.
    pub(crate) spare_arena: Option<Box<imba::arena::Arena>>,

    /// The C shell's two-step draw: `stage_layout` keeps the measured
    /// layout here, `stage_realize` consumes it into `staged_frame`,
    /// paint/events read it, release recycles it. At most one of each
    /// per view — the shell's calls are strictly sequential, so there
    /// is nothing for an id to tell apart.
    pub(crate) staged_layout: Option<crate::bridge::BridgeLayout>,
    pub(crate) staged_frame: Option<crate::bridge::Frame>,
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
            size: gfx::Size::new(-1.0, -1.0),
            stage_viewport: None,
            spare_arena: None,
            staged_layout: None,
            staged_frame: None,
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

    /// The registered BATCH LANES — reactions that run after every
    /// batch's queue drains, store in hand (the AHP session syncs
    /// register here; an engine with no plugin runs none). The lane
    /// reads `all_quiet` itself: a quiet batch certified it made no
    /// landings or edits, and each lane decides what still runs.
    batch_lanes: Vec<BatchLane>,

    /// The registered SWEEP hooks — the empty-entity housekeeping
    /// `sweep` runs beside its own frame invalidation.
    sweep_hooks: Vec<Arc<dyn Fn(&mut Store)>>,

    /// The registered UNMOUNT hooks — per-bridge bookkeeping a
    /// feature half forgets when a slot dies (the AHP plugin drops
    /// the mount's session note).
    unmount_hooks: Vec<Arc<dyn Fn(&mut Store, ViewBridgeId)>>,

    /// The stage table (docs/ui/imba-ffi.md): every layout and
    /// frame a host holds by handle, one handle space. Engine-owned
    /// mutating it.

    /// Mount ids for non-window views, minted from the high half —
    /// window bridges keep their window's raw id in the low half.
    next_mount: u64,

    pub(crate) pending_file_events: Vec<documents::watch::Subscription>,
    pub(crate) pending_watch_deaths: Vec<documents::watch::Subscription>,

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

    pub(crate) bridge_place_arms: Vec<crate::mounts::BridgePlaceArm>,

    /// A perform raised the settle bit (`Effects::settle`): run the
    /// synchronous `Event::Settle` pulse before the next paint so
    /// viewport corrections land in the SAME frame
    /// (docs/editor/viewport-preservation.md §3.2).
    pub(crate) settle_requested: bool,
    pub(crate) settling: bool,

    /// The per-bridge scroll-gesture streams (`crate::host`), ONE
    /// per origin whichever road the wheel arrives on (the
    /// dispatch coalescer, the stage road's offer): a touch
    /// beginning on one bridge must not break the gesture —
    /// momentum included — still flowing on another.
    pub(crate) scroll_states:
        std::collections::HashMap<ViewBridgeId, crate::host::BridgeScroll>,

    /// The installed host loop (`install_host_loop`): inbox, wakes,
    /// the owned runtime, drain pacing.
    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    pub(crate) host: Option<crate::host::HostLoop>,

    /// The registered batch TAILS: each frame-composition layer's
    /// reaction to a finished batch (the workbench reaps dead
    /// window slots, validates panes, follows the dock tree). They
    /// run after the engine's own fan-outs; registration order.
    tails: Vec<Box<dyn BatchTail>>,
}

/// One registered batch LANE — see [`Himark::register_batch_lane`].
/// Runs after every batch's queue drains, before the commit:
/// `(store, ui, all_quiet, fx)`.
pub type BatchLane =
    Arc<dyn Fn(&mut Store, &std::rc::Rc<UiCtx>, bool, &mut crate::perform::AppFx<'_>)>;

/// One registered batch-tail — see [`Himark::register_batch_tail`].
pub trait BatchTail: 'static {
    fn run(&mut self, app: &mut Himark, label: &str, theme_changed: bool);
}

impl<F: FnMut(&mut Himark, &str, bool) + 'static> BatchTail for F {
    fn run(&mut self, app: &mut Himark, label: &str, theme_changed: bool) {
        self(app, label, theme_changed)
    }
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
        // A platform without system fonts (the web) must still
        // answer `default_typeface` — the embedded face stands in.
        // On platforms with system fonts the install is never read.
        gfx::font::install_default_typeface(::editor::embedded_fonts::typeface());
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

        let workshop = Arc::new(::editor::env::Workshop::new(
            fonts.world(),
            ::editor::env::Themes::of(&store),
        ));
        let handlers = Arc::new(crate::effects::Handlers::default());
        crate::effects::register_builtins(&handlers, &workshop);
        store.put(ShellSeat(shell));
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
            next_mount: crate::bridge::MOUNT_BASE,
            revision: 0,
            frames_built: 0,
            pending_file_events: Vec::new(),
            pending_watch_deaths: Vec::new(),
            pending_reveals: Vec::new(),
            palette: None,
            bridge_place_arms: Vec::new(),
            settle_requested: false,
            settling: false,
            scroll_states: std::collections::HashMap::new(),
            #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
            host: None,
            tails: Vec::new(),
            batch_lanes: Vec::new(),
            sweep_hooks: Vec::new(),
            unmount_hooks: Vec::new(),
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

    /// The empty-entity housekeeping, run after every batch — the
    /// one thing the old gather/scatter rite still did. The hooks
    /// are registered (the AHP plugin sweeps its empty sessions).
    pub fn sweep(&mut self) {
        let hooks = self.sweep_hooks.clone();
        for hook in &hooks {
            hook(&mut self.store);
        }
        self.invalidate_frames();
    }

    /// The one `&mut store` door outside the batch loop: mutate,
    /// then outdate every standing frame.
    pub fn setup(&mut self, mutate: impl FnOnce(&mut Store)) {
        mutate(&mut self.store);
        self.invalidate_frames();
    }

    /// `setup` with an answer — the composition layers' door when a
    /// boot road mints ids it needs back.
    pub fn with_store<R>(&mut self, mutate: impl FnOnce(&mut Store) -> R) -> R {
        let answer = mutate(&mut self.store);
        self.invalidate_frames();
        answer
    }

    /// The store moved on: every standing frame is now stale. The
    /// bump is the whole invalidation — each bridge rebuilds
    /// lazily, against the store as it stands when next asked.
    pub fn invalidate_frames(&mut self) {
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
            pending_watch_deaths: &mut self.pending_watch_deaths,
            workshop: &self.workshop,
            bridges: &mut self.bridges,
        };
        let mut queue: std::collections::VecDeque<AppCommand> = commands.into();
        // EVERY command must declare itself display-neutral
        // (`fx.quiet()`) for the batch to commit quietly — one
        // unmarked arm and the frames invalidate as always.
        let mut all_quiet = true;
        while let Some(command) = queue.pop_front() {
            let slow_probe = std::env::var_os("HIMARK_TRACE_BATCH").map(|_| {
                (
                    crate::perform::command_label(&command),
                    std::time::Instant::now(),
                )
            });
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

        // The registered BATCH LANES (the AHP session syncs among
        // them): they react to landings and edits; each lane reads
        // `all_quiet` — a quiet batch certified it made none — and
        // decides what it still owes (housekeeping like enrich's
        // drop-unclaimed runs regardless).
        {
            let lanes = self.batch_lanes.clone();
            let mut fx = batch.effects();
            for lane in &lanes {
                lane(&mut store, &ui, all_quiet, &mut fx);
            }
        }
        // The safety net for a tail lane's follow-up: the queue loop
        // is over, so perform them here — late but never lost.
        let mut performer = crate::perform::Performer {
            pending_file_events: &mut self.pending_file_events,
            pending_watch_deaths: &mut self.pending_watch_deaths,
            workshop: &self.workshop,
            bridges: &mut self.bridges,
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
            self.run_tails(&label, theme_changed);
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


    /// A MOUNTED slot's current frame at its size — rebuilt
    /// lazily off the revision, exactly the window client's memo
    /// but for views the host composes directly. `size` sets a
    /// new layout size (the draw road); `None` reuses the last
    /// drawn one (the event road between paints).
    pub(crate) fn mounted_frame(
        &mut self,
        id: ViewBridgeId,
        size: Option<gfx::Size>,
    ) -> Option<(&mut crate::bridge::Frame, gfx::Size)> {
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
    ) -> Option<(&mut crate::bridge::Frame, gfx::Size)> {
        self.mounted_frame_inner(id, None, true)
    }

    fn mounted_frame_inner(
        &mut self,
        id: ViewBridgeId,
        size: Option<gfx::Size>,
        standing: bool,
    ) -> Option<(&mut crate::bridge::Frame, gfx::Size)> {
        let revision = self.revision;
        // A popover slot's layout runs at the PARENT'S standing
        // size, against a freshly pinned parent; its own `size`
        // stays the content-shaped cache key.
        let host_size = self.overlay_host_size(id);
        let pin = self.overlay_parent_pin(id);
        let slot = self.bridges.get_mut(&id)?;
        if let Some((parent, band)) = pin {
            if let Some(mount) = slot
                .bridge
                .view()
                .as_any()
                .and_then(|any| any.downcast_ref::<crate::mounts::OverlayMount>())
            {
                mount.pin_parent(parent, band);
            }
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
            self.frames_built += 1;
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
                .unwrap_or_else(|| gfx::Rect::from_size(size));
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

    /// One timed event into a bridge — the shell entry point: the
    /// dispatch, and the latency stamp when it answered.
    pub fn dispatch_bridge_timed(
        &mut self,
        id: ViewBridgeId,
        event: imba::event::Event<'_>,
        size: Option<gfx::Size>,
        event_started_at: f64,
    ) -> bool {
        let handled = self.dispatch_bridge(id, event, size).0;
        if handled {
            self.stats.mark_latency_event_start(event_started_at);
        }
        handled
    }

    /// Dispatch, and beside the redraw flag the shape the pointer
    /// takes: a move hit-tests first and the hit's answer IS the
    /// shape — derived per event; the shell applies it and nothing
    /// stores it.
    pub fn dispatch_bridge(
        &mut self,
        id: ViewBridgeId,
        event: imba::event::Event<'_>,
        size: Option<gfx::Size>,
    ) -> (bool, imba::event::PointerShape) {
        let (hit, shape) = match &event {
            imba::event::Event::MouseMove { point, mods } => self.dispatch_bridge_event(
                id,
                imba::event::Event::HitTest {
                    point: *point,
                    miss: false,
                    mods: *mods,
                },
                size,
            ),
            _ => (false, imba::event::PointerShape::Default),
        };
        let (changed, _) = self.dispatch_bridge_event(id, event, size);
        (changed || hit, shape)
    }

    fn dispatch_bridge_event(
        &mut self,
        id: ViewBridgeId,
        event: imba::event::Event<'_>,
        size: Option<gfx::Size>,
    ) -> (bool, imba::event::PointerShape) {
        use imba::event::{Event, EventResult};
        let clock = matches!(event, Event::AnimationClock { .. });
        let key_down = match event {
            Event::KeyDown { key, mods } => Some((key, mods)),
            _ => None,
        };
        let store = self.store.clone();
        let (result, fallback) = match &event {
            // Keyboard input routes through the SEMANTIC focus chain
            // — a state walk over the views. No tree is built for a
            // keystroke.
            Event::KeyDown { .. } | Event::TextInput { .. } => {
                let ui = self.ui.clone();
                let result = {
                    // The chain is the BRIDGE's view's walk — the
                    // workbench root walks its window, a mounted
                    // view walks itself.
                    let chain = self
                        .bridges
                        .get(&id)
                        .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()));
                    match (&event, chain) {
                        (Event::KeyDown { key, mods }, Some(mut data)) => data.key(*key, *mods),
                        (Event::TextInput { text }, Some(mut data)) => data.text(text),
                        _ => EventResult::Ignored,
                    }
                };
                if std::env::var_os("HIMARK_TRACE_KEYS").is_some() {
                    if let Some((key, mods)) = key_down {
                        eprintln!(
                            "[keys] {key:?} {mods:?}: chain says {}; binding {:?}",
                            match &result {
                                EventResult::Ignored => "ignored",
                                EventResult::Handled(handled) => {
                                    match (handled.commands.is_empty(), handled.reveal.is_some()) {
                                        (true, true) => "reveal",
                                        (true, false) => "handled",
                                        (false, _) => "command",
                                    }
                                }
                            },
                            crate::keymap::Keymaps::binding_of(&store, key, mods)
                        );
                    }
                }
                let empty_handed = match &result {
                    EventResult::Ignored => true,
                    EventResult::Handled(handled) => handled.commands.is_empty(),
                };
                let fallback = match (key_down, empty_handed) {
                    (Some((key, mods)), true) => {
                        crate::keymap::Keymaps::binding_of(&store, key, mods).and_then(|binding| {
                            // The chain source mirrors the dispatch
                            // above; bound EDITING keys (backspace,
                            // enter, arrows) resolve through
                            // presentables either way.
                            let chain = self
                                .bridges
                                .get(&id)
                                .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))
                                .map(|data| crate::focus::bind_mounted(&store, id, data));
                            let bound = chain.as_ref().map(|data| data.bindings.clone());
                            chain
                                .map(|data| data.commands)
                                .unwrap_or_default()
                                .into_iter()
                                .find(|presentable| presentable.id == binding.as_ref())
                                .map(|presentable| presentable.command)
                                .or_else(|| {
                                    // The registry resolves on EVERY
                                    // shell: the walk's bindings ride
                                    // the mint; a chain-less origin
                                    // resolves at dispatch instead.
                                    crate::commands::Commands::of(&store)
                                        .find(binding.as_ref())
                                        .map(|command| match &bound {
                                            Some(bound) => crate::perform::AppCommand::Bound(
                                                bound.clone(),
                                                command.clone(),
                                            ),
                                            None => crate::perform::AppCommand::AtOrigin(
                                                id,
                                                command.clone(),
                                            ),
                                        })
                                })
                        })
                    }
                    _ => None,
                };
                (result, fallback)
            }
            _ => (self.handle_mounted(id, &event, size), None),
        };

        if let Some(command) = fallback {
            if std::env::var_os("HIMARK_TRACE_KEYS").is_some() {
                eprintln!("[keys] fallback performs the bound command");
            }
            return (
                self.perform_batch(vec![command]),
                imba::event::PointerShape::Default,
            );
        }

        match result {
            EventResult::Ignored => (false, imba::event::PointerShape::Default),
            EventResult::Handled(handled) => {
                let shape = handled.pointer.unwrap_or_default();
                // A mounted view's reveal is the HOST's to scroll —
                // park it for `take_reveal`; a window root's dies
                // here as it always did (its scroller consumes en
                // route).
                let mut parked = false;
                if let Some(reveal) = handled.reveal {
                    if id.is_mounted() {
                        if std::env::var_os("HIMARK_TRACE_REVEAL").is_some() {
                            let slot = self.bridges.get(&id);
                            eprintln!(
                                "[reveal] parked (mounted dispatch) bridge {} {:?} band {:?} size {:?}",
                                id.raw(),
                                reveal,
                                slot.and_then(|slot| slot.stage_viewport),
                                slot.map(|slot| slot.size),
                            );
                        }
                        self.park_reveal(id, reveal);
                        parked = true;
                    }
                }
                let changed = match handled.commands.is_empty() {
                    // An empty answer on a clock tick is NOT a
                    // change — but a parked reveal is.
                    true => parked || !clock,
                    false => {
                        if clock {
                            crate::app::trace_reconcile("clock", &handled.commands);
                        }
                        self.perform_batch(handled.commands) || parked
                    }
                };
                (changed, shape)
            }
        }
    }

    /// One event into a bridge, against its standing frame. `size`
    /// is the host's live viewport when it knows one (the window
    /// road's dispatch passes it; geometry events rebuild at it);
    /// `None` rides the last drawn size.
    pub(crate) fn handle_mounted(
        &mut self,
        id: ViewBridgeId,
        event: &imba::event::Event<'_>,
        size: Option<gfx::Size>,
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
            false => self.mounted_frame(id, size),
        };
        let Some((frame, size)) = frame_and_size else {
            if trace {
                eprintln!("[mount] {id:?}: event against no frame");
            }
            return imba::event::EventResult::Ignored;
        };
        let viewport = band.unwrap_or_else(|| gfx::Rect::from_size(size));
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

    /// Paint a bridge into the host's canvas at the host's size —
    /// one road for windows and mounts alike.
    pub fn draw_mounted(
        &mut self,
        id: ViewBridgeId,
        canvas: &gfx::Canvas<'_>,
        size: gfx::Size,
    ) -> bool {
        // TRANSPARENT floor, like the stage road: the HOST behind
        // a mounted view owns the background (a glass island, the
        // document floor's color). A window root paints its own
        // ground and declines the clear (`clears_backdrop`).
        let clears = self
            .bridges
            .get(&id)
            .is_none_or(|slot| slot.bridge.view().clears_backdrop());
        if clears {
            canvas.clear(gfx::Color::TRANSPARENT);
        }
        let result = {
            let Some((frame, size)) = self.mounted_frame(id, Some(size)) else {
                return false;
            };
            frame.handle(
                &imba::event::Event::Paint {
                    canvas,
                    focused: true,
                },
                gfx::Rect::from_size(size),
            )
        };
        let imba::event::EventResult::Handled(handled) = result else {
            return false;
        };
        // A mounted view's reveal is the HOST's to scroll — park it.
        // A window root's scroller consumed reveals en route; one
        // that reached the top dies as it always did.
        let parked = match handled.reveal.filter(|_| id.is_mounted()) {
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

    /// Every bridge id, windows-as-bridges included — the fan-outs
    /// below iterate this; ids are collected first because a
    /// handled command's batch may mount or unmount mid-loop.
    pub(crate) fn bridge_ids(&self) -> Vec<ViewBridgeId> {
        self.bridges.keys().copied().collect()
    }

    /// Run one handled answer from a fan-out dispatch: park a
    /// mounted reveal, perform the commands.
    fn run_fanned(
        &mut self,
        id: ViewBridgeId,
        result: imba::event::EventResult<crate::perform::AppCommand>,
    ) {
        let imba::event::EventResult::Handled(handled) = result else {
            return;
        };
        if let Some(reveal) = handled.reveal.filter(|_| id.is_mounted()) {
            self.park_reveal(id, reveal);
        }
        if !handled.commands.is_empty() {
            self.perform_batch(handled.commands);
        }
    }

    /// The theme fan-out: refresh the engine-owned chrome, then
    /// tell EVERY bridge — windows and mounts alike (the mounted
    /// bridges used to be deaf to this; docs/himark-core.md calls
    /// for each frame holder reacting itself).
    pub(crate) fn fan_theme_changed(&mut self) {
        let theme = ::editor::env::Themes::of(&self.store);
        self.refresh_chrome(&theme);
        self.workshop.set_theme(theme);
        for id in self.bridge_ids() {
            let result = self.handle_mounted(id, &imba::event::Event::ThemeChanged, None);
            self.run_fanned(id, result);
        }
    }

    /// The file-event fan-out, to every bridge.
    pub(crate) fn fan_file_events(&mut self, event: &documents::watch::FilesChanged) {
        for id in self.bridge_ids() {
            let result = self.handle_mounted(id, &imba::event::Event::UserEvent(event), None);
            self.run_fanned(id, result);
        }
    }

    /// The settle loop, at the END of the bit-raising batch — not
    /// deferred to paint. The ordering is what buys exactness: a
    /// scroll batch pulses BEFORE any later batch performs, so a
    /// door never anchors on a top older than the last move; a
    /// door batch pulses before its frame, so no wrong frame
    /// paints (docs/editor/viewport-preservation.md §3.2). Bounded; the
    /// guard keeps the pulse's own performs from recursing.
    pub(crate) fn settle_rounds(&mut self) {
        if self.settling {
            return;
        }
        self.settling = true;
        let mut rounds = 0;
        while self.settle_requested && rounds < 3 {
            self.settle_requested = false;
            for id in self.bridge_ids() {
                let result = self.handle_mounted(id, &imba::event::Event::Settle, None);
                self.run_fanned(id, result);
            }
            rounds += 1;
        }
        self.settling = false;
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
            gfx::Rect,
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
    pub fn overlay_content(&self, id: ViewBridgeId) -> Option<gfx::Rect> {
        self.bridges
            .get(&id)?
            .bridge
            .view()
            .as_any()?
            .downcast_ref::<crate::mounts::OverlayMount>()?
            .content_rect()
    }

    /// The parent view a popover slot pins before laying out, with
    /// the parent's standing band — the slice the re-derivation
    /// realizes, never the whole content.
    fn overlay_parent_pin(
        &self,
        id: ViewBridgeId,
    ) -> Option<(
        std::sync::Arc<dyn crate::bridge::BridgeView>,
        Option<gfx::Rect>,
    )> {
        let parent = self.bridges.get(&id)?.overlay_parent?;
        let slot = self.bridges.get(&parent)?;
        Some((slot.bridge.view_rc(), slot.stage_viewport))
    }

    /// A popover slot lays out at its PARENT'S standing size — the
    /// popover window is content-sized, and tight constraints at
    /// that size would collapse the re-derivation inside.
    fn overlay_host_size(&self, id: ViewBridgeId) -> Option<gfx::Size> {
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


    /// Perform a REGISTERED command at an origin bridge — the C
    /// door's and the palette's shared road: the origin view's own
    /// walk answers first (its presentables carry their bindings),
    /// the global registry rides behind it over `AtOrigin`.
    pub fn perform_registered_at(&mut self, origin: ViewBridgeId, id: &str) -> bool {
        let ui = self.ui.clone();
        let store = self.store.clone();
        let command = {
            let chain = self
                .bridges
                .get(&origin)
                .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))
                .map(|data| crate::focus::bind_mounted(&store, origin, data));
            let bound = chain.as_ref().map(|data| data.bindings.clone());
            chain
                .map(|data| data.commands)
                .unwrap_or_default()
                .into_iter()
                .find(|presentable| presentable.id == id)
                .map(|presentable| presentable.command)
                .or_else(|| {
                    crate::commands::Commands::of(&store).find(id).cloned().map(
                        |command| match bound {
                            Some(bound) => crate::perform::AppCommand::Bound(bound, command),
                            None => crate::perform::AppCommand::AtOrigin(origin, command),
                        },
                    )
                })
        };
        match command {
            Some(command) => self.perform_batch(vec![command]),
            None => {
                eprintln!("[himark] no command registered as '{id}' — dropped");
                false
            }
        }
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
    /// Register a place arm — a plugin's answer to a place, any
    /// mount shape (see `mounts::BridgePlaceArm`): the arm mints
    /// through `Himark::mount` itself.
    pub fn register_bridge_arm(&mut self, arm: crate::mounts::BridgePlaceArm) {
        self.bridge_place_arms.push(arm);
    }

    pub fn palette_factory(&self) -> Option<crate::mounts::PaletteFactory> {
        self.palette.clone()
    }

    /// Register a batch TAIL — a frame-composition layer's reaction
    /// to a finished batch (the workbench's: reap dead window
    /// slots, validate panes, follow the dock tree). Tails run
    /// after the engine's own fan-outs, in registration order; a
    /// batch a tail performs skips the tails (the outer pass owns
    /// them).
    pub fn register_batch_tail(&mut self, tail: impl BatchTail) {
        self.tails.push(Box::new(tail));
    }

    /// Register a batch LANE — a reaction that runs after every
    /// batch's queue drains, store in hand (the AHP session syncs:
    /// comments, diagnostics, changes, the diff/dressing sweeps).
    /// Lanes run in registration order; each reads `all_quiet` and
    /// decides what a certified-quiet batch still owes it.
    pub fn register_batch_lane(&mut self, lane: BatchLane) {
        self.batch_lanes.push(lane);
    }

    /// Register a SWEEP hook — empty-entity housekeeping run by
    /// `sweep` after every non-quiet batch.
    pub fn register_sweep_hook(&mut self, hook: Arc<dyn Fn(&mut Store)>) {
        self.sweep_hooks.push(hook);
    }

    /// Register an UNMOUNT hook — run when a bridge slot dies.
    pub fn register_unmount_hook(&mut self, hook: Arc<dyn Fn(&mut Store, ViewBridgeId)>) {
        self.unmount_hooks.push(hook);
    }

    /// One bridge's offered commands — its view's own walk,
    /// commands only (the rest of the focus data borrows the
    /// walk's store, which dies with this call). The session
    /// plugin concatenates these across a workspace's mounts.
    pub fn bridge_commands(
        &self,
        id: ViewBridgeId,
    ) -> Vec<imba::PresentableCommand<crate::app::AppCommand>> {
        let Some(slot) = self.bridges.get(&id) else {
            return Vec::new();
        };
        let view = slot.bridge.view();
        let store = view.frame_store(&self.store, self.ui.as_ref());
        view.focus(&store, self.ui.as_ref())
            .map(|data| data.commands)
            .unwrap_or_default()
    }

    /// The engine's own batch tail: the theme, file-event and
    /// settle fan-outs over every bridge, then the registered
    /// tails.
    fn run_tails(&mut self, label: &str, theme_changed: bool) {
        if theme_changed {
            self.fan_theme_changed();
        }
        let changed: std::collections::HashSet<documents::watch::Subscription> =
            std::mem::take(&mut self.pending_file_events)
                .into_iter()
                .collect();
        if !changed.is_empty() {
            let event = documents::watch::FilesChanged(std::sync::Arc::new(changed));
            self.fan_file_events(&event);
        }
        let died: std::collections::HashSet<documents::watch::Subscription> =
            std::mem::take(&mut self.pending_watch_deaths)
                .into_iter()
                .collect();
        if !died.is_empty() {
            let event = documents::watch::WatchesDied(std::sync::Arc::new(died));
            for id in self.bridge_ids() {
                let result = self.handle_mounted(id, &imba::event::Event::UserEvent(&event), None);
                self.run_fanned(id, result);
            }
        }
        self.settle_rounds();
        // Taken out while they run: a batch a tail performs finds
        // the seat empty and skips — the OUTER pass owns the tails.
        let mut tails = std::mem::take(&mut self.tails);
        for tail in &mut tails {
            tail.run(self, label, theme_changed);
        }
        let late = std::mem::replace(&mut self.tails, tails);
        self.tails.extend(late);
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

    /// A mounted slot's last drawn size — the event roads'
    /// viewport stand-in where a window would answer with its own
    /// (the mounted dispatch arm routes against the standing frame
    /// and ignores the passed size; this keeps those roads from
    /// treating "no window viewport" as "gone").
    pub fn mounted_viewport(&self, id: ViewBridgeId) -> Option<gfx::Size> {
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

    /// Mount a view AT a caller-chosen id, idempotently — the
    /// window-pun compat door: the workbench adapter keys a
    /// window's bridge by the window's own raw id, so the C
    /// surface's handles stay stable. New composition layers use
    /// [`Himark::mount`]; this door exists for the adapter alone.
    pub fn mount_as(&mut self, id: ViewBridgeId, view: std::sync::Arc<dyn crate::bridge::BridgeView>) {
        self.bridges.entry(id).or_insert_with(|| Mounted::new(view));
    }

    /// Whether a bridge stands at this id.
    pub fn has_bridge(&self, id: ViewBridgeId) -> bool {
        self.bridges.contains_key(&id)
    }

    /// Drop every bridge slot the keeper declines — the
    /// composition layer's REAP (the workbench drops slots whose
    /// window closed this batch; mounted slots are host-owned and
    /// the keeper must spare them).
    pub fn retain_bridges(&mut self, keep: impl Fn(ViewBridgeId) -> bool) {
        self.bridges.retain(|id, _| keep(*id));
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

    /// Unmount: the slot and everything staged on it go together.
    pub fn unmount(&mut self, id: ViewBridgeId) -> bool {
        let Some(mut slot) = self.bridges.remove(&id) else {
            return false;
        };
        crate::bridge::quiesce(&mut slot);
        let hooks = self.unmount_hooks.clone();
        for hook in &hooks {
            hook(&mut self.store, id);
        }
        true
    }

    // ── the staged two-step draw (the C surface delegates here).
    // The shell cannot hold the Rust objects across the ABI, so the
    // engine keeps them on the slot: `staged_layout` from
    // `stage_layout`, consumed by `stage_realize` into
    // `staged_frame`, read by paint/events, recycled by release.
    // At most one of each per view — the shell's calls are strictly
    // sequential and never span a call site.

    /// Run layout at `size` and keep the result as the view's
    /// staged layout (replacing any previous one). False when no
    /// such view is mounted.
    pub fn stage_layout(&mut self, id: ViewBridgeId, size: gfx::Size) -> bool {
        // A popover slot ignores the shell's (content-sized) ask
        // and lays out at the parent's standing size, against a
        // freshly pinned parent.
        let size = self.overlay_host_size(id).unwrap_or(size);
        let pin = self.overlay_parent_pin(id);
        let Some(slot) = self.bridges.get_mut(&id) else {
            return false;
        };
        if let Some((parent, band)) = pin {
            if let Some(mount) = slot
                .bridge
                .view()
                .as_any()
                .and_then(|any| any.downcast_ref::<crate::mounts::OverlayMount>())
            {
                mount.pin_parent(parent, band);
            }
        }
        let arena = slot.spare_arena.take().unwrap_or_default();
        // An INFINITE height is the native scroller asking to
        // measure: width pinned, height the content's own — the
        // sized stage's `.size()` becomes the scroller's content
        // size. A finite size stays the window-shaped tight fit.
        let constraints = if size.height.is_infinite() {
            imba::constraints::Constraints {
                min: gfx::Size::new(size.width, 0.0),
                max: size,
            }
        } else {
            imba::constraints::Constraints::tight(size)
        };
        let staged = slot
            .bridge
            .layout(&self.store, &self.ui, constraints, arena);
        if id.is_mounted() {
            // The measured size becomes the slot's, so the IME fold
            // (`mounted_frame`) rebuilds the same layout the shell
            // shows.
            slot.size = staged.size();
        }
        slot.staged_layout = Some(staged);
        true
    }

    pub fn stage_layout_size(&self, id: ViewBridgeId) -> Option<gfx::Size> {
        self.bridges
            .get(&id)?
            .staged_layout
            .as_ref()
            .map(crate::bridge::BridgeLayout::size)
    }

    /// Consume the staged layout into the staged frame, realized
    /// for `viewport`. False when no layout is staged.
    pub fn stage_realize(&mut self, id: ViewBridgeId, viewport: gfx::Rect) -> bool {
        let Some(slot) = self.bridges.get_mut(&id) else {
            return false;
        };
        let Some(staged) = slot.staged_layout.take() else {
            return false;
        };
        let mut frame = staged.realize(viewport);
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
        slot.staged_frame = Some(frame);
        true
    }

    pub fn stage_layout_release(&mut self, id: ViewBridgeId) -> bool {
        self.bridges
            .get_mut(&id)
            .is_some_and(|slot| slot.staged_layout.take().is_some())
    }

    pub fn stage_frame(&self, id: ViewBridgeId) -> Option<&crate::bridge::Frame> {
        self.bridges.get(&id)?.staged_frame.as_ref()
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

    pub fn stage_frame_release(&mut self, id: ViewBridgeId) -> bool {
        let Some(slot) = self.bridges.get_mut(&id) else {
            return false;
        };
        let Some(frame) = slot.staged_frame.take() else {
            return false;
        };
        slot.spare_arena = Some(frame.recycle());
        true
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
