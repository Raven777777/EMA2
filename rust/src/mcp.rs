//! MCP 协议核心处理：初始化握手、工具列表和工具调用。

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::state::RelayState;
use crate::tools;

const MAX_SCREENSHOT_BASE64: usize = 32 * 1024 * 1024;
static NEXT_SCREENSHOT_ID: AtomicU64 = AtomicU64::new(0);

/// 处理一条 MCP JSON-RPC 请求。
pub async fn handle(state: &RelayState, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    if !is_valid_request(&request) {
        return Some(error_response(id, -32600, "Invalid Request"));
    }
    let is_notification = id.is_none();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    let response = match method {
        "ping" => Some(json!({ "jsonrpc": "2.0", "result": {} })),
        "initialize" => {
            let requested = request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2024-11-05");
            if !matches!(requested, "2024-11-05" | "2025-03-26") {
                return Some(error_response(id, -32602, "Unsupported protocol version"));
            }
            Some(json!({
                "jsonrpc": "2.0",
                "result": {
                    "protocolVersion": requested,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "edge-browser-mcp", "version": env!("CARGO_PKG_VERSION") }
                }
            }))
        }
        "notifications/initialized" => None,
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "result": { "tools": tools::tool_list() }
        })),
        "tools/call" => {
            let tool_name = request.pointer("/params/name").and_then(Value::as_str);
            let Some(tool_name) = tool_name.filter(|name| tools::contains(name)) else {
                return Some(error_response(id, -32602, "Unknown or missing tool name"));
            };
            let arguments = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Err(message) = tools::validate_arguments(tool_name, &arguments) {
                return Some(error_response(id, -32602, message));
            }
            if is_notification {
                let _ = state.notify_extension(&request);
                return None;
            }
            let mut response = match state.call_extension(&request).await {
                Ok(value) => value,
                Err(message) => json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32000, "message": message }
                }),
            };
            if tool_name == "edge_take_screenshot" {
                response = save_screenshot(state, response).await;
            }
            Some(response)
        }
        _ => Some(json!({
            "jsonrpc": "2.0",
            "error": { "code": -32601, "message": format!("Method not found: {method}") }
        })),
    };

    response
        .map(|mut value| {
            if is_notification {
                return Value::Null;
            }
            value["id"] = id.unwrap_or(Value::Null);
            value
        })
        .filter(|value| !value.is_null())
}

fn is_valid_request(request: &Value) -> bool {
    let Some(object) = request.as_object() else {
        return false;
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str).is_none()
    {
        return false;
    }
    if let Some(id) = object.get("id")
        && !(id.is_null() || id.is_string() || id.is_i64() || id.is_u64() || id.is_f64())
    {
        return false;
    }
    object
        .get("params")
        .is_none_or(|params| params.is_object() || params.is_array() || params.is_null())
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message }
    })
}

async fn save_screenshot(state: &RelayState, response: Value) -> Value {
    let Some(text) = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
    else {
        return response;
    };
    let Some(encoded) = text.strip_prefix("data:image/png;base64,") else {
        return response;
    };
    if encoded.len() > MAX_SCREENSHOT_BASE64 {
        return response;
    }
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(_) => return response,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = NEXT_SCREENSHOT_ID.fetch_add(1, Ordering::Relaxed);
    let path = state.output_dir.join(format!("ema2-{now}-{counter}.png"));
    if tokio::fs::create_dir_all(&state.output_dir).await.is_err() {
        return response;
    }
    match tokio::fs::write(&path, &bytes).await {
        Ok(()) => {
            let mut value = response;
            value["result"]["content"][0]["text"] = json!(format!(
                "screenshot saved: {} ({} bytes)",
                path.display(),
                bytes.len()
            ));
            value
        }
        Err(_) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::state::RelayState;
    use serde_json::json;
    use std::path::PathBuf;

    fn state() -> std::sync::Arc<RelayState> {
        RelayState::new(None, PathBuf::from("target/test-output"))
    }

    #[tokio::test]
    async fn rejects_invalid_json_rpc() {
        let response = handle(&state(), json!({ "method": "ping" })).await.unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn does_not_wait_for_tool_notifications() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "edge_get_tabs", "arguments": {} }
        });
        assert!(handle(&state(), request).await.is_none());
    }

    #[tokio::test]
    async fn rejects_unknown_tools() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "missing", "arguments": {} }
        });
        let response = handle(&state(), request).await.unwrap();
        assert_eq!(response["error"]["code"], -32602);
    }
}
