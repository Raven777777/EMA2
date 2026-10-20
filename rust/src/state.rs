//! 共享状态：扩展 WS 连接 + 请求/响应映射。
//!
//! v2.0 起中继无 token、无配对、无 MCP 会话——这里只剩 daemon 真正需要的部分：
//! 到扩展的单条 WS 连接、按 id 关联的 pending 请求表、健康信息。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

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
    tx: mpsc::UnboundedSender<Message>,
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
    started_at: Instant,
    next_id: AtomicU64,
}

impl RelayState {
    pub fn new(output_dir: PathBuf, port: u16) -> Arc<Self> {
        Arc::new(Self {
            ext: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            output_dir,
            port,
            pid: std::process::id(),
            extension_version: Mutex::new(None),
            last_pong: Mutex::new(None),
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
    fn touch_extension(&self) {
        *lock(&self.last_pong) = Some(Instant::now());
    }

    /// 将工具调用转换为扩展内部请求，并等待同一连接的响应。
    ///
    /// 登记 pending、读取发送通道、发送消息在同一把 `ext` 锁内完成，
    /// 因此不会出现“记录的是旧连接、消息却发给新连接”的错配。
    pub async fn call_extension(
        &self,
        request: &Value,
        timeout: std::time::Duration,
    ) -> Result<Value, String> {
        let internal_id = Value::String(format!("ema2-internal-{}", self.next_id()));
        let (tx, rx) = oneshot::channel();
        let generation = {
            let ext = lock(&self.ext);
            let Some(connection) = ext.as_ref() else {
                return Err("extension is not connected".into());
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

    /// 定期向扩展发送 ping（带 id，扩展会回响应），防止 MV3 service worker 回收断连。
    ///
    /// 返回 `false` 表示发送失败、连接已被判定为丢失。
    pub fn send_keepalive(&self) -> bool {
        let mut ext = lock(&self.ext);
        let Some(connection) = ext.as_ref() else {
            return true;
        };
        let ping = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("ema2-ping-{}", self.next_id()),
            "method": "ping",
        });
        let sent = connection
            .tx
            .send(Message::Text(ping.to_string().into()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

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
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.register_extension(tx);
        assert!(state.send_keepalive());
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
