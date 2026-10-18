//! 共享状态：扩展 WS 连接、MCP 请求映射和传输会话。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// JSON-RPC id（number / string / null）。
pub type RpcId = Value;

type SseEntry = (Instant, mpsc::UnboundedSender<Value>);

/// 一条已建立的扩展连接。
struct ExtConnection {
    generation: u64,
    tx: mpsc::UnboundedSender<Message>,
}

struct PendingEntry {
    generation: u64,
    sender: oneshot::Sender<Value>,
}

/// 保证 pending 表条目一定会被清理。
///
/// MCP 客户端中途断开时，axum 会丢弃 handler future，`call_extension` 里的
/// 超时逻辑就再也不会执行；没有这个守卫时该条目会一直留在 pending 表里，
/// 直到扩展下次重连才被清空。
struct PendingGuard<'a> {
    state: &'a RelayState,
    id: RpcId,
    generation: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.state.remove_pending(&self.id, self.generation);
    }
}

struct EventStream {
    id: u64,
    /// 只用于保持通道存活：丢弃 sender 会让对应的 SSE 流立刻结束。
    #[allow(dead_code)]
    tx: mpsc::UnboundedSender<Value>,
}

pub struct RelayState {
    /// 到 Edge 扩展的发送通道（扩展每次重连都会整体替换）。
    ///
    /// 所有对 `ext` 的读写都在同一把锁内完成，因此 `generation` 与 `tx`
    /// 永远来自同一条连接，不需要额外的“活跃 generation”守卫。
    ext: Mutex<Option<ExtConnection>>,
    /// 内部请求 id -> 请求所属连接和响应通道。
    pending: Mutex<HashMap<RpcId, PendingEntry>>,
    /// legacy SSE 事件通道：session id -> (last_active, sender)。
    pub sse_senders: Mutex<HashMap<String, SseEntry>>,
    /// Streamable HTTP 会话：session id -> 最后活跃时间。
    pub mcp_sessions: Mutex<HashMap<String, Instant>>,
    /// Streamable HTTP 服务端事件流：session id -> (stream id, sender)。
    event_streams: Mutex<HashMap<String, EventStream>>,
    /// 可选 MCP HTTP 鉴权 token（None = 不鉴权）。
    pub token: Option<String>,
    /// 截图等输出文件目录。
    pub output_dir: PathBuf,
    next_id: AtomicU64,
}

