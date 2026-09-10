pub use rquickjs;

mod run;
#[cfg(feature = "plugin")]
mod plugin;
#[cfg(feature = "plugin")]
pub use plugin::{
    resolve, RunScript, RunScriptEffect, RunScriptHandler, ScriptAgent, ScriptCapture,
    ScriptEdit, ScriptLanding, ScriptLanes, ScriptRecord, ScriptRuns, ScriptSnapshot, ShowDocuments,
    ScriptStored,
};
pub use run::{run_script, Limits, ScriptOutcome, ScriptWorld, ScriptWrite, WorldFuture};

pub fn eval_to_string(source: &str) -> Result<String, String> {
    let runtime = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
    let context = rquickjs::Context::full(&runtime).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        let value: rquickjs::Value = ctx.eval(source).map_err(|error| exception_text(&ctx, error))?;
        stringify(&ctx, value)
    })
}

fn stringify<'js>(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> Result<String, String> {
    let globals = ctx.globals();
    let json: rquickjs::Object = globals.get("JSON").map_err(|error| error.to_string())?;
    let stringify: rquickjs::Function = json.get("stringify").map_err(|error| error.to_string())?;
    let text: Option<String> = stringify.call((value,)).map_err(|error| error.to_string())?;
    Ok(text.unwrap_or_else(|| "undefined".to_owned()))
}

fn exception_text<'js>(ctx: &rquickjs::Ctx<'js>, error: rquickjs::Error) -> String {
    match error {
        rquickjs::Error::Exception => ctx
            .catch()
            .as_exception()
            .map(|exception| exception.to_string())
            .unwrap_or_else(|| "uncaught exception".to_owned()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn hello_world_evals() {
        assert_eq!(
            eval_to_string("(() => `hello ${40 + 2}`)()").expect("evals"),
            "\"hello 42\""
        );
    }

    #[test]
    fn json_round_trips() {
        assert_eq!(
            eval_to_string("JSON.parse('{\"a\":[1,2,{}]}').a[2]").expect("evals"),
            "{}"
        );
    }

    #[test]
    fn the_sandbox_has_no_ambient_authority() {
        for hole in ["std", "os", "require", "process", "fetch", "XMLHttpRequest"] {
            assert_eq!(
                eval_to_string(&format!("typeof {hole}")).expect("evals"),
                "\"undefined\"",
                "{hole} must not exist"
            );
        }
    }

    #[test]
    fn the_memory_limit_kills_an_allocation_bomb() {
        let runtime = rquickjs::Runtime::new().expect("runtime");
        runtime.set_memory_limit(8 << 20);
        let context = rquickjs::Context::full(&runtime).expect("context");
        let outcome: Result<rquickjs::Error, ()> = context.with(|ctx| {
            let result: Result<rquickjs::Value, _> =
                ctx.eval("const a = []; for (;;) a.push('x'.repeat(1 << 20));");
            match result {
                Err(error) => Ok(error),
                Ok(_) => Err(()),
            }
        });
        assert!(outcome.is_ok(), "the bomb must die by the cap");
    }

    #[test]
    fn the_interrupt_handler_kills_a_spinning_loop() {
        let runtime = rquickjs::Runtime::new().expect("runtime");
        let polls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&polls);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            seen.fetch_add(1, Ordering::Relaxed) > 3
        })));
        let context = rquickjs::Context::full(&runtime).expect("context");
        let interrupted = context.with(|ctx| {
            let result: Result<rquickjs::Value, _> = ctx.eval("for (;;) {}");
            result.is_err()
        });
        assert!(interrupted, "the loop must die by the interrupt");
        assert!(
            polls.load(Ordering::Relaxed) > 3,
            "the handler was actually polled"
        );
    }

    #[test]
    fn a_promise_resolves_through_the_job_queue() {
        let runtime = rquickjs::Runtime::new().expect("runtime");
        let context = rquickjs::Context::full(&runtime).expect("context");
        context.with(|ctx| {
            ctx.eval::<(), _>(
                "globalThis.landed = null; Promise.resolve(7).then(v => { globalThis.landed = v; });",
            )
            .expect("evals");
        });
        while runtime.is_job_pending() {
            runtime.execute_pending_job().expect("job runs");
        }
        context.with(|ctx| {
            let landed: i32 = ctx.eval("globalThis.landed").expect("evals");
            assert_eq!(landed, 7, "the then-callback ran on the drain");
        });
    }
}
