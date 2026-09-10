use std::sync::atomic::AtomicUsize;
use std::sync::Mutex as StdMutex;

use super::*;
use imba::effect::AnyEffect;

struct Probe {
    tag: &'static str,
}

impl Effect for Probe {
    type Result = String;
}

struct Recording(Arc<StdMutex<Vec<String>>>);

impl EffectHandler<Probe> for Recording {
    async fn handle(&self, effect: Probe) -> String {
        self.0
            .lock()
            .expect("recordings")
            .push(effect.tag.to_owned());
        format!("ran:{}", effect.tag)
    }
}

struct SlowProbe;

impl Effect for SlowProbe {
    type Result = String;
}

#[derive(Default)]
struct Slot(StdMutex<(Option<String>, Option<std::task::Waker>)>);

impl Slot {
    fn fulfill(&self, value: &str) {
        let mut slot = self.0.lock().expect("completion slot");
        slot.0 = Some(value.to_owned());
        if let Some(waker) = slot.1.take() {
            waker.wake();
        }
    }
}

struct External(Arc<Slot>);

impl EffectHandler<SlowProbe> for External {
    async fn handle(&self, _effect: SlowProbe) -> String {
        struct Awaiting(Arc<Slot>);
        impl std::future::Future for Awaiting {
            type Output = String;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<String> {
                let mut slot = self.0 .0.lock().expect("completion slot");
                match slot.0.take() {
                    Some(value) => std::task::Poll::Ready(value),
                    None => {
                        slot.1 = Some(cx.waker().clone());
                        std::task::Poll::Pending
                    }
                }
            }
        }
        Awaiting(Arc::clone(&self.0)).await
    }
}

fn lifted<E: Effect<Result = String>>(effect: E) -> AppEffect {
    let document = crate::DocumentId::from_raw(0);
    AnyEffect::new(effect)
        .map(move |text| AppCommand::Entity(document, ::editor::EditorCommand::InsertText { text }))
}

fn batch_of(effects: Vec<AppEffect>) -> AppEffects {
    let mut batch = AppEffects::new();
    for effect in effects {
        let _ = batch.push(effect);
    }
    batch
}

fn harness(
    handlers: Arc<Handlers>,
    launcher: &mut EffectLauncher,
) -> (
    BackgroundRunner,
    Arc<StdMutex<Vec<String>>>,
    Arc<AtomicUsize>,
) {
    let posted: Arc<StdMutex<Vec<String>>> = Arc::default();
    let wakes = Arc::new(AtomicUsize::new(0));
    let dispatcher: EffectDispatcher = {
        let posted = Arc::clone(&posted);
        Arc::new(move |command| {
            let AppCommand::Entity(_, ::editor::EditorCommand::InsertText { text }) = command
            else {
                panic!("the probes lift into InsertText");
            };
            posted.lock().expect("posted").push(text);
        })
    };
    let scheduler: Arc<dyn Fn() + Send + Sync> = {
        let wakes = Arc::clone(&wakes);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::Relaxed);
        })
    };
    let runner = launcher.attach(dispatcher, scheduler, handlers);
    (runner, posted, wakes)
}

#[test]
fn a_probe_lands_through_its_registered_handler_and_its_lift() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![lifted(Probe { tag: "one" })]));
    runner.run();

    assert_eq!(*received.lock().expect("recordings"), vec!["one"]);
    assert_eq!(*posted.lock().expect("posted"), vec!["ran:one"]);
}

#[test]
fn a_parked_external_execute_lands_on_the_wake_without_blocking_the_queue() {
    let handlers = Arc::new(Handlers::default());
    let slot = Arc::new(Slot::default());
    handlers.register::<SlowProbe>(External(Arc::clone(&slot)));
    handlers.register::<Probe>(Recording(Arc::default()));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, wakes) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![
        lifted(SlowProbe),
        lifted(Probe { tag: "compute" }),
    ]));
    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:compute"],
        "the parked external execute must not block the compute effect"
    );

    let woken = wakes.load(Ordering::Relaxed);
    let fulfiller = std::thread::spawn(move || slot.fulfill("external"));
    fulfiller.join().expect("fulfill thread");
    assert!(
        wakes.load(Ordering::Relaxed) > woken,
        "the completion asks the host for another pass"
    );

    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:compute", "external"]
    );
}

