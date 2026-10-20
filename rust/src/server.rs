//! daemon HTTP 层：单端口 7744 同时收 WS（扩展）与 HTTP（CLI）。
//!
//! 分流靠路由层完成：`GET /` 的 handler 以 `Result<WebSocketUpgrade, _>` 接收——
//! 带 `Upgrade: websocket` 头的请求走 WS 升级，其余当作普通 HTTP。
//! CLI 命令全部走 `POST /cmd`（`{"tool":..., "args":{...}, "out":可选路径}`）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::paths;
use crate::relay;
use crate::save;
use crate::state::RelayState;
use crate::tools;

/// daemon 主循环：绑定单端口，WS 与 HTTP 共用 listener，直到 `/shutdown`。
pub async fn run(port: u16) -> i32 {
    let output_dir = paths::output_dir();
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        tracing::error!(
            "cannot create output directory {}: {error}",
            output_dir.display()
        );
    }
    tracing::info!("output directory: {}", output_dir.display());

    let state = RelayState::new(output_dir, port);
    relay::spawn_keepalive(state.clone(), Duration::from_secs(10));
    let shutdown = Arc::new(Notify::new());
    let app = router(state.clone(), shutdown.clone());

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("cannot bind 127.0.0.1:{port}: {error}");
            return 2;
        }
    };
    let pid_path = paths::pid_file(port);
    if let Err(error) = std::fs::write(&pid_path, format!("{}\n", std::process::id())) {
        tracing::warn!("cannot write PID file {}: {error}", pid_path.display());
    }
    tracing::info!(
        "ema2 {} listening on http://127.0.0.1:{port}/ (health/cmd) and ws://127.0.0.1:{port}/ (extension)",
        env!("CARGO_PKG_VERSION")
    );

    let graceful = {
        let shutdown = shutdown.clone();
        async move {
            tokio::select! {
                _ = shutdown.notified() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    };
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await;

    // 只删除自己写的 PID 文件，避免误删同端口重启后的新实例。
    if std::fs::read_to_string(&pid_path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        == Some(std::process::id())
    {
        let _ = std::fs::remove_file(&pid_path);
    }
    match result {
        Ok(()) => {
            tracing::info!("daemon stopped");
            0
        }
        Err(error) => {
            tracing::error!("server error: {error}");
            1
        }
    }
}

pub fn router(state: Arc<RelayState>, shutdown: Arc<Notify>) -> Router {
    Router::new()
        .route("/", get(entry)) // 扩展 WS（Upgrade 头）或人类探测
        .route("/cmd", post(cmd))
        .route("/health", get(health))
        .route("/shutdown", post(handle_shutdown))
        .with_state((state, shutdown))
        // CLI 上传 body 上限：js code ≤100KB、paths ≤20 条，8MB 绰绰有余。
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
}

// ---------------------------------------------------------------- origin / host

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
///
/// 允许两类来源：无 Origin（CLI / curl / 脚本）与扩展自身（chrome-extension://）。
/// 浏览器页面只允许回环 origin——本工具本来就只服务本机。
fn origin_is_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" {
        return false;
    }
    if ["chrome-extension://", "moz-extension://", "extension://"]
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
fn check_origin_and_host(headers: &HeaderMap) -> Option<Response> {
    if !host_header_is_loopback(headers) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "forbidden: Host header must be 127.0.0.1, localhost or [::1]",
            )
                .into_response(),
        );
    }
    if !origin_is_allowed(headers) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "forbidden: Origin must be a loopback or chrome-extension origin",
            )
                .into_response(),
        );
    }
    None
}

fn error_body(code: &str, msg: &str, hint: &str) -> Value {
    json!({ "code": code, "msg": msg, "hint": hint })
}

fn error_response(status: StatusCode, code: &str, msg: &str, hint: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        error_body(code, msg, hint).to_string(),
    )
        .into_response()
}

// ---------------------------------------------------------------- handlers

