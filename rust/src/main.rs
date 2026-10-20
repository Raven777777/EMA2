//! EMA2 (Rust) —— 单进程中继：向 AI 提供 MCP over HTTP，向 Edge 扩展提供 WebSocket。

mod clients;
mod config;
mod http;
mod jsonc;
mod mcp;
mod pair;
mod relay;
mod state;
mod tools;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::state::{AuthConfig, RelayState};

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
        /// 显式允许无 token 运行（任何能访问本机回环端口的进程都能控制浏览器）
        #[arg(long = "insecure-allow-no-token")]
        insecure_allow_no_token: bool,
    },
    /// 部署到日常 Edge（首次需加载一次解压扩展）、注册登录自启、打开授权页
    Install {
        /// 监听端口；未指定时使用 EDGE_MCP_PORT，再回落到 8765
        #[arg(long)]
        port: Option<u16>,
        /// 只部署文件，不打开 Edge 窗口
        #[arg(long = "no-browser")]
        no_browser: bool,
        /// 不注册登录自启（由用户自己管理启动方式）
        #[arg(long = "no-autostart")]
        no_autostart: bool,
        /// 用独立 profile 启动 EMA2 专用窗口（默认部署到日常 Edge）
        #[arg(long = "managed-profile")]
        managed_profile: bool,
        /// 卸载：移除自启、客户端配置条目与托管 profile
        #[arg(long)]
        uninstall: bool,
        /// 卸载时一并删除截图/HAR 输出目录
        #[arg(long = "purge-output")]
        purge_output: bool,
    },
    /// 查看/重置配对状态
    Pair {
        /// 删除已保存的 token（需重启中继后重新授权）
        #[arg(long)]
        reset: bool,
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
            Commands::Run {
                port,
                insecure_allow_no_token,
            } => run(port, insecure_allow_no_token).await,
            Commands::Install {
                port,
                no_browser,
                no_autostart,
                managed_profile,
                uninstall,
                purge_output,
            } => install(
                port,
                InstallOptions {
                    no_browser,
                    no_autostart,
                    managed_profile,
                    uninstall,
                    purge_output,
                },
            ),
            Commands::Pair { reset } => pair_command(reset),
            Commands::Status { port } => check_status(port),
            Commands::Reload { port } => reload_extension(port),
        }
    });
}

async fn run(port: Option<u16>, insecure_allow_no_token: bool) {
    let mut cfg = config::Config::from_env();
    // CLI 显式指定时优先，否则沿用 EDGE_MCP_PORT / 默认值。
    if let Some(port) = port {
        cfg.port = port;
    }
    if insecure_allow_no_token {
        cfg.allow_anonymous = true;
    }
    let state_path = config::state_file();
    let stored = pair::load(&state_path);
    let token = config::resolve_token(cfg.token.clone(), stored.token);
    if token.is_none() && !cfg.allow_anonymous {
        // 首次运行：进入待授权态。MCP 一律 503，只有配对通道可用。
        tracing::warn!(
            "no token yet: waiting for browser authorization on 127.0.0.1:{}",
            cfg.port
        );
        tracing::warn!(
            "open the extension authorization page (toolbar icon) and click \"授权此浏览器\""
        );
    }
    if token.is_none() && cfg.allow_anonymous {
        tracing::error!(
            "running WITHOUT authentication on 127.0.0.1:{} (explicitly allowed): any local process can drive the browser",
            cfg.port
        );
    }
    tracing::info!("screenshot output directory: {}", cfg.output_dir.display());
    tracing::info!("state file: {}", state_path.display());

    let state = RelayState::with_auth(
        AuthConfig {
            token,
            anonymous: cfg.allow_anonymous,
            state_path: Some(state_path),
            clients: stored.clients,
            clients_chosen: stored.clients_chosen,
            paired_at: stored.paired_at,
        },
        cfg.output_dir.clone(),
        cfg.port,
    );
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

/// 一次 manifest 版本改动，保留原始内容以便 reload 失败时回滚。
struct VersionBump {
    path: PathBuf,
    original: String,
    version: String,
}

/// 每次 reload 前把扩展 manifest 的 patch 版本 +1。
///
/// Edge 会按 manifest 版本缓存扩展脚本，只发 `chrome.runtime.reload()` 不会重读磁盘；
/// 改版本号是让浏览器真正重新加载 extension/ 下文件的最省事办法（开发用）。
fn bump_extension_version() -> Option<VersionBump> {
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
        let Ok(original) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&original) else {
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
        return Some(VersionBump {
            path,
            original,
            version: new_version,
        });
    }
    None
}

