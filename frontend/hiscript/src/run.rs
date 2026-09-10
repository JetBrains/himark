use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Function, Module, Object, Persistent, Promise, Runtime, Value};

pub type WorldFuture<T> = Pin<Box<dyn Future<Output = T>>>;

pub struct ScriptWorld {
    pub read: Box<dyn FnMut(String) -> WorldFuture<Option<String>>>,

    pub ask: Box<dyn FnMut(String) -> WorldFuture<Result<String, String>>>,

    pub changes: Box<dyn FnMut() -> WorldFuture<Option<String>>>,
}

impl ScriptWorld {
    pub fn disconnected() -> Self {
        Self {
            read: Box::new(|_| Box::pin(std::future::ready(None))),
            ask: Box::new(|_| {
                Box::pin(std::future::ready(Err(
                    "no agent capability in this run".to_owned()
                )))
            }),
            changes: Box::new(|| Box::pin(std::future::ready(None))),
        }
    }
}

pub struct Limits {
    pub memory: usize,

    pub compute: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory: 64 << 20,
            compute: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptWrite {
    pub target: String,

    pub text: String,
}

#[derive(Debug, Default)]
pub struct ScriptOutcome {
    pub log: Vec<String>,
    pub writes: Vec<ScriptWrite>,

    pub shows: Vec<String>,

    pub error: Option<String>,
}

enum Ask {
    Read { path: String },
    Agent { prompt: String },
    Changes,
}

enum Answer {
    Value(Option<String>),
    Fallible(Result<String, String>),
}

struct Pending {
    ask: Ask,
    resolve: Persistent<Function<'static>>,
    reject: Persistent<Function<'static>>,
}

enum Step {
    Done(Result<(), String>),
    Asks(Vec<Pending>),

    Stuck,
}

pub async fn run_script(
    name: &str,
    source: &str,
    world: ScriptWorld,
    limits: Limits,
) -> ScriptOutcome {
    let mut world = world;
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let writes: Rc<RefCell<Vec<ScriptWrite>>> = Rc::default();
    let shows: Rc<RefCell<Vec<String>>> = Rc::default();
    let queue: Rc<RefCell<Vec<Pending>>> = Rc::default();

    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return failed(&log, format!("engine: {error}")),
    };
    runtime.set_memory_limit(limits.memory);

