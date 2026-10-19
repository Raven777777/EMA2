//! HTTP 层：MCP Streamable HTTP + legacy SSE + Edge 扩展 WebSocket + 健康检查。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::mcp;
use crate::relay;
use crate::state::RelayState;

pub fn router(state: Arc<RelayState>) -> Router {
    Router::new()
        .route("/", get(ws_upgrade)) // Edge 扩展（连接 ws://127.0.0.1:PORT）
        .route("/mcp", get(mcp_stream).post(mcp_post)) // MCP Streamable HTTP
        .route("/sse", get(legacy_sse)) // legacy SSE transport
        .route("/messages", post(legacy_messages))
        .route("/status", get(status))
        .route("/reload", post(reload_extension))
        .with_state(state)
        // 只压缩会和 SSE 冲突，本地回环也没有带宽压力，所以这里不做压缩。
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
}

/// 周期性清理长期不活跃的会话。
pub fn spawn_session_gc(state: Arc<RelayState>, ttl: Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            state.gc_sessions(ttl);
        }
    });
}

fn auth_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        "missing or invalid Authorization: Bearer <EDGE_MCP_TOKEN>",
    )
        .into_response()
}

/// SSE 生成器被客户端丢弃时也会执行 Drop：清理不再依赖生成器循环自然结束。
struct SessionStreamCleanup {
    state: Arc<RelayState>,
    session_id: String,
    stream_id: Option<u64>,
    legacy: bool,
}

impl SessionStreamCleanup {
    fn event_stream(state: Arc<RelayState>, session_id: String, stream_id: u64) -> Self {
        Self {
            state,
            session_id,
            stream_id: Some(stream_id),
            legacy: false,
        }
    }

    fn legacy(state: Arc<RelayState>, session_id: String) -> Self {
        Self {
            state,
            session_id,
            stream_id: None,
            legacy: true,
        }
    }
}

impl Drop for SessionStreamCleanup {
    fn drop(&mut self) {
        if self.legacy {
            self.state
                .sse_senders
                .lock()
                .unwrap()
                .remove(&self.session_id);
        } else if let Some(stream_id) = self.stream_id {
            self.state
                .unregister_event_stream(&self.session_id, stream_id);
        }
    }
}

/// 判断 Host / `host:port` / `[::1]:port` 是否指向本机。
fn host_is_loopback(authority: &str) -> bool {
    let authority = authority.rsplit('@').next().unwrap_or(authority).trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(
        host.trim_end_matches('.'),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn host_header_is_loopback(headers: &HeaderMap) -> bool {
    // HTTP/1.1 的 Host 一定存在；HTTP/2 用 :authority，可能没有 Host。
    match headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        Some(host) => host_is_loopback(host),
        None => true,
    }
}

/// Origin 校验：挡住恶意网页（含 DNS rebinding）借用本机回环端口。
fn origin_is_allowed(headers: &HeaderMap, allow_extension_origin: bool) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        // 非浏览器客户端（curl / opencode / 脚本）通常不带 Origin。
        return true;
    };
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" {
        return false;
    }
    if allow_extension_origin
        && ["chrome-extension://", "moz-extension://", "extension://"]
            .iter()
            .any(|scheme| origin.starts_with(scheme))
    {
        return true;
    }
    match origin.split_once("://") {
        Some(("http" | "https", rest)) => {
            let authority = rest.split('/').next().unwrap_or_default();
            host_is_loopback(authority)
        }
        _ => false,
    }
}

/// 返回 `Some(response)` 表示应当直接拒绝该请求。
fn check_origin_and_host(headers: &HeaderMap, allow_extension_origin: bool) -> Option<Response> {
    if !host_header_is_loopback(headers) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "forbidden: Host header must be 127.0.0.1, localhost or [::1]",
            )
                .into_response(),
        );
    }
    if !origin_is_allowed(headers, allow_extension_origin) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "forbidden: Origin must be a loopback origin",
            )
                .into_response(),
        );
    }
    None
}

fn content_type_is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let mime = value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            mime == "application/json" || mime.ends_with("+json")
        })
}

/// Edge 扩展通过 WebSocket 连到根路径。
async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // 扩展的 Origin 是 chrome-extension://<id>，因此这里允许 extension scheme，
    // 但仍然拒绝任意网页来源。
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    // 浏览器扩展无法设置 Authorization，因此也接受 ?token=。
    let token_ok = match auth_header(&headers) {
        Some(header) => state.authorized(Some(header)),
        None => state.authorized_token(params.get("token").map(String::as_str)),
    };
    if !token_ok {
        return unauthorized();
    }
    ws.max_message_size(64 * 1024 * 1024)
        .max_frame_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| relay::handle_extension_ws(socket, state))
}