/// `GET /`：带 `Upgrade: websocket` 头 → 扩展 WS；否则是人类/脚本的探测。
async fn entry(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State((state, _shutdown)): State<(Arc<RelayState>, Arc<Notify>)>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers) {
        return response;
    }
    match ws {
        Ok(ws) => ws
            .max_message_size(64 * 1024 * 1024)
            .max_frame_size(64 * 1024 * 1024)
            .on_upgrade(move |socket| relay::handle_extension_ws(socket, state)),
        // 扩展的 WS 握手一定带 Upgrade 头；到这里说明是普通 HTTP 客户端。
        Err(_) => (
            StatusCode::OK,
            format!(
                "ema2 {} relay\nws=ws://127.0.0.1:{} (extension) http=http://127.0.0.1:{}/cmd (cli)\n",
                env!("CARGO_PKG_VERSION"),
                state.port,
                state.port
            ),
        )
            .into_response(),
    }
}

/// `POST /cmd`：CLI 工具调用。
///
/// 请求体：`{"tool": "tabs", "args": {...}, "out": "可选落盘路径"}`。
/// 响应 200：`{"result": <扩展返回的 result>}`；4xx/5xx：`{"code","msg","hint"}`。
#[derive(Deserialize)]
struct CmdRequest {
    tool: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    out: Option<PathBuf>,
}

