//! 共享状态：扩展 WS 连接 + 请求/响应映射。
//!
//! v2.0 起中继无 token、无配对、无 MCP 会话——这里只剩 daemon 真正需要的部分：
//! 到扩展的单条 WS 连接、按 id 关联的 pending 请求表、健康信息。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

/// 到扩展的发送队列容量。
///
/// 有界队列把“扩展不读 WS”变成可观测的拥塞：`try_send` 返回 `Full` 时
/// 消息丢弃但连接保留（半开检测交给 `last_pong` janitor），避免无界内存在
/// daemon 里悄悄堆积。
pub(crate) const EXT_CHANNEL_CAPACITY: usize = 64;

/// 向扩展发送单条消息的最长等待时间。
pub(crate) const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// `call_extension` 的失败原因。
///
/// 不同原因映射到不同的 HTTP 状态/错误码：没有扩展和连接刚断是可重试的
/// 503，只有“扩展收下了但没回”才是 504。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// 没有扩展连接。
    NotConnected,
    /// 发送时通道已关闭，或扩展连接在等待响应期间丢失。
    Lost,
    /// 扩展没有在预算内返回响应。
    Timeout(Duration),
    /// daemon 正在关闭，调用被主动打断。
    ShuttingDown,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => formatter.write_str("extension is not connected"),
            Self::Lost => formatter.write_str("extension connection lost"),
            Self::Timeout(timeout) => write!(
                formatter,
                "extension timed out after {:.1}s",
                timeout.as_secs_f64()
            ),
            Self::ShuttingDown => formatter.write_str("daemon is shutting down"),
        }
    }
}

impl std::error::Error for CallError {}

/// 取锁并忽略“中毒”状态。
///
/// 持锁线程 panic 会毒化 `std::sync::Mutex`；对 daemon 这种单例状态机来说，
/// 让后续所有请求都跟着 `unwrap()` panic 远比为一次异常继续服务更糟。
/// 数据仍被保护，只是无法保证 panic 前的逻辑完整性——调用点都在做“读/替换”级操作。
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// JSON-RPC id（number / string / null）。
pub type RpcId = Value;

/// 一条已建立的扩展连接。
struct ExtConnection {
    generation: u64,
    tx: mpsc::Sender<Message>,
    extension_id: String,
}

/// 完成 `bridge/hello` 之前，连接只能待在这里：不能执行工具、不计入 status。
struct PendingConnection {
    generation: u64,
    tx: mpsc::Sender<Message>,
}

struct PendingEntry {
    generation: u64,
    sender: oneshot::Sender<Value>,
}

/// 保证 pending 表条目一定会被清理。
///
/// CLI 客户端中途断开时，axum 会丢弃 handler future，`call_extension` 里的
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

pub struct RelayState {
    /// 到 Edge 扩展的发送通道（扩展每次重连都会整体替换）。
    ///
    /// 所有对 `ext` 的读写都在同一把锁内完成，因此 `generation` 与 `tx`
    /// 永远来自同一条连接，不需要额外的“活跃 generation”守卫。
    ext: Mutex<Option<ExtConnection>>,
    /// 已升级 WS、但还没通过 `bridge/hello` 验证的连接。
    pending_ext: Mutex<Option<PendingConnection>>,
    /// 内部请求 id -> 请求所属连接和响应通道。
    pending: Mutex<HashMap<RpcId, PendingEntry>>,
    /// 截图/HAR 等输出文件目录。
    pub output_dir: PathBuf,
    /// 实际监听端口（/health 汇报用）。
    pub port: u16,
    /// daemon 进程 pid（/health 汇报用）。
    pub pid: u32,
    /// 扩展上报的版本（诊断用）。
    extension_version: Mutex<Option<String>>,
    /// 最近一次收到扩展消息（pong/响应都算）的时间；`None` = 从未收到。
    last_pong: Mutex<Option<Instant>>,
    /// 关闭信号：`true` 后 `call_extension` 立即返回 `ShuttingDown`。
    ///
    /// 用 `watch` 而不是 `Notify`：晚订阅的 waiter 也能看到“已经关闭”，
    /// 不会因为 handler 先于 graceful future poll 而丢信号。
    shutting_down: watch::Sender<bool>,
    started_at: Instant,
    next_id: AtomicU64,
}