/// reload 未成功时把 manifest 写回原样，避免仓库里留下未提交的版本漂移。
fn rollback_manifest(bump: &VersionBump) {
    match std::fs::write(&bump.path, &bump.original) {
        Ok(()) => println!(
            "manifest version rolled back ({} kept at the committed value)",
            bump.path.display()
        ),
        Err(error) => println!(
            "warning: cannot roll back {} to the committed version: {error}",
            bump.path.display()
        ),
    }
}

/// 请求中继让扩展重新加载（开发时改完 extension/ 用这个，不用手点 Edge）。
///
/// 先检查中继与扩展是否在线，再改 manifest 版本；服务不通时不能污染仓库内的版本号。
fn reload_extension(port: Option<u16>) {
    let port = port
        .or_else(config::env_port)
        .unwrap_or(config::DEFAULT_PORT);
    let token = paired_token();
    match request(port, "GET", "/status", token.as_deref()) {
        Ok((status, body)) if status.contains(" 200 ") => {
            let connected = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value.get("extension_connected").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            if !connected {
                println!("extension is not connected; manifest version was not changed.");
                std::process::exit(1);
            }
        }
        Ok((status, _body)) => {
            println!("cannot verify EMA2 /status: {status}");
            if status.contains(" 401 ") {
                println!("hint: set EDGE_MCP_TOKEN (same value as the relay) and retry.");
            }
            std::process::exit(1);
        }
        Err(error) => {
            println!(
                "cannot reach EMA2 on 127.0.0.1:{port}: {error}; manifest version was not changed."
            );
            std::process::exit(1);
        }
    }
    let bump = bump_extension_version();
    if let Some(bump) = &bump {
        println!("extension version -> {}", bump.version);
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
            if let Some(bump) = &bump {
                rollback_manifest(bump);
            }
            std::process::exit(1);
        }
        Err(error) => {
            println!("cannot reach EMA2 on 127.0.0.1:{port}: {error}");
            if let Some(bump) = &bump {
                rollback_manifest(bump);
            }
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
    let token = paired_token();
    // 未配对时用无鉴权的 /pair/status，让 `ema2.exe status` 也能给出可执行的指引。
    let (path, auth) = match token.as_deref() {
        Some(token) => ("/status", Some(token)),
        None => ("/pair/status", None),
    };
    match request(port, "GET", path, auth) {
        Ok((status, body)) if status.contains(" 200 ") => {
            match serde_json::from_str::<serde_json::Value>(body.trim()) {
                Ok(value) => match serde_json::to_string_pretty(&value) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{}", body.trim()),
                },
                Err(_) => println!("{}", body.trim()),
            }
            if path == "/pair/status" {
                println!("\nhint: 尚未授权。打开 EMA2 Edge 窗口，点击“授权此浏览器”。");
            }
        }
        Ok((status, _body)) => {
            println!("EMA2 /status returned: {status}");
            if status.contains(" 401 ") {
                println!("hint: run `ema2.exe pair --reset` and authorize again.");
            }
            std::process::exit(1);
        }
        Err(error) => {
            println!("EMA2 is NOT running on 127.0.0.1:{port}: {error}");
            std::process::exit(1);
        }
    }
}

/// 已配对 token：环境变量优先，其次状态文件。
fn paired_token() -> Option<String> {
    config::resolve_token(config::env_token(), pair::load(&config::state_file()).token)
}

// ---------------------------------------------------------------- install / pair

/// 随 exe 一起分发的扩展文件；`install` 负责展开到用户目录。
const EXTENSION_FILES: &[(&str, &[u8])] = &[
    (
        "manifest.json",
        include_bytes!("../../extension/manifest.json"),
    ),
    (
        "background.js",
        include_bytes!("../../extension/background.js"),
    ),
    ("pair.html", include_bytes!("../../extension/pair.html")),
    ("pair.js", include_bytes!("../../extension/pair.js")),
    (
        "icons/icon16.png",
        include_bytes!("../../extension/icons/icon16.png"),
    ),
    (
        "icons/icon48.png",
        include_bytes!("../../extension/icons/icon48.png"),
    ),
    (
        "icons/icon128.png",
        include_bytes!("../../extension/icons/icon128.png"),
    ),
];

#[derive(Default)]
struct InstallOptions {
    no_browser: bool,
    no_autostart: bool,
    managed_profile: bool,
    uninstall: bool,
    purge_output: bool,
}

struct LoadedExtension {
    id: String,
    path: PathBuf,
}

