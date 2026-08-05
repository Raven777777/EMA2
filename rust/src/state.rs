//! 共享状态：扩展 WS 连接、MCP 待响应请求映射、SSE 会话通道。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// JSON-RPC id（number / string / null）。
pub type RpcId = Value;

/// 一个 legacy SSE 会话：最后活跃时间 + 事件发送通道。
type SseEntry = (Instant, mpsc::UnboundedSender<Value>);

pub struct RelayState {
    /// 到 Edge 扩展的发送通道（每次扩展重连会替换）。
    pub ext_tx: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    /// 等待扩展响应的 MCP 请求：id -> oneshot（响应 JSON-RPC 完整消息）。
    pub pending: Mutex<HashMap<RpcId, oneshot::Sender<Value>>>,
    /// legacy SSE 事件通道：session id -> (last_active, sender)。
    pub sse_senders: Mutex<HashMap<String, SseEntry>>,
    /// 可选 MCP HTTP 鉴权 token（None = 不鉴权）。
    pub token: Option<String>,
    /// 截图等输出文件目录。
    pub output_dir: PathBuf,
}

impl RelayState {
    pub fn new(token: Option<String>, output_dir: PathBuf) -> Arc<Self> {
        Arc::new(RelayState {
            ext_tx: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            sse_senders: Mutex::new(HashMap::new()),
            token,
            output_dir,
        })
    }

    /// 扩展是否在线。
    pub fn extension_connected(&self) -> bool {
        self.ext_tx.lock().unwrap().is_some()
    }

    /// 校验 Authorization: Bearer <token>。
    pub fn authorized(&self, auth_header: Option<&str>) -> bool {
        match &self.token {
            None => true,
            Some(tok) => auth_header
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|b| b == tok)
                .unwrap_or(false),
        }
    }

    /// 把 MCP 请求转发给扩展，并等待响应。
    pub async fn call_extension(&self, request: &Value) -> Result<Value, String> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(id.clone(), tx);
        }
        let ext = self.ext_tx.lock().unwrap().clone();
        match ext {
            Some(tx) => {
                if tx.send(Message::Text(request.to_string().into())).is_err() {
                    self.pending.lock().unwrap().remove(&id);
                    return Err("extension connection lost".into());
                }
            }
            None => {
                self.pending.lock().unwrap().remove(&id);
                return Err("Edge extension is not connected".into());
            }
        }
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err("extension channel closed".into()),
            Err(_) => Err("extension timed out (30s)".into()),
        }
    }
}
