//! EMA2 v2.2 —— 单个 exe 同时是 daemon 与短命 CLI。
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

mod cli;
mod client;
mod paths;
mod relay;
mod save;
mod server;
mod state;
mod tools;

use std::fs;
use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;
use clap::error::ErrorKind;
use serde_json::{Value, json};

use cli::Cli;

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
        cli::Command::Daemon => run_daemon(port),
        cli::Command::Server { action } => match action {
            Some(cli::ServerAction::Stop) => stop_server(port),
            None => start_server(port),
        },
        cli::Command::Status => status(port),
        cli::Command::Init => init_agents(),
        cli::Command::Tabs(args) => run_built(cli::build_tabs(&args), port),
        cli::Command::Open {
            url,
            urls,
            concurrency,
            tab,
            replace,
            reuse,
            until,
            wait,
            timeout,
            background,
        } => run_built(
            cli::build_open(
                &url,
                &urls,
                concurrency,
                tab,
                replace,
                &reuse,
                &until,
                &wait,
                &timeout,
                background,
            ),
            port,
        ),
        cli::Command::Read {
            fanout,
            tab,
            mode,
            selector,
            no_main,
            budget,
            cursor,
        } => run_built(
            cli::build_read(&fanout, tab, &mode, &selector, no_main, budget, cursor),
            port,
        ),
        cli::Command::Snapshot {
            fanout,
            tab,
            filter,
            near,
            budget,
            cursor,
            values,
        } => run_built(
            cli::build_snapshot(&fanout, tab, &filter, &near, budget, cursor, values),
            port,
        ),
        cli::Command::Act { tokens } => {
            // trailing_var_arg + allow_hyphen_values 会把 `act click e1 --help` 的
            // `--help` 当成位置参数；这里先识别（`--` 之后的除外），避免它变成
            // “unknown flag”。`type e1 --help` 要输入字面量时用 `--` 分隔。
            let mut wants_help = false;
            for token in &tokens {
                if token == "--" {
                    break;
                }
                if token == "--help" || token == "-h" {
                    wants_help = true;
                    break;
                }
            }
            if wants_help {
                print_subcommand_help("act");
                0
            } else {
                run_built(cli::build_act(&tokens), port)
            }
        }
        cli::Command::Wait {
            fanout,
            kind,
            value,
            timeout,
            tab,
        } => run_built(cli::build_wait(&fanout, &kind, &value, &timeout, tab), port),
        cli::Command::Await { value, tab } => run_built(cli::build_await(&value, tab), port),
        cli::Command::Js {
            fanout,
            code,
            file,
            tab,
            budget,
            cursor,
            timeout,
        } => run_built(
            cli::build_js(&fanout, code.as_ref(), &file, tab, budget, cursor, &timeout),
            port,
        ),
        cli::Command::Observe {
            kind,
            tab,
            since,
            level,
            url_filter,
            filter,
            collect,
            wait,
            body_filter,
            no_bodies,
            bodies,
            har,
            headers,
            budget,
        } => run_built(
            cli::build_observe(
                &kind,
                tab,
                since,
                &level,
                &url_filter,
                &filter,
                &collect,
                &wait,
                &body_filter,
                no_bodies,
                bodies,
                har,
                &headers,
                budget,
            ),
            port,
        ),
        cli::Command::Files(args) => run_built(cli::build_files(&args), port),
        cli::Command::Shot {
            tab,
            inline,
            annotate,
            format,
            out,
        } => run_built(cli::build_shot(tab, inline, annotate, &format, out), port),
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
            out_line(format!(
                "relay=already_running pid={} port={port}",
                json_u64(&health, "pid")
            ));
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
                out_line(format!(
                    "relay=running pid={} port={port} log={}",
                    json_u64(&health, "pid"),
                    log_path.display()
                ));
                return 0;
            }
            return port_in_use(port);
        }
        if let Ok(Some(status)) = child.try_wait() {
            terminate_child(&mut child);
            eprintln!(
                "error=relay_start_failed msg=daemon exited early ({status}) hint={}",
                log_hint(&log_path)
            );
            print_log_tail(&log_path);
            return 2;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // 超时后自己 spawn 的 daemon 可能仍在跑：必须先收尸，否则会留下一个
    // 没有 PID 记录、CLI 也不认识的孤儿进程。
    terminate_child(&mut child);
    eprintln!(
        "error=relay_start_failed msg=daemon did not answer /health within 12s hint={}",
        log_hint(&log_path)
    );
    print_log_tail(&log_path);
    2
}