/// POST /mcp —— MCP Streamable HTTP 请求。
async fn mcp_post(
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    // 不校验 Content-Type 就等于允许 text/plain 这类“简单请求”绕过 CORS 预检。
    if !content_type_is_json(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        )
            .into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON-RPC").into_response(),
    };
    let is_initialize = req.get("method").and_then(Value::as_str) == Some("initialize");
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok());
    if !is_initialize {
        let Some(session_id) = session_id else {
            return (StatusCode::BAD_REQUEST, "Mcp-Session-Id is required").into_response();
        };
        if !state.touch_mcp_session(session_id) {
            return (StatusCode::NOT_FOUND, "unknown Mcp-Session-Id").into_response();
        }
    }

    match mcp::handle(&state, req).await {
        Some(resp) => {
            let mut builder = Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "no-store");
            if is_initialize && resp.get("result").is_some() {
                builder = builder.header("mcp-session-id", state.create_mcp_session());
            }
            builder
                .body(axum::body::Body::from(resp.to_string()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// GET /mcp —— Streamable HTTP 服务端事件流（保持打开）。
async fn mcp_stream(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
    else {
        return (StatusCode::BAD_REQUEST, "Mcp-Session-Id is required").into_response();
    };
    let (stream_id, mut receiver) = match state.register_event_stream(session_id) {
        Ok(value) => value,
        Err(_) => return (StatusCode::NOT_FOUND, "unknown Mcp-Session-Id").into_response(),
    };
    let session_id = session_id.to_owned();
    let cleanup_state = state.clone();
    let stream = async_stream::stream! {
        let _cleanup = SessionStreamCleanup::event_stream(cleanup_state, session_id, stream_id);
        while let Some(value) = receiver.recv().await {
            if let Ok(event) = Event::default().event("message").json_data(value) {
                yield Ok::<Event, Infallible>(event);
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// GET /sse —— legacy SSE transport 入口。
async fn legacy_sse(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    let session_id = state.new_legacy_session_id();
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    state
        .sse_senders
        .lock()
        .unwrap()
        .insert(session_id.clone(), (Instant::now(), tx));
    let endpoint = format!("/messages?sessionId={session_id}");
    let cleanup_state = state.clone();
    let cleanup_id = session_id.clone();
    let stream = async_stream::stream! {
        let _cleanup = SessionStreamCleanup::legacy(cleanup_state, cleanup_id);
        yield Ok::<Event, Infallible>(Event::default().event("endpoint").data(endpoint));
        while let Some(value) = rx.recv().await {
            if let Ok(event) = Event::default().event("message").json_data(value) {
                yield Ok::<Event, Infallible>(event);
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// POST /messages?sessionId=... —— legacy SSE transport 消息入口。
async fn legacy_messages(
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
    body: Bytes,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    if !content_type_is_json(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        )
            .into_response();
    }
    let Some(session_id) = params.get("sessionId").cloned() else {
        return (StatusCode::BAD_REQUEST, "sessionId is required").into_response();
    };
    if !state.sse_senders.lock().unwrap().contains_key(&session_id) {
        return (StatusCode::NOT_FOUND, "unknown sessionId").into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON-RPC").into_response(),
    };
    if let Some(resp) = mcp::handle(&state, req).await {
        let tx = {
            let mut map = state.sse_senders.lock().unwrap();
            match map.get_mut(&session_id) {
                Some((last, tx)) => {
                    *last = Instant::now();
                    Some(tx.clone())
                }
                None => None,
            }
        };
        if let Some(tx) = tx {
            let _ = tx.send(resp);
        }
    }
    StatusCode::ACCEPTED.into_response()
}

/// POST /reload —— 让扩展重新加载自身（等价于在 edge://extensions 点“重新加载”）。
async fn reload_extension(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    match state.notify_extension(&json!({ "jsonrpc": "2.0", "method": "bridge/reload" })) {
        Ok(()) => (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"reloading":true}"#,
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("extension not connected: {error}"),
        )
            .into_response(),
    }
}

/// GET /status —— 健康检查。
async fn status(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if let Some(response) = check_origin_and_host(&headers, true) {
        return response;
    }
    if !state.authorized(auth_header(&headers)) {
        return unauthorized();
    }
    let body = json!({
        "name": "ema2",
        "version": env!("CARGO_PKG_VERSION"),
        "extension_connected": state.extension_connected(),
        "pending": state.pending_count(),
        "sessions": state.legacy_session_count(),
        "mcp_sessions": state.mcp_session_count(),
        "mcp_streams": state.event_stream_count(),
        "protocol_versions": crate::mcp::PROTOCOL_VERSIONS,
        "token_required": state.token.is_some(),
    })
    .to_string();
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::{SessionStreamCleanup, router};
    use crate::state::RelayState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::time::Instant;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    fn state(token: Option<&str>) -> std::sync::Arc<RelayState> {
        RelayState::new(
            token.map(str::to_owned),
            PathBuf::from("target/test-output"),
        )
    }

    fn post(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap()
    }

    fn initialize() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }
        })
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn initialize_returns_a_session_header() {
        let app = router(state(None));
        let response = app.oneshot(post("/mcp", initialize())).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("mcp-session-id").is_some());
        let body = body_json(response).await;
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    }

    #[tokio::test]
    async fn tools_call_requires_a_session() {
        let app = router(state(None));
        let request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        });
        let response = app.oneshot(post("/mcp", request)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn tools_call_reports_disconnected_extension() {
        let app = router(state(None));
        let response = app
            .clone()
            .oneshot(post("/mcp", initialize()))
            .await
            .unwrap();
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "tabs", "arguments": { "action": "list" } }
        });
        let http_request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", session)
            .body(Body::from(request.to_string()))
            .unwrap();
        let response = app.oneshot(http_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["result"]["isError"], true);
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("extension_error"), "{text}");
        assert!(text.contains("not connected"), "{text}");
    }

    #[tokio::test]
    async fn notifications_are_accepted_without_a_body() {
        let app = router(state(None));
        let response = app
            .clone()
            .oneshot(post("/mcp", initialize()))
            .await
            .unwrap();
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", session)
            .body(Body::from(
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn rejects_non_json_content_types() {
        let app = router(state(None));
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from(initialize().to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn rejects_foreign_origins_and_hosts() {
        let app = router(state(None));
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::ORIGIN, "http://evil.example")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(initialize().to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "evil.example:8765")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(initialize().to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn accepts_loopback_origins_and_extension_origins() {
        for origin in [
            "http://127.0.0.1:3000",
            "http://localhost:5173",
            "chrome-extension://abcdefghijklmnop",
        ] {
            let app = router(state(None));
            let request = Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "127.0.0.1:8765")
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(initialize().to_string()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "origin {origin}");
        }
    }

    #[tokio::test]
    async fn requires_a_bearer_token_when_configured() {
        let app = router(state(Some("s3cret")));
        let response = app
            .clone()
            .oneshot(post("/mcp", initialize()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer s3cret")
            .body(Body::from(initialize().to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_reports_the_extension_state() {
        let app = router(state(None));
        let response = app.oneshot(get("/status")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["extension_connected"], false);
        assert_eq!(body["token_required"], false);
        assert!(body["protocol_versions"].as_array().unwrap().len() >= 3);
    }

    #[tokio::test]
    async fn reload_reports_missing_extension() {
        let app = router(state(None));
        let request = Request::builder()
            .method("POST")
            .uri("/reload")
            .header(header::HOST, "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn event_stream_requires_a_known_session() {
        let app = router(state(None));
        let request = Request::builder()
            .method("GET")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1:8765")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn stream_cleanup_guard_removes_event_stream_when_dropped() {
        let state = state(None);
        let session = state.create_mcp_session();
        let (stream_id, _rx) = state.register_event_stream(&session).unwrap();
        {
            let _guard =
                SessionStreamCleanup::event_stream(state.clone(), session.clone(), stream_id);
            assert_eq!(state.event_stream_count(), 1);
        }
        assert_eq!(state.event_stream_count(), 0);
    }

    #[test]
    fn stream_cleanup_guard_removes_legacy_session_when_dropped() {
        let state = state(None);
        let session = state.new_legacy_session_id();
        let (tx, _rx) = mpsc::unbounded_channel::<Value>();
        state
            .sse_senders
            .lock()
            .unwrap()
            .insert(session.clone(), (Instant::now(), tx));
        {
            let _guard = SessionStreamCleanup::legacy(state.clone(), session.clone());
            assert_eq!(state.legacy_session_count(), 1);
        }
        assert_eq!(state.legacy_session_count(), 0);
    }
}
