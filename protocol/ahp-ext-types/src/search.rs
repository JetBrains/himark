use serde::{Deserialize, Serialize};

use crate::Uri;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchKind {
    #[default]
    Text,

    Regex,

    Fuzzy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchTarget {
    #[default]
    Content,

    Path,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub channel: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<Uri>>,
    pub query: String,
    #[serde(default)]
    pub kind: SearchKind,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub target: SearchTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<Uri>,
    #[serde(default)]
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_serialize_spec_shaped_and_default_on_the_way_in() {
        let params = SearchParams {
            channel: "hihost:/s1".to_owned(),
            folders: None,
            query: "conflation".to_owned(),
            kind: SearchKind::Fuzzy,
            case_sensitive: false,
            target: SearchTarget::Path,
            limit: Some(10),
        };
        let wire = serde_json::to_value(&params).expect("encodes");
        assert_eq!(wire["kind"], "fuzzy");
        assert_eq!(wire["target"], "path");
        assert_eq!(wire["caseSensitive"], false);
        assert!(wire.get("folders").is_none(), "absent, not null");

        let minimal: SearchParams = serde_json::from_value(serde_json::json!({
            "channel": "hihost:/s1", "query": "x",
        }))
        .expect("spec defaults fill in");
        assert_eq!(minimal.kind, SearchKind::Text);
        assert_eq!(minimal.target, SearchTarget::Content);
        assert!(!minimal.case_sensitive);
        assert!(minimal.folders.is_none() && minimal.limit.is_none());
    }
}