/// 杀掉并回收自己 spawn 的子进程；失败（如已经退出）忽略。
fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn stop_server(port: u16) -> i32 {
    let recorded_pid = read_pid_file(port);
    if !port_alive(port) {
        // 端口关闭不等于进程退出：axum graceful shutdown 会先 drop listener，
        // 再等在途请求（可能正在等扩展）。此时必须继续等 PID 真的消失。
        if let Some(pid) = recorded_pid.filter(|&pid| process_alive(pid)) {
            if wait_until_down(port, Some(pid), Duration::from_secs(6)) {
                cleanup_pid_file(port);
                out_line(format!("relay=stopped port={port} pid={pid}"));
                return 0;
            }
            eprintln!(
                "error=shutdown_failed msg=127.0.0.1:{port} is not listening but pid={pid} is still running hint=wait a moment and retry; {} was kept because the process is alive",
                paths::pid_file(port).display()
            );
            return 2;
        }
        cleanup_pid_file(port);
        out_line(format!("relay=not_running port={port}"));
        return 0;
    }
    let health_value = health(port);
    let answered = health_value.is_some();
    let ema2_health = health_value.filter(is_ema2);
    if ema2_health.is_none() {
        // /health 超时不等于“进程已经退出”：daemon 可能只是卡住了。
        // 先确认端口是否真的在监听，再按 PID 文件兜底。
        if answered {
            eprintln!(
                "error=port_in_use msg=127.0.0.1:{port} is answering, but it is not an ema2 daemon hint=nothing was stopped"
            );
            return 2;
        }
        if let Some(pid) = recorded_pid
            && pid_file_allows_kill(port, pid)
            && kill_process(pid)
            && wait_until_down(port, Some(pid), Duration::from_secs(4))
        {
            cleanup_pid_file(port);
            out_line(format!("relay=stopped port={port} pid={pid} forced=1"));
            return 0;
        }
        eprintln!(
            "error=shutdown_failed msg=127.0.0.1:{port} is still listening but /health did not answer hint=close the daemon process manually; check {} (the port check prevents killing a reused PID)",
            paths::pid_file(port).display()
        );
        return 2;
    }
    let health = ema2_health.expect("checked above");
    let pid = health
        .get("pid")
        .and_then(Value::as_u64)
        .map(|pid| pid as u32)
        .or(recorded_pid);

    let _ = client::request(
        port,
        "POST",
        "/shutdown",
        Some("{}"),
        Duration::from_secs(3),
    );
    if wait_until_down(port, pid, Duration::from_secs(6)) {
        cleanup_pid_file(port);
        out_line(format!(
            "relay=stopped port={port}{}",
            pid.map(|pid| format!(" pid={pid}")).unwrap_or_default()
        ));
        return 0;
    }

    // server 卡死：用 PID 文件兜底，但必须先确认这个 PID 确实还是 ema2，
    // 且 PID 文件记录的端口与目标一致（防 PID 复用误杀其他实例）。
    if let Some(pid) = pid
        && pid_file_allows_kill(port, pid)
        && kill_process(pid)
        && wait_until_down(port, Some(pid), Duration::from_secs(4))
    {
        cleanup_pid_file(port);
        out_line(format!("relay=stopped port={port} pid={pid} forced=1"));
        return 0;
    }
    eprintln!(
        "error=shutdown_failed msg=cannot stop the daemon on 127.0.0.1:{port} hint=close the daemon process manually or reboot; check {} (the port check prevents killing a reused PID)",
        paths::pid_file(port).display()
    );
    2
}

