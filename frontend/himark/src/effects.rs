use std::sync::Arc;
#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use imba::effect::{CancellationToken, Effect, EffectFuture, EffectPayload};

use crate::AppCommand;
#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
use crate::AppEffects;

pub use imba::effect::EffectHandler;
pub type EffectDispatcher = Arc<dyn Fn(AppCommand) + Send + Sync>;

pub type AppEffect = imba::effect::AnyEffect<AppCommand>;

pub(crate) fn trace_effects(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_EFFECTS").is_some()) {
        eprintln!("[effects] {}", message());
    }
}

pub(crate) enum EffectMessage {
    Launch(CancellationToken, EffectPayload<AppCommand>),
    Cancel(CancellationToken),

    Relaunch(
        CancellationToken,
        CancellationToken,
        EffectPayload<AppCommand>,
    ),
}

type DispatchThunk = Box<
    dyn Fn(
            Box<dyn std::any::Any + Send + Sync>,
        ) -> EffectFuture<Box<dyn std::any::Any + Send + Sync>>
        + Send
        + Sync,
>;

#[derive(Default)]
pub(crate) struct Handlers(
    std::sync::Mutex<std::collections::HashMap<std::any::TypeId, DispatchThunk>>,
);

impl Handlers {
    pub(crate) fn register<E: Effect>(&self, handler: impl imba::effect::EffectHandler<E>)
    where
        E::Result: Send + Sync,
    {
        let handler = Arc::new(handler);
        let thunk: DispatchThunk = Box::new(move |payload| {
            let effect = *payload
                .downcast::<E>()
                .expect("dispatch registry keyed by TypeId");
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                Box::new(handler.handle(effect).await) as Box<dyn std::any::Any + Send + Sync>
            })
        });
        self.0
            .lock()
            .expect("handler registry")
            .insert(std::any::TypeId::of::<E>(), thunk);
    }

    pub(crate) fn dispatch(
        &self,
        payload: EffectPayload<AppCommand>,
    ) -> Result<(EffectFuture<Box<dyn std::any::Any + Send + Sync>>, Lift), EffectPayload<AppCommand>>
    {
        let registry = self.0.lock().expect("handler registry");
        if !registry.contains_key(&payload.type_id()) {
            return Err(payload);
        }
        let type_id = payload.type_id();
        let (value, lift) = payload.split();
        let thunk = registry.get(&type_id).expect("checked above");
        Ok((thunk(value), lift))
    }

    pub(crate) fn call_future(
        &self,
        type_id: std::any::TypeId,
        payload: Box<dyn std::any::Any + Send + Sync>,
    ) -> Option<EffectFuture<Box<dyn std::any::Any + Send + Sync>>> {
        let registry = self.0.lock().expect("handler registry");
        let thunk = registry.get(&type_id)?;
        Some(thunk(payload))
    }
}

type Lift = Box<dyn FnOnce(Box<dyn std::any::Any + Send + Sync>) -> Option<AppCommand> + Send>;

pub(crate) fn orphan(type_id: std::any::TypeId) {
    debug_assert!(false, "no handler registered for effect type {type_id:?}");
    #[cfg(not(debug_assertions))]
    eprintln!("[himark] dropping effect: no handler registered for {type_id:?}");
}

