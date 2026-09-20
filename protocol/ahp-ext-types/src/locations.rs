// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use crate::{SearchKind, Uri};

pub const LOCATIONS_EXTEND: &str = "locations/extend";

/// One document location with its display context baked in, so a
/// consumer renders a result row without fetching the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub uri: Uri,

    /// 0-based line.
    pub line: u32,

    /// 0-based column, in UTF-8 bytes within the line — the unit the
    /// Documents extension pins for `TextPosition.character` and the
    /// LSP utf-8 negotiation forwards.
    pub column: u32,

    /// Match length in bytes; may run past the line end (a multi-line
    /// match), consumers clamp for display.
    #[serde(default)]
    pub length: u32,

    /// A slice of the match's line: usually the whole hard line,
    /// for huge lines a bounded window around the match.
    pub context: String,

    /// The column (same unit) where `context` begins — 0 unless the
    /// line was too long to ship whole. The match renders inside
    /// `context` at `column - contextColumnStart`.
    #[serde(default)]
    pub context_column_start: u32,
}

/// Both the state and the action of an `ahp-locations:/…` channel.
/// The reducer is concatenation: `locations` append, the flags latch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocationList {
    pub locations: Vec<Location>,

    /// The producer finished. Latches true.
    #[serde(default)]
    pub done: bool,

    /// The producer stopped early — a limit or a cancellation cut the
    /// result off. Latches true.
    #[serde(default)]
    pub truncated: bool,
}

impl LocationList {
    /// The channel reducer.
    pub fn concat(&mut self, action: LocationList) {
        self.locations.extend(action.locations);
        self.done |= action.done;
        self.truncated |= action.truncated;
    }
}

/// `searchLocations` — the streaming content search. Answers the
/// minted `ahp-locations:/…` channel; results stream as
/// `locations/extend` actions; unsubscribing cancels the walk.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLocationsParams {
    pub channel: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<Uri>>,
    pub query: String,
    /// `text` or `regex`; `fuzzy` is refused — this is content search.
    #[serde(default)]
    pub kind: SearchKind,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Total location cap; server-capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// `lsp/locations` — the location-answering LSP asks (references,
/// implementations), streamed over the same channel shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspLocationsParams {
    pub channel: Uri,
    /// Whitelisted: `textDocument/references`,
    /// `textDocument/implementation`.
    pub method: String,
    /// The LSP request's params, verbatim (utf-8 positions).
    pub params: serde_json::Value,
}

/// The answer of both minting requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationsChannelResult {
    pub channel: Uri,
}

pub fn action_value<T: Serialize>(kind: &str, body: &T) -> serde_json::Value {
    let mut value = serde_json::to_value(body).expect("a locations action serializes");
    value["type"] = serde_json::Value::String(kind.to_owned());
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(line: u32) -> Location {
        Location {
            uri: "file:///w/src/lib.rs".to_owned(),
            line,
            column: 4,
            length: 9,
            context: "    conflation of concerns".to_owned(),
            context_column_start: 0,
        }
    }

    #[test]
    fn wire_shape_and_defaults() {
        let list = LocationList {
            locations: vec![location(7)],
            done: false,
            truncated: false,
        };
        let wire = serde_json::to_value(&list).expect("encodes");
        assert_eq!(wire["locations"][0]["contextColumnStart"], 0);
        assert_eq!(wire["locations"][0]["line"], 7);

        let minimal: Location = serde_json::from_value(serde_json::json!({
            "uri": "file:///w/a", "line": 1, "column": 2, "context": "x",
        }))
        .expect("length and contextColumnStart default");
        assert_eq!(minimal.length, 0);
        assert_eq!(minimal.context_column_start, 0);

        let params: SearchLocationsParams = serde_json::from_value(serde_json::json!({
            "channel": "hihost:/s1", "query": "x",
        }))
        .expect("spec defaults fill in");
        assert_eq!(params.kind, SearchKind::Text);
        assert!(!params.case_sensitive);
        assert!(params.folders.is_none() && params.limit.is_none());
    }

    #[test]
    fn reducer_concatenates_and_latches() {
        let mut state = LocationList::default();
        state.concat(LocationList {
            locations: vec![location(1), location(2)],
            done: false,
            truncated: false,
        });
        state.concat(LocationList {
            locations: vec![location(3)],
            done: true,
            truncated: true,
        });
        state.concat(LocationList::default());
        assert_eq!(
            state.locations.iter().map(|l| l.line).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(state.done && state.truncated, "flags latch");
    }

    #[test]
    fn action_rides_the_type_field() {
        let action = action_value(
            LOCATIONS_EXTEND,
            &LocationList {
                locations: vec![location(1)],
                done: true,
                truncated: false,
            },
        );
        assert_eq!(action["type"], "locations/extend");
        assert_eq!(action["done"], true);
        let body: LocationList = serde_json::from_value(action).expect("type field is ignored");
        assert_eq!(body.locations.len(), 1);
    }
}
