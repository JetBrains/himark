use ahp_types::messages::{
    JsonRpcError, JsonRpcErrorResponse, JsonRpcMessage, JsonRpcNotification,
    JsonRpcSuccessResponse, JsonRpcVersion,
};
use serde::Serialize;

pub(crate) fn line(message: &JsonRpcMessage) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(message).expect("JSON-RPC messages encode");
    bytes.push(b'\n');
    bytes
}

pub(crate) fn success(id: u64, result: impl Serialize) -> JsonRpcMessage {
    JsonRpcMessage::SuccessResponse(JsonRpcSuccessResponse {
        jsonrpc: JsonRpcVersion::default(),
        id,
        result: serde_json::to_value(result).expect("results encode"),
    })
}

pub(crate) fn failure(id: u64, code: i32, message: impl Into<String>) -> JsonRpcMessage {
    JsonRpcMessage::ErrorResponse(JsonRpcErrorResponse {
        jsonrpc: JsonRpcVersion::default(),
        id,
        error: JsonRpcError {
            code,
            message: message.into(),
            data: None,
        },
    })
}

pub(crate) fn notification(method: &str, params: impl Serialize) -> JsonRpcMessage {
    JsonRpcMessage::Notification(JsonRpcNotification {
        jsonrpc: JsonRpcVersion::default(),
        method: method.to_owned(),
        params: Some(serde_json::to_value(params).expect("params encode")),
    })
}
