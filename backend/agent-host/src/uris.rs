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

/// The canonical `file://` uri for a local path, percent-encoded by the
/// url crate: `/tmp/a b` → `file:///tmp/a%20b`, and a literal `%`, `?`
/// or `#` in a name is escaped so it never reads as an escape, query
/// or fragment on the way back in. Every uri the host hands out goes
/// through here, so `file_path` is its exact inverse.
pub(crate) fn file_uri(path: &Path) -> String {
    // A file url is absolute by definition; resolve a relative path
    // against the working directory rather than refuse it.
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match url::Url::from_file_path(&absolute) {
        Ok(url) => url.into(),
        // Only an empty path or an unreadable working directory lands
        // here, and no correct uri exists for either: flag it in debug
        // builds, and in release hand back a best-effort spelling
        // rather than panic the host from a watcher or request handler.
        Err(()) => {
            debug_assert!(false, "file_uri on an unresolvable path: {path:?}");
            format!("file://{}", absolute.to_string_lossy())
        }
    }
}

/// The local path of a `file://` uri, parsed as a real uri: percent
/// escapes are decoded (`file:///a%20b` → `/a b`) and a Windows drive
/// (`file:///C:/x`) comes back as `C:\x`. Besides the host's own
/// encoded uris this still accepts the legacy raw spelling clients
/// send (`file:///a b`), which parses to the same path.
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
        assert_eq!(file_path("ahp-content:/x"), None);
    }

    #[test]
    fn legacy_raw_spaces_still_resolve() {
        assert_eq!(
            file_path("file:///tmp/raw name.txt"),
            Some(PathBuf::from("/tmp/raw name.txt"))
        );
        assert_eq!(file_path("file:///tmp/a b"), file_path("file:///tmp/a%20b"));
    }

    #[test]
    fn emitted_uris_are_encoded() {
        assert_eq!(file_uri(Path::new("/tmp/a b")), "file:///tmp/a%20b");
        assert_eq!(
            file_uri(Path::new("/tmp/report%20final.txt")),
            "file:///tmp/report%2520final.txt"
        );
        assert_eq!(file_uri(Path::new("/tmp/q?.txt")), "file:///tmp/q%3F.txt");
        assert_eq!(file_uri(Path::new("/tmp/h#1.txt")), "file:///tmp/h%231.txt");
    }

    #[test]
    fn literal_escape_query_and_fragment_names_round_trip() {
        for name in [
            "report%20final.txt",
            "q?.txt",
            "h#1.txt",
            "a b",
            "100%.txt",
            "café.md",
            "mix %20?#&+;=.txt",
        ] {
            let path = Path::new("/tmp/dir").join(name);
            assert_eq!(
                file_path(&file_uri(&path)).as_deref(),
                Some(path.as_path()),
                "{name}"
            );
        }
    }

    #[test]
    fn relative_paths_resolve_against_the_working_directory() {
        let expected = std::env::current_dir().unwrap().join("rel/a b");
        assert_eq!(
            file_path(&file_uri(Path::new("rel/a b"))).as_deref(),
            Some(expected.as_path())
        );
    }
}
