// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use crate::Uri;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uid(pub u128);

impl std::fmt::Display for Uid {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{:032x}", self.0)
    }
}

impl std::fmt::Debug for Uid {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Uid({:032x})", self.0)
    }
}

impl std::str::FromStr for Uid {
    type Err = String;
    fn from_str(spelled: &str) -> Result<Self, String> {
        let compact: String = spelled.chars().filter(|c| *c != '-').collect();
        if compact.len() != 32 {
            return Err(format!("a uid is 32 hex digits, got {spelled:?}"));
        }
        u128::from_str_radix(&compact, 16)
            .map(Uid)
            .map_err(|_| format!("not hex: {spelled:?}"))
    }
}

impl Serialize for Uid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Uid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spelled = String::deserialize(deserializer)?;
        spelled.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: u64,
    pub character: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Replacement {
    pub range: TextRange,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct TextOperation {
    pub replacements: Vec<Replacement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocumentParams {
    pub channel: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<Uri>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocumentResult {
    pub document: Uri,

    pub version: Uid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<Uri>,
    pub text: String,

    pub version: Uid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreDocumentParams {
    pub channel: Uri,
    pub uri: Uri,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreDocumentResult {
    pub version: Uid,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentApplied {
    pub base: Uid,
    pub operation: TextOperation,

    pub id: Uid,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

pub const DOCUMENT_APPLIED: &str = "document/applied";

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentClosed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub const DOCUMENT_CLOSED: &str = "document/closed";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uids_round_trip_hex_and_tolerate_dashes() {
        let uid: Uid = "00000000-0000-0000-0000-0000000000ff"
            .parse()
            .expect("parses");
        assert_eq!(uid, Uid(0xff));
        assert_eq!(uid.to_string(), "000000000000000000000000000000ff");
        let wire = serde_json::to_value(Uid(0xabc)).expect("encodes");
        assert_eq!(wire, serde_json::json!("00000000000000000000000000000abc"));
        assert!("not-a-uid".parse::<Uid>().is_err());
        assert!(serde_json::from_value::<Uid>(serde_json::json!("xyz")).is_err());
    }
}
