use std::path::{Path, PathBuf};

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

pub(crate) fn file_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

pub(crate) fn file_path_str(uri: &str) -> Option<&str> {
    uri.strip_prefix("file://")
}
