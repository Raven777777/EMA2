//! HTTP 层：MCP Streamable HTTP + legacy SSE + Edge 扩展 WebSocket + 健康检查。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::mcp;
use crate::relay;
use crate::state::RelayState;

pub fn router(state: Arc<RelayState>) -> Router {
    Router::new()
        .route("/", get(ws_upgrade)) // Edge 扩展（连接 ws://127.0.0.1:PORT）
        .route("/mcp", get(mcp_sse).post(mcp_post)) // MCP Streamable HTTP
        .route("/sse", get(legacy_sse)) // legacy SSE transport
        .route("/messages", axum::routing::post(legacy_messages))
        .route("/status", get(status))
        .with_state(state)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
        .layer(tower_http::compression::CompressionLayer::new())
}

fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("ema2-{nanos:x}")
}

/// 周期性清理长期不活跃的 legacy SSE 会话。
pub fn spawn_sse_gc(state: Arc<RelayState>, ttl: Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let now = Instant::now();
            let mut map = state.sse_senders.lock().unwrap();
            let before = map.len();
            map.retain(|_, (last, _)| now.duration_since(*last) < ttl);
            if before != map.len() {
                tracing::info!("sse GC: {before} -> {} sessions", map.len());
            }
            let mut mcp_sessions = state.mcp_sessions.lock().unwrap();
            let expired: Vec<String> = mcp_sessions
                .iter()
                .filter(|(_, last)| now.duration_since(**last) >= ttl)
                .map(|(id, _)| id.clone())
                .collect();
            for id in expired {
                mcp_sessions.remove(&id);
                state.mcp_event_senders.lock().unwrap().remove(&id);
            }
        }
    });
}

/// Edge 扩展通过 WebSocket 连到根路径。
async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !state.authorized(auth_header(&headers).or_else(|| params.get("token").map(String::as_str)))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.max_message_size(8 * 1024 * 1024)
        .max_frame_size(8 * 1024 * 1024)
        .on_upgrade(move |socket| relay::handle_extension_ws(socket, state))
}

fn auth_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
}

/// POST /mcp —— MCP Streamable HTTP 请求。
async fn mcp_post(
    State(state): State<Arc<RelayState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.authorized(auth_header(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid JSON-RPC").into_response();
        }
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
            return StatusCode::NOT_FOUND.into_response();
        }
    }

    let response = mcp::handle(&state, req).await;

    match response {
        Some(mut resp) => {
            let mut builder = Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "no-store");
            if is_initialize && resp.get("result").is_some() {
                let sid = state.create_mcp_session();
                builder = builder.header("mcp-session-id", &sid);
                resp["result"]["_meta"]["sessionId"] = json!(sid);
            }
            builder
                .body(axum::body::Body::from(resp.to_string()))
                .unwrap()
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// GET /mcp —— Streamable HTTP 事件流（保持打开，接收服务端事件）。
async fn mcp_sse(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if !state.authorized(auth_header(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
    else {
        return (StatusCode::BAD_REQUEST, "Mcp-Session-Id is required").into_response();
    };
    if !state.touch_mcp_session(session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    state
        .mcp_event_senders
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), tx);
    let session_id = session_id.to_owned();
    let cleanup_state = state.clone();
    let stream = async_stream::stream! {
        while let Some(value) = rx.recv().await {
            if let Ok(event) = Event::default().event("message").json_data(value) {
                yield Ok::<Event, serde_json::Error>(event);
            }
        }
        cleanup_state.mcp_event_senders.lock().unwrap().remove(&session_id);
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// GET /sse —— legacy SSE transport 入口。
async fn legacy_sse(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if !state.authorized(auth_header(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let session_id = new_session_id();
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
        yield Ok::<Event, Infallible>(Event::default().event("endpoint").data(endpoint));
        while let Some(v) = rx.recv().await {
            yield Ok::<Event, Infallible>(
                Event::default().event("message").json_data(v).unwrap_or_default()
            );
        }
        cleanup_state.sse_senders.lock().unwrap().remove(&cleanup_id);
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
    if !state.authorized(auth_header(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(session_id) = params.get("sessionId").cloned() else {
        return (StatusCode::BAD_REQUEST, "sessionId is required").into_response();
    };
    if !state.sse_senders.lock().unwrap().contains_key(&session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let response = mcp::handle(&state, req).await;
    if let Some(resp) = response {
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

/// GET /status —— 健康检查。
async fn status(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if !state.authorized(auth_header(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body = json!({
        "name": "ema2",
        "version": env!("CARGO_PKG_VERSION"),
        "extension_connected": state.extension_connected(),
        "pending": state.pending_count(),
        "sessions": state.sse_senders.lock().unwrap().len(),
        "mcp_sessions": state.mcp_sessions.lock().unwrap().len(),
    })
    .to_string();
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}
