//! Edge 扩展的 WebSocket 连接处理 + 保活。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::clients;
use crate::config::Config;
use crate::state::RelayState;

/// 连接存续期间持有；Drop 时注销 generation 并通知 MCP 客户端。
///
/// 用 Drop 而不是在循环末尾显式调用：任务被 panic 取消时（例如扩展送回畸形
/// 消息触发序列化异常）也必须注销，否则中继会一直以为扩展还连着。
struct ExtensionSession {
    state: Arc<RelayState>,
    generation: u64,
}

impl Drop for ExtensionSession {
    fn drop(&mut self) {
        if self.state.unregister_extension(self.generation) {
            tracing::info!("extension disconnected (generation {})", self.generation);
            broadcast_extension_state(&self.state, "extension disconnected", "warning");
        }
    }
}

/// 把扩展连接状态变化推给所有 `GET /mcp` 事件流。
///
/// 这是服务端主动通知的唯一来源：AI 客户端在扩展掉线时能立刻知道，
/// 而不是等下一次 `tools/call` 拿到 `extension_error`。
fn broadcast_extension_state(state: &Arc<RelayState>, message: &str, level: &str) {
    let event = json!({
        "jsonrpc": "2.0",
        "method": "notifications/message",
        "params": {
            "level": level,
            "logger": "ema2",
            "data": message,
        }
    });
    let delivered = state.broadcast_event(&event);
    if delivered > 0 {
        tracing::debug!("broadcast \"{message}\" to {delivered} MCP event stream(s)");
    }
}

/// 扩展 WS 连接主循环：把响应按 id 送回到等待的 MCP 请求。
pub async fn handle_extension_ws(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let generation = state.register_extension(tx);
    let _session = ExtensionSession {
        state: state.clone(),
        generation,
    };
    tracing::info!("extension connected (generation {generation})");
    broadcast_extension_state(&state, "extension connected", "info");

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
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        match method {
            // 扩展连上后先声明角色；中继回 welcome。扩展在收到（并通过验签的）
            // welcome 之前不执行任何工具调用，避免误连到占用同一端口的其它本地服务。
            "bridge/hello" => {
                let version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let challenge = value.get("challenge").and_then(Value::as_str);
                state.set_extension_version(version);
                tracing::info!("bridge/hello from extension {version}");
                let mut welcome = json!({
                    "jsonrpc": "2.0",
                    "method": "bridge/welcome",
                    "name": "ema2",
                    "version": env!("CARGO_PKG_VERSION"),
                    "auth": "hmac-v1",
                });
                // 带 token 时证明自己是“持有 token 的那个中继”；没 token（匿名模式）
                // 或旧扩展没发 challenge 时保持旧行为。
                if let Some(challenge) = challenge
                    && crate::pair::is_valid_challenge(challenge)
                    && let Some(proof) = state.welcome_proof(challenge)
                {
                    welcome["proof"] = Value::String(proof);
                }
                return Reply::Welcome(welcome);
            }
            "ping" | "pong" => return Reply::None,
            _ => {}
        }
    }
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || (value.get("result").is_none() && value.get("error").is_none())
    {
        tracing::warn!("ignored invalid extension response");
        return Reply::None;
    }
    if let Some(id) = value.get("id") {
        let id = id.clone();
        state.complete_pending(&id, generation, value);
    }
    Reply::None
}

/// 配对通道：只接受 hello / pair-claim，不注册为工具连接。
///
/// 授权由扩展页面的按钮触发：页面让 service worker 发出 `bridge/pair-claim`，
/// 中继生成 token、落盘、写客户端配置，再把 token 交给扩展。
pub async fn handle_pairing_ws(socket: WebSocket, state: Arc<RelayState>) {
    let (mut sink, mut stream) = socket.split();
    let mut version = "unknown".to_owned();
    tracing::info!("pairing channel opened");

    loop {
        let incoming =
            tokio::time::timeout(std::time::Duration::from_secs(120), stream.next()).await;
        let Ok(Some(Ok(Message::Text(text)))) = incoming else {
            break;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match value.get("method").and_then(Value::as_str) {
            Some("bridge/hello") => {
                version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let welcome = json!({
                    "jsonrpc": "2.0",
                    "method": "bridge/welcome",
                    "name": "ema2",
                    "version": env!("CARGO_PKG_VERSION"),
                    "pairing": true,
                });
                if sink
                    .send(Message::Text(welcome.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Some("bridge/pair-claim") => {
                // 授权页勾选的终端；缺省 = 检测到的全部。
                let requested = value.get("clients").and_then(Value::as_array).map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                });
                let selection = clients::normalize(requested.as_deref());
                match state.grant_pairing(&version, &selection) {
                    Ok(token) => {
                        let configured = clients::apply(&selection, state.port, &token);
                        for result in &configured {
                            match &result.error {
                                Some(error) => tracing::warn!(
                                    "client config {} at {} failed: {error}",
                                    result.id,
                                    result.path.display()
                                ),
                                None => tracing::info!(
                                    "client config {} at {}: {}",
                                    result.id,
                                    result.path.display(),
                                    result.action
                                ),
                            }
                        }
                        let granted = json!({
                            "jsonrpc": "2.0",
                            "method": "bridge/pair-granted",
                            "token": token,
                            "port": state.port,
                            "clients": configured.iter().map(|result| result.to_json()).collect::<Vec<_>>(),
                            "selection": selection,
                        });
                        let _ = sink.send(Message::Text(granted.to_string().into())).await;
                        tracing::info!("pairing granted to extension {version}");
                        break;
                    }
                    Err(error) => {
                        let denied = json!({
                            "jsonrpc": "2.0",
                            "method": "bridge/pair-denied",
                            "reason": error,
                        });
                        let _ = sink.send(Message::Text(denied.to_string().into())).await;
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    tracing::info!("pairing channel closed");
}

/// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
pub fn spawn_keepalive(state: Arc<RelayState>, cfg: Config) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(cfg.keepalive);
        interval.tick().await; // 跳过首次立即触发
        loop {
            interval.tick().await;
            if !state.send_keepalive() {
                broadcast_extension_state(&state, "extension disconnected", "warning");
            }
        }
    });
}
