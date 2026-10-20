//! EMA2 v2.0 —— 单个 exe 同时是 daemon 与短命 CLI。
//!
//! ```text
//! ema2 server           一次性，daemon 化，立即返回
//! ema2 server stop      显式关闭 daemon
//! ema2 status           GET /health + 扩展标签页摘要
//! ema2 init             写 AGENTS.md
//! ema2 <tool> ...       POST /cmd，打印行式结果
//! ```
//!
//! 无 MCP、无 token、无授权、无配置文件；默认端口 7744。

mod cli_args;
mod client;
mod paths;
mod relay;
mod save;
mod server;
mod state;
mod tools;

use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;
use clap::error::ErrorKind;
use serde_json::{Value, json};

use cli_args::Cli;

fn main() {
    // clap 默认把用法错误映射到退出码 2；EMA2 的 2 表示“中继不可用”，
    // 所以这里统一成 1，只有 --help/--version 保持 0。
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            let _ = error.print();
            std::process::exit(code);
        }
    };
    let port = cli.port.unwrap_or(paths::DEFAULT_PORT);
    let code = match cli.command {
        cli_args::Command::Daemon => run_daemon(port),
        cli_args::Command::Server { action } => match action {
            Some(cli_args::ServerAction::Stop) => stop_server(port),
            None => start_server(port),
        },
        cli_args::Command::Status => status(port),
        cli_args::Command::Init => init_agents(),
        cli_args::Command::Tabs(args) => run_built(cli_args::build_tabs(&args), port),
        cli_args::Command::Open {
            url,
            tab,
            replace,
            reuse,
            wait,
            timeout,
        } => run_built(
            cli_args::build_open(&url, tab, replace, &reuse, &wait, timeout),
            port,
        ),
        cli_args::Command::Read {
            tab,
            mode,
            selector,
            no_main,
            budget,
            cursor,
        } => run_built(
            Ok(cli_args::build_read(
                tab, &mode, &selector, no_main, budget, cursor,
            )),
            port,
        ),
        cli_args::Command::Snapshot {
            tab,
            filter,
            near,
            budget,
            cursor,
        } => run_built(
            Ok(cli_args::build_snapshot(
                tab, &filter, &near, budget, cursor,
            )),
            port,
        ),
        cli_args::Command::Act { tokens } => run_built(cli_args::build_act(&tokens), port),
        cli_args::Command::Wait {
            kind,
            value,
            timeout,
            tab,
        } => run_built(cli_args::build_wait(&kind, &value, &timeout, tab), port),
        cli_args::Command::Await { value, tab } => {
            run_built(cli_args::build_await(&value, tab), port)
        }
        cli_args::Command::Js {
            code,
            file,
            tab,
            budget,
        } => run_built(cli_args::build_js(code.as_ref(), &file, tab, budget), port),
        cli_args::Command::Observe {
            kind,
            tab,
            since,
            level,
            filter,
            wait,
            body_filter,
            bodies,
            har,
            budget,
        } => run_built(
            cli_args::build_observe(
                &kind,
                tab,
                since,
                &level,
                &filter,
                &wait,
                &body_filter,
                bodies,
                har,
                budget,
            ),
            port,
        ),
        cli_args::Command::Files(args) => run_built(cli_args::build_files(&args), port),
        cli_args::Command::Shot {
            tab,
            inline,
            annotate,
            format,
            out,
        } => run_built(
            cli_args::build_shot(tab, inline, annotate, &format, out),
            port,
        ),
        cli_args::Command::Batch { tokens } => run_built(cli_args::build_batch(&tokens), port),
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------- daemon

fn run_daemon(port: u16) -> ! {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_io()
        .enable_time()
        .build()
        .expect("failed to build tokio runtime");
    let code = runtime.block_on(server::run(port));
    std::process::exit(code);
}

fn start_server(port: u16) -> i32 {
    if let Some(health) = health(port) {
        if is_ema2(&health) {
            println!(
                "relay=already_running pid={} port={port}",
                json_u64(&health, "pid")
            );
            return 0;
        }
        return port_in_use(port);
    }
    // 端口上有东西但不是 ema2：不自动换端口，避免与扩展 popup 的配置错位。
    if TcpStream::connect_timeout(&loopback(port), Duration::from_millis(300)).is_ok() {
        return port_in_use(port);
    }

    let log_path = paths::log_file(port);
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!(
                "error=relay_start_failed msg=cannot locate current exe: {error} hint=run `ema2 server` from the installed binary"
            );
            return 2;
        }
    };
    let mut command = Process::new(exe);
    command
        .arg("daemon")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null());
    match &log {
        Ok(log) => {
            if let Ok(stdout) = log.try_clone() {
                command.stdout(stdout);
            }
            if let Ok(stderr) = log.try_clone() {
                command.stderr(stderr);
            }
        }
        Err(_) => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    detach(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!(
                "error=relay_start_failed msg=cannot spawn daemon: {error} hint=check file permissions"
            );
            return 2;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Some(health) = health(port) {
            if is_ema2(&health) {
                println!(
                    "relay=running pid={} port={port} log={}",
                    json_u64(&health, "pid"),
                    log_path.display()
                );
                return 0;
            }
            return port_in_use(port);
        }
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!(
                "error=relay_start_failed msg=daemon exited early ({status}) hint={}",
                log_hint(&log_path)
            );
            print_log_tail(&log_path);
            return 2;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "error=relay_start_failed msg=daemon did not answer /health within 12s hint={}",
        log_hint(&log_path)
    );
    print_log_tail(&log_path);
    2
}

