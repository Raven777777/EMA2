//! MCP 协议处理（v1.4）：版本协商、工具调用、截图落盘。
//!
//! 面向 AI 的输出约定：
//! - 工具错误不走 JSON-RPC error，而是 `isError: true` 的文本结果
//!   （`error=<code> msg=... hint=...`），让模型能读到并自我纠正
//! - 只有“工具不存在 / 协议非法”才走 JSON-RPC error

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::state::RelayState;
use crate::tools;

/// 截图 base64 上限（与扩展侧 WS 消息上限一致）。
const MAX_SCREENSHOT_BASE64: usize = 48 * 1024 * 1024;
/// 支持的 MCP 协议版本，第一个为最新（未知版本回落值）。
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
static NEXT_SCREENSHOT_ID: AtomicU64 = AtomicU64::new(0);

pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|version| {
            PROTOCOL_VERSIONS
                .iter()
                .copied()
                .find(|supported| *supported == version)
        })
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

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
                .and_then(Value::as_str);
            let version = negotiate_protocol_version(requested);
            if requested != Some(version) {
                tracing::info!(
                    "protocol version {:?} is not supported, negotiated {version}",
                    requested
                );
            }
            Some(json!({
                "jsonrpc": "2.0",
                "result": {
                    "protocolVersion": version,
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
                return Some(tool_error(
                    id,
                    tool_name,
                    "invalid_argument",
                    &message,
                    "fix the arguments; see tools/list for the schema",
                ));
            }
            if is_notification {
                let _ = state.notify_extension(&request);
                return None;
            }
            let timeout = tools::timeout_for(tool_name, &arguments);
            let mut response = match state.call_extension(&request, timeout).await {
                Ok(value) => value,
                Err(message) => {
                    return Some(tool_error(
                        id,
                        tool_name,
                        "extension_error",
                        &message,
                        "check `ema2.exe status`; the extension must be connected",
                    ));
                }
            };
            let inline = arguments
                .get("inline")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if tool_name == "shot" && !inline {
                response = save_screenshot(state, response).await;
            }
            if tool_name == "observe" && arguments.get("har").and_then(Value::as_bool) == Some(true)
            {
                response = save_har(state, response).await;
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

/// AI 友好的工具错误：作为工具结果返回，而不是协议错误。
fn tool_error(id: Option<Value>, tool: &str, code: &str, message: &str, hint: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": {
            "content": [{ "type": "text", "text": format!(
                "error={code} tool={tool} msg={message} hint={hint}"
            )}],
            "isError": true
        }
    })
}

fn extract_image(content: &Value) -> Option<(String, String)> {
    if content.get("type").and_then(Value::as_str) == Some("image") {
        let data = content.get("data").and_then(Value::as_str)?;
        let mime = content
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("image/png");
        return Some((data.to_owned(), mime.to_owned()));
    }
    let text = content.get("text").and_then(Value::as_str)?;
    for mime in ["image/png", "image/jpeg", "image/webp"] {
        if let Some(data) = text.strip_prefix(&format!("data:{mime};base64,")) {
            return Some((data.to_owned(), mime.to_owned()));
        }
    }
    None
}

fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// 截图默认落盘，只把路径给 AI（避免 base64 塞爆上下文）。
async fn save_screenshot(state: &RelayState, mut response: Value) -> Value {
    let Some(content) = response
        .pointer_mut("/result/content")
        .and_then(Value::as_array_mut)
    else {
        return response;
    };
    let Some(index) = content
        .iter()
        .position(|item| extract_image(item).is_some())
    else {
        return response;
    };
    let Some((encoded, mime)) = extract_image(&content[index]) else {
        return response;
    };
    if encoded.len() > MAX_SCREENSHOT_BASE64 {
        content[index] = json!({
            "type": "text",
            "text": format!(
                "error=screenshot_too_large msg={} bytes of base64 (limit {MAX_SCREENSHOT_BASE64}) hint=use shot without inline, or capture a smaller window",
                encoded.len()
            ),
        });
        mark_error(&mut response);
        return response;
    }
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&encoded) {
        Ok(bytes) => bytes,
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!("error=screenshot_decode_failed msg={error} hint=retry; if it persists reload the extension"),
            });
            mark_error(&mut response);
            return response;
        }
    };
    if bytes.is_empty() {
        content[index] = json!({
            "type": "text",
            "text": "error=screenshot_empty msg=capture returned 0 bytes hint=make the browser window visible and retry",
        });
        mark_error(&mut response);
        return response;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = NEXT_SCREENSHOT_ID.fetch_add(1, Ordering::Relaxed);
    let path = state
        .output_dir
        .join(format!("ema2-{now}-{counter}.{}", image_extension(&mime)));
    if let Err(error) = tokio::fs::create_dir_all(&state.output_dir).await {
        content[index] = json!({
            "type": "text",
            "text": format!(
                "error=screenshot_not_saved msg=cannot create {}: {error} hint=set EDGE_MCP_OUTPUT_DIR to a writable path",
                state.output_dir.display()
            ),
        });
        mark_error(&mut response);
        return response;
    }
    match tokio::fs::write(&path, &bytes).await {
        Ok(()) => {
            content[index] = json!({
                "type": "text",
                "text": format!("path={}
            bytes={} mime={mime}", path.display(), bytes.len()),
            });
            response
        }
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!(
                    "error=screenshot_not_saved msg={} : {error} hint=set EDGE_MCP_OUTPUT_DIR to a writable path",
                    path.display()
                ),
            });
            mark_error(&mut response);
            response
        }
    }
}

