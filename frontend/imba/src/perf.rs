use std::io::Write;
use std::path::PathBuf;

pub fn record(bench: &str, metric: &str, value_ms: f64) {
    if let Err(error) = try_record(bench, metric, value_ms) {
        eprintln!("[perf] recording {bench}.{metric} failed: {error}");
    }
}

fn try_record(bench: &str, metric: &str, value_ms: f64) -> std::io::Result<()> {
    let Some(root) = repo_root() else {
        return Ok(());
    };
    let path = root.join("perf.csv");
    let header = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let mut line = String::new();
    if header {
        line.push_str("timestamp,commit,profile,bench,metric,value_ms\n");
    }
    line.push_str(&format!(
        "{timestamp},{},{profile},{bench},{metric},{value_ms:.4}\n",
        commit(&root),
    ));

    file.write_all(line.as_bytes())
}

fn commit(root: &std::path::Path) -> String {
    let head = match std::fs::read_to_string(root.join(".git/HEAD")) {
        Ok(head) => head,
        Err(_) => return "unknown".to_owned(),
    };
    let head = head.trim();
    let full = match head.strip_prefix("ref: ") {
        Some(reference) => match std::fs::read_to_string(root.join(".git").join(reference)) {
            Ok(hash) => hash.trim().to_owned(),
            Err(_) => return "unknown".to_owned(),
        },
        None => head.to_owned(),
    };
    full.chars().take(9).collect()
}

fn repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}
