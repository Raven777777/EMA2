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

use crate::state::RelayState;

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
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
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
                        if sink.send(msg).await.is_err() { break; }
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
    // 任何扩展消息都是活性证据：ping 响应、工具结果都计入 last_ping。
    if let Some(id) = value.get("id") {
        if !active {
            tracing::warn!("ignored extension response before bridge/hello");
            return Reply::None;
        }
        let id = id.clone();
        state.complete_pending(&id, generation, value);
        return Reply::None;
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        tracing::warn!("ignored invalid extension message");
        return Reply::None;
    };
    match method {
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
        _ => {
            if !active {
                return handshake_error("bridge/hello is required before tools/call");
            }
            Reply::None
        }
    }
}

/// Chrome extension ID：32 个 a-p 字符。Origin 必须逐字等于 `chrome-extension://<id>`。
pub(crate) fn extension_id_valid(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|ch| ('a'..='p').contains(&ch))
}

/// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
pub fn spawn_keepalive(state: Arc<RelayState>, interval: Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(interval);
        interval.tick().await; // 跳过首次立即触发
        loop {
            interval.tick().await;
            state.send_keepalive();
        }
    });
}
