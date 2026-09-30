//! JSON-RPC 2.0 message types (PROTOCOL.md §1.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request id: integer or string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Int(i64),
    Str(String),
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Id::Int(n) => write!(f, "{n}"),
            Id::Str(s) => write!(f, "{s}"),
        }
    }
}

/// Any incoming JSON-RPC message, classified by shape.
#[derive(Debug, Clone, Deserialize)]
pub struct RawMessage {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Id>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Clone)]
pub enum Incoming {
    Request {
        id: Id,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    /// A response. FSP servers never send requests, so a conforming client
    /// never produces these; tolerated and ignored for robustness.
    Response,
}

impl RawMessage {
    pub fn classify(self) -> Incoming {
        match (self.id, self.method) {
            (Some(id), Some(method)) => Incoming::Request {
                id,
                method,
                params: self.params,
            },
            (None, Some(method)) => Incoming::Notification {
                method,
                params: self.params,
            },
            _ => Incoming::Response,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Error codes used by FSP (PROTOCOL.md §11).
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const CONTENT_MODIFIED: i64 = -32801;
    pub const REQUEST_FAILED: i64 = -32803;
}

/// A request as a client sends it (servers parse via `RawMessage`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMessage {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    pub params: Value,
}

impl RequestMessage {
    pub fn new(id: Id, method: impl Into<String>, params: Value) -> Self {
        RequestMessage {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseMessage {
    pub jsonrpc: &'static str,
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl ResponseMessage {
    pub fn ok(id: Id, result: Value) -> Self {
        ResponseMessage {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Id, code: i64, message: impl Into<String>) -> Self {
        ResponseMessage {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(ResponseError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationMessage {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: Value,
}

impl NotificationMessage {
    pub fn new(method: &'static str, params: Value) -> Self {
        NotificationMessage {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}
