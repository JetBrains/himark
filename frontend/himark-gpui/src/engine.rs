// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine handle a gpui shell holds. The ADAPTER adopts a
//! COMPOSED engine — the app layer boots `Himark` however it likes
//! (bare + plugins for an example, the full workbench composition
//! for a shipping shell) and hands it in; this side only wires the
//! host loop into gpui: effects on a worker thread, cross-thread
//! wakes landing as drains on the ui thread, a changed drain
//! repainting every mounted [`crate::BridgeView`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

use futures::StreamExt as _;
use gpui::{App, WeakEntity};
use himark::app::Himark;

use crate::bridge_view::BridgeView;

enum WorkerMessage {
    Run,
    Stop,
}

struct Worker {
    sender: mpsc::Sender<WorkerMessage>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    fn start(worker: himark::host::Worker) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    WorkerMessage::Run => worker.run_pending(),
                    WorkerMessage::Stop => break,
                }
            }
        });
        Self {
            sender,
            handle: Some(handle),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerMessage::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Shared {
    app: RefCell<Himark>,
    views: RefCell<Vec<WeakEntity<BridgeView>>>,
    /// One clock, many views: the first paint of a frame ticks the
    /// engine; siblings painting at the same instant reuse the
    /// answer instead of advancing the clock again.
    last_tick: Cell<(f64, bool)>,
    _worker: Worker,
}

/// ONE per process, cheap to clone — every [`crate::BridgeView`]
/// holds one.
#[derive(Clone)]
pub struct Engine {
    shared: Rc<Shared>,
}

impl Engine {
    /// Adopt a composed engine and wire its wakes into gpui. The
    /// host loop must already be installed (`Himark::install_host_loop`,
    /// which `himark_plugins::install_host` performs).
    pub fn adopt(app: Himark, cx: &mut App) -> Self {
        let worker = Worker::start(app.worker());
        app.set_effect_wake({
            let sender = worker.sender.clone();
            move || {
                let _ = sender.send(WorkerMessage::Run);
            }
        });

        let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
        app.set_wake(move || {
            let _ = wake_tx.unbounded_send(());
        });

        // The boot effects were enqueued before the wakes existed.
        // One kick drains them; without it the shell never sees the
        // agent host.
        let _ = worker.sender.send(WorkerMessage::Run);

        let this = Self {
            shared: Rc::new(Shared {
                app: RefCell::new(app),
                views: RefCell::new(Vec::new()),
                last_tick: Cell::new((f64::NEG_INFINITY, false)),
                _worker: worker,
            }),
        };

        // The drain pump — the Swift shell's `drain` + `repaintAll`,
        // as a foreground task.
        cx.spawn({
            let shared = this.shared.clone();
            async move |cx| {
                while wake_rx.next().await.is_some() {
                    let changed = shared.app.borrow_mut().drain();
                    if !changed {
                        continue;
                    }
                    shared
                        .views
                        .borrow_mut()
                        .retain(|view| view.update(cx, |_, cx| cx.notify()).is_ok());
                }
            }
        })
        .detach();

        this
    }

    /// The composition door: the app layer reaches the engine for
    /// everything that is not this adapter's business — minting
    /// bridges, opening documents, registering commands.
    pub fn with_app<R>(&self, f: impl FnOnce(&mut Himark) -> R) -> R {
        f(&mut self.shared.app.borrow_mut())
    }

    /// One `animation_tick` per frame instant, however many views
    /// paint it.
    pub(crate) fn tick(&self, now_ms: f64) -> bool {
        let (last, animating) = self.shared.last_tick.get();
        if now_ms <= last {
            return animating;
        }
        let animating = self.shared.app.borrow_mut().animation_tick(now_ms);
        self.shared.last_tick.set((now_ms, animating));
        animating
    }

    pub(crate) fn raw(&self) -> &RefCell<Himark> {
        &self.shared.app
    }

    pub(crate) fn attach(&self, view: WeakEntity<BridgeView>) {
        self.shared.views.borrow_mut().push(view);
    }
}
