// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

/// Write `bytes` to `path` ATOMICALLY: a temp file in the same
/// directory, then `rename` over the target. A bare `fs::write`
/// truncates then fills, and this host's own mirror watcher
/// (`reload_mirror`) can read the file in that empty window and
/// clobber the synced channel to empty. Rename replaces in one step,
/// so no reader — ours or an external tool — ever sees a torn file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = parent.join(format!(
        ".{name}.himark-{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A fresh temp file gets the whole content, flushed, before it is
    // renamed over the target in one step — no reader ever sees a
    // truncated or partial file.
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub(crate) fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

pub(crate) fn encoded_file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// The local path of a `file://` uri, parsed as a real uri: percent
/// escapes are decoded (`file:///a%20b` → `/a b`) and a Windows drive
/// (`file:///C:/x`) comes back as `C:\x`. The host's own `file_uri`
/// emits raw paths, and a raw path with no `%` or `?`/`#` parses the
/// same way, so both spellings resolve to one file.
pub(crate) fn file_path(uri: &str) -> Option<PathBuf> {
    if !uri.starts_with("file://") {
        return None;
    }
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

pub(crate) fn file_path_str(uri: &str) -> Option<String> {
    file_path(uri).map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_are_percent_decoded() {
        assert_eq!(
            file_path("file:///tmp/my%20docs/a%20b.txt"),
            Some(PathBuf::from("/tmp/my docs/a b.txt"))
        );
        assert_eq!(
            file_path("file:///tmp/caf%C3%A9.md"),
            Some(PathBuf::from("/tmp/café.md"))
        );
        assert_eq!(
            file_path("file:///tmp/raw name.txt"),
            Some(PathBuf::from("/tmp/raw name.txt"))
        );
        assert_eq!(
            file_path("file:///tmp/100%.txt"),
            Some(PathBuf::from("/tmp/100%.txt"))
        );
        assert_eq!(
            file_path("file:///tmp/a%20b?x=1#frag"),
            Some(PathBuf::from("/tmp/a b"))
        );
        assert_eq!(file_path("file:///tmp/a b"), file_path("file:///tmp/a%20b"));
        assert_eq!(file_path("ahp-content:/x"), None);
    }

    #[test]
    fn encoding_round_trips() {
        let path = Path::new("/tmp/a b/c%d/é.txt");
        assert_eq!(file_path(&encoded_file_uri(path)).as_deref(), Some(path));
    }
}
