//! daemon HTTP 层：单端口 7744 同时收 WS（扩展）与 HTTP（CLI）。
//!
//! 分流靠路由层完成：`GET /` 的 handler 以 `Result<WebSocketUpgrade, _>` 接收——
//! 带 `Upgrade: websocket` 头的请求走 WS 升级，其余当作普通 HTTP。
//! CLI 命令全部走 `POST /cmd`（`{"tool":..., "args":{...}, "out":可选路径}`）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::paths;
use crate::relay;
use crate::save;
use crate::state::{CallError, RelayState};
use crate::tools;

/// daemon 主循环：绑定单端口，WS 与 HTTP 共用 listener，直到 `/shutdown`。
pub async fn run(port: u16) -> i32 {
    if port == 0 {
        tracing::error!("invalid port 0: EMA2 needs a fixed port for the pid/log files");
        return 2;
    }
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

    // 硬期限：graceful shutdown 会等所有在途请求跑完，而 batch/js 可能正在等
    // 扩展（最长 600s）。收到关闭信号后最多再给 3s，然后直接退出，避免
    // “端口已关闭但旧 daemon 还握着扩展 WS”导致新实例收不到扩展。
    {
        let state = state.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = state.wait_for_shutdown() => {}
                _ = tokio::signal::ctrl_c() => {
                    state.begin_shutdown();
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            tracing::warn!("graceful shutdown did not finish within 3s; exiting");
            std::process::exit(0);
        });
    }

    let app = router(state.clone(), shutdown.clone());

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("cannot bind 127.0.0.1:{port}: {error}");
            return 2;
        }
    };
    let pid_path = paths::pid_file(port);
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    // 记录 port/started_at：`server stop` 强杀前可以确认 PID 没被复用。
    if let Err(error) = std::fs::write(
        &pid_path,
        format!("{} {port} {started_at}\n", std::process::id()),
    ) {
        tracing::warn!("cannot write PID file {}: {error}", pid_path.display());
    }
    tracing::info!(
        "ema2 {} listening on http://127.0.0.1:{port}/ (health/cmd) and ws://127.0.0.1:{port}/ (extension)",
        paths::PRODUCT_VERSION
    );

    let graceful = {
        let shutdown = shutdown.clone();
        let state = state.clone();
        async move {
            tokio::select! {
                _ = shutdown.notified() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            state.begin_shutdown();
        }
    };
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await;

    // 只删除自己写的 PID 文件，避免误删同端口重启后的新实例。
    if std::fs::read_to_string(&pid_path)
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<u32>().ok())
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
        //
        // tower-http 的 RequestBodyLimitLayer 只限制 body 流；axum 的 Json/Bytes
        // 提取器还各自带默认 2MB 的 DefaultBodyLimit。两者必须同时放开，否则
        // 上传会在 2MB 处先被提取器拒绝（HTTP 413）。
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
}

// ---------------------------------------------------------------- origin / host

/// CLI 请求必须带的自定义头。
///
/// 浏览器页面可以发跨源 `POST`，但自定义头会触发 CORS preflight；EMA2 不返回
/// CORS 许可，所以恶意网页无法伪造这个头。表单（无 preflight）也带不了自定义头。
/// 这不是 token：本机进程仍然可以直接调用；它挡的是“网页借用户浏览器打本机端口”。
const CLIENT_HEADER: &str = "x-ema2-client";
const CLIENT_VALUE: &str = "ema2-cli";

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

fn header_origin(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
}

/// 只接受 Edge 扩展的 Origin，且 ID 必须是 32 个 a-p 字符。
fn origin_is_extension(origin: &str) -> bool {
    origin
        .strip_prefix("chrome-extension://")
        .is_some_and(relay::extension_id_valid)
}

#[derive(Clone, Copy)]
enum Access {
    /// `/health` 允许扩展 popup 读取；不允许网页 Origin。
    Health,
    /// `/cmd`、`/shutdown` 只允许无 Origin 的 CLI，并额外要求 CSRF 头。
    Command,
}