fn status(port: u16) -> i32 {
    let Some(health) = health(port).filter(is_ema2) else {
        out_line(format!("relay=not_running port={port}"));
        return 2;
    };
    out_line(format!(
        "relay=running pid={} port={} uptime={}",
        json_u64(&health, "pid"),
        port,
        format_uptime(
            health
                .get("uptime_secs")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )
    ));
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
        out_line(format!(
            "extension=connected version={version} last_ping={last_ping}"
        ));
    } else {
        out_line(format!("extension=disconnected last_ping={last_ping}"));
        out_line(format!(
            "hint=click the EMA2 extension icon, keep port={port} and click reconnect; if it still fails, reload the extension in edge://extensions"
        ));
    }

    if connected {
        let (tabs, active) = tabs_summary(port);
        out_line(format!("tabs={tabs} active_tab={active}"));
    } else {
        out_line("tabs=unknown active_tab=unknown");
    }
    0
}

fn tabs_summary(port: u16) -> (String, String) {
    let Ok(result) = send_cmd(
        port,
        "tabs",
        &json!({ "action": "list", "budget": 200000 }),
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
            // footer 是 `total=2 shown=2 next=none` 一行，不能把整行当数量。
            total = value
                .split_whitespace()
                .next()
                .unwrap_or("unknown")
                .to_owned();
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

/// 读取现有 AGENTS.md；只有“文件不存在”才等价于空文件。
///
/// `unwrap_or_default()` 会把 EACCES、InvalidData（非 UTF-8）等错误也当成空文件，
/// 然后 `init` 用模板覆盖掉用户原来的内容。这里必须区分“不存在”和“读不了”。
fn read_existing_agents(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

/// 同目录临时文件 + rename，避免写到一半崩溃/断电后留下半个 AGENTS.md。
///
/// 临时文件必须和目标是同一目录，跨卷 rename 不是原子操作；写入后 `sync_all`
/// 再替换。Windows 上 rename 可以覆盖已存在文件（MoveFileEx + REPLACE_EXISTING），
/// 失败时清理临时文件，绝不回退成直接覆盖。
fn write_atomic(path: &Path, body: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("AGENTS.md");
    for attempt in 0..32 {
        let temp = dir.join(format!(
            ".{file_name}.ema2-{}-{attempt}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = file
            .write_all(body.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&temp, path));
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot create a unique temporary file next to AGENTS.md",
    ))
}

/// 返回新文件内容与 action 标记。模板区块外的用户内容原样保留。
fn merge_agents(existing: &str, block: &str) -> (String, &'static str) {
    if existing.trim().is_empty() {
        return (format!("{block}\n"), "created");
    }
    if let Some(start) = existing.find(AGENTS_BEGIN) {
        if let Some(offset) = existing[start..].find(AGENTS_END) {
            let end = start + offset + AGENTS_END.len();
            return (
                format!("{}{}{}", &existing[..start], block, &existing[end..]),
                "updated",
            );
        }
    }
    (format!("{}\n\n{block}\n", existing.trim_end()), "appended")
}

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
    let existing = match read_existing_agents(&path) {
        Ok(existing) => existing,
        Err(error) => {
            eprintln!(
                "error=init_failed msg=cannot read {}: {error} hint=fix the file permissions or encoding; the existing file was not touched",
                path.display()
            );
            return 1;
        }
    };
    let block = AGENTS_TEMPLATE.trim_end();
    let (body, action) = merge_agents(&existing, block);
    if let Err(error) = write_atomic(&path, &body) {
        eprintln!(
            "error=init_failed msg=cannot write {}: {error} hint=check directory permissions; the existing file was not replaced",
            path.display()
        );
        return 1;
    }
    out_line(format!("path={}\naction={action}", path.display()));
    0
}

// ---------------------------------------------------------------- tool calls

fn run_built(call: Result<cli::ToolCall, String>, port: u16) -> i32 {
    match call {
        Ok(call) => run_tool(call, port),
        Err(message) => {
            eprintln!(
                "error=invalid_argument msg={} hint=run `ema2 <command> --help`; for .ema2 scripts run `ema2 act --help`",
                one_line(&message)
            );
            1
        }
    }
}

fn run_tool(call: cli::ToolCall, port: u16) -> i32 {
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
                emit_stdout(&text);
                if !text.ends_with('\n') {
                    emit_stdout("\n");
                }
            }
            for image in images {
                out_line(image);
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
        .map_err(|error| request_error(port, &error))?;
    let value = serde_json::from_str::<Value>(&response.body).unwrap_or(Value::Null);
    if response.status == 200 {
        let result = value
            .get("result")
            .cloned()
            .filter(|value| !value.is_null());
        return result.ok_or_else(|| CmdError {
            code: "relay_error".into(),
            msg: format!(
                "HTTP 200 from relay without a result: {}",
                response.body.chars().take(300).collect::<String>()
            ),
            hint: "check `ema2 status`; reload the extension if this persists".into(),
        });
    }
    if response.status == 413 {
        return Err(CmdError {
            code: "payload_too_large".into(),
            msg: format!(
                "HTTP 413 from relay: {}",
                response.body.chars().take(300).collect::<String>()
            ),
            hint: "reduce the upload size (files upload is limited to 5 MiB of raw bytes), or split the call".into(),
        });
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

/// 区分“连不上 daemon”和“连上了但请求中途失败”。
/// 后者不能被报成 relay_not_running，否则 AI 会去重启一个还在运行的中继。
fn request_error(port: u16, error: &std::io::Error) -> CmdError {
    use std::io::ErrorKind;
    let unreachable = matches!(
        error.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::AddrNotAvailable
            | ErrorKind::NotConnected
            | ErrorKind::NotFound
    );
    if unreachable {
        CmdError {
            code: "relay_not_running".into(),
            msg: format!("cannot connect to 127.0.0.1:{port}: {error}"),
            hint: "start it with `ema2 server`".into(),
        }
    } else {
        CmdError {
            code: "relay_error".into(),
            msg: format!("request to 127.0.0.1:{port} failed: {error}"),
            hint: "check `ema2 status`; the daemon is reachable but the request did not complete"
                .into(),
        }
    }
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
        || code == "too_many_steps"
        || code == "payload_too_large"
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

/// 打印某个子命令的帮助。
///
/// `act` 使用 trailing_var_arg，clap 不会把位置参数之后的 `--help` 当帮助标志；
/// 入口在解析前捕获它，这里复用同一份 clap 定义，避免维护第二份帮助文本。
fn print_subcommand_help(name: &str) {
    use clap::CommandFactory as _;
    let mut command = Cli::command();
    if let Some(subcommand) = command.find_subcommand_mut(name) {
        let _ = subcommand.print_help();
        println!();
    }
}

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
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    lexical_normalize(&joined)
}

/// 清理 `.` / `..`，但不解析符号链接，也不要求目标已存在。
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.is_absolute() {
                    out.push("..");
                }
            }
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// PID 文件内容：`"<pid> <port> <started_at_unix>"`；旧格式只有 pid。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PidRecord {
    pid: u32,
    /// 旧格式没有端口；`None` 表示不做端口校验。
    port: Option<u16>,
}

impl PidRecord {
    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split_whitespace();
        let pid = fields.next()?.parse().ok()?;
        let port = fields.next().and_then(|port| port.parse().ok());
        Some(Self { pid, port })
    }

    /// 这条记录是否还允许对 `(port, pid)` 执行强杀。
    fn allows_kill(&self, port: u16, pid: u32) -> bool {
        self.pid == pid && self.port.is_none_or(|recorded| recorded == port)
    }
}

fn read_pid_record(port: u16) -> Option<PidRecord> {
    let text = fs::read_to_string(paths::pid_file(port)).ok()?;
    PidRecord::parse(&text)
}

fn read_pid_file(port: u16) -> Option<u32> {
    read_pid_record(port).map(|record| record.pid)
}

/// 强杀前校验 PID 文件：PID 必须就是要杀的那个，记录的端口也必须匹配。
///
/// 文件名里已经有端口，但内容是判断“这个 PID 现在还是不是当初那一个”的唯一
/// 依据：PID 被系统复用后，旧文件可能指向另一个 pid/端口的 ema2 实例。
/// 没有 PID 文件时退化为调用方的镜像名检查（保持旧行为）。
fn pid_file_allows_kill(port: u16, pid: u32) -> bool {
    read_pid_record(port).is_none_or(|record| record.allows_kill(port, pid))
}

fn cleanup_pid_file(port: u16) {
    let _ = fs::remove_file(paths::pid_file(port));
}

/// 只有“端口不再监听”且“PID 对应进程确实不存在”才算退出。
///
/// graceful shutdown 先 drop listener，端口会早于进程消失；单独用端口判断会
/// 让 `server stop` 在旧 daemon 仍持有扩展 WS 时误报成功。
fn wait_until_down(port: u16, pid: Option<u32>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let down = !port_alive(port) && pid.is_none_or(|pid| !process_alive(pid));
        if down {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn port_alive(port: u16) -> bool {
    TcpStream::connect_timeout(&loopback(port), Duration::from_millis(300)).is_ok()
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

/// 写入 stdout 并忽略 BrokenPipe（EPIPE）。
///
/// `println!`/`print!` 在管道提前关闭时会 panic（`shot --inline | head -c 20` → 101 +
/// Rust backtrace）。CLI 的正常契约是“输出被消费方截断”不算错误，因此这里手动写
/// 并吞掉错误，让进程以 0 退出。
fn emit_stdout(text: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

fn out_line(text: impl AsRef<str>) {
    emit_stdout(text.as_ref());
    emit_stdout("\n");
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
fn tasklist_image_is_ema2(listed: &str) -> bool {
    // /FO CSV /NH 的一行：`"ema2.exe","1234","Console",...`。
    // 只认第一列镜像名，避免命令行/路径里恰好含 ema2 的进程被误杀。
    listed.lines().any(|line| {
        line.split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .eq_ignore_ascii_case("ema2.exe")
    })
}

/// `tasklist /FI "PID eq N"` 的输出里是否有这个 PID（不关心镜像名）。
#[cfg(windows)]
fn tasklist_lists_pid(listed: &str, pid: u32) -> bool {
    listed.lines().any(|line| {
        let mut fields = line.split(',');
        let image = fields.next().unwrap_or_default().trim().trim_matches('"');
        let id = fields.next().unwrap_or_default().trim().trim_matches('"');
        !image.is_empty() && id == pid.to_string()
    })
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let listed = Process::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    tasklist_lists_pid(&listed, pid)
}

#[cfg(windows)]
fn kill_process(pid: u32) -> bool {
    let pid_text = pid.to_string();
    let listed = Process::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    if !tasklist_image_is_ema2(&listed) {
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
fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // kill(pid, 0) 只做存在性检查：0 = 存活；EPERM 也说明进程存在。
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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

#[cfg(test)]
mod tests {
    use super::{
        AGENTS_BEGIN, AGENTS_END, lexical_normalize, merge_agents, read_existing_agents,
        write_atomic,
    };
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn scratch_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "ema2-init-test-{}-{name}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_treats_only_missing_file_as_empty() {
        let dir = scratch_dir("read");
        let missing = dir.join("missing.md");
        assert_eq!(read_existing_agents(&missing).unwrap(), "");

        let invalid = dir.join("AGENTS.md");
        std::fs::write(&invalid, [0xff, 0xfe, 0xfd]).unwrap();
        let error = read_existing_agents(&invalid).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_replaces_the_block_and_preserves_user_text() {
        let before =
            format!("# My notes\n\nkeep me\n\n{AGENTS_BEGIN}\nold\ntemplate\n{AGENTS_END}\n\ntail");
        let (merged, action) = merge_agents(&before, "new block");
        assert_eq!(action, "updated");
        assert!(merged.starts_with("# My notes\n\nkeep me\n\n"));
        assert!(merged.contains("new block"));
        assert!(!merged.contains("old\ntemplate"));
        assert!(merged.ends_with("\n\ntail"));

        let (created, action) = merge_agents("  \n", "block");
        assert_eq!(action, "created");
        assert_eq!(created, "block\n");

        let (appended, action) = merge_agents("user text", "block");
        assert_eq!(action, "appended");
        assert_eq!(appended, "user text\n\nblock\n");
    }

    #[test]
    fn init_writes_atomically_and_leaves_no_temp_file() {
        let dir = scratch_dir("write");
        let path = dir.join("AGENTS.md");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary file left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn tasklist_guard_matches_only_the_ema2_image() {
        assert!(super::tasklist_image_is_ema2(
            "\"ema2.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\""
        ));
        assert!(super::tasklist_image_is_ema2(
            "\"EMA2.EXE\",\"1234\",\"Console\",\"1\",\"10,000 K\""
        ));
        assert!(!super::tasklist_image_is_ema2(
            "\"notema2.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\""
        ));
        assert!(!super::tasklist_image_is_ema2(
            "\"python.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\""
        ));
    }

    #[cfg(windows)]
    #[test]
    fn tasklist_pid_check_ignores_the_image_name() {
        assert!(super::tasklist_lists_pid(
            "\"ema2.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\"",
            1234
        ));
        assert!(super::tasklist_lists_pid(
            "\"python.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\"",
            1234
        ));
        assert!(!super::tasklist_lists_pid(
            "\"ema2.exe\",\"1234\",\"Console\",\"1\",\"10,000 K\"",
            4321
        ));
        // tasklist 找不到 PID 时的固定提示不能被当成命中。
        assert!(!super::tasklist_lists_pid(
            "INFO: No tasks are running which match the specified criteria.",
            1234
        ));
    }

    #[test]
    fn output_paths_are_normalized_without_touching_disk() {
        assert_eq!(
            lexical_normalize(Path::new("./out/../shot.png")),
            PathBuf::from("shot.png")
        );
        assert_eq!(
            lexical_normalize(Path::new("out/./nested/../shot.png")),
            PathBuf::from("out/shot.png")
        );
    }

    #[test]
    fn pid_file_accepts_new_and_legacy_formats() {
        let record = super::PidRecord::parse("1234 7744 1750000000\n").unwrap();
        assert_eq!(record.pid, 1234);
        assert_eq!(record.port, Some(7744));

        // 旧格式只有 pid：仍然可用，但不做端口校验。
        let record = super::PidRecord::parse("1234\n").unwrap();
        assert_eq!(record.pid, 1234);
        assert_eq!(record.port, None);

        assert!(super::PidRecord::parse("not-a-pid 7744 0").is_none());
        assert!(super::PidRecord::parse("").is_none());
    }

    #[test]
    fn pid_file_port_mismatch_is_detected() {
        let record = super::PidRecord::parse("42 7744 0").unwrap();
        assert!(record.allows_kill(7744, 42));
        assert!(!record.allows_kill(7745, 42), "port must match");
        assert!(!record.allows_kill(7744, 43), "pid must match");

        // 旧格式没有端口：只能校验 PID。
        let legacy = super::PidRecord::parse("42").unwrap();
        assert!(legacy.allows_kill(7744, 42));
        assert!(!legacy.allows_kill(7744, 43));
    }

    #[test]
    fn current_process_is_alive_and_zero_is_not() {
        assert!(super::process_alive(std::process::id()));
        assert!(!super::process_alive(0));
    }

    #[test]
    fn wait_until_down_returns_true_for_a_free_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(super::wait_until_down(
            port,
            None,
            Duration::from_millis(200)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn absolute_output_paths_do_not_keep_dot_segments() {
        assert_eq!(
            lexical_normalize(Path::new(r"C:\work\.\out\..\shot.png")),
            PathBuf::from(r"C:\work\shot.png")
        );
    }
}
