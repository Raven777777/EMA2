//! MCP 协议核心处理：初始化握手、工具列表、工具调用（转发给扩展）。

use serde_json::{json, Value};

use crate::state::RelayState;
use crate::tools;

/// 处理一条 MCP JSON-RPC 请求。
/// 返回 `Some(完整响应)` 表示需要回复；`None` 表示通知（无需回复）。
pub async fn handle(state: &RelayState, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    let response: Option<Value> = match method {
        "ping" => Some(json!({ "jsonrpc": "2.0", "result": "pong" })),
        "initialize" => {
            let requested = request
                .pointer("/params/protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05");
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
        "tools/list" => Some(json!({ "jsonrpc": "2.0", "result": { "tools": tools::tool_list() } })),
        "tools/call" => {
            let tool_name = request
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut resp = match state.call_extension(&request).await {
                Ok(ext_resp) => ext_resp,
                Err(msg) => json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32000, "message": msg }
                }),
            };
            if tool_name == "edge_take_screenshot" {
                resp = save_screenshot(state, resp);
            }
            Some(resp)
        }
        _ => Some(json!({
            "jsonrpc": "2.0",
            "error": { "code": -32601, "message": format!("Method not found: {method}") }
        })),
    };

    match response {
        Some(mut r) => {
            r["id"] = id.unwrap_or(Value::Null);
            Some(r)
        }
        None => None,
    }
}

/// 把 edge_take_screenshot 返回的 base64 data URL 落盘，并把响应改为文件路径。
fn save_screenshot(state: &RelayState, resp: Value) -> Value {
    let Some(text) = resp
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
    else {
        return resp;
    };
    let Some(b64) = text.strip_prefix("data:image/png;base64,") else {
        return resp;
    };
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => b,
        Err(_) => return resp,
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = state.output_dir.join(format!("ema2-{ts}.png"));
    if std::fs::create_dir_all(&state.output_dir).is_err() {
        return resp;
    }
    match std::fs::write(&path, &bytes) {
        Ok(_) => {
            let mut r = resp;
            r["result"]["content"][0]["text"] = json!(format!(
                "screenshot saved: {} ({} bytes)",
                path.display(),
                bytes.len()
            ));
            r
        }
        Err(_) => resp,
    }
}
