// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's HOST EDGE (docs/himark-core.md): the inbox the
//! effect landings ride, the wake slots, the drain with its chunk
//! and budget pacing, the owned runtime — and the scroll COALESCER,
//! the per-bridge gesture streams every shell shares. None of this
//! is C marshaling; a Rust shell speaks it directly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gfx::Point;
use imba::anim::AnimationClock;
use imba::event::Event;

use crate::bridge::ViewBridgeId;
use crate::himark::Himark;

type WakeCallback = Box<dyn Fn() + Send + 'static>;

#[derive(Default)]
pub(crate) struct WakeSlot {
    callback: Mutex<Option<WakeCallback>>,
}

impl WakeSlot {
    pub(crate) fn fire(&self) {
        if let Some(wake) = self.callback.lock().expect("wake slot").as_ref() {
            wake();
        }
    }
    fn set(&self, callback: impl Fn() + Send + 'static) {
        *self.callback.lock().expect("wake slot") = Some(Box::new(callback));
    }
}

/// The serial seat that polls the effect futures — handed to the
/// shell's worker thread; `run_pending` must never be re-entered.
#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
#[derive(Clone)]
pub struct Worker {
    shared: Arc<Shared>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl Worker {
    pub fn run_pending(&self) {
        self.shared.runner.run();
    }
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
struct Shared {
    inbox: Arc<Mutex<std::collections::VecDeque<crate::perform::AppCommand>>>,

    wake: Arc<WakeSlot>,

    effect_wake: Arc<WakeSlot>,

    runner: crate::effects::BackgroundRunner,
}

/// The host loop a shell drives: landings queue in the inbox from
/// any thread, `wake` says "you are owed a drain", and `drain` runs
/// them on the UI thread in budgeted chunks.
#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
pub(crate) struct HostLoop {
    shared: Arc<Shared>,

    #[cfg(not(target_arch = "wasm32"))]
    runtime: tokio::runtime::Runtime,

    drain_chunk: usize,
    drain_budget: std::time::Duration,
}

const DRAIN_CHUNK: usize = 32;
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl Himark {
    /// Install the host loop: the inbox, the wake slots and the
    /// owned runtime, wired into the effects road. Idempotent-free —
    /// the composition root calls it once at boot.
    pub fn install_host_loop(&mut self) {
        let inbox: Arc<Mutex<std::collections::VecDeque<crate::perform::AppCommand>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let wake: Arc<WakeSlot> = Arc::default();
        let effect_wake: Arc<WakeSlot> = Arc::default();
        let dispatcher: crate::effects::EffectDispatcher = {
            let inbox = Arc::clone(&inbox);
            let wake = Arc::clone(&wake);
            Arc::new(move |command| {
                inbox.lock().expect("inbox").push_back(command);
                wake.fire();
            })
        };
        let scheduler: Arc<dyn Fn() + Send + Sync> = {
            let effect_wake = Arc::clone(&effect_wake);
            Arc::new(move || effect_wake.fire())
        };
        let runner = self.attach_host(dispatcher, scheduler);
        #[cfg(not(target_arch = "wasm32"))]
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("himark-runtime")
            .enable_all()
            .build()
            .expect("the himark runtime");
        self.host = Some(HostLoop {
            shared: Arc::new(Shared {
                inbox,
                wake,
                effect_wake,
                runner,
            }),
            #[cfg(not(target_arch = "wasm32"))]
            runtime,
            drain_chunk: DRAIN_CHUNK,
            drain_budget: DRAIN_BUDGET,
        });
    }

    fn host(&self) -> &HostLoop {
        self.host.as_ref().expect("the host loop is installed at boot")
    }

    pub fn worker(&self) -> Worker {
        Worker {
            shared: Arc::clone(&self.host().shared),
        }
    }

    pub fn set_wake(&self, callback: impl Fn() + Send + 'static) {
        self.host().shared.wake.set(callback);
    }

    pub fn set_effect_wake(&self, callback: impl Fn() + Send + 'static) {
        self.host().shared.effect_wake.set(callback);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.host().runtime.handle().clone()
    }

    #[doc(hidden)]
    pub fn set_drain_pacing(&mut self, chunk: usize, budget: std::time::Duration) {
        let host = self.host.as_mut().expect("the host loop is installed at boot");
        host.drain_chunk = chunk;
        host.drain_budget = budget;
    }

    #[doc(hidden)]
    pub fn queued_landings(&self) -> usize {
        self.host().shared.inbox.lock().expect("inbox").len()
    }

    /// Run the owed landings on the UI thread, a budgeted chunk at a
    /// time; re-fires the wake when the budget ran out first.
    pub fn drain(&mut self) -> bool {
        let shared = Arc::clone(&self.host().shared);
        let (chunk_size, budget) = {
            let host = self.host();
            (host.drain_chunk.max(1), host.drain_budget)
        };
        let started = std::time::Instant::now();
        let mut needs_redraw = false;
        loop {
            let chunk: Vec<crate::perform::AppCommand> = {
                let mut inbox = shared.inbox.lock().expect("inbox");
                let take = inbox.len().min(chunk_size);
                inbox.drain(..take).collect()
            };
            if chunk.is_empty() {
                break;
            }
            needs_redraw |= self.perform_batch(chunk);
            if started.elapsed() >= budget {
                break;
            }
        }
        if !shared.inbox.lock().expect("inbox").is_empty() {
            shared.wake.fire();
        }
        needs_redraw
    }
}

// ─── the scroll coalescer: per-bridge gesture streams ───

/// One bridge's scroll-gesture stream: the capture cell scroll
/// events ride, the time of the last event for the phaseless pause
/// heuristic, and the deltas accumulated since the last flush.
#[derive(Default)]
pub(crate) struct BridgeScroll {
    /// The gesture CLAIM cell, shared by identity (the surface that
    /// claimed the flick keeps it through momentum) — an `Rc` so a
    /// dispatch can hold it across the engine's own `&mut`.
    gesture: std::rc::Rc<imba::event::ScrollGesture>,
    last: Option<std::time::Instant>,
    pending: Option<PendingScroll>,
}

/// Wheel deltas coalesced between flushes: the batch behind a scroll
/// (frame invalidation, the settle pulse, the focus walk) runs once
/// per display tick, not once per host event. Only same-point deltas
/// merge — a moved point, a gesture boundary or any other input
/// flushes first, so routing stays event-exact.
struct PendingScroll {
    point: Point,
    delta_x: f32,
    delta_y: f32,
    /// The OLDEST unflushed event's start — latency is measured from
    /// the input that has waited longest.
    event_started_at: f64,
}

/// The host's scroll phases, shell-agnostic (the C surface mirrors
/// them as `HIMARK_SCROLL_PHASE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollPhase {
    /// No phase information: a 250 ms pause is the gesture boundary.
    None,
    MayBegin,
    Began,
    Changed,
    Ended,
    Cancelled,
    MomentumBegan,
    MomentumChanged,
    MomentumEnded,
}

impl ScrollPhase {
    fn boundary(self, paused: bool) -> bool {
        match self {
            ScrollPhase::MayBegin | ScrollPhase::Began => true,
            ScrollPhase::None => paused,
            _ => false,
        }
    }

