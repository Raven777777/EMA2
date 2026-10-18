//! EMA2 (Rust) —— 单进程中继：向 AI 提供 MCP over HTTP，向 Edge 扩展提供 WebSocket。

mod config;
mod http;
mod mcp;
mod relay;
mod state;
mod tools;

use std::io::{Read, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::state::RelayState;

#[derive(Parser)]
#[command(
    name = "ema2",
    version,
    about = "Edge MCP Agent 2 (Rust) - single-process relay between AI (MCP over HTTP) and the Edge browser extension"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行常驻中继（MCP HTTP + 扩展 WebSocket）
    Run {
        /// 监听端口；未指定时使用 EDGE_MCP_PORT，再回落到 8765
        #[arg(long)]
        port: Option<u16>,
    },
    /// 检查中继是否运行、扩展是否已连接
    Status {
        /// 监听端口；未指定时使用 EDGE_MCP_PORT，再回落到 8765
        #[arg(long)]
        port: Option<u16>,
    },
    /// 让扩展重新加载自身（开发用，等价于 edge://extensions 点“重新加载”）
    Reload {
        /// 监听端口；未指定时使用 EDGE_MCP_PORT，再回落到 8765
        #[arg(long)]
        port: Option<u16>,
    },
}

fn main() {
    tracing_subscriber::fmt().with_target(false).init();
    let cli = Cli::parse();
    // 轻 I/O 桥接场景：固定少量 worker，避免按 CPU 核数过量创建线程
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_io()
        .enable_time()
        .build()
        .expect("failed to build tokio runtime");
    rt.block_on(async move {
        match cli.command {
            Commands::Run { port } => run(port).await,
            Commands::Status { port } => check_status(port),
            Commands::Reload { port } => reload_extension(port),
        }
    });
}

async fn run(port: Option<u16>) {
    let mut cfg = config::Config::from_env();
    // CLI 显式指定时优先，否则沿用 EDGE_MCP_PORT / 默认值。
    if let Some(port) = port {
        cfg.port = port;
    }
    if cfg.token.is_none() {
        tracing::warn!(
            "EDGE_MCP_TOKEN is not set: any local process that can reach 127.0.0.1:{} can drive the browser",
            cfg.port
        );
    }
    tracing::info!("screenshot output directory: {}", cfg.output_dir.display());

    let state = RelayState::new(cfg.token.clone(), cfg.output_dir.clone());
    relay::spawn_keepalive(state.clone(), cfg.clone());
    http::spawn_session_gc(state.clone(), Duration::from_secs(600));
    let app = http::router(state);

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", cfg.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("failed to bind 127.0.0.1:{}: {error}", cfg.port);
            std::process::exit(1);
        }
    };
    tracing::info!(
        "EMA2 listening on http://127.0.0.1:{}/mcp (MCP) and ws://127.0.0.1:{}/ (extension)",
        cfg.port,
        cfg.port
    );
    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("server error: {error}");
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// 最小 HTTP/1.1 请求（仅本机回环），返回 (状态行, body)。
fn request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> std::io::Result<(String, String)> {
    let nl = format!("{}{}", char::from(13u8), char::from(10u8));
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut lines = vec![
        format!("{method} {path} HTTP/1.1"),
        format!("Host: 127.0.0.1:{port}"),
        "Content-Type: application/json".to_string(),
        "Content-Length: 0".to_string(),
        "Connection: close".to_string(),
    ];
    if let Some(token) = token {
        lines.push(format!("Authorization: Bearer {token}"));
    }
    let req = lines.join(&nl) + &nl + &nl;
    stream.write_all(req.as_bytes())?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let marker = format!("{nl}{nl}");
    let (head, body) = raw.split_once(&marker).unwrap_or((raw.as_str(), ""));
    let status = head.lines().next().unwrap_or_default().to_owned();
    Ok((status, body.to_owned()))
}

/// 每次 reload 前把扩展 manifest 的 patch 版本 +1。
///
/// Edge 会按 manifest 版本缓存扩展脚本，只发 `chrome.runtime.reload()` 不会重读磁盘；
/// 改版本号是让浏览器真正重新加载 extension/ 下文件的最省事办法（开发用）。
fn bump_extension_version() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();
    if let Some(dir) = exe.parent() {
        candidates.push(dir.join("../extension/manifest.json"));
        candidates.push(dir.join("extension/manifest.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("extension/manifest.json"));
    }
    for path in candidates {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let old = value
            .get("version")
            .and_then(|version| version.as_str())
            .unwrap_or("1.5.0")
            .to_owned();
        let mut parts: Vec<u64> = old
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        while parts.len() < 3 {
            parts.push(0);
        }
        parts[2] += 1;
        let new_version = format!("{}.{}.{}", parts[0], parts[1], parts[2]);
        value["version"] = serde_json::Value::String(new_version.clone());
        let Ok(pretty) = serde_json::to_string_pretty(&value) else {
            continue;
        };
        if std::fs::write(&path, pretty + "\n").is_err() {
            continue;
        }
        return Some(new_version);
    }
    None
}

/// 请求中继让扩展重新加载（开发时改完 extension/ 用这个，不用手点 Edge）。
fn reload_extension(port: Option<u16>) {
    let port = port
        .or_else(config::env_port)
        .unwrap_or(config::DEFAULT_PORT);
    let token = config::env_token();
    if let Some(version) = bump_extension_version() {
        println!("extension version -> {version}");
    }
    match request(port, "POST", "/reload", token.as_deref()) {
        Ok((status, body)) if status.contains(" 200 ") => {
            println!("{}", body.trim());
        }
        Ok((status, body)) => {
            println!("reload failed: {status}");
            if !body.trim().is_empty() {
                println!("{}", body.trim());
            }
            std::process::exit(1);
        }
        Err(error) => {
            println!("cannot reach EMA2 on 127.0.0.1:{port}: {error}");
            std::process::exit(1);
        }
    }
}

/// 通过 `/status` 做健康检查。
///
/// 必须带上 `EDGE_MCP_TOKEN`（如果配置了），否则只能拿到 401；
/// 失败时返回非 0 退出码，方便脚本判断。
fn check_status(port: Option<u16>) {
    let port = port
        .or_else(config::env_port)
        .unwrap_or(config::DEFAULT_PORT);
    let token = config::env_token();

    let mut stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(stream) => stream,
        Err(error) => {
            println!("EMA2 is NOT running on 127.0.0.1:{port}: {error}");
            std::process::exit(1);
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut request =
        format!("GET /status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(token) = &token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    let _ = stream.write_all(request.as_bytes());

    let mut raw = String::new();
    if let Err(error) = stream.read_to_string(&mut raw) {
        println!("EMA2 status request failed on 127.0.0.1:{port}: {error}");
        std::process::exit(1);
    }
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        println!("EMA2 /status returned: {status_line}");
        if status_line.contains(" 401 ") {
            println!("hint: set EDGE_MCP_TOKEN (same value as the relay) and retry.");
        }
        std::process::exit(1);
    }
    match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{}", body.trim()),
        },
        Err(_) => println!("{}", body.trim()),
    }
}
