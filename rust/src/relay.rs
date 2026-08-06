//! Edge 扩展的 WebSocket 连接处理 + 保活。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::state::RelayState;

/// 扩展 WS 连接主循环：把响应按 id 送回到等待的 MCP 请求。
pub async fn handle_extension_ws(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let generation = state.register_extension(tx);
    tracing::info!("extension connected");

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        on_extension_message(&state, generation, &text);
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

    state.unregister_extension(generation);
    tracing::info!("extension disconnected");
}

fn on_extension_message(state: &Arc<RelayState>, generation: u64, text: &str) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if method == "bridge/hello" || method == "pong" {
        return;
    }
    if v.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || (v.get("result").is_none() && v.get("error").is_none())
    {
        tracing::warn!("ignored invalid extension response");
        return;
    }
    if let Some(id) = v.get("id") {
        let id = id.clone();
        state.complete_pending(&id, generation, v);
    }
}

/// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
pub fn spawn_keepalive(state: Arc<RelayState>, cfg: Config) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(cfg.keepalive);
        interval.tick().await; // 跳过首次立即触发
        loop {
            interval.tick().await;
            state.send_keepalive();
        }
    });
}
