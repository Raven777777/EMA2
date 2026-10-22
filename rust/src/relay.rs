//! Edge 扩展的 WebSocket 连接处理 + 保活。
//!
//! v2.0 起无 token、无验签：扩展裸连 `ws://127.0.0.1:7744/`，
//! hello/welcome 只交换角色与版本，随后扩展按 `tools/call` 执行工具。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::state::{EXT_CHANNEL_CAPACITY, RelayState, SEND_TIMEOUT};

/// 连接存续期间持有；Drop 时注销 generation。
///
/// 用 Drop 而不是在循环末尾显式调用：任务被 panic 取消时（例如扩展送回畸形
/// 消息触发序列化异常）也必须注销，否则 daemon 会一直以为扩展还连着。
struct ExtensionSession {
    state: Arc<RelayState>,
    generation: u64,
    active: bool,
}

impl Drop for ExtensionSession {
    fn drop(&mut self) {
        if self.active {
            if self.state.unregister_extension(self.generation) {
                tracing::info!("extension disconnected (generation {})", self.generation);
            }
        } else {
            self.state.unregister_pending_extension(self.generation);
        }
    }
}

/// 扩展 WS 连接主循环：先完成 hello 验证，再把响应按 id 送回到等待的 CLI 请求。
pub async fn handle_extension_ws(
    socket: WebSocket,
    state: Arc<RelayState>,
    origin: Option<String>,
) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(EXT_CHANNEL_CAPACITY);
    let generation = state.register_pending_extension(tx);
    let mut session = ExtensionSession {
        state: state.clone(),
        generation,
        active: false,
    };
    tracing::info!("extension WebSocket opened (generation {generation}), awaiting hello");

    let handshake = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(handshake);
    loop {
        tokio::select! {
            _ = &mut handshake, if !session.active => {
                tracing::warn!("extension handshake timed out (generation {generation})");
                break;
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match on_extension_message(
                            &state,
                            generation,
                            &text,
                            origin.as_deref(),
                            session.active,
                        ) {
                            Reply::None => {}
                            Reply::Welcome(welcome) => {
                                session.active = true;
                                tracing::info!("extension connected (generation {generation})");
                                if sink.send(Message::Text(welcome.to_string().into())).await.is_err() {
                                    break;
                                }
                            }
                            Reply::Reject(error) => {
                                let payload = json!({ "jsonrpc": "2.0", "error": error });
                                let _ = sink.send(Message::Text(payload.to_string().into())).await;
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            outgoing = rx.recv() => {
                match outgoing {
                    Some(msg) => {
                        // 扩展不读 WS（或 TCP 半开）时 sink.send 会一直挂起，
                        // 把整个 relay 任务连同新命令一起拖死；超时即视为断连。
                        match tokio::time::timeout(SEND_TIMEOUT, sink.send(msg)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break,
                            Err(_) => {
                                tracing::warn!(
                                    "extension send timed out after {}s (generation {generation}); dropping connection",
                                    SEND_TIMEOUT.as_secs()
                                );
                                // 注销由这里和 ExtensionSession::drop 共同兜底：
                                // 先显式注销，Drop 再调用一次是无害的幂等操作。
                                state.unregister_extension(generation);
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

enum Reply {
    None,
    Welcome(Value),
    Reject(Value),
}

fn handshake_error(message: &str) -> Reply {
    Reply::Reject(json!({ "code": -32000, "message": message }))
}

fn on_extension_message(
    state: &Arc<RelayState>,
    generation: u64,
    text: &str,
    origin: Option<&str>,
    active: bool,
) -> Reply {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("ignored non-JSON extension message");
        return Reply::None;
    };
    // 握手完成后，任何 JSON 消息都证明连接活着（含无 id 的 pong/心跳）。
    if active {
        state.touch_extension();
    }
    // 先看 method：只有 method 缺省/null 且带 result|error 的才是响应。
    // 扩展主动发来的 RPC 请求（method 非空、带 id）绝不能被 complete_pending 吞掉。
    if let Some(method) = value.get("method").filter(|method| !method.is_null()) {
        let Some(method) = method.as_str() else {
            tracing::warn!("ignored extension message with a non-string method");
            return Reply::None;
        };
        return match method {
            // 扩展连上后先声明角色；daemon 校验 role、Origin 和 extensionId 后才启用工具通道。
            "bridge/hello" => {
                if active {
                    return Reply::None;
                }
                if value.get("role").and_then(Value::as_str) != Some("extension") {
                    return handshake_error("bridge/hello must declare role=extension");
                }
                let Some(extension_id) = value.get("extensionId").and_then(Value::as_str) else {
                    return handshake_error("bridge/hello must include extensionId");
                };
                let expected = format!("chrome-extension://{extension_id}");
                if !extension_id_valid(extension_id) || origin != Some(expected.as_str()) {
                    return handshake_error("extensionId does not match the WebSocket Origin");
                }
                let version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if let Err(error) = state.activate_extension(generation, extension_id, version) {
                    return handshake_error(&error);
                }
                tracing::info!("bridge/hello from extension {version} ({extension_id})");
                Reply::Welcome(json!({
                    "jsonrpc": "2.0",
                    "method": "bridge/welcome",
                    "name": "ema2",
                    "version": crate::paths::PRODUCT_VERSION,
                }))
            }
            // 旧扩展会对无 id 的 ping 回一条无 id 的 pong；忽略即可。
            "ping" | "pong" => Reply::None,
            other => {
                tracing::warn!(
                    "ignored unsupported extension method {other:?} (generation {generation})"
                );
                Reply::None
            }
        };
    }
    if value.get("result").is_none() && value.get("error").is_none() {
        tracing::warn!("ignored extension message without method, result or error");
        return Reply::None;
    }
    if !active {
        tracing::warn!("ignored extension response before bridge/hello");
        return Reply::None;
    }
    let Some(id) = value.get("id").cloned() else {
        tracing::warn!("ignored extension response without an id");
        return Reply::None;
    };
    state.complete_pending(&id, generation, value);
    Reply::None
}

/// Chrome extension ID：32 个 a-p 字符。Origin 必须逐字等于 `chrome-extension://<id>`。
pub(crate) fn extension_id_valid(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|ch| ('a'..='p').contains(&ch))
}

/// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
///
/// `interval` 同时是半开连接的判定基准：超过 3 个周期没有收到任何扩展消息
/// （`state::send_keepalive` 内的 janitor）就把 `ext` 摘掉，`/health` 随即显示断开。
pub fn spawn_keepalive(state: Arc<RelayState>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // 跳过首次立即触发
        loop {
            ticker.tick().await;
            state.send_keepalive(interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const EXT_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
    const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

    fn state() -> Arc<RelayState> {
        RelayState::new(PathBuf::from("target/test-output"), 7744)
    }

    fn hello(extension_id: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "bridge/hello",
            "role": "extension",
            "extensionId": extension_id,
            "version": "9.9.9",
        })
        .to_string()
    }

    fn tool_call() -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        })
    }

    #[test]
    fn non_json_messages_are_ignored() {
        let state = state();
        assert!(matches!(
            on_extension_message(&state, 1, "not json", Some(ORIGIN), false),
            Reply::None
        ));
        assert!(!state.extension_connected());
    }

    #[test]
    fn hello_with_a_matching_origin_activates_the_extension() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let generation = state.register_pending_extension(tx);
        let reply = on_extension_message(&state, generation, &hello(EXT_ID), Some(ORIGIN), false);
        assert!(matches!(reply, Reply::Welcome(_)), "hello must be accepted");
        assert!(state.extension_connected());
        assert_eq!(state.extension_version().as_deref(), Some("9.9.9"));
    }

    #[test]
    fn hello_with_a_mismatched_origin_or_extension_id_is_rejected() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let generation = state.register_pending_extension(tx);
        let reply = on_extension_message(
            &state,
            generation,
            &hello(EXT_ID),
            Some("chrome-extension://ponmlkjihgfedcbaponmlkjihgfedcba"),
            false,
        );
        assert!(matches!(reply, Reply::Reject(_)));
        assert!(!state.extension_connected());

        // Origin 对但 extensionId 不是合法的 32 个 a-p 字符。
        let reply = on_extension_message(
            &state,
            generation,
            &hello("not-a-valid-extension-id"),
            Some(ORIGIN),
            false,
        );
        assert!(matches!(reply, Reply::Reject(_)));
        assert!(!state.extension_connected());
    }

    /// hello 之前的“响应”必须丢弃，否则旧连接的响应会误完成新请求。
    #[tokio::test]
    async fn responses_before_hello_are_discarded() {
        let state = state();
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let generation = state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&tool_call(), std::time::Duration::from_secs(5))
                    .await
            })
        };
        let Some(Message::Text(text)) = rx.recv().await else {
            panic!("expected a forwarded tools/call");
        };
        let forwarded: Value = serde_json::from_str(&text).unwrap();
        let id = forwarded["id"].clone();
        let response = json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string();

        let reply = on_extension_message(&state, generation, &response, Some(ORIGIN), false);
        assert!(matches!(reply, Reply::None));
        assert_eq!(
            state.pending_count(),
            1,
            "pre-hello response resolved a call"
        );

        state.complete_pending(
            &id,
            generation,
            json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        );
        assert!(call.await.unwrap().is_ok());
    }

    /// 扩展主动发来的 RPC 请求带 id 但 method 非空，不能被当作响应。
    #[tokio::test]
    async fn requests_with_a_method_are_not_treated_as_responses() {
        let state = state();
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let generation = state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&tool_call(), std::time::Duration::from_secs(5))
                    .await
            })
        };
        let Some(Message::Text(text)) = rx.recv().await else {
            panic!("expected a forwarded tools/call");
        };
        let forwarded: Value = serde_json::from_str(&text).unwrap();
        let id = forwarded["id"].clone();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "bridge/extension-request",
            "params": {},
        })
        .to_string();

        let reply = on_extension_message(&state, generation, &request, Some(ORIGIN), true);
        assert!(matches!(reply, Reply::None));
        assert_eq!(
            state.pending_count(),
            1,
            "an extension request must not resolve the pending call"
        );

        // ping/pong 也不应被当成响应。
        let ping = json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string();
        let _ = on_extension_message(&state, generation, &ping, Some(ORIGIN), true);
        assert_eq!(state.pending_count(), 1);

        state.complete_pending(
            &id,
            generation,
            json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        );
        assert!(call.await.unwrap().is_ok());
    }

    #[test]
    fn extension_ids_are_32_a_to_p_characters() {
        assert!(extension_id_valid("abcdefghijklmnopabcdefghijklmnop"));
        assert!(!extension_id_valid("abcdefghijklmnopabcdefghijklmno"));
        assert!(!extension_id_valid("abcdefghijklmnopabcdefghijklmnopq"));
        assert!(!extension_id_valid("abcdefghijklmnopabcdefghijklmno0"));
        assert!(!extension_id_valid("ABCDEFGHIJKLMNOPABCDEFGHIJKLMNOP"));
        assert!(!extension_id_valid(""));
    }
}