    fn bookkeeping_edge(self) -> bool {
        !matches!(self, ScrollPhase::None | ScrollPhase::Changed)
    }
}

impl Himark {
    /// A scroll carrying the host's gesture phase. The phase decides
    /// where one gesture ends and the next begins, which is what pins
    /// a whole flick — momentum included — to the scroller it started
    /// on, the way native scroll views route.
    pub fn scroll_phased_at_time(
        &mut self,
        id: ViewBridgeId,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        phase: ScrollPhase,
        event_started_at: f64,
    ) -> bool {
        let now = std::time::Instant::now();
        let boundary = {
            let state = self.scroll_states.entry(id).or_default();
            let paused = state
                .last
                .is_none_or(|last| now.duration_since(last).as_millis() > 250);
            state.last = Some(now);
            phase.boundary(paused)
        };
        if boundary {
            // The old gesture's deltas route under ITS claim: flush
            // before the owner resets.
            self.flush_scroll(id);
            self.scroll_states.entry(id).or_default().gesture.begin();
        }
        if delta_x == 0.0 && delta_y == 0.0 && phase.bookkeeping_edge() {
            // A bookkeeping edge (mayBegin, began, ended, momentum
            // edges) with nothing to scroll: the surface under the
            // pointer must not claim the fresh gesture before its
            // direction is known.
            return false;
        }
        let point = Point::new(x, y);
        if self
            .scroll_states
            .get(&id)
            .and_then(|state| state.pending.as_ref())
            .is_some_and(|pending| pending.point != point)
        {
            // The pointer moved mid-stream (a momentum drift over
            // another pane): the standing deltas route from the OLD
            // point, the new ones start fresh.
            self.flush_scroll(id);
        }
        let state = self.scroll_states.entry(id).or_default();
        match &mut state.pending {
            Some(pending) => {
                pending.delta_x += delta_x;
                pending.delta_y += delta_y;
            }
            None => {
                state.pending = Some(PendingScroll {
                    point,
                    delta_x,
                    delta_y,
                    event_started_at,
                })
            }
        }
        true
    }