pub(crate) fn register_builtins(handlers: &Arc<Handlers>, workshop: &Arc<::editor::Workshop>) {
    handlers.register::<::editor::RepairEffect>(::editor::RepairHandler(Arc::clone(workshop)));
    handlers.register::<::editor::ReparseEffect>(::editor::ReparseHandler(Arc::clone(workshop)));
    {
        let registry = Arc::downgrade(handlers);
        let caller = imba::effect::EffectCaller::new(Arc::new(move |type_id, payload| {
            registry.upgrade()?.call_future(type_id, payload)
        }));
        handlers.register::<::editor::EnrichEffect>(::editor::EnrichHandler {
            workshop: Arc::clone(workshop),
            caller,
        });
    }
    handlers
        .register::<::editor::RepairDiffEffect>(::editor::RepairDiffHandler(Arc::clone(workshop)));
    handlers.register::<crate::diffs::DiffNormalizeEffect>(crate::diffs::DiffNormalizeHandler);
    handlers.register::<crate::toc::OutlineEffect>(crate::toc::OutlineHandler);
    handlers.register::<crate::find::FindScanEffect>(crate::find::FindScanHandler);
    handlers
        .register::<crate::speedsearch::SpeedSearchEffect>(crate::speedsearch::SpeedSearchHandler);
    handlers.register::<crate::watch::RefetchDiffEffect>(crate::watch::RefetchDiffHandler);
    handlers.register::<crate::app::OpenEffect>(crate::app::OpenHandler(Arc::clone(workshop)));
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
pub struct BackgroundRunner {
    arriving: Mutex<std::sync::mpsc::Receiver<EffectMessage>>,
    dispatcher: EffectDispatcher,
    started: Arc<AtomicBool>,
    handlers: Arc<Handlers>,

    pending: Mutex<PollSet>,
    scheduler: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
#[derive(Default)]
struct PollSet {
    tasks: Vec<Task>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
unsafe impl Send for PollSet {}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
struct Task {
    token: CancellationToken,
    name: &'static str,
    future: EffectFuture<Box<dyn std::any::Any + Send + Sync>>,
    lift: Lift,
    wake: Arc<TaskWake>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
struct TaskWake {
    ready: std::sync::atomic::AtomicBool,
    scheduler: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl std::task::Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.ready.store(true, Ordering::Release);
        (self.scheduler)();
    }
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl Task {
    fn poll(&mut self) -> std::task::Poll<Box<dyn std::any::Any + Send + Sync>> {
        let waker = std::task::Waker::from(Arc::clone(&self.wake));
        let mut cx = std::task::Context::from_waker(&waker);
        self.future.as_mut().poll(&mut cx)
    }
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl BackgroundRunner {
    pub fn run(&self) {
        self.started.store(true, Ordering::Release);
        let arriving = self.arriving.lock().expect("effect channel poisoned");
        let mut pending = self
            .pending
            .try_lock()
            .expect("BackgroundRunner::run re-entered: the host worker must be serial");

        let mut queue = std::collections::VecDeque::new();
        loop {
            while let Ok(message) = arriving.try_recv() {
                match message {
                    EffectMessage::Launch(token, payload) => {
                        queue.push_back((token, payload));
                    }
                    EffectMessage::Cancel(token) => {
                        queue.retain(|(queued, _)| *queued != token);
                        pending.tasks.retain(|task| task.token != token);
                    }
                    EffectMessage::Relaunch(previous, token, payload) => {
                        pending.tasks.retain(|task| task.token != previous);
                        match queue.iter_mut().find(|(queued, _)| *queued == previous) {
                            Some(slot) => *slot = (token, payload),
                            None => queue.push_back((token, payload)),
                        }
                    }
                }
            }

            let mut progressed = false;
            pending.tasks.retain_mut(|task| {
                if !task.wake.ready.swap(false, Ordering::AcqRel) {
                    return true;
                }
                progressed = true;
                match task.poll() {
                    std::task::Poll::Ready(outcome) => {
                        let lift = std::mem::replace(&mut task.lift, Box::new(|_| unreachable!()));
                        trace_effects(|| format!("landed {}", task.name));

                        if let Some(command) = lift(outcome) {
                            (self.dispatcher)(command);
                        }
                        false
                    }
                    std::task::Poll::Pending => true,
                }
            });

            if let Some((token, payload)) = queue.pop_front() {
                let type_id = payload.type_id();
                let name = payload.name();
                match self.handlers.dispatch(payload) {
                    Err(_orphan_payload) => {
                        eprintln!("[himark] ORPHAN effect: {name}");
                        orphan(type_id)
                    }
                    Ok((future, lift)) => {
                        let mut task = Task {
                            token,
                            name,
                            future,
                            lift,
                            wake: Arc::new(TaskWake {
                                ready: std::sync::atomic::AtomicBool::new(false),
                                scheduler: Arc::clone(&self.scheduler),
                            }),
                        };
                        match task.poll() {
                            std::task::Poll::Ready(outcome) => {
                                trace_effects(|| format!("landed {}", task.name));
                                if let Some(command) = (task.lift)(outcome) {
                                    (self.dispatcher)(command)
                                }
                            }
                            std::task::Poll::Pending => pending.tasks.push(task),
                        }
                    }
                }
                continue;
            }

            if !progressed {
                break;
            }
        }
    }
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
pub(crate) struct EffectLauncher {
    sender: std::sync::mpsc::Sender<EffectMessage>,

    receiver: Option<std::sync::mpsc::Receiver<EffectMessage>>,

    scheduler: Option<Arc<dyn Fn() + Send + Sync>>,

    started: Arc<AtomicBool>,
}

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
impl EffectLauncher {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            sender,
            receiver: Some(receiver),
            scheduler: None,
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn launch(&self, batch: AppEffects) {
        if batch.is_empty() {
            return;
        }
        for message in batch.drain() {
            let _ = self.sender.send(match message {
                imba::effect::Message::Launch(token, effect) => {
                    trace_effects(|| format!("launch {}", effect.name()));
                    EffectMessage::Launch(token, effect.into_payload())
                }
                imba::effect::Message::Cancel(token) => {
                    trace_effects(|| format!("cancel {token:?}"));
                    EffectMessage::Cancel(token)
                }
                imba::effect::Message::Relaunch(previous, token, effect) => {
                    trace_effects(|| format!("relaunch {}", effect.name()));
                    EffectMessage::Relaunch(previous, token, effect.into_payload())
                }
            });
        }
        if let Some(scheduler) = &self.scheduler {
            scheduler();
        }
    }

    pub(crate) fn attach(
        &mut self,
        dispatcher: EffectDispatcher,
        scheduler: Arc<dyn Fn() + Send + Sync>,
        handlers: Arc<Handlers>,
    ) -> BackgroundRunner {
        self.scheduler = Some(Arc::clone(&scheduler));
        BackgroundRunner {
            arriving: Mutex::new(self.receiver.take().expect("attach_host is called once")),
            dispatcher,
            started: Arc::clone(&self.started),
            handlers,
            pending: Mutex::new(PollSet::default()),
            scheduler,
        }
    }
}

#[cfg(test)]
mod tests;