fn stop_server(port: u16) -> i32 {
    let Some(health) = health(port).filter(is_ema2) else {
        cleanup_pid_file(port);
        println!("relay=not_running port={port}");
        return 0;
    };
    let pid = health
        .get("pid")
        .and_then(Value::as_u64)
        .map(|pid| pid as u32)
        .or_else(|| read_pid_file(port));

    let _ = client::request(
        port,
        "POST",
        "/shutdown",
        Some("{}"),
        Duration::from_secs(3),
    );
    if wait_until_down(port, Duration::from_secs(6)) {
        cleanup_pid_file(port);
        println!(
            "relay=stopped port={port}{}",
            pid.map(|pid| format!(" pid={pid}")).unwrap_or_default()
        );
        return 0;
    }

    // server 卡死：用 PID 文件兜底，但必须先确认这个 PID 确实还是 ema2。
    if let Some(pid) = pid
        && kill_process(pid)
        && wait_until_down(port, Duration::from_secs(4))
    {
        cleanup_pid_file(port);
        println!("relay=stopped port={port} pid={pid} forced=1");
        return 0;
    }
    eprintln!(
        "error=shutdown_failed msg=cannot stop the daemon on 127.0.0.1:{port} hint=close the daemon process manually or reboot; check {}",
        paths::pid_file(port).display()
    );
    2
}

fn status(port: u16) -> i32 {
    let Some(health) = health(port).filter(is_ema2) else {
        println!("relay=not_running port={port}");
        return 2;
    };
    println!(
        "relay=running pid={} port={} uptime={}",
        json_u64(&health, "pid"),
        port,
        format_uptime(
            health
                .get("uptime_secs")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )
    );
    let extension = health.get("extension").cloned().unwrap_or(Value::Null);
    let connected = extension
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let version = extension
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let last_ping = extension
        .get("last_ping_secs")
        .and_then(Value::as_u64)
        .map(|secs| format!("{secs}s"))
        .unwrap_or_else(|| "never".to_owned());
    if connected {
        println!("extension=connected version={version} last_ping={last_ping}");
    } else {
        println!("extension=disconnected last_ping={last_ping}");
    }

    if connected {
        let (tabs, active) = tabs_summary(port);
        println!("tabs={tabs} active_tab={active}");
    } else {
        println!("tabs=unknown active_tab=unknown");
    }
    0
}