fn caller_for(handlers: &Arc<Handlers>) -> imba::effect::EffectCaller {
    let handlers = Arc::downgrade(handlers);
    imba::effect::EffectCaller::new(Arc::new(move |type_id, payload| {
        handlers.upgrade()?.call_future(type_id, payload)
    }))
}

struct RelayProbe;

impl Effect for RelayProbe {
    type Result = String;
}

struct Relay(imba::effect::EffectCaller);

impl EffectHandler<RelayProbe> for Relay {
    async fn handle(&self, _effect: RelayProbe) -> String {
        match self.0.call(Probe { tag: "child" }).await {
            Some(answer) => format!("relay:{answer}"),
            None => "relay:absent".to_owned(),
        }
    }
}

struct HostRelayProbe;

impl Effect for HostRelayProbe {
    type Result = String;
}

struct HostRelay(imba::effect::EffectCaller);

impl EffectHandler<HostRelayProbe> for HostRelay {
    async fn handle(&self, _effect: HostRelayProbe) -> String {
        match self.0.call(SlowProbe).await {
            Some(answer) => format!("asked:{answer}"),
            None => "asked:nobody".to_owned(),
        }
    }
}

#[test]
fn a_handler_calls_a_compute_effect_and_owns_the_result() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    handlers.register::<RelayProbe>(Relay(caller_for(&handlers)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![lifted(RelayProbe)]));
    runner.run();

    assert_eq!(
        *received.lock().expect("recordings"),
        vec!["child"],
        "the child executed through its registered handler"
    );
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["relay:ran:child"],
        "one landing: the parent's — the called child never posts"
    );
}

#[test]
fn a_handler_awaits_a_parked_host_call_and_lands_after_the_fulfill() {
    let handlers = Arc::new(Handlers::default());
    let slot = Arc::new(Slot::default());
    handlers.register::<SlowProbe>(External(Arc::clone(&slot)));
    handlers.register::<HostRelayProbe>(HostRelay(caller_for(&handlers)));
    handlers.register::<Probe>(Recording(Arc::default()));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, wakes) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![
        lifted(HostRelayProbe),
        lifted(Probe { tag: "compute" }),
    ]));
    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:compute"],
        "the parent parked on its nested host call; the queue drained past it"
    );

    let woken = wakes.load(Ordering::Relaxed);
    let fulfiller = std::thread::spawn(move || slot.fulfill("files"));
    fulfiller.join().expect("fulfill thread");
    assert!(
        wakes.load(Ordering::Relaxed) > woken,
        "the fulfill reaches the PARENT task's waker through the nested await"
    );

    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:compute", "asked:files"],
        "the host's answer landed inside the awaiting handler"
    );
}

#[test]
fn a_call_on_an_unregistered_type_answers_none() {
    let handlers = Arc::new(Handlers::default());
    handlers.register::<HostRelayProbe>(HostRelay(caller_for(&handlers)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![lifted(HostRelayProbe)]));
    runner.run();

    assert_eq!(*posted.lock().expect("posted"), vec!["asked:nobody"]);
}

#[test]
fn competing_effects_both_land_in_order() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![
        lifted(Probe { tag: "first" }),
        lifted(Probe { tag: "second" }),
        lifted(Probe { tag: "third" }),
    ]));
    runner.run();

    assert_eq!(
        *received.lock().expect("recordings"),
        vec!["first", "second", "third"],
        "every launch runs, FIFO"
    );
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:first", "ran:second", "ran:third"]
    );
}