    let deadline = Arc::new(Mutex::new(Instant::now() + limits.compute));
    {
        let deadline = Arc::clone(&deadline);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            Instant::now() > *deadline.lock().expect("deadline")
        })));
    }
    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => return failed(&log, format!("engine: {error}")),
    };

    let staged: Result<(Persistent<Object<'static>>, Persistent<Promise<'static>>, Persistent<Object<'static>>), String> =
        context.with(|ctx| {
            let himark =
            himark_object(&ctx, &log, &writes, &shows, &queue).map_err(|e| text(&ctx, e))?;
            let (module, evaluated) = Module::declare(ctx.clone(), name, source)
                .and_then(|module| module.eval())
                .map_err(|e| text(&ctx, e))?;
            let namespace = module.namespace().map_err(|e| text(&ctx, e))?;
            Ok((
                Persistent::save(&ctx, himark),
                Persistent::save(&ctx, evaluated),
                Persistent::save(&ctx, namespace),
            ))
        });
    let (himark, evaluated, namespace) = match staged {
        Ok(staged) => staged,
        Err(error) => return failed(&log, error),
    };
    pump(&runtime);
    let root: Result<Persistent<Promise<'static>>, String> = context.with(|ctx| {
        let evaluated = evaluated.restore(&ctx).map_err(|e| e.to_string())?;
        if let rquickjs::promise::PromiseState::Rejected = evaluated.state() {
            return Err(reject_text(&ctx, &evaluated));
        }
        let namespace = namespace.restore(&ctx).map_err(|e| e.to_string())?;
        let default: Function = namespace
            .get("default")
            .map_err(|_| "the module exports no default function".to_owned())?;
        let himark = himark.restore(&ctx).map_err(|e| e.to_string())?;
        let value: Value = default.call((himark,)).map_err(|e| text(&ctx, e))?;
        let promise = Promise::from_value(value)
            .map_err(|_| "the default export must be async (answer a promise)".to_owned())?;
        Ok(Persistent::save(&ctx, promise))
    });
    let root = match root {
        Ok(root) => root,
        Err(error) => return failed(&log, error),
    };

    loop {
        pump(&runtime);
        let step = context.with(|ctx| {
            let root = match root.clone().restore(&ctx) {
                Ok(root) => root,
                Err(error) => return Step::Done(Err(error.to_string())),
            };
            match root.state() {
                rquickjs::promise::PromiseState::Resolved => Step::Done(Ok(())),
                rquickjs::promise::PromiseState::Rejected => {
                    Step::Done(Err(reject_text(&ctx, &root)))
                }
                rquickjs::promise::PromiseState::Pending => {
                    let asks: Vec<Pending> = queue.borrow_mut().drain(..).collect();
                    match asks.is_empty() {
                        true => Step::Stuck,
                        false => Step::Asks(asks),
                    }
                }
            }
        });
        match step {
            Step::Done(Ok(())) => {
                return ScriptOutcome {
                    log: log.take(),
                    writes: writes.take(),
                    shows: shows.take(),
                    error: None,
                }
            }
            Step::Done(Err(error)) => return failed(&log, error),
            Step::Stuck => {
                return failed(
                    &log,
                    "the script awaits something no capability will answer".to_owned(),
                )
            }
            Step::Asks(asks) => {
                for pending in asks {
                    let Pending {
                        ask,
                        resolve,
                        reject,
                    } = pending;
                    let answer = match ask {
                        Ask::Read { path } => Answer::Value((world.read)(path).await),
                        Ask::Agent { prompt } => Answer::Fallible((world.ask)(prompt).await),
                        Ask::Changes => Answer::Value((world.changes)().await),
                    };
                    *deadline.lock().expect("deadline") = Instant::now() + limits.compute;
                    let landed: Result<(), String> = context.with(|ctx| {
                        let resolve = resolve.restore(&ctx).map_err(|e| e.to_string())?;

                        let outcome = match answer {
                            Answer::Value(Some(value)) => resolve.call::<_, ()>((value,)),
                            Answer::Value(None) => resolve.call::<_, ()>((rquickjs::Null,)),
                            Answer::Fallible(Ok(value)) => resolve.call::<_, ()>((value,)),
                            Answer::Fallible(Err(error)) => reject
                                .clone()
                                .restore(&ctx)
                                .map_err(|e| e.to_string())?
                                .call::<_, ()>((error,)),
                        }
                        .map_err(|e| text(&ctx, e));
                        outcome.or_else(|error| {
                            reject
                                .restore(&ctx)
                                .map_err(|e| e.to_string())?
                                .call::<_, ()>((error.clone(),))
                                .map_err(|_| error)
                        })
                    });
                    if let Err(error) = landed {
                        return failed(&log, error);
                    }
                }
            }
        }
    }
}

fn failed(log: &Rc<RefCell<Vec<String>>>, error: String) -> ScriptOutcome {
    ScriptOutcome {
        log: log.take(),

        writes: Vec::new(),
        shows: Vec::new(),
        error: Some(error),
    }
}

fn pump(runtime: &Runtime) {
    while runtime.is_job_pending() {
        if runtime.execute_pending_job().is_err() {
        }
    }
}

fn himark_object<'js>(
    ctx: &Ctx<'js>,
    log: &Rc<RefCell<Vec<String>>>,
    writes: &Rc<RefCell<Vec<ScriptWrite>>>,
    shows: &Rc<RefCell<Vec<String>>>,
    queue: &Rc<RefCell<Vec<Pending>>>,
) -> rquickjs::Result<Object<'js>> {
    let himark = Object::new(ctx.clone())?;
    himark.set("log", {
        let log = Rc::clone(log);
        Function::new(ctx.clone(), move |line: String| {
            log.borrow_mut().push(line);
        })?
    })?;
    let docs = Object::new(ctx.clone())?;
    docs.set("read", {
        let queue = Rc::clone(queue);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, path: String| -> rquickjs::Result<Promise<'js>> {
                let (promise, resolve, reject) = Promise::new(&ctx)?;
                queue.borrow_mut().push(Pending {
                    ask: Ask::Read { path },
                    resolve: Persistent::save(&ctx, resolve),
                    reject: Persistent::save(&ctx, reject),
                });
                Ok(promise)
            },
        )?
    })?;
    docs.set("write", {
        let writes = Rc::clone(writes);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, target: String, text: String| -> rquickjs::Result<Promise<'js>> {
                writes.borrow_mut().push(ScriptWrite { target, text });

                let (promise, resolve, _reject) = Promise::new(&ctx)?;
                resolve.call::<_, ()>(())?;
                Ok(promise)
            },
        )?
    })?;
    docs.set("show", {
        let shows = Rc::clone(shows);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, path: String| -> rquickjs::Result<Promise<'js>> {
                shows.borrow_mut().push(path);

                let (promise, resolve, _reject) = Promise::new(&ctx)?;
                resolve.call::<_, ()>(())?;
                Ok(promise)
            },
        )?
    })?;
    himark.set("docs", docs)?;
    let agent = Object::new(ctx.clone())?;
    agent.set("ask", {
        let queue = Rc::clone(queue);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, prompt: String| -> rquickjs::Result<Promise<'js>> {
                let (promise, resolve, reject) = Promise::new(&ctx)?;
                queue.borrow_mut().push(Pending {
                    ask: Ask::Agent { prompt },
                    resolve: Persistent::save(&ctx, resolve),
                    reject: Persistent::save(&ctx, reject),
                });
                Ok(promise)
            },
        )?
    })?;
    himark.set("agent", agent)?;
    let vcs = Object::new(ctx.clone())?;
    vcs.set("changes", {
        let queue = Rc::clone(queue);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>| -> rquickjs::Result<Promise<'js>> {
                let (promise, resolve, reject) = Promise::new(&ctx)?;
                queue.borrow_mut().push(Pending {
                    ask: Ask::Changes,
                    resolve: Persistent::save(&ctx, resolve),
                    reject: Persistent::save(&ctx, reject),
                });
                Ok(promise)
            },
        )?
    })?;
    himark.set("vcs", vcs)?;
    Ok(himark)
}

