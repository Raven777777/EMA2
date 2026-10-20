//! 共享状态：扩展 WS 连接、MCP 请求映射和传输会话。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// 取锁并忽略“中毒”状态。
///
/// 持锁线程 panic 会毒化 `std::sync::Mutex`；对中继这种单例状态机来说，
/// 让后续所有请求都跟着 `unwrap()` panic 远比为一次异常继续服务更糟。
/// 数据仍被保护，只是无法保证 panic 前的逻辑完整性——调用点都在做“读/替换”级操作。
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    /// 服务端事件的唯一生产者出口；`broadcast_event` 会向它写入
    /// `notifications/message`。丢弃 sender 会让对应 SSE 流立刻结束。
    tx: mpsc::UnboundedSender<Value>,
}

struct AuthState {
    /// 扩展与 MCP 客户端共享的 token；`None` 表示尚未配对。
    token: Option<String>,
    /// 显式匿名模式（`--insecure-allow-no-token`），不参与配对。
    anonymous: bool,
    /// 配对结果落盘位置；测试可以为 `None`。
    state_path: Option<PathBuf>,
    /// 授权页勾选的终端 id。
    clients: Vec<String>,
    /// 是否明确提交过终端选择。
    clients_chosen: bool,
    /// 授权时间（Unix 秒）。
    paired_at: Option<u64>,
}

/// 构造 `RelayState` 时的鉴权配置。
pub struct AuthConfig {
    pub token: Option<String>,
    pub anonymous: bool,
    pub state_path: Option<PathBuf>,
    /// 已授权的终端选择（来自 state.json）。
    pub clients: Vec<String>,
    pub clients_chosen: bool,
    pub paired_at: Option<u64>,
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
    /// token / 匿名模式 / 配对文件位置。
    auth: Mutex<AuthState>,
    /// 截图等输出文件目录。
    pub output_dir: PathBuf,
    /// 实际监听端口（写客户端配置时用）。
    pub port: u16,
    /// 扩展上报的版本（诊断用）。
    extension_version: Mutex<Option<String>>,
    started_at: Instant,
    next_id: AtomicU64,
}

impl RelayState {
    /// 兼容测试与旧调用：无 token 视为匿名（旧的“不鉴权”语义）。
    #[cfg(test)]
    pub fn new(token: Option<String>, output_dir: PathBuf) -> Arc<Self> {
        let anonymous = token.is_none();
        Self::with_auth(
            AuthConfig {
                token,
                anonymous,
                state_path: None,
                clients: Vec::new(),
                clients_chosen: false,
                paired_at: None,
            },
            output_dir,
            0,
        )
    }

    pub fn with_auth(auth: AuthConfig, output_dir: PathBuf, port: u16) -> Arc<Self> {
        Arc::new(Self {
            ext: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            sse_senders: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(HashMap::new()),
            event_streams: Mutex::new(HashMap::new()),
            auth: Mutex::new(AuthState {
                token: auth.token,
                anonymous: auth.anonymous,
                state_path: auth.state_path,
                clients: auth.clients,
                clients_chosen: auth.clients_chosen,
                paired_at: auth.paired_at,
            }),
            output_dir,
            port,
            extension_version: Mutex::new(None),
            started_at: Instant::now(),
            next_id: AtomicU64::new(0),
        })
    }

    pub fn extension_connected(&self) -> bool {
        lock(&self.ext).is_some()
    }

    pub fn pending_count(&self) -> usize {
        lock(&self.pending).len()
    }

    pub fn mcp_session_count(&self) -> usize {
        lock(&self.mcp_sessions).len()
    }

    pub fn legacy_session_count(&self) -> usize {
        lock(&self.sse_senders).len()
    }

    pub fn event_stream_count(&self) -> usize {
        lock(&self.event_streams).len()
    }