fn tabs_summary(port: u16) -> (String, String) {
    let Ok(result) = send_cmd(
        port,
        "tabs",
        &json!({ "action": "list" }),
        None,
        Duration::from_secs(5),
    ) else {
        return ("unknown".into(), "unknown".into());
    };
    let (text, _) = render_result(&result);
    let mut total = "unknown".to_owned();
    let mut active = "unknown".to_owned();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("total=") {
            total = value.trim().to_owned();
        }
        let mut fields = line.split('\t');
        let id = fields.next();
        let marker = fields.next();
        if marker == Some("*")
            && let Some(id) = id
            && id.chars().all(|ch| ch.is_ascii_digit())
        {
            active = id.to_owned();
        }
    }
    (total, active)
}

fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m{}s", seconds % 60);
    }
    let hours = minutes / 60;
    format!("{hours}h{}m", minutes % 60)
}

// ---------------------------------------------------------------- init

const AGENTS_BEGIN: &str = "<!-- ema2:begin -->";
const AGENTS_END: &str = "<!-- ema2:end -->";
const AGENTS_TEMPLATE: &str = include_str!("../assets/AGENTS.md");

fn init_agents() -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!(
                "error=init_failed msg=cannot read cwd: {error} hint=run ema2 init in a writable directory"
            );
            return 1;
        }
    };
    let path = cwd.join("AGENTS.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let block = AGENTS_TEMPLATE.trim_end();
    let (body, action) = if existing.trim().is_empty() {
        (format!("{block}\n"), "created")
    } else if let Some(start) = existing.find(AGENTS_BEGIN) {
        if let Some(offset) = existing[start..].find(AGENTS_END) {
            let end = start + offset + AGENTS_END.len();
            (
                format!("{}{}{}", &existing[..start], block, &existing[end..]),
                "updated",
            )
        } else {
            (format!("{}\n\n{block}\n", existing.trim_end()), "appended")
        }
    } else {
        (format!("{}\n\n{block}\n", existing.trim_end()), "appended")
    };
    if let Err(error) = fs::write(&path, body) {
        eprintln!(
            "error=init_failed msg=cannot write {}: {error} hint=check directory permissions",
            path.display()
        );
        return 1;
    }
    println!("path={}\naction={action}", path.display());
    0
}

// ---------------------------------------------------------------- tool calls

fn run_built(call: Result<cli_args::ToolCall, String>, port: u16) -> i32 {
    match call {
        Ok(call) => run_tool(call, port),
        Err(message) => {
            eprintln!(
                "error=invalid_argument msg={} hint=see `ema2 <command> --help` or docs/TOOLS.md",
                one_line(&message)
            );
            1
        }
    }
}

fn run_tool(call: cli_args::ToolCall, port: u16) -> i32 {
    let port = call.port.unwrap_or(port);
    let out = call.out.as_ref().map(|path| absolute(path));
    // CLI 侧只做“比 daemon 更宽松”的兜底：具体预算由 tools::timeout_for 决定，
    // 硬编码一个常数会在 batch（600s 上限）和长 js 上先于 daemon 断开。
    let timeout = tools::timeout_for(call.tool, &call.args) + Duration::from_secs(5);
    match send_cmd(port, call.tool, &call.args, out.as_deref(), timeout) {
        Ok(result) => {
            let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
            let (text, images) = render_result(&result);
            if is_error {
                if text.is_empty() {
                    eprintln!(
                        "error=extension_error msg=tool returned isError without text hint=reload the extension"
                    );
                    return 4;
                }
                eprintln!("{text}");
                return exit_for_error_text(&text);
            }
            if !text.is_empty() {
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
            }
            for image in images {
                println!("{}", image);
            }
            0
        }
        Err(error) => {
            eprintln!(
                "error={} msg={} hint={}",
                error.code,
                one_line(&error.msg),
                error.hint
            );
            exit_for_code(&error.code)
        }
    }
}

struct CmdError {
    code: String,
    msg: String,
    hint: String,
}

