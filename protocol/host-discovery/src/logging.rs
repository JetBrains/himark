use std::sync::OnceLock;

static GUARD: OnceLock<Option<tracing_appender::non_blocking::WorkerGuard>> = OnceLock::new();

fn log_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("HIMARK_LOG_DIR") {
        return Some(std::path::PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("Himark"),
    )
}

pub fn init(role: &str) {
    GUARD.get_or_init(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let filter = || {
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        };
        let mirror = std::env::var("HIHOST_TRACE").is_ok();
        let file = log_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;

            if let Ok(entries) = std::fs::read_dir(&dir) {
                let week = std::time::Duration::from_secs(7 * 24 * 3600);
                for entry in entries.flatten() {
                    let stale = entry
                        .metadata()
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > week);
                    if stale {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{role}-{}.log", std::process::id())))
                .ok()?;
            Some(tracing_appender::non_blocking(file))
        });
        let guard = match file {
            Some((writer, guard)) => {
                let file_layer = tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_thread_names(true)
                    .with_target(true);
                let stderr_layer = mirror.then(|| {
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(false)
                        .with_thread_names(true)
                });
                let _ = tracing_subscriber::registry()
                    .with(filter())
                    .with(file_layer)
                    .with(stderr_layer)
                    .try_init();
                Some(guard)
            }
            None => {
                let _ = tracing_subscriber::registry()
                    .with(filter())
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(std::io::stderr)
                            .with_ansi(false)
                            .with_thread_names(true),
                    )
                    .try_init();
                None
            }
        };
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(target: "panic", "{info}\n{backtrace}");
            previous(info);
        }));
        guard
    });
}

pub fn brief(line: &str) -> String {
    match line.len() > 200 {
        true => format!(
            "{}… ({} bytes)",
            &line[..line.floor_char_boundary(200)],
            line.len()
        ),
        false => line.to_owned(),
    }
}
