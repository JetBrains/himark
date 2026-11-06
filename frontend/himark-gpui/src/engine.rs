// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine handle a gpui shell holds — the `HimarkEngine.swift`
//! twin. Boots the production engine the way every shell does,
//! runs effects on a worker thread, and lands cross-thread wakes
//! as drains on the ui thread; a changed drain repaints every
//! mounted [`crate::BridgeView`].

use workbench_shell::capi::WindowEngine as _;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use futures::StreamExt as _;
use gpui::{App, WeakEntity};

use crate::bridge_view::BridgeView;

/// A workbench window minted from the engine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window(pub(crate) u64);

/// A mountable view the engine serves — what a [`crate::BridgeView`]
/// hosts. The window's root composition is one of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bridge(pub(crate) u64);

enum WorkerMessage {
    Run,
    Stop,
}

struct Worker {
    sender: mpsc::Sender<WorkerMessage>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    fn start(worker: himark_api::HimarkWorker) -> Self {
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
    engine: RefCell<himark_api::HimarkEngine>,
    views: RefCell<Vec<WeakEntity<BridgeView>>>,
    _worker: Worker,
}

/// ONE per process, cheap to clone — every [`crate::BridgeView`]
/// holds one.
#[derive(Clone)]
pub struct Engine {
    shared: Rc<Shared>,
}

impl Engine {
    /// Boot the production engine and wire its wakes into gpui.
    pub fn boot(cx: &mut App) -> Self {
        let mut engine = himark_api::production_engine(None);

        let worker = Worker::start(engine.worker());
        engine.set_effect_wake({
            let sender = worker.sender.clone();
            move || {
                let _ = sender.send(WorkerMessage::Run);
            }
        });

        let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
        engine.set_wake(move || {
            let _ = wake_tx.unbounded_send(());
        });

        // The boot effects — the agent-host dial among them — were
        // enqueued before the wakes existed. One kick drains them;
        // without it the shell never sees the agent host.
        let _ = worker.sender.send(WorkerMessage::Run);

        let this = Self {
            shared: Rc::new(Shared {
                engine: RefCell::new(engine),
                views: RefCell::new(Vec::new()),
                _worker: worker,
            }),
        };

        // The drain pump — the Swift shell's `drain` +
        // `repaintAll`, as a foreground task.
        cx.spawn({
            let shared = this.shared.clone();
            async move |cx| {
                while wake_rx.next().await.is_some() {
                    let changed = shared.engine.borrow_mut().drain();
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

    pub fn add_window(&self) -> Window {
        Window(self.shared.engine.borrow_mut().add_window())
    }

    /// The workbench window as a bridge — the degenerate case the
    /// root view mounts.
    pub fn window_bridge(&self, window: Window) -> Bridge {
        Bridge(window.0)
    }

    pub fn open_document(&self, window: Window, name: &str, source: &str, primary: bool) -> bool {
        self.shared
            .engine
            .borrow_mut()
            .open_document(window.0, name, source, primary)
    }

    pub(crate) fn raw(&self) -> &RefCell<himark_api::HimarkEngine> {
        &self.shared.engine
    }

    pub(crate) fn attach(&self, view: WeakEntity<BridgeView>) {
        self.shared.views.borrow_mut().push(view);
    }
}
