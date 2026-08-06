//! EMA2 (Rust) —— 单进程中继：向 AI 提供 MCP over HTTP，向 Edge 扩展提供 WebSocket。

mod config;
mod http;
mod mcp;
mod relay;
mod state;
mod tools;

use clap::{Parser, Subcommand};
use std::io::{Read, Write};

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
        #[arg(long, default_value_t = 8765)]
        port: u16,
    },
    /// 检查中继是否运行、扩展是否已连接
    Status {
        #[arg(long, default_value_t = 8765)]
        port: u16,
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
        }
    });
}

async fn run(port: u16) {
    let cfg = config::Config {
        port,
        ..config::Config::from_env()
    };
    let state = RelayState::new(cfg.token.clone(), cfg.output_dir.clone());
    relay::spawn_keepalive(state.clone(), cfg.clone());
    http::spawn_sse_gc(state.clone(), std::time::Duration::from_secs(600));
    let app = http::router(state);

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", cfg.port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind 127.0.0.1:{}: {e}", cfg.port);
            std::process::exit(1);
        }
    };
    tracing::info!(
        "EMA2 listening on http://127.0.0.1:{}/mcp (MCP) and ws://127.0.0.1:{}/ (extension)",
        cfg.port,
        cfg.port
    );
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("server error: {e}");
    }
}

fn check_status(port: u16) {
    let mut stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => {
            println!("EMA2 is NOT running on port {port}.");
            std::process::exit(1);
        }
    };
    let req = "GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(req.as_bytes());
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    if let Some(body) = buf.split("\r\n\r\n").nth(1) {
        println!("{body}");
    } else {
        println!("{buf}");
    }
}