fn text(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    match error {
        rquickjs::Error::Exception => caught(ctx),
        other => other.to_string(),
    }
}

fn reject_text(ctx: &Ctx<'_>, promise: &Promise<'_>) -> String {
    match promise.result::<Value>() {
        Some(Err(rquickjs::Error::Exception)) => caught(ctx),
        Some(Err(other)) => other.to_string(),
        _ => "script failed".to_owned(),
    }
}

fn caught(ctx: &Ctx<'_>) -> String {
    let caught = ctx.catch();
    caught
        .as_exception()
        .map(|exception| exception.to_string())
        .unwrap_or_else(|| format!("{caught:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive<T>(mut future: Pin<Box<dyn Future<Output = T> + '_>>) -> T {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn raw() -> RawWaker {
            const VTABLE: RawWakerVTable =
                RawWakerVTable::new(|_| raw(), |_| {}, |_| {}, |_| {});
            RawWaker::new(std::ptr::null(), &VTABLE)
        }

        let waker = unsafe { Waker::from_raw(raw()) };
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    fn run(source: &str, world: ScriptWorld, limits: Limits) -> ScriptOutcome {
        drive(Box::pin(run_script("test.js", source, world, limits)))
    }

    fn table_world(entries: &[(&str, &str)]) -> ScriptWorld {
        let table: std::collections::HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ScriptWorld {
            read: Box::new(move |path| {
                let answer = table.get(&path).cloned();
                Box::pin(std::future::ready(answer))
            }),
            ..ScriptWorld::disconnected()
        }
    }

    #[test]
    fn a_script_reads_computes_and_writes() {
        let outcome = run(
            r#"export default async function (himark) {
                himark.log("start");
                const current = await himark.docs.read("notes.md");
                await himark.docs.write("notes.md", current + "\nMORE");
                himark.log("done");
            }"#,
            table_world(&[("notes.md", "alpha")]),
            Limits::default(),
        );
        assert_eq!(outcome.error, None, "log: {:?}", outcome.log);
        assert_eq!(outcome.log, vec!["start", "done"]);
        assert_eq!(
            outcome.writes,
            vec![ScriptWrite {
                target: "notes.md".to_owned(),
                text: "alpha\nMORE".to_owned(),
            }]
        );
    }

    #[test]
    fn a_missing_read_answers_null() {
        let outcome = run(
            r#"export default async function (himark) {
                const gone = await himark.docs.read("gone.md");
                himark.log(gone === null ? "absent" : "present");
            }"#,
            table_world(&[]),
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.log, vec!["absent"]);
    }

    #[test]
    fn parallel_reads_resolve_together() {
        let outcome = run(
            r#"export default async function (himark) {
                const [a, b] = await Promise.all([
                    himark.docs.read("a.md"),
                    himark.docs.read("b.md"),
                ]);
                await himark.docs.write("out.md", a + "+" + b);
            }"#,
            table_world(&[("a.md", "A"), ("b.md", "B")]),
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.writes[0].text, "A+B");
    }

    #[test]
    fn a_throw_lands_as_the_error_and_discards_writes() {
        let outcome = run(
            r#"export default async function (himark) {
                await himark.docs.write("x.md", "half");
                throw new Error("boom");
            }"#,
            table_world(&[]),
            Limits::default(),
        );
        assert!(
            outcome.error.as_deref().is_some_and(|e| e.contains("boom")),
            "error: {:?}",
            outcome.error
        );
        assert!(outcome.writes.is_empty(), "a failed run commits nothing");
    }

    #[test]
    fn a_spinning_script_dies_by_the_deadline() {
        let started = Instant::now();
        let outcome = run(
            "export default async function () { for (;;) {} }",
            ScriptWorld::disconnected(),
            Limits {
                compute: Duration::from_millis(100),
                ..Limits::default()
            },
        );
        assert!(outcome.error.is_some(), "the loop must die");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "and die promptly"
        );
    }

    #[test]
    fn an_allocation_bomb_dies_by_the_cap() {
        let outcome = run(
            r#"export default async function () {
                const hog = [];
                for (;;) hog.push("x".repeat(1 << 20));
            }"#,
            ScriptWorld::disconnected(),
            Limits {
                memory: 8 << 20,
                ..Limits::default()
            },
        );
        assert!(outcome.error.is_some(), "the bomb must die by the cap");
    }

    #[test]
    fn awaiting_the_unanswerable_is_a_clean_error() {
        let outcome = run(
            "export default async function () { await new Promise(() => {}); }",
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|e| e.contains("no capability")),
            "error: {:?}",
            outcome.error
        );
    }

    #[test]
    fn a_malformed_module_names_its_problem() {
        let outcome = run(
            "export const x = 1;",
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|e| e.contains("default")),
            "error: {:?}",
            outcome.error
        );
    }

    #[test]
    fn shows_are_intents_and_die_with_the_run() {
        let outcome = run(
            r#"export default async function (himark) {
                await himark.docs.write("out.md", "made");
                await himark.docs.show("out.md");
            }"#,
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.shows, vec!["out.md"]);
        let outcome = run(
            r#"export default async function (himark) {
                await himark.docs.show("out.md");
                throw new Error("late");
            }"#,
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert!(outcome.error.is_some());
        assert!(outcome.shows.is_empty(), "a failed run shows nothing");
    }

    #[test]
    fn an_agent_ask_round_trips() {
        let world = ScriptWorld {
            ask: Box::new(|prompt| {
                Box::pin(std::future::ready(Ok(format!("echo: {prompt}"))))
            }),
            ..ScriptWorld::disconnected()
        };
        let outcome = run(
            r#"export default async function (himark) {
                const reply = await himark.agent.ask("narrate");
                await himark.docs.write("out.md", reply);
            }"#,
            world,
            Limits::default(),
        );
        assert_eq!(outcome.error, None, "log: {:?}", outcome.log);
        assert_eq!(outcome.writes[0].text, "echo: narrate");
    }

    #[test]
    fn an_agentless_ask_rejects_catchably() {
        let outcome = run(
            r#"export default async function (himark) {
                try {
                    await himark.agent.ask("anything");
                    himark.log("unreachable");
                } catch (error) {
                    himark.log(`caught: ${error}`);
                }
            }"#,
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.log.len(), 1);
        assert!(
            outcome.log[0].contains("no agent capability"),
            "log: {:?}",
            outcome.log
        );
    }

    #[test]
    fn vcs_changes_answer_text_or_null() {
        let world = ScriptWorld {
            changes: Box::new(|| Box::pin(std::future::ready(Some("M a.rs".to_owned())))),
            ..ScriptWorld::disconnected()
        };
        let outcome = run(
            r#"export default async function (himark) {
                himark.log(await himark.vcs.changes());
            }"#,
            world,
            Limits::default(),
        );
        assert_eq!(outcome.log, vec!["M a.rs"]);
        let outcome = run(
            r#"export default async function (himark) {
                himark.log(String((await himark.vcs.changes()) === null));
            }"#,
            ScriptWorld::disconnected(),
            Limits::default(),
        );
        assert_eq!(outcome.log, vec!["true"]);
    }

    #[test]
    fn runs_share_no_state() {
        let plant = "export default async function (himark) { globalThis.seed = 42; himark.log(String(globalThis.seed)); }";
        let probe = "export default async function (himark) { himark.log(typeof globalThis.seed); }";
        assert_eq!(
            run(plant, ScriptWorld::disconnected(), Limits::default()).log,
            vec!["42"]
        );
        assert_eq!(
            run(probe, ScriptWorld::disconnected(), Limits::default()).log,
            vec!["undefined"],
            "the second run started from zero"
        );
    }
}