fn install(port: Option<u16>, options: InstallOptions) {
    let port = port
        .or_else(config::env_port)
        .unwrap_or(config::DEFAULT_PORT);
    let state_dir = config::env_state_dir();
    if options.uninstall {
        uninstall(&state_dir, options.purge_output);
        return;
    }

    // 已经在日常 Edge 里加载过就从原路径更新：Edge 下次启动/重载时自动读到新文件。
    let loaded = find_loaded_extension();
    let extension_dir = loaded
        .as_ref()
        .map(|extension| extension.path.clone())
        .unwrap_or_else(|| state_dir.join("extension"));
    if let Err(error) = write_extension(&extension_dir) {
        eprintln!(
            "ema2 install: cannot deploy extension to {}: {error}",
            extension_dir.display()
        );
        std::process::exit(1);
    }
    println!("extension -> {}", extension_dir.display());

    match write_hidden_launcher(&state_dir, port) {
        Ok(path) => println!("launcher  -> {}", path.display()),
        Err(error) => eprintln!("ema2 install: cannot write launcher: {error}"),
    }
    if options.no_autostart {
        println!("autostart -> skipped (--no-autostart)");
    } else {
        match register_autostart(&state_dir) {
            Ok(path) => println!("autostart -> {}", path.display()),
            Err(error) => eprintln!("ema2 install: cannot register autostart: {error}"),
        }
    }

    if let Err(error) = ensure_relay_running(port, &state_dir) {
        eprintln!("ema2 install: relay did not start on 127.0.0.1:{port}: {error}");
    }

    // 已加载的扩展重新读取磁盘文件；不动 manifest 版本，避免无意义的版本漂移。
    if loaded.is_some() {
        match paired_token() {
            Some(token) => match request(port, "POST", "/reload", Some(&token)) {
                Ok((status, _)) if status.contains(" 200 ") => {
                    println!("reload    -> extension reloaded")
                }
                Ok((status, _)) if status.contains(" 202 ") => {
                    println!("reload    -> extension offline; new files apply on next start")
                }
                _ => println!(
                    "reload    -> skipped (extension not connected; it reloads on next start)"
                ),
            },
            None => {
                println!("reload    -> skipped (authorize first, then it reloads automatically)")
            }
        }
    }

    if options.no_browser {
        return;
    }
    let Some(edge) = edge_path() else {
        eprintln!("ema2 install: Edge not found; set EMA2_EDGE_EXE or install Microsoft Edge");
        return;
    };
    if options.managed_profile {
        let profile_dir = state_dir.join("edge-profile");
        if let Err(error) = fs::create_dir_all(&profile_dir) {
            eprintln!(
                "ema2 install: cannot create {}: {error}",
                profile_dir.display()
            );
            return;
        }
        match launch_edge(&edge, &profile_dir, &extension_dir) {
            Ok(()) => println!("browser   -> EMA2 window (managed profile)"),
            Err(error) => eprintln!("ema2 install: cannot launch Edge: {error}"),
        }
        return;
    }

    match &loaded {
        Some(loaded) => {
            let url = format!("chrome-extension://{}/pair.html", loaded.id);
            match open_edge_url(&edge, &url) {
                Ok(()) => println!("browser   -> daily Edge: {url}"),
                Err(error) => eprintln!("ema2 install: cannot open {url}: {error}"),
            }
        }
        None => {
            if let Err(error) = open_edge_url(&edge, "edge://extensions/") {
                eprintln!("ema2 install: cannot open edge://extensions: {error}");
            }
            println!("browser   -> daily Edge: edge://extensions");
            println!();
            println!("Edge 不允许程序静默安装扩展，需要你做一次（只此一次）：");
            println!("  1. 打开“开发人员模式”");
            println!("  2. 点“加载解压缩的扩展”，选择下面这个目录：");
            println!("     {}", extension_dir.display());
            println!("  3. 加载后扩展会自动打开授权页，点“授权此浏览器”");
            println!();
            println!("以后升级只需再运行 install：同一目录原地更新，无需重新加载。");
        }
    }
}

