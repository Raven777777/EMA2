//! 共享状态：扩展 WS 连接、MCP 请求映射和传输会话。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// JSON-RPC id（number / string / null）。
pub type RpcId = Value;

type SseEntry = (Instant, mpsc::UnboundedSender<Value>);

struct PendingEntry {
    generation: u64,
    sender: oneshot::Sender<Value>,
}

pub struct RelayState {
    /// 到 Edge 扩展的发送通道（每次扩展重连会替换）。
    pub ext_tx: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    pub ext_generation: AtomicU64,
    pub active_ext_generation: Mutex<Option<u64>>,
    /// 内部请求 id -> 请求所属连接和响应通道。
    pending: Mutex<HashMap<RpcId, PendingEntry>>,
    /// legacy SSE 事件通道：session id -> (last_active, sender)。
    pub sse_senders: Mutex<HashMap<String, SseEntry>>,
    /// Streamable HTTP 会话：session id -> 最后活跃时间。
    pub mcp_sessions: Mutex<HashMap<String, Instant>>,
    /// Streamable HTTP 服务端事件通道。
    pub mcp_event_senders: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    /// 可选 MCP HTTP 鉴权 token（None = 不鉴权）。
    pub token: Option<String>,
    /// 截图等输出文件目录。
    pub output_dir: PathBuf,
    next_request_id: AtomicU64,
}

impl RelayState {
    pub fn new(token: Option<String>, output_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            ext_tx: Mutex::new(None),
            ext_generation: AtomicU64::new(0),
            active_ext_generation: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            sse_senders: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(HashMap::new()),
            mcp_event_senders: Mutex::new(HashMap::new()),
            token,
            output_dir,
            next_request_id: AtomicU64::new(0),
        })
    }

    pub fn extension_connected(&self) -> bool {
        self.ext_tx.lock().unwrap().is_some()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub fn authorized(&self, auth_header: Option<&str>) -> bool {
        match &self.token {
            None => true,
            Some(token) => auth_header
                .and_then(|value| value.strip_prefix("Bearer "))
                .is_some_and(|value| value == token),
        }
    }

    /// 将 MCP 请求转换为扩展内部请求，并等待同一连接的响应。
    pub async fn call_extension(&self, request: &Value) -> Result<Value, String> {
        let internal_id = Value::String(format!(
            "ema2-internal-{}",
            self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1
        ));
        let (tx, rx) = oneshot::channel();
        let (ext, generation) = {
            // 注册 pending 与读取 sender 必须观察同一个连接 generation。
            let _active = self.active_ext_generation.lock().unwrap();
            let generation = match *_active {
                Some(value) => value,
                None => return Err("Edge extension is not connected".into()),
            };
            self.pending.lock().unwrap().insert(
                internal_id.clone(),
                PendingEntry {
                    generation,
                    sender: tx,
                },
            );
            (self.ext_tx.lock().unwrap().clone(), generation)
        };

        let mut forwarded = request.clone();
        forwarded["id"] = internal_id.clone();
        match ext {
            Some(tx) if tx.send(Message::Text(forwarded.to_string().into())).is_ok() => {}
            Some(_) => {
                self.remove_pending(&internal_id, generation);
                return Err("extension connection lost".into());
            }
            None => {
                self.remove_pending(&internal_id, generation);
                return Err("Edge extension is not connected".into());
            }
        }

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await;
        if result.is_err() {
            self.remove_pending(&internal_id, generation);
        }
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err("extension channel closed".into()),
            Err(_) => Err("extension timed out (30s)".into()),
        }
    }

    pub fn notify_extension(&self, request: &Value) -> Result<(), String> {
        match self.ext_tx.lock().unwrap().clone() {
            Some(tx) => tx
                .send(Message::Text(request.to_string().into()))
                .map_err(|_| "extension connection lost".into()),
            None => Err("Edge extension is not connected".into()),
        }
    }

    pub fn send_keepalive(&self) {
        let (generation, tx) = {
            let _active = self.active_ext_generation.lock().unwrap();
            let Some(generation) = *_active else {
                return;
            };
            (generation, self.ext_tx.lock().unwrap().clone())
        };
        if tx
            .and_then(|sender| {
                sender
                    .send(Message::Text(
                        r#"{"jsonrpc":"2.0","method":"ping"}"#.to_string().into(),
                    ))
                    .ok()
            })
            .is_none()
        {
            self.unregister_extension(generation);
        }
    }

    pub fn complete_pending(&self, id: &RpcId, generation: u64, response: Value) {
        let entry = {
            let mut pending = self.pending.lock().unwrap();
            match pending.get(id) {
                Some(entry) if entry.generation == generation => pending.remove(id),
                _ => None,
            }
        };
        if let Some(entry) = entry {
            let _ = entry.sender.send(response);
        }
    }

    fn remove_pending(&self, id: &RpcId, generation: u64) {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .get(id)
            .is_some_and(|entry| entry.generation == generation)
        {
            pending.remove(id);
        }
    }

    pub fn create_mcp_session(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let counter = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let id = format!("ema2-{now:x}-{counter:x}");
        self.mcp_sessions
            .lock()
            .unwrap()
            .insert(id.clone(), Instant::now());
        id
    }

    pub fn touch_mcp_session(&self, id: &str) -> bool {
        let mut sessions = self.mcp_sessions.lock().unwrap();
        if let Some(last_active) = sessions.get_mut(id) {
            *last_active = Instant::now();
            true
        } else {
            false
        }
    }

    pub fn register_extension(&self, tx: mpsc::UnboundedSender<Message>) -> u64 {
        let generation = self.ext_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut active = self.active_ext_generation.lock().unwrap();
        self.pending.lock().unwrap().clear();
        *self.ext_tx.lock().unwrap() = Some(tx);
        *active = Some(generation);
        generation
    }

    pub fn unregister_extension(&self, generation: u64) {
        let mut active = self.active_ext_generation.lock().unwrap();
        if *active == Some(generation) {
            self.pending.lock().unwrap().clear();
            *self.ext_tx.lock().unwrap() = None;
            *active = None;
        }
    }
}
