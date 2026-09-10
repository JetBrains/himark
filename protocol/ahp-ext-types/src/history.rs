use serde::{Deserialize, Serialize};

use crate::Uri;

pub const HISTORY_CHANGE_KIND: &str = "history";

pub const HISTORY_PREPENDED: &str = "history/prepended";

pub const HISTORY_APPENDED: &str = "history/appended";

pub const HISTORY_RESET: &str = "history/reset";

pub const HISTORY_GROW: &str = "history/grow";

pub const HISTORY_COMMIT: &str = "history/commit";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryState {
    pub status: HistoryStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub head: HistoryHead,

    pub commits: Vec<Commit>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HistoryStatus {
    #[default]
    Computing,
    Ready,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryHead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub id: String,

    pub parents: Vec<String>,

    pub summary: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub author: CommitAuthor,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<CommitRef>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outgoing: bool,

    pub changeset: Uri,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommitAuthor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    pub name: String,

    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPrepended {
    pub commits: Vec<Commit>,

    pub head: HistoryHead,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAppended {
    pub commits: Vec<Commit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryReset {
    pub state: HistoryState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryGrow {
    pub before: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCommit {
    pub message: String,
}

pub fn action_value<T: Serialize>(kind: &str, body: &T) -> serde_json::Value {
    let mut value = serde_json::to_value(body).expect("a history action serializes");
    value["type"] = serde_json::Value::String(kind.to_owned());
    value
}
