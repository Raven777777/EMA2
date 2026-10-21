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
}

impl Drop for ExtensionSession {
    fn drop(&mut self) {
        if self.state.unregister_extension(self.generation) {
            tracing::info!("extension disconnected (generation {})", self.generation);
        }
    }
}

/// 扩展 WS 连接主循环：把响应按 id 送回到等待的 CLI 请求。
pub async fn handle_extension_ws(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let generation = state.register_extension(tx);
    let _session = ExtensionSession {
        state: state.clone(),
        generation,
    };
    tracing::info!("extension connected (generation {generation})");

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Reply::Welcome(welcome) = on_extension_message(&state, generation, &text) {
                            if sink.send(Message::Text(welcome.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
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
}

fn on_extension_message(state: &Arc<RelayState>, generation: u64, text: &str) -> Reply {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("ignored non-JSON extension message");
        return Reply::None;
    };
    // 任何扩展消息都是活性证据：ping 响应、工具结果都计入 last_ping。
    if let Some(id) = value.get("id") {
        let id = id.clone();
        state.complete_pending(&id, generation, value);
        return Reply::None;
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        tracing::warn!("ignored invalid extension message");
        return Reply::None;
    };
    match method {
        // 扩展连上后先声明角色；daemon 回 welcome（无 auth）。
        "bridge/hello" => {
            let version = value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            state.set_extension_version(version);
            tracing::info!("bridge/hello from extension {version}");
            Reply::Welcome(json!({
                "jsonrpc": "2.0",
                "method": "bridge/welcome",
                "name": "ema2",
                "version": crate::paths::PRODUCT_VERSION,
            }))
        }
        // 旧扩展会对无 id 的 ping 回一条无 id 的 pong；忽略即可。
        "ping" | "pong" => Reply::None,
        _ => Reply::None,
    }
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