fn send_cmd(
    port: u16,
    tool: &str,
    args: &Value,
    out: Option<&Path>,
    timeout: Duration,
) -> Result<Value, CmdError> {
    let mut body = json!({ "tool": tool, "args": args });
    if let Some(out) = out {
        body["out"] = json!(out);
    }
    let response = client::request(port, "POST", "/cmd", Some(&body.to_string()), timeout)
        .map_err(|error| CmdError {
            code: "relay_not_running".into(),
            msg: format!("cannot connect to 127.0.0.1:{port}: {error}"),
            hint: "start it with `ema2 server`".into(),
        })?;
    let value = serde_json::from_str::<Value>(&response.body).unwrap_or(Value::Null);
    if response.status == 200 {
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("relay_error")
        .to_owned();
    let msg = value
        .get("msg")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "HTTP {} from relay: {}",
                response.status,
                response.body.chars().take(300).collect::<String>()
            )
        });
    let hint = value
        .get("hint")
        .and_then(Value::as_str)
        .unwrap_or("check `ema2 status`")
        .to_owned();
    Err(CmdError { code, msg, hint })
}

/// 把扩展返回的 content[] 渲染成 CLI 文本；图片转 data URI。
fn render_result(result: &Value) -> (String, Vec<String>) {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        texts.push(text.to_owned());
                    }
                }
                Some("image") => {
                    if let Some(data) = item.get("data").and_then(Value::as_str) {
                        let mime = item
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png");
                        images.push(format!("data:{mime};base64,{data}"));
                    }
                }
                _ => {}
            }
        }
    }
    (texts.join("\n"), images)
}

fn exit_for_error_text(text: &str) -> i32 {
    let code = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("error="))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or("extension_error");
    exit_for_code(code)
}

fn exit_for_code(code: &str) -> i32 {
    if code.starts_with("invalid")
        || code.starts_with("missing")
        || code.starts_with("unknown")
        || code == "too_many_files"
    {
        1
    } else if code == "relay_not_running" || code == "port_in_use" {
        2
    } else if code == "extension_not_connected" {
        3
    } else {
        4
    }
}

// ---------------------------------------------------------------- helpers

fn health(port: u16) -> Option<Value> {
    client::get_json(port, "/health", Duration::from_secs(2))
}

fn is_ema2(health: &Value) -> bool {
    health.get("name").and_then(Value::as_str) == Some("ema2")
}

fn port_in_use(port: u16) -> i32 {
    eprintln!(
        "error=port_in_use msg=127.0.0.1:{port} is already occupied by another service hint=run `ema2 server stop`, or start both sides with another --port"
    );
    2
}

fn loopback(port: u16) -> std::net::SocketAddr {
    ("127.0.0.1", port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .expect("127.0.0.1 resolves")
}

fn json_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn read_pid_file(port: u16) -> Option<u32> {
    fs::read_to_string(paths::pid_file(port))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

fn cleanup_pid_file(port: u16) {
    let _ = fs::remove_file(paths::pid_file(port));
}

fn wait_until_down(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if health(port).filter(is_ema2).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    health(port).filter(is_ema2).is_none()
}

fn log_hint(path: &Path) -> String {
    format!("check {} for the daemon log", path.display())
}

fn print_log_tail(path: &Path) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let tail: Vec<&str> = text.lines().rev().take(8).collect();
    if tail.is_empty() {
        return;
    }
    eprintln!("hint=daemon log tail:");
    for line in tail.into_iter().rev() {
        eprintln!("  {line}");
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(windows)]
fn detach(command: &mut Process) {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS | CREATE_NO_WINDOW：父进程立即返回，不留下控制台窗口。
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn detach(command: &mut Process) {
    use std::os::unix::process::CommandExt;
    // setsid(): 脱离控制终端，终端关闭不会给 daemon 发 SIGHUP。
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) -> bool {
    let pid_text = pid.to_string();
    let listed = Process::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_ascii_lowercase())
        .unwrap_or_default();
    if !listed.contains("ema2") {
        return false;
    }
    Process::new("taskkill")
        .args(["/PID", &pid_text, "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn kill_process(pid: u32) -> bool {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok();
    let is_ema2 = exe
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().contains("ema2"))
        })
        .unwrap_or(false);
    if !is_ema2 {
        return false;
    }
    Process::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