#[test]
fn a_cancelled_queued_effect_never_lands() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let stale = batch.push(lifted(Probe { tag: "stale" }));
    let _fresh = batch.push(lifted(Probe { tag: "fresh" }));
    batch.cancel(stale);
    launcher.launch(batch);
    runner.run();

    assert_eq!(
        *received.lock().expect("recordings"),
        vec!["fresh"],
        "the cancelled launch never ran"
    );
    assert_eq!(*posted.lock().expect("posted"), vec!["ran:fresh"]);
}

#[test]
fn a_cancelled_parked_execute_never_lands() {
    let handlers = Arc::new(Handlers::default());
    let slot = Arc::new(Slot::default());
    handlers.register::<SlowProbe>(External(Arc::clone(&slot)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let token = batch.push(lifted(SlowProbe));
    launcher.launch(batch);
    runner.run();

    let mut cancel = AppEffects::new();
    cancel.cancel(token);
    launcher.launch(cancel);
    runner.run();

    slot.fulfill("too-late");
    runner.run();
    assert!(
        posted.lock().expect("posted").is_empty(),
        "a cancelled parked execute never lands"
    );
}

#[test]
fn a_cancel_after_completion_is_a_no_op() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let token = batch.push(lifted(Probe { tag: "done" }));
    launcher.launch(batch);
    runner.run();
    assert_eq!(*posted.lock().expect("posted"), vec!["ran:done"]);

    let mut cancel = AppEffects::new();
    cancel.cancel(token);
    launcher.launch(cancel);
    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:done"],
        "the late cancel changed nothing"
    );
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "no handler registered")]
fn an_unregistered_effect_type_is_a_dev_panic() {
    let handlers = Arc::new(Handlers::default());
    let mut launcher = EffectLauncher::new();
    let (runner, _, _) = harness(handlers, &mut launcher);

    launcher.launch(batch_of(vec![lifted(Probe { tag: "orphan" })]));
    runner.run();
}

#[test]
fn a_relaunch_keeps_its_predecessors_place_in_the_queue() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, _, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let stale = batch.push(lifted(Probe { tag: "stale" }));
    let _later = batch.push(lifted(Probe { tag: "later" }));
    launcher.launch(batch);

    let mut batch = AppEffects::new();
    let mut slot = Some(stale);
    batch
        .effects()
        .relaunch_erased(&mut slot, lifted(Probe { tag: "fresh" }));
    launcher.launch(batch);

    runner.run();
    assert_eq!(
        *received.lock().expect("recordings"),
        vec!["fresh", "later"],
        "the relaunch kept its place in line"
    );
}

#[test]
fn a_relaunch_of_a_parked_execute_drops_it_and_runs_the_fresh_payload() {
    let handlers = Arc::new(Handlers::default());
    let slot = Arc::new(Slot::default());
    handlers.register::<SlowProbe>(External(Arc::clone(&slot)));
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let parked = batch.push(lifted(SlowProbe));
    launcher.launch(batch);
    runner.run();

    let mut batch = AppEffects::new();
    let mut lane = Some(parked);
    batch
        .effects()
        .relaunch_erased(&mut lane, lifted(Probe { tag: "fresh" }));
    launcher.launch(batch);
    runner.run();

    slot.fulfill("too-late");
    runner.run();
    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:fresh"],
        "the superseded parked execute never landed"
    );
}

#[test]
fn a_relaunch_after_completion_runs_like_a_launch() {
    let handlers = Arc::new(Handlers::default());
    let received = Arc::default();
    handlers.register::<Probe>(Recording(Arc::clone(&received)));
    let mut launcher = EffectLauncher::new();
    let (runner, posted, _) = harness(handlers, &mut launcher);

    let mut batch = AppEffects::new();
    let done = batch.push(lifted(Probe { tag: "done" }));
    launcher.launch(batch);
    runner.run();

    let mut batch = AppEffects::new();
    let mut lane = Some(done);
    batch
        .effects()
        .relaunch_erased(&mut lane, lifted(Probe { tag: "again" }));
    launcher.launch(batch);
    runner.run();

    assert_eq!(
        *posted.lock().expect("posted"),
        vec!["ran:done", "ran:again"],
        "both ran; the late supersession changed nothing about the landing"
    );
}
