pub mod logging;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LOCAL_FS_SESSION: &str = "hihost-fs:/local";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    pub pid: u32,
    pub socket: PathBuf,
    pub protocol_version: String,
    pub started_at: u64,

    #[serde(default)]
    pub build: Option<String>,

    #[serde(default)]
    pub http: Option<String>,
}

pub fn default_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HIMARK_HOST_HOME") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".himark").join("agent-host"))
}

pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("host.lock")
}

pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join("host.sock")
}

pub fn build_stamp() -> String {
    match std::env::current_exe() {
        Ok(exe) => build_stamp_of(&exe),
        Err(_) => "unknown".to_owned(),
    }
}

pub fn build_stamp_of(binary: &Path) -> String {
    let Ok(meta) = std::fs::metadata(binary) else {
        return "unknown".to_owned();
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    format!("{}:{}:{}", binary.display(), meta.len(), mtime)
}

pub const DAEMON_BINARY: &str = "himark-agent-host";

pub fn resolve_daemon_binary() -> Option<PathBuf> {
    if let Ok(named) = std::env::var("HIMARK_AGENT_HOST_BIN") {
        let named = PathBuf::from(named);
        if named.is_file() {
            return Some(named);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(DAEMON_BINARY);
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(DAEMON_BINARY))
        .find(|candidate| candidate.is_file())
}

pub fn autostart() -> Option<Lockfile> {
    let dir = default_dir()?;
    let daemon = resolve_daemon_binary();
    if let Some(live) = read_live(&dir) {
        let matches = match &daemon {
            Some(daemon) => live.build.as_deref() == Some(build_stamp_of(daemon).as_str()),
            None => true,
        };
        if matches {
            return Some(live);
        }
        logging::init("app");
        tracing::warn!(
            target: "ahp_autostart",
            pid = live.pid,
            dir = %dir.display(),
            "a live host from another build stands — taking over"
        );
        eprintln!(
            "[agent-host] a live host from another build holds {} — taking over",
            dir.display()
        );
        terminate(live.pid);
        for _ in 0..100 {
            if read_live(&dir).is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if let Some(stubborn) = read_live(&dir) {
            tracing::warn!(
                target: "ahp_autostart",
                pid = stubborn.pid,
                "the standing host ignored the takeover — adopting it"
            );
            eprintln!("[agent-host] the standing host ignored the takeover — adopting it");
            return Some(stubborn);
        }
    }
    let binary = daemon?;
    logging::init("app");
    tracing::info!(target: "ahp_autostart", binary = %binary.display(), "spawning the daemon");
    std::process::Command::new(binary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    for _ in 0..75 {
        if let Some(live) = read_live(&dir) {
            return Some(live);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}

pub fn terminate(pid: u32) {
    unsafe {
        let _ = libc_kill(pid as i32, 15);
    }
}

pub fn write(dir: &Path, socket: &Path, protocol_version: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let lock = Lockfile {
        pid: std::process::id(),
        socket: socket.to_owned(),
        protocol_version: protocol_version.to_owned(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(0),
        build: Some(build_stamp()),
        http: None,
    };
    std::fs::write(
        lock_path(dir),
        serde_json::to_vec(&lock).expect("lockfiles encode"),
    )
}

pub fn update_http(http: Option<String>) {
    let Some(dir) = default_dir() else { return };
    let path = lock_path(&dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut lock) = serde_json::from_slice::<Lockfile>(&bytes) else {
        return;
    };
    if lock.pid != std::process::id() {
        return;
    }
    lock.http = http;
    let _ = std::fs::write(path, serde_json::to_vec(&lock).expect("lockfiles encode"));
}

pub fn read_live(dir: &Path) -> Option<Lockfile> {
    let raw = std::fs::read_to_string(lock_path(dir)).ok()?;
    let lock: Lockfile = serde_json::from_str(&raw).ok()?;
    alive(lock.pid).then_some(lock)
}

fn alive(pid: u32) -> bool {
    let outcome = unsafe { libc_kill(pid as i32, 0) };
    outcome == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_stamp_lockfiles_parse_with_no_build() {
        let raw = r#"{"pid":1,"socket":"/tmp/s","protocolVersion":"0.7.0","startedAt":0}"#;
        let lock: Lockfile = serde_json::from_str(raw).expect("old lockfiles parse");
        assert_eq!(lock.build, None);
    }

    #[test]
    fn the_build_stamp_is_stable_and_real() {
        let stamp = build_stamp();
        assert_ne!(stamp, "unknown");
        assert_eq!(stamp, build_stamp());
    }
}