impl RelayState {
    pub fn new(output_dir: PathBuf, port: u16) -> Arc<Self> {
        let (shutting_down, _) = watch::channel(false);
        Arc::new(Self {
            ext: Mutex::new(None),
            pending_ext: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            output_dir,
            port,
            pid: std::process::id(),
            extension_version: Mutex::new(None),
            last_pong: Mutex::new(None),
            shutting_down,
            started_at: Instant::now(),
            next_id: AtomicU64::new(0),
        })
    }

    pub fn extension_connected(&self) -> bool {
        lock(&self.ext).is_some()
    }

    /// 标记 daemon 进入关闭流程；在途 `call_extension` 会被立即唤醒。
    pub fn begin_shutdown(&self) {
        // 用 send_replace 而不是 send：构造时初始 receiver 已被丢弃，
        // channel 处于 closed 状态，send 会直接失败而不更新值。
        self.shutting_down.send_replace(true);
    }

    pub fn is_shutting_down(&self) -> bool {
        *self.shutting_down.borrow()
    }

    /// 等待 daemon 进入关闭状态；已经关闭时立即返回。
    pub async fn wait_for_shutdown(&self) {
        let mut shutdown = self.shutting_down.subscribe();
        wait_for_shutdown(&mut shutdown).await;
    }

    pub fn pending_count(&self) -> usize {
        lock(&self.pending).len()
    }

    #[cfg(test)]
    pub fn set_extension_version(&self, version: &str) {
        *lock(&self.extension_version) = Some(version.to_owned());
        // hello 也是一次扩展活动；status 不应该在已连接时显示 last_ping=never。
        self.touch_extension();
    }

    pub fn extension_version(&self) -> Option<String> {
        lock(&self.extension_version).clone()
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// 距最近一次扩展活动的秒数；从未收到过消息时返回 `None`。
    pub fn last_ping_secs(&self) -> Option<u64> {
        lock(&self.last_pong).map(|at| at.elapsed().as_secs())
    }

    /// 任何来自扩展的消息都证明连接活着；ping 保活依赖这一点记录 last_ping。
    pub(crate) fn touch_extension(&self) {
        *lock(&self.last_pong) = Some(Instant::now());
    }

    /// 测试钩子：把 `last_pong` 回拨，用来模拟半开连接。
    #[cfg(test)]
    pub fn set_last_pong_age(&self, age: Duration) {
        *lock(&self.last_pong) = Instant::now().checked_sub(age);
    }

    /// 将工具调用转换为扩展内部请求，并等待同一连接的响应。
    ///
    /// 登记 pending、克隆发送通道在同一把 `ext` 锁内完成，因此不会出现
    /// “记录的是旧连接、消息却发给新连接”的错配；锁在发送前释放，
    /// 有界队列满时最多等 `SEND_TIMEOUT`，绝不跨 await 持有 `MutexGuard`。
    pub async fn call_extension(
        &self,
        request: &Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        if self.is_shutting_down() {
            return Err(CallError::ShuttingDown);
        }
        let internal_id = Value::String(format!("ema2-internal-{}", self.next_id()));
        let (tx, rx) = oneshot::channel();
        let (sender, generation) = {
            let ext = lock(&self.ext);
            let Some(connection) = ext.as_ref() else {
                return Err(CallError::NotConnected);
            };
            lock(&self.pending).insert(
                internal_id.clone(),
                PendingEntry {
                    generation: connection.generation,
                    sender: tx,
                },
            );
            (connection.tx.clone(), connection.generation)
        };
        let mut forwarded = request.clone();
        forwarded["id"] = internal_id.clone();
        let message = Message::Text(forwarded.to_string().into());
        let mut shutdown = self.shutting_down.subscribe();
        let sent = tokio::select! {
            biased;
            // 队列拥塞时也要能被 shutdown 立即打断，而不是等满 5s。
            () = wait_for_shutdown(&mut shutdown) => {
                self.remove_pending(&internal_id, generation);
                return Err(CallError::ShuttingDown);
            }
            result = tokio::time::timeout(SEND_TIMEOUT, sender.send(message)) => result,
        };
        if !matches!(sent, Ok(Ok(()))) {
            // 发送失败/超时：这条请求永远不会到扩展，立刻摘掉 pending。
            self.remove_pending(&internal_id, generation);
            return Err(CallError::Lost);
        }

        // 正常完成/超时/客户端断开/shutdown 都会在离开作用域时清掉 pending 条目。
        let _guard = PendingGuard {
            state: self,
            id: internal_id.clone(),
            generation,
        };
        tokio::select! {
            biased;
            () = wait_for_shutdown(&mut shutdown) => Err(CallError::ShuttingDown),
            result = tokio::time::timeout(timeout, rx) => match result {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(_)) => Err(CallError::Lost),
                Err(_) => Err(CallError::Timeout(timeout)),
            },
        }
    }