/// 在 Edge 的用户数据里找已加载的 EMA2 扩展（解包扩展不会复制进 Extensions/）。
fn find_loaded_extension() -> Option<LoadedExtension> {
    let root = edge_user_data_dir()?;
    let mut profiles: Vec<PathBuf> = fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name == "Default" || name.starts_with("Profile ")
        })
        .collect();
    profiles.sort_by_key(|path| path.file_name().and_then(|name| name.to_str()) != Some("Default"));

    for profile in profiles {
        for file in ["Preferences", "Secure Preferences"] {
            let Ok(raw) = fs::read_to_string(profile.join(file)) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let Some(settings) = value
                .get("extensions")
                .and_then(|extensions| extensions.get("settings"))
                .and_then(|settings| settings.as_object())
            else {
                continue;
            };
            for (id, config) in settings {
                let path = config
                    .get("path")
                    .and_then(|path| path.as_str())
                    .unwrap_or_default();
                if path.is_empty() {
                    continue;
                }
                // 只认可从磁盘读取的扩展：解包（4）或命令行 --load-extension（8）。
                // 商店/策略安装（1/2/3/9）的目录是只读的，覆盖会被 Edge 还原。
                if config
                    .get("location")
                    .and_then(|location| location.as_u64())
                    .is_some_and(|location| location != 4 && location != 8)
                {
                    continue;
                }
                let dir = PathBuf::from(path);
                let manifest_path = dir.join("manifest.json");
                if !manifest_path.is_file() {
                    continue;
                }
                let name = config
                    .get("manifest")
                    .and_then(|manifest| manifest.get("name"))
                    .and_then(|name| name.as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        fs::read_to_string(&manifest_path)
                            .ok()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                            .and_then(|manifest| {
                                manifest
                                    .get("name")
                                    .and_then(|name| name.as_str())
                                    .map(str::to_owned)
                            })
                    });
                if name.as_deref().is_some_and(|name| name.contains("EMA2")) {
                    return Some(LoadedExtension {
                        id: id.clone(),
                        path: dir,
                    });
                }
            }
        }
    }
    None
}

fn edge_user_data_dir() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("EMA2_EDGE_USER_DATA") {
        return Some(PathBuf::from(custom));
    }
    std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join("Microsoft")
            .join("Edge")
            .join("User Data")
    })
}

fn open_edge_url(edge: &Path, url: &str) -> std::io::Result<()> {
    Command::new(edge).arg(url).spawn().map(|_| ())
}

fn write_extension(dir: &Path) -> std::io::Result<()> {
    // 目录里可能正被 Edge 监听：原地覆盖文件，不要整体删除再建。
    for (relative, bytes) in EXTENSION_FILES {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;
    }
    Ok(())
}

/// 用 wscript 隐藏窗口启动中继（HKCU Run 里的控制台程序会闪窗）。
fn write_hidden_launcher(state_dir: &Path, port: u16) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let path = state_dir.join("run-hidden.vbs");
    // VBS 不继承调用者的环境；自定义状态目录必须显式写进子进程环境。
    let mut body = String::from("Set shell = CreateObject(\"WScript.Shell\")\r\n");
    if let Some(custom) = std::env::var_os("EDGE_MCP_STATE_DIR") {
        body.push_str(&format!(
            "shell.Environment(\"PROCESS\")(\"EDGE_MCP_STATE_DIR\") = \"{}\"\r\n",
            custom.to_string_lossy().replace('"', "\"\"")
        ));
    }
    body.push_str(&format!(
        "shell.Run \"\"\"{}\"\" run --port {}\", 0, False\r\n",
        exe.display(),
        port
    ));
    fs::create_dir_all(state_dir)?;
    fs::write(&path, body)?;
    Ok(path)
}

/// 登录自启走 Startup 目录里的 `.vbs`（不依赖 reg.exe，受管机器上也能用）。
fn register_autostart(state_dir: &Path) -> std::io::Result<PathBuf> {
    let startup = startup_dir()?;
    fs::create_dir_all(&startup)?;
    let target = startup.join("EMA2.vbs");
    fs::copy(state_dir.join("run-hidden.vbs"), &target)?;
    Ok(target)
}

fn startup_dir() -> std::io::Result<PathBuf> {
    let appdata =
        std::env::var_os("APPDATA").ok_or_else(|| std::io::Error::other("APPDATA is not set"))?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup"))
}

/// 删除自启；同时清理旧版本写过的 `HKCU\...\Run\EMA2`。
fn clear_autostart() -> Vec<PathBuf> {
    let mut removed = Vec::new();
    if let Ok(startup) = startup_dir() {
        let script = startup.join("EMA2.vbs");
        if script.is_file() && fs::remove_file(&script).is_ok() {
            removed.push(script);
        }
    }
    delete_legacy_run_value();
    removed
}

#[cfg(windows)]
fn delete_legacy_run_value() {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_SET_VALUE,
    ) {
        // 不存在时返回 NotFound，忽略即可。
        let _ = key.delete_value("EMA2");
    }
}

#[cfg(not(windows))]
fn delete_legacy_run_value() {}

