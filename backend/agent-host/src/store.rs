use std::path::{Path, PathBuf};

use ahp_types::actions::StateAction;
use ahp_types::common::Uri;
use ahp_types::state::Annotation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub session: Uri,

    pub native_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub provider: String,
    pub working_directories: Vec<Uri>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<Uri>,
    pub default_chat: Uri,
    pub title: String,
    pub created_at: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree: bool,

    #[serde(default = "listed_default", skip_serializing_if = "Clone::clone")]
    pub listed: bool,
}

fn listed_default() -> bool {
    true
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn session_dir(&self, native_id: &str) -> PathBuf {
        self.root.join("sessions").join(native_id)
    }

    pub fn write_manifest(&self, manifest: &Manifest) -> std::io::Result<()> {
        let dir = self.session_dir(&manifest.native_id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_vec_pretty(manifest).expect("manifests encode"),
        )
    }

    pub fn remove_session(&self, native_id: &str) -> std::io::Result<()> {
        let dir = self.session_dir(native_id);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn manifests(&self) -> Vec<Manifest> {
        let sessions = self.root.join("sessions");
        let Ok(entries) = std::fs::read_dir(&sessions) else {
            return Vec::new();
        };
        let mut found: Vec<(std::time::SystemTime, Manifest)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path().join("session.json");
                let raw = std::fs::read_to_string(&path).ok()?;
                let manifest = serde_json::from_str(&raw).ok()?;
                let stamp = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some((stamp, manifest))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().map(|(_, manifest)| manifest).collect()
    }

    fn chat_log(&self, native_id: &str, chat: &Uri) -> PathBuf {
        let name = chat.rsplit('/').next().unwrap_or("chat");
        self.session_dir(native_id).join(format!("{name}.ndjson"))
    }

    pub fn append(&self, native_id: &str, chat: &Uri, action: &StateAction) {
        let path = self.chat_log(native_id, chat);
        let Some(parent) = path.parent() else { return };
        let _ = std::fs::create_dir_all(parent);
        let Ok(mut line) = serde_json::to_vec(action) else {
            return;
        };
        line.push(b'\n');
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
        {
            let _ = file.write_all(&line);
        }
    }

    pub fn replay(&self, native_id: &str, chat: &Uri) -> Vec<StateAction> {
        let Ok(raw) = std::fs::read_to_string(self.chat_log(native_id, chat)) else {
            return Vec::new();
        };
        raw.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