    /// 定期向扩展发送 ping（带 id，扩展会回响应），防止 MV3 service worker 回收断连。
    ///
    /// 同时承担 janitor：超过 `3 * interval` 没收到任何扩展消息时判定为半开
    /// 连接并摘除。`interval` 由调用方的 keepalive 周期传入。
    ///
    /// 返回 `false` 表示连接已被判定为丢失（半开/通道关闭）。
    pub fn send_keepalive(&self, interval: Duration) -> bool {
        let threshold = interval.saturating_mul(3);
        let stale = lock(&self.last_pong).is_some_and(|last| last.elapsed() > threshold);
        if stale {
            let mut ext = lock(&self.ext);
            let Some(connection) = ext.take() else {
                return true;
            };
            lock(&self.pending).clear();
            tracing::warn!(
                "dropping half-open extension connection (generation {}, no message for {}s)",
                connection.generation,
                lock(&self.last_pong)
                    .map(|last| last.elapsed().as_secs())
                    .unwrap_or_default()
            );
            return false;
        }

        let ping = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("ema2-ping-{}", self.next_id()),
            "method": "ping",
        });
        let generation = {
            let ext = lock(&self.ext);
            let Some(connection) = ext.as_ref() else {
                return true;
            };
            match connection
                .tx
                .try_send(Message::Text(ping.to_string().into()))
            {
                Ok(()) => return true,
                // 队列满只说明扩展读得慢，不能因此断连：消息可以丢，连接还在。
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::debug!(
                        "extension send queue is full (generation {}); keepalive dropped",
                        connection.generation
                    );
                    return true;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => connection.generation,
            }
        };
        {
            let mut ext = lock(&self.ext);
            if ext
                .as_ref()
                .is_some_and(|connection| connection.generation == generation)
            {
                *ext = None;
            }
        }
        lock(&self.pending).clear();
        tracing::warn!("extension connection lost (generation {generation})");
        false
    }

    pub fn complete_pending(&self, id: &RpcId, generation: u64, response: Value) {
        self.touch_extension();
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

    /// 进程内单调递增的计数器；所有 id 生成都走这里，不混用 `+ 1`。
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register_pending_extension(&self, tx: mpsc::Sender<Message>) -> u64 {
        let generation = self.next_id();
        *lock(&self.pending_ext) = Some(PendingConnection { generation, tx });
        generation
    }

    /// 完成握手：只有 hello 里的 extensionId 与 WS Origin 一致时才会调用。
    ///
    /// 已有一个不同 ID 的扩展连着时拒绝新连接，避免任意本机扩展抢占控制通道。
    /// 同一 ID 重连（service worker 重启）则替换旧连接并清掉旧 pending 请求。
    pub fn activate_extension(
        &self,
        generation: u64,
        extension_id: &str,
        version: &str,
    ) -> Result<(), String> {
        let pending = {
            let mut pending = lock(&self.pending_ext);
            match pending.as_ref() {
                Some(connection) if connection.generation == generation => {
                    pending.take().expect("checked above")
                }
                _ => return Err("handshake expired or generation mismatch".into()),
            }
        };
        let mut ext = lock(&self.ext);
        if let Some(active) = ext.as_ref()
            && active.extension_id != extension_id
        {
            return Err("another extension is already connected".into());
        }
        lock(&self.pending).clear();
        *ext = Some(ExtConnection {
            generation,
            tx: pending.tx,
            extension_id: extension_id.to_owned(),
        });
        *lock(&self.extension_version) = Some(version.to_owned());
        self.touch_extension();
        Ok(())
    }

    /// 握手超时/连接关闭时清理 pending；旧 generation 的清理不影响新连接。
    pub fn unregister_pending_extension(&self, generation: u64) -> bool {
        let mut pending = lock(&self.pending_ext);
        if pending
            .as_ref()
            .is_some_and(|connection| connection.generation == generation)
        {
            *pending = None;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub fn register_extension(&self, tx: mpsc::Sender<Message>) -> u64 {
        let generation = self.register_pending_extension(tx);
        self.activate_extension(generation, "test-extension", "test")
            .expect("test handshake");
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

/// 等待 watch 通道变成 `true`；当前值已经是 `true` 时立即返回。
async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        // 发送端随 `RelayState` 存活；通道关闭按“正在关闭”处理，避免永久挂起。
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> Arc<RelayState> {
        RelayState::new(PathBuf::from("target/test-output"), 7744)
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
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
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
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
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
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        let budget = Duration::from_millis(20);
        let error = state.call_extension(&request(), budget).await.unwrap_err();
        assert_eq!(error, CallError::Timeout(budget));
        assert_eq!(state.pending_count(), 0);
    }

    #[tokio::test]
    async fn dropping_the_call_releases_the_pending_entry() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
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
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
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
        assert_eq!(error, CallError::Lost);
    }

    #[test]
    fn reports_disconnected_extension() {
        let state = state();
        assert!(!state.extension_connected());
    }

    #[tokio::test]
    async fn call_without_an_extension_reports_not_connected() {
        let state = state();
        let error = state
            .call_extension(&request(), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error, CallError::NotConnected);
        assert_eq!(error.to_string(), "extension is not connected");
    }

    #[tokio::test]
    async fn pending_connection_is_not_usable_before_hello() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let generation = state.register_pending_extension(tx);
        assert!(!state.extension_connected());
        assert_eq!(state.last_ping_secs(), None);
        assert!(state.unregister_pending_extension(generation));
        assert!(!state.unregister_pending_extension(generation));
    }

    #[tokio::test]
    async fn activation_rejects_a_different_extension_id() {
        let state = state();
        let (first, _first_rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let first_generation = state.register_pending_extension(first);
        state
            .activate_extension(first_generation, "extension-one", "1")
            .unwrap();

        let (second, _second_rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let second_generation = state.register_pending_extension(second);
        let error = state
            .activate_extension(second_generation, "extension-two", "2")
            .unwrap_err();
        assert!(error.contains("already connected"), "{error}");
        assert!(state.extension_connected());
    }

    #[tokio::test]
    async fn same_extension_id_can_reconnect_and_replaces_pending_requests() {
        let state = state();
        let (first, mut first_rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let first_generation = state.register_pending_extension(first);
        state
            .activate_extension(first_generation, "same-extension", "1")
            .unwrap();
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(5))
                    .await
            })
        };
        assert!(first_rx.recv().await.is_some());

        let (second, _second_rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        let second_generation = state.register_pending_extension(second);
        state
            .activate_extension(second_generation, "same-extension", "2")
            .unwrap();
        let error = call.await.unwrap().unwrap_err();
        assert_eq!(error, CallError::Lost);
    }

    #[test]
    fn activity_updates_last_ping() {
        let state = state();
        assert_eq!(state.last_ping_secs(), None);
        state.complete_pending(
            &json!("nobody"),
            0,
            json!({ "jsonrpc": "2.0", "id": "nobody", "result": {} }),
        );
        assert_eq!(state.last_ping_secs(), Some(0));
    }

    #[tokio::test]
    async fn keepalive_ping_carries_an_id_the_extension_answers() {
        let state = state();
        let (tx, mut rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        assert!(state.send_keepalive(Duration::from_secs(10)));
        let Some(Message::Text(text)) = rx.recv().await else {
            panic!("expected a ping message");
        };
        let ping: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(ping["method"], "ping");
        assert!(
            ping["id"].as_str().unwrap().starts_with("ema2-ping-"),
            "ping must be answerable: {text}"
        );
    }

    /// 半开连接：超过 3 个 keepalive 周期没有收到任何消息就必须摘除。
    #[tokio::test]
    async fn janitor_drops_a_half_open_connection() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        assert!(state.extension_connected());

        state.set_last_pong_age(Duration::from_secs(31));
        assert!(!state.send_keepalive(Duration::from_secs(10)));
        assert!(
            !state.extension_connected(),
            "half-open connection must be dropped"
        );
        assert_eq!(state.pending_count(), 0);
    }

    /// 队列满只算拥塞：keepalive 丢弃，但连接必须保留。
    #[tokio::test]
    async fn keepalive_congestion_does_not_drop_the_connection() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);

        for _ in 0..(EXT_CHANNEL_CAPACITY * 2) {
            assert!(state.send_keepalive(Duration::from_secs(10)));
        }
        assert!(
            state.extension_connected(),
            "a full send queue must not be treated as a dead connection"
        );
    }

    /// 关闭信号必须打断在途请求，而不是让 handler 等满工具预算。
    #[tokio::test]
    async fn shutdown_wakes_a_pending_call() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(30))
                    .await
            })
        };
        for _ in 0..100 {
            if state.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(state.pending_count(), 1);

        state.begin_shutdown();
        let error = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("shutdown must wake call_extension")
            .unwrap()
            .unwrap_err();
        assert_eq!(error, CallError::ShuttingDown);
        assert_eq!(state.pending_count(), 0);
        assert!(state.is_shutting_down());
    }

    /// 关闭后的新请求直接失败，不再尝试发送。
    #[tokio::test]
    async fn calls_after_shutdown_fail_immediately() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        state.begin_shutdown();
        let error = state
            .call_extension(&request(), Duration::from_secs(30))
            .await
            .unwrap_err();
        assert_eq!(error, CallError::ShuttingDown);
    }

    /// 队列拥塞导致 send 挂起时，shutdown 也必须立即打断。
    #[tokio::test]
    async fn shutdown_wakes_a_call_blocked_by_congestion() {
        let state = state();
        let (tx, _rx) = mpsc::channel(EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        for _ in 0..EXT_CHANNEL_CAPACITY {
            assert!(state.send_keepalive(Duration::from_secs(10)));
        }

        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .call_extension(&request(), Duration::from_secs(30))
                    .await
            })
        };
        for _ in 0..100 {
            if state.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(state.pending_count(), 1);

        state.begin_shutdown();
        let error = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("shutdown must interrupt a blocked send")
            .unwrap()
            .unwrap_err();
        assert_eq!(error, CallError::ShuttingDown);
        assert_eq!(state.pending_count(), 0);
    }

    #[test]
    fn lock_survives_a_poisoned_mutex() {
        let state = state();
        // 模拟持锁线程 panic：panic 后锁被毒化，但状态必须仍然可用。
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.extension_version.lock().unwrap();
            panic!("boom");
        }));
        assert!(poisoned.is_err());
        state.set_extension_version("2.0.0");
        assert_eq!(state.extension_version().as_deref(), Some("2.0.0"));
    }
}
