use ahp_types::common::Uri;
use ahp_types::state::{ContentRef, ToolResultFileEditContent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnapshotRef {
    pub uri: Uri,
    pub content: ContentRef,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffCounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FileEditRefs {
    pub before: Option<FileSnapshotRef>,
    pub after: Option<FileSnapshotRef>,
    pub counts: DiffCounts,
}

impl FileEditRefs {
    pub fn parse(edit: &ToolResultFileEditContent) -> Option<Self> {
        let side = |value: &Option<serde_json::Value>| -> Option<FileSnapshotRef> {
            value
                .as_ref()
                .and_then(|value| serde_json::from_value(value.clone()).ok())
        };
        let before = side(&edit.before);
        let after = side(&edit.after);
        if before.is_none() && after.is_none() {
            return None;
        }
        let counts = edit
            .diff
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        Some(Self {
            before,
            after,
            counts,
        })
    }

    pub fn to_content(&self) -> ToolResultFileEditContent {
        ToolResultFileEditContent {
            before: self
                .before
                .as_ref()
                .map(|side| serde_json::to_value(side).expect("a snapshot serializes")),
            after: self
                .after
                .as_ref()
                .map(|side| serde_json::to_value(side).expect("a snapshot serializes")),
            diff: Some(serde_json::to_value(self.counts).expect("counts serialize")),
        }
    }

    pub fn display_name(&self) -> String {
        let uri = self
            .after
            .as_ref()
            .or(self.before.as_ref())
            .map(|side| side.uri.as_str())
            .unwrap_or("file");
        uri.rsplit('/').next().unwrap_or(uri).to_owned()
    }
}

pub fn snapshot(uri: &str, content_uri: &str) -> FileSnapshotRef {
    FileSnapshotRef {
        uri: uri.to_owned(),
        content: ContentRef {
            uri: content_uri.to_owned(),
            size_hint: None,
            content_type: None,
            nonce: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_round_trip_the_wire_shape() {
        let refs = FileEditRefs {
            before: Some(snapshot("src/scroll.rs", "ahp-content:/b1")),
            after: Some(snapshot("src/scroll.rs", "ahp-content:/a1")),
            counts: DiffCounts {
                added: Some(7),
                removed: Some(2),
            },
        };
        let parsed = FileEditRefs::parse(&refs.to_content()).expect("parses back");
        assert_eq!(parsed.after.unwrap().content.uri, "ahp-content:/a1");
        assert_eq!(parsed.before.unwrap().uri, "src/scroll.rs");
        assert_eq!(parsed.counts.added, Some(7));
        assert_eq!(refs.display_name(), "scroll.rs");
    }

    #[test]
    fn unrecognizable_sides_parse_to_none() {
        let junk = ToolResultFileEditContent {
            before: Some(serde_json::json!({ "weird": true })),
            after: None,
            diff: None,
        };
        assert!(FileEditRefs::parse(&junk).is_none());
    }
}
