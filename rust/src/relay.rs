//! Edge 扩展的 WebSocket 连接处理 + 保活。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::state::RelayState;

/// 扩展 WS 连接主循环：把响应按 id 送回到等待的 MCP 请求。
pub async fn handle_extension_ws(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let generation = state.register_extension(tx);
    tracing::info!("extension connected (generation {generation})");

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
    tracing::info!("extension disconnected (generation {generation})");
}

fn on_extension_message(state: &Arc<RelayState>, generation: u64, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("ignored non-JSON extension message");
        return;
    };
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        match method {
            // 扩展连上后先声明角色；中继回 welcome。扩展在收到 welcome 之前
            // 不执行任何工具调用，避免误连到占用同一端口的其它本地服务。
            "bridge/hello" => {
                let version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                tracing::info!("bridge/hello from extension {version}");
                let welcome = json!({
                    "jsonrpc": "2.0",
                    "method": "bridge/welcome",
                    "name": "ema2",
                    "version": env!("CARGO_PKG_VERSION"),
                });
                if let Err(error) = state.notify_extension(&welcome) {
                    tracing::warn!("failed to send bridge/welcome: {error}");
                }
                return;
            }
            "ping" | "pong" => return,
            _ => {}
        }
    }
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || (value.get("result").is_none() && value.get("error").is_none())
    {
        tracing::warn!("ignored invalid extension response");
        return;
    }
    if let Some(id) = value.get("id") {
        let id = id.clone();
        state.complete_pending(&id, generation, value);
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
