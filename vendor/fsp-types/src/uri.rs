//! The `Uri` newtype and `file://` URI ↔ `PathBuf` conversion.
//!
//! URIs cross the wire as plain strings (`#[serde(transparent)]`), but
//! inside the server they are never bare `String`s — the newtype keeps
//! paths, relative paths and URIs from being mixed up.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Uri(String);

impl Uri {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// Builds a `file:` URI for an absolute filesystem path.
    pub fn from_file_path(path: &Path) -> Uri {
        Uri(Url::from_file_path(path)
            .map(String::from)
            .unwrap_or_else(|_| format!("file://{}", path.display())))
    }

    /// The filesystem path of a `file:` URI; errors on other schemes.
    pub fn to_file_path(&self) -> Result<PathBuf, UriError> {
        let url = Url::parse(&self.0).map_err(|e| UriError(format!("{self}: {e}")))?;
        if url.scheme() != "file" {
            return Err(UriError(format!("{self}: scheme is not file")));
        }
        url.to_file_path().map_err(|_| UriError(self.0.clone()))
    }
}

impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Uri {
    fn from(s: String) -> Uri {
        Uri(s)
    }
}

impl From<&str> for Uri {
    fn from(s: &str) -> Uri {
        Uri(s.to_string())
    }
}

impl AsRef<str> for Uri {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("not a usable file: URI: {0}")]
pub struct UriError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        // An absolute path per platform: url refuses drive-less paths on
        // Windows in both directions.
        #[cfg(windows)]
        let p = PathBuf::from(r"C:\tmp\some dir\файл.rs");
        #[cfg(not(windows))]
        let p = PathBuf::from("/tmp/some dir/файл.rs");
        let uri = Uri::from_file_path(&p);
        assert!(uri.as_str().starts_with("file:///"));
        assert_eq!(uri.to_file_path().unwrap(), p);
    }

    #[test]
    fn rejects_non_file() {
        assert!(Uri::from("https://example.com/x").to_file_path().is_err());
    }

    #[test]
    fn serde_transparent() {
        let uri: Uri = serde_json::from_str(r#""file:///a b""#).unwrap();
        assert_eq!(uri.as_str(), "file:///a b");
        assert_eq!(serde_json::to_string(&uri).unwrap(), r#""file:///a b""#);
    }
}