/// 把 `observe har=true` 的 HAR JSON 落盘，只把路径返回给 AI。
async fn save_har(state: &RelayState, mut response: Value) -> Value {
    let Some(content) = response
        .pointer_mut("/result/content")
        .and_then(Value::as_array_mut)
    else {
        return response;
    };
    let Some(index) = content
        .iter()
        .position(|item| item.get("type").and_then(Value::as_str) == Some("text"))
    else {
        return response;
    };
    let Some(text) = content[index].get("text").and_then(Value::as_str) else {
        return response;
    };
    if !text.trim_start().starts_with('{') {
        return response;
    }
    let entries = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/log/entries")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = NEXT_SCREENSHOT_ID.fetch_add(1, Ordering::Relaxed);
    let path = state.output_dir.join(format!("ema2-{now}-{counter}.har"));
    if let Err(error) = tokio::fs::create_dir_all(&state.output_dir).await {
        content[index] = json!({
            "type": "text",
            "text": format!("error=har_not_saved msg=cannot create {}: {error} hint=set EDGE_MCP_OUTPUT_DIR to a writable path", state.output_dir.display()),
        });
        mark_error(&mut response);
        return response;
    }
    match tokio::fs::write(&path, text.as_bytes()).await {
        Ok(()) => {
            content[index] = json!({
                "type": "text",
                "text": format!("path={}
            bytes={} entries={entries}", path.display(), text.len()),
            });
            response
        }
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!("error=har_not_saved msg={} : {error} hint=set EDGE_MCP_OUTPUT_DIR to a writable path", path.display()),
            });
            mark_error(&mut response);
            response
        }
    }
}

fn mark_error(response: &mut Value) {
    if let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) {
        result.insert("isError".to_owned(), Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use super::{handle, negotiate_protocol_version, save_screenshot};
    use crate::state::RelayState;
    use serde_json::{Value, json};
    use std::path::PathBuf;

    fn state() -> std::sync::Arc<RelayState> {
        RelayState::new(None, PathBuf::from("target/test-output"))
    }

    fn temp_state() -> (std::sync::Arc<RelayState>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ema2-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (RelayState::new(None, dir.clone()), dir)
    }

    fn image_response(data: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "result": {
                "content": [{ "type": "image", "data": data, "mimeType": "image/png" }]
            }
        })
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
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        });
        assert!(handle(&state(), request).await.is_none());
    }

    #[tokio::test]
    async fn rejects_unknown_tools_with_protocol_error() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "edge_get_tabs", "arguments": {} }
        });
        let response = handle(&state(), request).await.unwrap();
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn invalid_arguments_become_tool_errors_with_hint() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "wait", "arguments": { "time": 9999 } }
        });
        let response = handle(&state(), request).await.unwrap();
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("error=invalid_argument"), "{text}");
        assert!(text.contains("hint="), "{text}");
    }

    #[tokio::test]
    async fn missing_extension_is_a_tool_error() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        });
        let response = handle(&state(), request).await.unwrap();
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("extension_error"), "{text}");
    }

    #[tokio::test]
    async fn lists_all_tools() {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let response = handle(&state(), request).await.unwrap();
        assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 11);
    }

    #[test]
    fn negotiates_protocol_versions() {
        assert_eq!(negotiate_protocol_version(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate_protocol_version(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate_protocol_version(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate_protocol_version(Some("2099-01-01")), "2025-06-18");
        assert_eq!(negotiate_protocol_version(None), "2025-06-18");
    }

    #[tokio::test]
    async fn screenshot_is_saved_as_a_compact_path_line() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
        let response = save_screenshot(&state, image_response(&data)).await;
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("path="), "{text}");
        assert!(text.contains("bytes=14"), "{text}");
        assert!(response["result"].get("isError").is_none());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn annotated_screenshot_keeps_text_and_saves_the_image() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"annotated-png");
        let response = json!({
            "jsonrpc": "2.0",
            "result": { "content": [
                { "type": "text", "text": "[e1] button \"Submit\" @10,20 30x8
        marks=1" },
                { "type": "image", "data": data, "mimeType": "image/png" }
            ] }
        });
        let saved = save_screenshot(&state, response).await;
        let content = saved["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert!(content[0]["text"].as_str().unwrap().starts_with("[e1]"));
        assert!(content[1]["text"].as_str().unwrap().starts_with("path="));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn screenshot_decode_failure_is_compact_error() {
        let (state, dir) = temp_state();
        let response = save_screenshot(&state, image_response("not base64!!!")).await;
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("error=screenshot_decode_failed"), "{text}");
        assert!(text.contains("hint="), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn non_screenshot_results_are_left_untouched() {
        let (state, _dir) = temp_state();
        let response = json!({
            "jsonrpc": "2.0",
            "result": { "content": [{ "type": "text", "text": "hello" }] }
        });
        let saved = save_screenshot(&state, response.clone()).await;
        assert_eq!(saved, response);
    }
}