/// 返回 `Some(response)` 表示应当直接拒绝该请求。
fn check_access(headers: &HeaderMap, access: Access) -> Option<Response> {
    if !host_header_is_loopback(headers) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "forbidden: Host header must be 127.0.0.1, localhost or [::1]",
            )
                .into_response(),
        );
    }
    let origin = header_origin(headers);
    match access {
        Access::Health => {
            if origin.is_some_and(|origin| !origin_is_extension(origin)) {
                return Some(
                    (
                        StatusCode::FORBIDDEN,
                        "forbidden: only the Edge extension may read /health from a browser context",
                    )
                        .into_response(),
                );
            }
        }
        Access::Command => {
            if origin.is_some() {
                return Some(
                    (
                        StatusCode::FORBIDDEN,
                        "forbidden: CLI commands must not carry a browser Origin header",
                    )
                        .into_response(),
                );
            }
            let client = headers
                .get(CLIENT_HEADER)
                .and_then(|value| value.to_str().ok());
            if client != Some(CLIENT_VALUE) {
                return Some(
                    (
                        StatusCode::FORBIDDEN,
                        "forbidden: missing CSRF header; use the ema2 CLI",
                    )
                        .into_response(),
                );
            }
        }
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
    if !host_header_is_loopback(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "forbidden: Host header must be 127.0.0.1, localhost or [::1]",
        )
            .into_response();
    }
    let origin = header_origin(&headers).map(str::to_owned);
    if origin
        .as_deref()
        .is_some_and(|origin| !origin_is_extension(origin))
    {
        return (
            StatusCode::FORBIDDEN,
            "forbidden: the extension WebSocket Origin must be chrome-extension://<id>",
        )
            .into_response();
    }
    match ws {
        Ok(ws) => ws
            .max_message_size(64 * 1024 * 1024)
            .max_frame_size(64 * 1024 * 1024)
            .on_upgrade(move |socket| relay::handle_extension_ws(socket, state, origin)),
        // 扩展的 WS 握手一定带 Upgrade 头；到这里说明是普通 HTTP 客户端。
        Err(_) => (
            StatusCode::OK,
            format!(
                "ema2 {} relay\nws=ws://127.0.0.1:{} (extension) http=http://127.0.0.1:{}/cmd (cli)\n",
                paths::PRODUCT_VERSION,
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
    if let Some(response) = check_access(&headers, Access::Command) {
        return response;
    }
    if !tools::contains(&request.tool) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "unknown_tool",
            &format!("unknown tool: {}", request.tool),
            "tools: tabs open read snapshot act wait js observe files shot batch",
        );
    }
    let Some(arguments) = request.args.as_object().cloned() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_arguments",
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
            "fix the arguments; run `ema2 <command> --help` (scripts: `ema2 act --help`)",
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
        Err(error) => {
            // 没有扩展/扩展刚断是“可重连”的 503；只有扩展收下却没回才是 504。
            let (status, code, hint) = match &error {
                CallError::NotConnected | CallError::Lost => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "extension_not_connected",
                    "check the extension popup: its port must match (default 7744), then click reconnect or reload the extension",
                ),
                CallError::Timeout(_) => (
                    StatusCode::GATEWAY_TIMEOUT,
                    "extension_error",
                    "check `ema2 status`; the extension must be connected and responsive",
                ),
                CallError::ShuttingDown => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "shutting_down",
                    "the daemon is stopping; wait a moment and start it again with `ema2 server`",
                ),
            };
            return error_response(status, code, &error.to_string(), hint);
        }
    };
    // JSON-RPC 层错误（未知 method / 扩展内部异常）没有 result；不能把它当成空成功。
    if let Some(error) = response.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .map(|code| format!(" {code}"))
            .unwrap_or_default();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("extension returned a JSON-RPC error");
        return error_response(
            StatusCode::BAD_GATEWAY,
            "extension_error",
            &format!("extension returned JSON-RPC error{code}: {message}"),
            "reload the extension and retry; check `ema2 status`",
        );
    }

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
    let Some(result) = response.get("result") else {
        return error_response(
            StatusCode::BAD_GATEWAY,
            "extension_error",
            "extension returned a JSON-RPC response without a result",
            "reload the extension and retry; check `ema2 status`",
        );
    };
    let body = json!({ "result": result });
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
    if let Some(response) = check_access(&headers, Access::Health) {
        return response;
    }
    let body = json!({
        "name": "ema2",
        "version": paths::PRODUCT_VERSION,
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
    State((state, shutdown)): State<(Arc<RelayState>, Arc<Notify>)>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = check_access(&headers, Access::Command) {
        return response;
    }
    // 先让在途 call_extension 立即返回 ShuttingDown（watch 不会丢信号），
    // 再唤醒 graceful future。notify_one 额外保留一个 permit：handler 可能
    // 早于 graceful future 的首次 poll，notify_waiters 此时是空操作。
    state.begin_shutdown();
    shutdown.notify_waiters();
    shutdown.notify_one();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "relay": "stopping", "stopping": true }).to_string(),
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
    use std::time::Duration;
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
            .header("x-ema2-client", "ema2-cli")
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
        assert_eq!(body["code"], "unknown_tool");
    }

    #[tokio::test]
    async fn non_object_arguments_are_rejected() {
        let (app, _) = app();
        let response = app
            .oneshot(post("/cmd", json!({ "tool": "tabs", "args": [] })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(body["code"], "invalid_arguments");
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

    /// axum 的 Json 提取器默认只接受 2MB；CLI 上传可以接近 8MB。
    /// 这个测试保证请求能穿过提取器到达参数校验（400），而不是被 413 截断。
    #[tokio::test]
    async fn large_command_body_passes_the_extractor_limit() {
        let (app, _) = app();
        let response = app
            .oneshot(post(
                "/cmd",
                json!({ "tool": "js", "args": { "code": "x".repeat(2_500_000) } }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(body["code"], "invalid_argument");
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
    async fn rejects_foreign_origins_loopback_pages_and_missing_csrf_headers() {
        let (app, _) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/cmd")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::ORIGIN, "http://evil.example")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-ema2-client", "ema2-cli")
            .body(Body::from(
                json!({ "tool": "tabs", "args": {} }).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // 本机网页同样是跨源页面：不能借用户浏览器调用 /cmd。
        let request = Request::builder()
            .method("POST")
            .uri("/cmd")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::ORIGIN, "http://127.0.0.1:3000")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-ema2-client", "ema2-cli")
            .body(Body::from(
                json!({ "tool": "tabs", "args": {} }).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // 无 Origin 但没有 CSRF 头：表单/简单请求必须被拒。
        let request = Request::builder()
            .method("POST")
            .uri("/shutdown")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
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
    async fn accepts_extension_origin_for_health_only() {
        let origin = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
        let (app, _) = app();
        let request = Request::builder()
            .method("GET")
            .uri("/health")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // 扩展 Origin 不能调用 /cmd：CLI 永远不带 Origin。
        let request = Request::builder()
            .method("POST")
            .uri("/cmd")
            .header(header::HOST, "127.0.0.1:7744")
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-ema2-client", "ema2-cli")
            .body(Body::from(
                json!({ "tool": "tabs", "args": {} }).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        for origin in [
            "chrome-extension://too-short",
            "chrome-extension://abcdefghijklmnopqrstuvwxyzabcde0",
            "http://127.0.0.1:3000",
            "https://localhost:5173",
        ] {
            let request = Request::builder()
                .method("GET")
                .uri("/health")
                .header(header::HOST, "127.0.0.1:7744")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "origin {origin}");
        }
    }

    #[tokio::test]
    async fn shutdown_triggers_the_notify_and_marks_the_state() {
        let state = RelayState::new(PathBuf::from("target/test-output"), 7744);
        let shutdown = Arc::new(Notify::new());
        let app = router(state.clone(), shutdown.clone());
        let response = app.oneshot(post("/shutdown", json!({}))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["stopping"], true);
        assert_eq!(body["relay"], "stopping");
        assert!(state.is_shutting_down());
        // permit 必须保留给随后才被 poll 的 graceful future。
        assert!(
            tokio::time::timeout(Duration::from_millis(200), shutdown.notified())
                .await
                .is_ok(),
            "shutdown notification was lost"
        );
    }

    /// 关闭过程中 /cmd 必须立刻返回 503 shutting_down，而不是等满工具超时。
    #[tokio::test]
    async fn cmd_reports_shutting_down_while_stopping() {
        let (_app, state) = app();
        let (tx, _rx) = tokio::sync::mpsc::channel(crate::state::EXT_CHANNEL_CAPACITY);
        state.register_extension(tx);
        state.begin_shutdown();

        let app = router(state, Arc::new(Notify::new()));
        let response = app
            .oneshot(post(
                "/cmd",
                json!({ "tool": "tabs", "args": { "action": "list" } }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["code"], "shutting_down");
    }

    /// JSON-RPC 错误没有 result；daemon 必须报错，而不是返回 200 + null 让 CLI 静默成功。
    #[tokio::test]
    async fn cmd_maps_json_rpc_error_to_extension_error() {
        let (_app, state) = app();
        let (tx, mut rx) = tokio::sync::mpsc::channel(crate::state::EXT_CHANNEL_CAPACITY);
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
        let reply = tokio::spawn(async move {
            use axum::extract::ws::Message;
            let Some(Message::Text(text)) = rx.recv().await else {
                panic!("expected a forwarded tools/call");
            };
            let forwarded: Value = serde_json::from_str(&text).unwrap();
            let id = forwarded["id"].clone();
            state.complete_pending(
                &id,
                generation,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Method not found" }
                }),
            );
        });
        let response = call.await.unwrap();
        reply.await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(response).await;
        assert_eq!(body["code"], "extension_error");
        assert!(
            body["msg"]
                .as_str()
                .unwrap_or_default()
                .contains("Method not found"),
            "{}",
            body
        );
    }

    /// 工具执行成功路径：注册一个假扩展连接，/cmd 必须透传它的 result。
    #[tokio::test]
    async fn cmd_forwards_to_the_extension_and_returns_its_result() {
        let (_app, state) = app();
        let (tx, mut rx) = tokio::sync::mpsc::channel(crate::state::EXT_CHANNEL_CAPACITY);
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