impl RelayState {
    pub fn new(token: Option<String>, output_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            ext: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            sse_senders: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(HashMap::new()),
            event_streams: Mutex::new(HashMap::new()),
            token,
            output_dir,
            next_id: AtomicU64::new(0),
        })
    }

    pub fn extension_connected(&self) -> bool {
        self.ext.lock().unwrap().is_some()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub fn mcp_session_count(&self) -> usize {
        self.mcp_sessions.lock().unwrap().len()
    }

    pub fn legacy_session_count(&self) -> usize {
        self.sse_senders.lock().unwrap().len()
    }

    pub fn event_stream_count(&self) -> usize {
        self.event_streams.lock().unwrap().len()
    }

    pub fn authorized(&self, auth_header: Option<&str>) -> bool {
        let candidate = auth_header
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim);
        self.authorized_token(candidate)
    }

    /// 直接比较原始 token（扩展 WS 走 `?token=` 查询参数，不带 Bearer 前缀）。
    pub fn authorized_token(&self, token: Option<&str>) -> bool {
        match &self.token {
            None => true,
            Some(expected) => token
                .map(str::trim)
                .is_some_and(|value| constant_time_eq(value, expected)),
        }
    }

    /// 将 MCP 请求转换为扩展内部请求，并等待同一连接的响应。
    ///
    /// 登记 pending、读取发送通道、发送消息在同一把 `ext` 锁内完成，
    /// 因此不会出现“记录的是旧连接、消息却发给新连接”的错配。
    pub async fn call_extension(
        &self,
        request: &Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let internal_id = Value::String(format!(
            "ema2-internal-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        ));
        let (tx, rx) = oneshot::channel();
        let generation = {
            let ext = self.ext.lock().unwrap();
            let Some(connection) = ext.as_ref() else {
                return Err("Edge extension is not connected".into());
            };
            self.pending.lock().unwrap().insert(
                internal_id.clone(),
                PendingEntry {
                    generation: connection.generation,
                    sender: tx,
                },
            );
            let mut forwarded = request.clone();
            forwarded["id"] = internal_id.clone();
            match connection
                .tx
                .send(Message::Text(forwarded.to_string().into()))
            {
                Ok(()) => connection.generation,
                Err(_) => {
                    self.pending.lock().unwrap().remove(&internal_id);
                    return Err("extension connection lost".into());
                }
            }
        };

        // 正常完成/超时/客户端断开都会在离开作用域时清掉 pending 条目。
        let _guard = PendingGuard {
            state: self,
            id: internal_id.clone(),
            generation,
        };
        let result = tokio::time::timeout(timeout, rx).await;
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err("extension channel closed".into()),
            Err(_) => Err(format!(
                "extension timed out after {:.1}s",
                timeout.as_secs_f64()
            )),
        }
    }

    pub fn notify_extension(&self, request: &Value) -> Result<(), String> {
        let ext = self.ext.lock().unwrap();
        match ext.as_ref() {
            Some(connection) => connection
                .tx
                .send(Message::Text(request.to_string().into()))
                .map_err(|_| "extension connection lost".into()),
            None => Err("Edge extension is not connected".into()),
        }
    }

    /// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
    pub fn send_keepalive(&self) {
        let mut ext = self.ext.lock().unwrap();
        let Some(connection) = ext.as_ref() else {
            return;
        };
        let sent = connection
            .tx
            .send(Message::Text(
                r#"{"jsonrpc":"2.0","method":"ping"}"#.to_string().into(),
            ))
            .is_ok();
        if !sent {
            let generation = connection.generation;
            ext.take();
            self.pending.lock().unwrap().clear();
            tracing::warn!("extension connection lost (generation {generation})");
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

    /// 生成会话 / 请求 id 的公共前缀，保证进程内唯一。
    fn next_unique_id(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let counter = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("ema2-{now:x}-{counter:x}")
    }

    pub fn create_mcp_session(&self) -> String {
        let id = self.next_unique_id();
        self.mcp_sessions
            .lock()
            .unwrap()
            .insert(id.clone(), Instant::now());
        id
    }

    /// legacy SSE 会话 id（由 `/sse` + `/messages` 使用）。
    pub fn new_legacy_session_id(&self) -> String {
        self.next_unique_id()
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

    /// 为一个已知 session 注册服务端事件流，返回流 id（用于精确注销）。
    pub fn register_event_stream(
        &self,
        session_id: &str,
    ) -> Result<(u64, mpsc::UnboundedReceiver<Value>), String> {
        if !self.touch_mcp_session(session_id) {
            return Err("unknown session".into());
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.event_streams
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), EventStream { id, tx });
        Ok((id, rx))
    }

    /// 只注销自己注册的那条流：同一 session 重连后旧流的清理不会误删新流。
    pub fn unregister_event_stream(&self, session_id: &str, stream_id: u64) {
        let mut streams = self.event_streams.lock().unwrap();
        if streams
            .get(session_id)
            .is_some_and(|stream| stream.id == stream_id)
        {
            streams.remove(session_id);
        }
    }

    /// 清理超过 ttl 未活跃的会话（legacy SSE / streamable HTTP）。
    pub fn gc_sessions(&self, ttl: Duration) {
        let now = Instant::now();
        {
            let mut legacy = self.sse_senders.lock().unwrap();
            let before = legacy.len();
            legacy.retain(|_, (last, _)| now.duration_since(*last) < ttl);
            if before != legacy.len() {
                tracing::info!("sse GC: {before} -> {} sessions", legacy.len());
            }
        }
        let expired: Vec<String> = {
            let mut sessions = self.mcp_sessions.lock().unwrap();
            let expired: Vec<String> = sessions
                .iter()
                .filter(|(_, last)| now.duration_since(**last) >= ttl)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &expired {
                sessions.remove(id);
            }
            expired
        };
        if !expired.is_empty() {
            let mut streams = self.event_streams.lock().unwrap();
            for id in &expired {
                streams.remove(id);
            }
            tracing::info!("mcp session GC: {} sessions", expired.len());
        }
    }

    pub fn register_extension(&self, tx: mpsc::UnboundedSender<Message>) -> u64 {
        let generation = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut ext = self.ext.lock().unwrap();
        // 旧连接上的等待者立刻失败，而不是白白等到超时。
        self.pending.lock().unwrap().clear();
        *ext = Some(ExtConnection { generation, tx });
        generation
    }

    pub fn unregister_extension(&self, generation: u64) {
        let mut ext = self.ext.lock().unwrap();
        if ext
            .as_ref()
            .is_some_and(|connection| connection.generation == generation)
        {
            *ext = None;
            self.pending.lock().unwrap().clear();
        }
    }
}

/// 常量时间比较，避免通过响应时间侧信道逐字节猜 token。
fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> Arc<RelayState> {
        RelayState::new(None, PathBuf::from("target/test-output"))
    }

    fn request() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        })
    }

    #[tokio::test]
    async fn forwards_request_with_internal_id_and_completes_it() {
        let state = state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let generation = state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(5))
                    .await
            })
        };
        let Some(Message::Text(text)) = rx.recv().await else {
            panic!("expected a forwarded text message");
        };
        let forwarded: Value = serde_json::from_str(&text).unwrap();
        assert!(
            forwarded["id"]
                .as_str()
                .unwrap()
                .starts_with("ema2-internal-")
        );
        let internal_id = forwarded["id"].clone();
        state.complete_pending(
            &internal_id,
            generation,
            json!({ "jsonrpc": "2.0", "id": internal_id, "result": { "content": [] } }),
        );
        let response = call.await.unwrap().unwrap();
        assert_eq!(response["result"]["content"], json!([]));
        assert_eq!(state.pending_count(), 0);
    }

    #[tokio::test]
    async fn ignores_responses_from_a_stale_generation() {
        let state = state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let generation = state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(5))
                    .await
            })
        };
        let Some(Message::Text(text)) = rx.recv().await else {
            panic!("expected a forwarded text message");
        };
        let internal_id: Value = serde_json::from_str::<Value>(&text).unwrap()["id"].clone();
        state.complete_pending(
            &internal_id,
            generation + 1,
            json!({ "jsonrpc": "2.0", "id": internal_id, "result": {} }),
        );
        assert_eq!(state.pending_count(), 1, "stale response must not resolve");
        state.complete_pending(
            &internal_id,
            generation,
            json!({ "jsonrpc": "2.0", "id": internal_id, "result": {} }),
        );
        assert!(call.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn times_out_and_releases_pending_entry() {
        let state = state();
        let (tx, _rx) = mpsc::unbounded_channel();
        state.register_extension(tx);
        let error = state
            .call_extension(&request(), Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.contains("timed out"), "unexpected error: {error}");
        assert_eq!(state.pending_count(), 0);
    }

    #[tokio::test]
    async fn dropping_the_call_releases_the_pending_entry() {
        let state = state();
        let (tx, _rx) = mpsc::unbounded_channel();
        state.register_extension(tx);
        let request = request();
        let mut call = Box::pin(state.call_extension(&request, Duration::from_secs(30)));
        // 第一次 poll 会登记 pending 并发送；随后超时只取消等待，不丢弃 call 本身。
        let _ = tokio::time::timeout(Duration::from_millis(100), &mut call).await;
        assert_eq!(state.pending_count(), 1);
        drop(call);
        assert_eq!(
            state.pending_count(),
            0,
            "cancelled call leaked a pending entry"
        );
    }

    #[tokio::test]
    async fn fails_fast_when_extension_disconnects() {
        let state = state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let generation = state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(30))
                    .await
            })
        };
        let _ = rx.recv().await;
        state.unregister_extension(generation);
        let error = call.await.unwrap().unwrap_err();
        assert!(error.contains("closed"), "unexpected error: {error}");
    }

    #[test]
    fn reports_disconnected_extension() {
        let state = state();
        assert!(!state.extension_connected());
    }

    #[test]
    fn token_auth_is_exact() {
        let state = RelayState::new(Some("s3cret".into()), PathBuf::from("target"));
        assert!(state.authorized(Some("Bearer s3cret")));
        assert!(!state.authorized(Some("Bearer s3cret2")));
        assert!(!state.authorized(Some("s3cret")));
        assert!(!state.authorized(None));
        // 扩展 WS 用原始 token（无 Bearer 前缀）。
        assert!(state.authorized_token(Some("s3cret")));
        assert!(state.authorized_token(Some(" s3cret ")));
        assert!(!state.authorized_token(Some("Bearer s3cret")));
        assert!(!state.authorized_token(None));
    }

    #[test]
    fn constant_time_eq_matches_normal_equality() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }

    #[test]
    fn event_stream_cleanup_is_scoped_to_its_own_stream() {
        let state = state();
        let session = state.create_mcp_session();
        let (first, _first_rx) = state.register_event_stream(&session).unwrap();
        let (second, _second_rx) = state.register_event_stream(&session).unwrap();
        state.unregister_event_stream(&session, first);
        assert_eq!(
            state.event_stream_count(),
            1,
            "stale cleanup removed the new stream"
        );
        state.unregister_event_stream(&session, second);
        assert_eq!(state.event_stream_count(), 0);
    }

    #[test]
    fn gc_expires_idle_sessions() {
        let state = state();
        let session = state.create_mcp_session();
        let _stream = state.register_event_stream(&session).unwrap();
        state.gc_sessions(Duration::ZERO);
        assert_eq!(state.mcp_session_count(), 0);
        assert_eq!(state.event_stream_count(), 0);
    }
}