    pub fn authorized(&self, auth_header: Option<&str>) -> bool {
        let candidate = auth_header
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim);
        self.authorized_token(candidate)
    }

    /// 直接比较原始 token（扩展 WS 走 `?token=` 查询参数，不带 Bearer 前缀）。
    ///
    /// 未配对（`token == None` 且非匿名）时一律拒绝——这正是“待授权态”的语义。
    pub fn authorized_token(&self, token: Option<&str>) -> bool {
        let auth = lock(&self.auth);
        if auth.anonymous {
            return true;
        }
        match &auth.token {
            None => false,
            Some(expected) => token
                .map(str::trim)
                .is_some_and(|value| constant_time_eq(value, expected)),
        }
    }

    /// 尚未配对且未显式匿名：此时只允许配对通道，MCP 一律 503。
    pub fn pairing_allowed(&self) -> bool {
        let auth = lock(&self.auth);
        auth.token.is_none() && !auth.anonymous
    }

    pub fn is_anonymous(&self) -> bool {
        lock(&self.auth).anonymous
    }

    pub fn token(&self) -> Option<String> {
        lock(&self.auth).token.clone()
    }

    pub fn token_required(&self) -> bool {
        let auth = lock(&self.auth);
        auth.token.is_some() && !auth.anonymous
    }

    /// 是否已经完成授权（匿名模式视为已授权）。
    pub fn paired(&self) -> bool {
        let auth = lock(&self.auth);
        auth.anonymous || auth.token.is_some()
    }

    /// 已勾选要写入的终端 id。
    pub fn paired_clients(&self) -> Vec<String> {
        lock(&self.auth).clients.clone()
    }

    pub fn clients_chosen(&self) -> bool {
        lock(&self.auth).clients_chosen
    }

    /// 更新终端选择（授权页“更新配置”按钮），并同步到 state.json。
    pub fn set_clients(&self, clients: Vec<String>) {
        let mut auth = lock(&self.auth);
        auth.clients = clients.clone();
        auth.clients_chosen = true;
        let Some(path) = auth.state_path.clone() else {
            return;
        };
        let mut state = crate::pair::load(&path);
        if state.token.is_some() {
            state.clients = clients;
            state.clients_chosen = true;
            if let Err(error) = crate::pair::save(&path, &state) {
                tracing::warn!("cannot persist client selection: {error}");
            }
        }
    }

    pub fn paired_at(&self) -> Option<u64> {
        lock(&self.auth).paired_at
    }

    pub fn set_extension_version(&self, version: &str) {
        *lock(&self.extension_version) = Some(version.to_owned());
    }

    pub fn extension_version(&self) -> Option<String> {
        lock(&self.extension_version).clone()
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// token 指纹：界面上用来确认“还是那把 token”，不可反推。
    pub fn token_fingerprint(&self) -> Option<String> {
        self.token().map(|token| crate::pair::fingerprint(&token))
    }

    /// 首次授权：生成 token、落盘，并让后续连接立即要求鉴权。
    ///
    /// 只有未配对时可用；并发/重复 claim 只有一个会成功。
    pub fn grant_pairing(
        &self,
        extension_version: &str,
        clients: &[String],
    ) -> Result<String, String> {
        let mut auth = lock(&self.auth);
        if auth.anonymous {
            return Err("relay runs without authentication".into());
        }
        if auth.token.is_some() {
            return Err("already paired".into());
        }
        let token = crate::pair::generate_token();
        if let Some(path) = auth.state_path.as_deref() {
            let state = crate::pair::PersistedState {
                token: Some(token.clone()),
                paired_at: Some(crate::pair::now_secs()),
                extension_version: Some(extension_version.to_owned()),
                clients: clients.to_vec(),
                clients_chosen: true,
            };
            crate::pair::save(path, &state)
                .map_err(|error| format!("cannot persist pairing state: {error}"))?;
        }
        auth.token = Some(token.clone());
        auth.paired_at = Some(crate::pair::now_secs());
        auth.clients = clients.to_vec();
        auth.clients_chosen = true;
        Ok(token)
    }

    /// 计算欢迎消息里的 HMAC proof；未配对时返回 `None`。
    pub fn welcome_proof(&self, challenge: &str) -> Option<String> {
        let token = self.token()?;
        Some(crate::pair::welcome_proof(&token, challenge))
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
        let internal_id = Value::String(format!("ema2-internal-{}", self.next_id()));
        let (tx, rx) = oneshot::channel();
        let generation = {
            let ext = lock(&self.ext);
            let Some(connection) = ext.as_ref() else {
                return Err("Edge extension is not connected".into());
            };
            lock(&self.pending).insert(
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
                    lock(&self.pending).remove(&internal_id);
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
        let ext = lock(&self.ext);
        match ext.as_ref() {
            Some(connection) => connection
                .tx
                .send(Message::Text(request.to_string().into()))
                .map_err(|_| "extension connection lost".into()),
            None => Err("Edge extension is not connected".into()),
        }
    }

    /// 定期向扩展发送 ping，防止 MV3 service worker 闲置回收断连。
    ///
    /// 返回 `false` 表示发送失败、连接已被判定为丢失（调用方负责广播状态变化）。
    pub fn send_keepalive(&self) -> bool {
        let mut ext = lock(&self.ext);
        let Some(connection) = ext.as_ref() else {
            return true;
        };
        let sent = connection
            .tx
            .send(Message::Text(
                r#"{"jsonrpc":"2.0","method":"ping"}"#.to_string().into(),
            ))
            .is_ok();
        if sent {
            return true;
        }
        let generation = connection.generation;
        ext.take();
        lock(&self.pending).clear();
        tracing::warn!("extension connection lost (generation {generation})");
        false
    }

    pub fn complete_pending(&self, id: &RpcId, generation: u64, response: Value) {
        let entry = {
            let mut pending = lock(&self.pending);
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
        let mut pending = lock(&self.pending);
        if pending
            .get(id)
            .is_some_and(|entry| entry.generation == generation)
        {
            pending.remove(id);
        }
    }

    /// 进程内单调递增的计数器；所有 id 生成都走这里，不再混用 `+ 1`。
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 生成会话 / 请求 id 的公共前缀，保证进程内唯一。
    fn next_unique_id(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("ema2-{now:x}-{:x}", self.next_id())
    }

    pub fn create_mcp_session(&self) -> String {
        let id = self.next_unique_id();
        lock(&self.mcp_sessions).insert(id.clone(), Instant::now());
        id
    }

    /// legacy SSE 会话 id（由 `/sse` + `/messages` 使用）。
    pub fn new_legacy_session_id(&self) -> String {
        self.next_unique_id()
    }

    pub fn touch_mcp_session(&self, id: &str) -> bool {
        let mut sessions = lock(&self.mcp_sessions);
        if let Some(last_active) = sessions.get_mut(id) {
            *last_active = Instant::now();
            true
        } else {
            false
        }
    }

    /// 为一个已知 session 注册服务端事件流。
    ///
    /// 同一 session 只保留一条流：重复 `GET /mcp` 会替换旧流（旧 sender 被丢弃，
    /// 旧 SSE 连接随即结束），并返回被替换的流 id 供日志使用——不会叠加注册。
    pub fn register_event_stream(
        &self,
        session_id: &str,
    ) -> Result<(u64, mpsc::UnboundedReceiver<Value>, Option<u64>), String> {
        if !self.touch_mcp_session(session_id) {
            return Err("unknown session".into());
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_id();
        let superseded = lock(&self.event_streams)
            .insert(session_id.to_owned(), EventStream { id, tx })
            .map(|old| old.id);
        Ok((id, rx, superseded))
    }

    /// 向所有已注册的服务端事件流投递一条 JSON-RPC 通知，返回成功投递的流数量。
    ///
    /// 这是 `GET /mcp` 的**唯一**生产入口：扩展连接状态变化时由 relay 层调用。
    pub fn broadcast_event(&self, event: &Value) -> usize {
        let streams = lock(&self.event_streams);
        let mut delivered = 0;
        for stream in streams.values() {
            if stream.tx.send(event.clone()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// 只注销自己注册的那条流：同一 session 重连后旧流的清理不会误删新流。
    pub fn unregister_event_stream(&self, session_id: &str, stream_id: u64) {
        let mut streams = lock(&self.event_streams);
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
            let mut legacy = lock(&self.sse_senders);
            let before = legacy.len();
            legacy.retain(|_, (last, _)| now.duration_since(*last) < ttl);
            if before != legacy.len() {
                tracing::info!("sse GC: {before} -> {} sessions", legacy.len());
            }
        }
        let expired: Vec<String> = {
            let mut sessions = lock(&self.mcp_sessions);
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
            let mut streams = lock(&self.event_streams);
            for id in &expired {
                streams.remove(id);
            }
            tracing::info!("mcp session GC: {} sessions", expired.len());
        }
    }

    pub fn register_extension(&self, tx: mpsc::UnboundedSender<Message>) -> u64 {
        let generation = self.next_id();
        let mut ext = lock(&self.ext);
        // 旧连接上的等待者立刻失败，而不是白白等到超时。
        lock(&self.pending).clear();
        *ext = Some(ExtConnection { generation, tx });
        generation
    }

    /// 注销扩展连接；返回是否真的移除了这条连接（旧 generation 的注销不生效）。
    pub fn unregister_extension(&self, generation: u64) -> bool {
        let mut ext = lock(&self.ext);
        if ext
            .as_ref()
            .is_some_and(|connection| connection.generation == generation)
        {
            *ext = None;
            lock(&self.pending).clear();
            true
        } else {
            false
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
        let (first, _first_rx, superseded) = state.register_event_stream(&session).unwrap();
        assert_eq!(superseded, None);
        let (second, _second_rx, superseded) = state.register_event_stream(&session).unwrap();
        assert_eq!(
            superseded,
            Some(first),
            "re-registering must report the replaced stream"
        );
        assert_eq!(
            state.event_stream_count(),
            1,
            "one session must own at most one event stream"
        );
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
    fn re_registering_closes_the_superseded_stream() {
        let state = state();
        let session = state.create_mcp_session();
        let (_first_id, mut first_rx, _) = state.register_event_stream(&session).unwrap();
        let (_second_id, mut second_rx, _) = state.register_event_stream(&session).unwrap();
        // 旧 sender 已被替换掉：旧 SSE 生成器会立即结束，不会挂到 GC。
        assert_eq!(
            first_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        );

        let event = json!({ "jsonrpc": "2.0", "method": "notifications/message" });
        assert_eq!(state.broadcast_event(&event), 1, "only the live stream");
        assert_eq!(second_rx.try_recv().unwrap(), event);
    }

    #[test]
    fn broadcast_reaches_every_session_stream() {
        let state = state();
        let alpha = state.create_mcp_session();
        let beta = state.create_mcp_session();
        let (_alpha_id, mut alpha_rx, _) = state.register_event_stream(&alpha).unwrap();
        let (_beta_id, mut beta_rx, _) = state.register_event_stream(&beta).unwrap();
        let event = json!({ "jsonrpc": "2.0", "method": "notifications/message" });
        assert_eq!(state.broadcast_event(&event), 2);
        assert_eq!(alpha_rx.try_recv().unwrap(), event);
        assert_eq!(beta_rx.try_recv().unwrap(), event);
    }

    #[test]
    fn unpaired_state_rejects_every_token() {
        let state = RelayState::with_auth(
            AuthConfig {
                token: None,
                anonymous: false,
                state_path: None,
                clients: Vec::new(),
                clients_chosen: false,
                paired_at: None,
            },
            PathBuf::from("target/test-output"),
            8765,
        );
        assert!(state.pairing_allowed());
        assert!(!state.paired());
        assert!(!state.authorized(None));
        assert!(!state.authorized(Some("Bearer anything")));
        assert!(!state.authorized_token(Some("anything")));
    }

    #[test]
    fn grant_pairing_is_one_shot_and_enables_token_auth() {
        let dir = std::env::temp_dir().join(format!(
            "ema2-grant-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = crate::pair::state_path(&dir);
        let state = RelayState::with_auth(
            AuthConfig {
                token: None,
                anonymous: false,
                state_path: Some(path.clone()),
                clients: Vec::new(),
                clients_chosen: false,
                paired_at: None,
            },
            PathBuf::from("target/test-output"),
            8765,
        );

        let token = state
            .grant_pairing("1.8.1", &["opencode".to_owned(), "codex".to_owned()])
            .unwrap();
        assert_eq!(token.len(), 64);
        assert!(!state.pairing_allowed());
        assert!(state.paired());
        assert!(state.authorized_token(Some(&token)));
        assert!(!state.authorized_token(Some("wrong")));
        assert_eq!(
            state.grant_pairing("1.8.1", &[]).unwrap_err(),
            "already paired"
        );

        let persisted = crate::pair::load(&path);
        assert_eq!(persisted.token.as_deref(), Some(token.as_str()));
        assert_eq!(persisted.extension_version.as_deref(), Some("1.8.1"));
        assert_eq!(persisted.clients, vec!["opencode", "codex"]);
        assert_eq!(state.paired_clients(), vec!["opencode", "codex"]);
        assert!(state.clients_chosen());
        assert_eq!(state.paired_at(), persisted.paired_at);

        // 明确“一个都不选”后仍然算已选择，页面不会重新全选。
        state.set_clients(Vec::new());
        assert!(state.paired_clients().is_empty());
        assert!(state.clients_chosen());
        assert!(crate::pair::load(&path).clients.is_empty());
        assert!(crate::pair::load(&path).clients_chosen);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn welcome_proof_requires_a_token() {
        let unpaired = RelayState::with_auth(
            AuthConfig {
                token: None,
                anonymous: false,
                state_path: None,
                clients: Vec::new(),
                clients_chosen: false,
                paired_at: None,
            },
            PathBuf::from("target"),
            0,
        );
        assert!(unpaired.welcome_proof("abc").is_none());

        let paired = RelayState::new(Some("s3cret".into()), PathBuf::from("target"));
        assert_eq!(
            paired.welcome_proof("abc").unwrap(),
            crate::pair::welcome_proof("s3cret", "abc")
        );
    }

    #[test]
    fn lock_survives_a_poisoned_mutex() {
        let state = state();
        let session = state.create_mcp_session();
        // 模拟持锁线程 panic：panic 后锁被毒化，但状态必须仍然可用。
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.mcp_sessions.lock().unwrap();
            panic!("boom");
        }));
        assert!(poisoned.is_err());
        assert!(
            state.touch_mcp_session(&session),
            "poisoned lock must still work"
        );
        assert_eq!(state.mcp_session_count(), 1);
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
