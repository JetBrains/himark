// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine handle a rune shell holds — the gpui `Engine`'s
//! sibling: adopts a COMPOSED `Himark`, runs effects on a worker
//! thread, and lands cross-thread wakes through rune's `Waker`
//! (the app's `AppSpec::setup` hands it in; `poll` drains).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

use himark::app::Himark;

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
    /// One clock, many paints — the first of a frame instant ticks.
    last_tick: Cell<(f64, bool)>,
    started_at: std::time::Instant,
    _worker: Worker,
}

/// ONE per process, cheap to clone.
#[derive(Clone)]
pub struct Engine {
    shared: Rc<Shared>,
}

impl Engine {
    /// Adopt a composed engine. The host loop must already be
    /// installed (`Himark::install_host_loop`, which
    /// `himark_plugins::install_host` performs). Hand rune's
    /// `Waker` in from `AppSpec::setup` via [`Engine::set_waker`];
    /// drain from `AppSpec::poll` via [`Engine::drain`].
    pub fn adopt(app: Himark) -> Self {
        let worker = Worker::start(app.worker());
        app.set_effect_wake({
            let sender = worker.sender.clone();
            move || {
                let _ = sender.send(WorkerMessage::Run);
            }
        });
        // The boot effects were enqueued before the wakes existed —
        // one kick drains them.
        let _ = worker.sender.send(WorkerMessage::Run);
        Self {
            shared: Rc::new(Shared {
                app: RefCell::new(app),
                last_tick: Cell::new((f64::NEG_INFINITY, false)),
                started_at: std::time::Instant::now(),
                _worker: worker,
            }),
        }
    }

    /// Wire the engine's cross-thread wake into rune's event loop.
    pub fn set_waker(&self, waker: rune::Waker) {
        self.shared.app.borrow().set_wake(move || waker());
    }

    /// Run the owed landings — `AppSpec::poll`'s body. Answers
    /// whether anything changed (rune redraws on true).
    pub fn drain(&self) -> bool {
        self.shared.app.borrow_mut().drain()
    }

    /// The composition door — minting bridges, opening documents,
    /// registering commands.
    pub fn with_app<R>(&self, f: impl FnOnce(&mut Himark) -> R) -> R {
        f(&mut self.shared.app.borrow_mut())
    }

    /// Seconds since adoption — the event timestamps' clock.
    pub(crate) fn now(&self) -> f64 {
        self.shared.started_at.elapsed().as_secs_f64()
    }

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
}