async fn cmd(
    State((state, _shutdown)): State<(Arc<RelayState>, Arc<Notify>)>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<CmdRequest>,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers) {
        return response;
    }
    if !tools::contains(&request.tool) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &format!("unknown tool: {}", request.tool),
            "tools: tabs open read snapshot act wait js observe files shot batch",
        );
    }
    let Some(arguments) = request.args.as_object().cloned() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "args must be a JSON object",
            "pass tool arguments as a JSON object",
        );
    };
    let arguments = Value::Object(arguments);
    if let Err(message) = tools::validate_arguments(&request.tool, &arguments) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &message,
            "fix the arguments; see docs/TOOLS.md",
        );
    }
    if !state.extension_connected() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "extension_not_connected",
            "server is running but the Edge extension is not connected",
            "check the extension popup: its port must match (default 7744), then reload the extension",
        );
    }
    let call = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": request.tool, "arguments": arguments },
    });
    let timeout = tools::timeout_for(&request.tool, &arguments);
    let mut response = match state.call_extension(&call, timeout).await {
        Ok(value) => value,
        Err(message) => {
            return error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "extension_error",
                &message,
                "check `ema2 status`; the extension must be connected and responsive",
            );
        }
    };

    // 工具结果（含 isError 的业务错误）走 200；CLI 负责 stderr + 退出码。
    let inline = arguments
        .get("inline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let har = arguments.get("har").and_then(Value::as_bool) == Some(true);
    if let Some(result) = response.get_mut("result") {
        if request.tool == "shot" && !inline {
            *result =
                save::save_screenshot(&state, std::mem::take(result), request.out.as_deref()).await;
        } else if request.tool == "observe" && har {
            *result = save::save_har(&state, std::mem::take(result), request.out.as_deref()).await;
        }
    }
    let body = json!({ "result": response.get("result").cloned().unwrap_or(Value::Null) });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// `GET /health`：CLI `ema2 status` 的数据源。
async fn health(
    State((state, _shutdown)): State<(Arc<RelayState>, Arc<Notify>)>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers) {
        return response;
    }
    let body = json!({
        "name": "ema2",
        "version": env!("CARGO_PKG_VERSION"),
        "port": state.port,
        "pid": state.pid,
        "uptime_secs": state.uptime_secs(),
        "pending": state.pending_count(),
        "extension": {
            "connected": state.extension_connected(),
            "version": state.extension_version(),
            "last_ping_secs": state.last_ping_secs(),
        },
    });
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

/// `POST /shutdown`：`ema2 server stop` 的干净通道。
async fn handle_shutdown(
    State((_state, shutdown)): State<(Arc<RelayState>, Arc<Notify>)>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = check_origin_and_host(&headers) {
        return response;
    }
    shutdown.notify_waiters();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"stopping":true}"#,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::state::RelayState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    fn app() -> (axum::Router, Arc<RelayState>) {
        let state = RelayState::new(PathBuf::from("target/test-output"), 7744);
        (router(state.clone(), Arc::new(Notify::new())), state)
    }

    fn post(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:7744")
            .body(Body::empty())
            .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_reports_daemon_state() {
        let (app, _state) = app();
        let response = app.oneshot(get("/health")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["name"], "ema2");
        assert_eq!(body["port"], 7744);
        assert_eq!(body["extension"]["connected"], false);
        assert_eq!(body["pid"], std::process::id());
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let (app, _) = app();
        let response = app
            .oneshot(post("/cmd", json!({ "tool": "explode", "args": {} })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(body["code"], "invalid_argument");
    }

    #[tokio::test]
    async fn invalid_arguments_carry_the_validator_message() {
        let (app, _) = app();
        let response = app
            .oneshot(post("/cmd", json!({ "tool": "wait", "args": {} })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(body["code"], "invalid_argument");
        assert!(
            body["msg"]
                .as_str()
                .unwrap_or_default()
                .contains("exactly one"),
            "{}",
            body
        );
    }

    #[tokio::test]
    async fn missing_extension_is_a_503_with_code() {
        let (app, _) = app();
        let response = app
            .oneshot(post(
                "/cmd",
                json!({ "tool": "tabs", "args": { "action": "list" } }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["code"], "extension_not_connected");
        assert!(
            body["hint"].as_str().unwrap_or_default().contains("popup"),
            "{}",
            body
        );
    }

    #[tokio::test]
    async fn root_answers_plain_http_without_upgrade() {
        let (app, _) = app();
        let response = app.oneshot(get("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("ema2"), "{text}");
        assert!(text.contains("/cmd"), "{text}");
    }

    #[tokio::test]
    async fn rejects_foreign_origins_and_hosts() {
        let (app, _) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/cmd")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::ORIGIN, "http://evil.example")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "tool": "tabs", "args": {} }).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let request = Request::builder()
            .method("GET")
            .uri("/health")
            .header(header::HOST, "evil.example:7744")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn accepts_loopback_and_extension_origins() {
        for origin in [
            "http://127.0.0.1:3000",
            "http://localhost:5173",
            "chrome-extension://abcdefghijklmnop",
        ] {
            let (app, _) = app();
            let request = Request::builder()
                .method("GET")
                .uri("/health")
                .header(header::HOST, "127.0.0.1:7744")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "origin {origin}");
        }
    }

    #[tokio::test]
    async fn shutdown_triggers_the_notify() {
        let (app, _) = app();
        let response = app.oneshot(post("/shutdown", json!({}))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["stopping"], true);
    }

    /// 工具执行成功路径：注册一个假扩展连接，/cmd 必须透传它的 result。
    #[tokio::test]
    async fn cmd_forwards_to_the_extension_and_returns_its_result() {
        let (_app, state) = app();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let generation = state.register_extension(tx);

        let call = {
            let state = state.clone();
            tokio::spawn(async move {
                let app = router(state.clone(), Arc::new(Notify::new()));
                app.oneshot(post(
                    "/cmd",
                    json!({ "tool": "tabs", "args": { "action": "list" } }),
                ))
                .await
                .unwrap()
            })
        };
        // 假扩展：读出转发的 tools/call，回一个 result。
        let reply = tokio::spawn(async move {
            use axum::extract::ws::Message;
            let Some(Message::Text(text)) = rx.recv().await else {
                panic!("expected a forwarded tools/call");
            };
            let forwarded: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(forwarded["method"], "tools/call");
            assert_eq!(forwarded["params"]["name"], "tabs");
            let id = forwarded["id"].clone();
            state.complete_pending(
                &id,
                generation,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": "total=0 shown=0 next=none" }] }
                }),
            );
        });
        let response = call.await.unwrap();
        reply.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["result"]["content"][0]["text"],
            "total=0 shown=0 next=none"
        );
    }
}