    /// Dispatch the bridge's accumulated wheel deltas: the per-tick
    /// application of a scroll. Also the ordering flush before any
    /// other input, so a click or keystroke lands on scrolled
    /// content, and before a gesture boundary or a moved point, so
    /// claims resolve exactly as they would have per event.
    pub fn flush_scroll(&mut self, id: ViewBridgeId) -> bool {
        let Some(pending) = self
            .scroll_states
            .get_mut(&id)
            .and_then(|state| state.pending.take())
        else {
            return false;
        };
        // The view's live viewport when its composition tracks one
        // (the workbench window); the last drawn size otherwise.
        let live = self
            .bridges
            .get(&id)
            .and_then(|slot| slot.bridge.view().live_viewport(&self.store))
            .or_else(|| {
                self.bridges
                    .get(&id)
                    .map(|slot| slot.size)
                    .filter(|size| size.width > 0.0)
            });
        let Some(size) = live else {
            // The bridge is gone; its deltas die with it.
            if std::env::var_os("HIMARK_TRACE_SCROLL").is_some() {
                eprintln!("[scroll] flush {}: NO viewport — deltas die", id.raw());
            }
            return false;
        };
        let gesture = std::rc::Rc::clone(&self.scroll_states[&id].gesture);
        let changed = self.dispatch_bridge_timed(
            id,
            Event::Scroll {
                point: pending.point,
                delta_x: pending.delta_x,
                delta_y: pending.delta_y,
                gesture: &gesture,
            },
            Some(size),
            pending.event_started_at,
        );
        if std::env::var_os("HIMARK_TRACE_SCROLL").is_some() {
            eprintln!(
                "[scroll] flush {}: dy {} at {:?} viewport {size:?} -> changed {changed}",
                id.raw(),
                pending.delta_y,
                pending.point
            );
        }
        changed
    }

    /// Every bridge's standing deltas, applied — one batch each,
    /// right before the clock.
    pub fn flush_scrolls(&mut self) -> bool {
        let pending: Vec<ViewBridgeId> = self
            .scroll_states
            .iter()
            .filter(|(_, state)| state.pending.is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut flushed = false;
        for id in pending {
            flushed |= self.flush_scroll(id);
        }
        flushed
    }

    /// One clock tick into every bridge: flush the standing scrolls,
    /// then fan the clock. Answers whether anything still animates.
    pub fn animation_tick(&mut self, now_ms: f64) -> bool {
        let mut animating = self.flush_scrolls();
        static TRACE_ANIM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *TRACE_ANIM.get_or_init(|| std::env::var_os("HIMARK_TRACE_ANIM").is_some());
        for id in self.bridge_ids() {
            let moved = self
                .dispatch_bridge(
                    id,
                    Event::AnimationClock {
                        now: AnimationClock::from_millis(now_ms),
                    },
                    None,
                )
                .0;
            if moved && trace {
                eprintln!("[anim] bridge {} still animating", id.raw());
            }
            animating |= moved;
        }
        animating
    }

    /// A synchronous wheel OFFER to a stage frame: the overlays that
    /// still own their scrolling after the descrollering — a
    /// completion popup's list, a horizontal pan in unwrapped code —
    /// consume it; `false` hands the wheel back to the host's
    /// scroller, which owns the vertical physics on the stage road.
    #[allow(clippy::too_many_arguments)]
    pub fn offer_stage_scroll(
        &mut self,
        frame: u64,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        phase: ScrollPhase,
        viewport: gfx::Rect,
    ) -> bool {
        // Frames are one-pass handles — the gesture stream keys by
        // the frame's ORIGIN bridge, which outlives them all.
        let Some(origin) = self.stage_frame_origin(frame) else {
            return false;
        };
        let now = std::time::Instant::now();
        let state = self.scroll_states.entry(origin).or_default();
        let paused = state
            .last
            .is_none_or(|last| now.duration_since(last).as_millis() > 250);
        state.last = Some(now);
        if phase.boundary(paused) {
            state.gesture.begin();
        }
        let gesture = std::rc::Rc::clone(&state.gesture);
        let result = {
            let Some(stage) = self.stage_frame(frame) else {
                return false;
            };
            stage.handle(
                &Event::Scroll {
                    point: Point::new(x, y),
                    delta_x,
                    delta_y,
                    gesture: &gesture,
                },
                viewport,
            )
        };
        let trace = std::env::var_os("HIMARK_TRACE_OFFER").is_some();
        let imba::event::EventResult::Handled(handled) = result else {
            if trace {
                eprintln!("[offer] ({x},{y}) d({delta_x},{delta_y}) -> IGNORED (host keeps it)");
            }
            return false;
        };
        if trace {
            eprintln!(
                "[offer] ({x},{y}) d({delta_x},{delta_y}) -> consumed: {} commands, reveal? {}",
                handled.commands.len(),
                handled.reveal.is_some()
            );
        }
        if let Some(reveal) = handled.reveal {
            if let Some(id) = self.stage_frame_origin(frame) {
                self.park_reveal(id, reveal);
            }
        }
        if !handled.commands.is_empty() {
            self.perform_batch(handled.commands);
        }
        true
    }
}