/// 卸载：自启、客户端配置条目、托管 profile、部署的扩展与配对状态。
fn uninstall(state_dir: &Path, purge_output: bool) {
    if state_dir.components().count() < 3 || state_dir.parent().is_none() {
        eprintln!(
            "ema2 uninstall: refusing to operate on suspicious state dir {}",
            state_dir.display()
        );
        std::process::exit(1);
    }

    for path in clear_autostart() {
        println!("removed   -> {}", path.display());
    }

    for result in clients::unconfigure_all() {
        match &result.error {
            Some(error) => println!("client {}: error: {error}", result.id),
            None => println!("client {}: {}", result.id, result.action),
        }
    }

    let profile = state_dir.join("edge-profile");
    if profile.is_dir() {
        match fs::remove_dir_all(&profile) {
            Ok(()) => println!("removed   -> {}", profile.display()),
            Err(error) => eprintln!("cannot remove {}: {error}", profile.display()),
        }
    }

    let extension = state_dir.join("extension");
    if extension.join("manifest.json").is_file() {
        match fs::remove_dir_all(&extension) {
            Ok(()) => println!("removed   -> {}", extension.display()),
            Err(error) => eprintln!("cannot remove {}: {error}", extension.display()),
        }
    }

    for name in ["state.json", "run-hidden.vbs"] {
        let path = state_dir.join(name);
        if path.is_file() {
            match fs::remove_file(&path) {
                Ok(()) => println!("removed   -> {}", path.display()),
                Err(error) => eprintln!("cannot remove {}: {error}", path.display()),
            }
        }
    }

    if purge_output {
        let output = config::env_output_dir();
        if output.is_dir() && output.starts_with(state_dir) {
            match fs::remove_dir_all(&output) {
                Ok(()) => println!("removed   -> {}", output.display()),
                Err(error) => eprintln!("cannot remove {}: {error}", output.display()),
            }
        }
    }

    if state_dir.is_dir()
        && fs::read_dir(state_dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    {
        let _ = fs::remove_dir(state_dir);
    }

    println!();
    println!("卸载完成。还有三件事只能由你在界面上完成：");
    println!("  - edge://extensions 里移除 EMA2（浏览器不允许程序卸载扩展）");
    println!("  - 重启 AI 客户端，让它忘记旧的 MCP 配置");
    println!("  - 中继若仍在运行，退出后不再自启");
}

fn ensure_relay_running(port: u16, state_dir: &Path) -> Result<(), String> {
    if request(port, "GET", "/pair/status", None).is_ok() {
        println!("relay     -> already running on 127.0.0.1:{port}");
        return Ok(());
    }
    let vbs = state_dir.join("run-hidden.vbs");
    Command::new("wscript")
        .arg("//B")
        .arg(&vbs)
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        if request(port, "GET", "/pair/status", None).is_ok() {
            println!("relay     -> started on 127.0.0.1:{port}");
            return Ok(());
        }
    }
    Err("timed out waiting for /pair/status".into())
}

fn edge_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("EMA2_EDGE_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Some(base) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(base));
    }
    if let Some(base) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(base));
    }
    for base in candidates {
        let path = base
            .join("Microsoft")
            .join("Edge")
            .join("Application")
            .join("msedge.exe");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn launch_edge(edge: &Path, profile_dir: &Path, extension_dir: &Path) -> std::io::Result<()> {
    Command::new(edge)
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(format!("--load-extension={}", extension_dir.display()))
        .arg(format!(
            "--disable-extensions-except={}",
            extension_dir.display()
        ))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("about:blank")
        .spawn()?;
    Ok(())
}

fn pair_command(reset: bool) {
    let path = config::state_file();
    let stored = pair::load(&path);
    if !reset {
        println!("state file: {}", path.display());
        match stored.token {
            Some(_) => {
                println!("paired: yes");
                if let Some(at) = stored.paired_at {
                    println!("paired_at: {at}");
                }
                if let Some(version) = stored.extension_version {
                    println!("extension: {version}");
                }
                if !stored.clients.is_empty() {
                    println!("clients: {}", stored.clients.join(", "));
                }
            }
            None => {
                println!("paired: no (open the extension page from the Edge toolbar and authorize)")
            }
        }
        return;
    }
    if path.exists() {
        match fs::remove_file(&path) {
            Ok(()) => {
                println!("pairing reset: {}", path.display());
                println!(
                    "restart the relay (or sign out/in) and authorize again in the extension page."
                );
            }
            Err(error) => {
                eprintln!("cannot remove {}: {error}", path.display());
                std::process::exit(1);
            }
        }
    } else {
        println!("nothing to reset (no pairing state at {})", path.display());
    }
}
