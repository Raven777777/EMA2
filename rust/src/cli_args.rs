//! CLI 参数 → 工具 arguments 的映射。
//!
//! 职责：把 `ema2 act click e12 --wait-for "url=api/save"` 这类行式命令编译成
//! `POST /cmd` 的 JSON；参数错误在这里报（stderr `error=invalid_argument` + exit 1），
//! daemon 侧的 `tools::validate_arguments` 仍然兜底。

use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

/// 解析完成的一次工具调用（CLI 子命令的编译产物）。
pub struct ToolCall {
    pub tool: &'static str,
    pub args: Value,
    /// shot --out / observe --har 的显式落盘路径。
    pub out: Option<PathBuf>,
    /// act/batch 的 trailing tokens 里可能携带 --port；覆盖全局值。
    pub port: Option<u16>,
}

impl ToolCall {
    fn new(tool: &'static str, args: Value) -> Self {
        Self {
            tool,
            args,
            out: None,
            port: None,
        }
    }
}

// ---------------------------------------------------------------- clap 结构

#[derive(Parser)]
#[command(
    name = "ema2",
    version,
    about = "AI-driven control of the user's real Edge browser (daemon + CLI, no MCP)",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// 覆盖默认端口 7744；server 与所有客户端命令须用同一端口
    #[arg(long, global = true)]
    pub port: Option<u16>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 后台启动中继 daemon（一次性，立即返回）
    Server {
        #[command(subcommand)]
        action: Option<ServerAction>,
    },
    /// 中继/扩展连接状态
    Status,
    /// 在当前目录写 AGENTS.md（命令表 + 错误码 + 诊断）
    Init,
    /// 列出 / 当前 / 新建 / 关闭 / 激活标签页
    Tabs(TabsArgs),
    /// 打开 URL，等页面就绪，装网络 hook
    Open {
        url: String,
        /// 导航指定标签（不计入会话）
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 在当前活动标签里导航（不新建）
        #[arg(long)]
        replace: bool,
        /// 复用本会话最近打开的标签（session）
        #[arg(long)]
        reuse: Option<String>,
        /// 等待级别：load|dom|none
        #[arg(long)]
        wait: Option<String>,
        /// 等文档接位的秒数（默认 15，1..60）
        #[arg(long)]
        timeout: Option<f64>,
    },
    /// 读页面为 md|text|html
    Read {
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 输出格式：md|text|html
        #[arg(long)]
        mode: Option<String>,
        /// 聚焦元素（支持 shadow DOM / 同源 iframe）
        #[arg(long)]
        selector: Option<String>,
        /// 关闭正文提取（读整页）
        #[arg(long = "no-main")]
        no_main: bool,
        #[arg(long)]
        budget: Option<u64>,
        #[arg(long)]
        cursor: Option<u64>,
    },
    /// ref 快照：[e12@d1] button "X"
    Snapshot {
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// interactive|full|links|buttons|form
        #[arg(long)]
        filter: Option<String>,
        /// 限定在该 ref 最近的容器内
        #[arg(long)]
        near: Option<String>,
        #[arg(long)]
        budget: Option<u64>,
        #[arg(long)]
        cursor: Option<u64>,
    },
    /// 单步或多步操作：act click e12 / act --steps '...' / act --script run.ema2
    Act {
        /// action [ref] [text] 与 --selector/--target/--key/--delta-y/--submit/--tab/
        /// --wait-for/--steps/--script/--port 混合
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        tokens: Vec<String>,
    },
    /// 等 text / textGone / selector / time / idle
    Wait {
        /// text|textGone|selector|time|idle
        kind: String,
        /// 条件值（文本 / CSS / 时长，如 "5s"）
        value: Option<String>,
        #[arg(long)]
        timeout: Option<String>,
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
    /// 等上一个 act 动作的页面效果完成（导航/页面稳定）
    Await {
        /// 最长等待时长，如 10s；缺省 3s
        #[arg(value_name = "DURATION")]
        value: Option<String>,
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
    /// 页面主世界执行 JS
    Js {
        /// JS 表达式或语句体（与 --file 互斥）
        code: Option<String>,
        #[arg(long = "file")]
        file: Option<PathBuf>,
        #[arg(long = "tab")]
        tab: Option<u64>,
        #[arg(long)]
        budget: Option<u64>,
    },
    /// console 增量 / network（真实状态码+响应体，HAR）
    Observe {
        /// console|network
        kind: String,
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 从上次输出的 next 续读
        #[arg(long)]
        since: Option<u64>,
        /// log|warn|error（console）
        #[arg(long)]
        level: Option<String>,
        /// URL 子串过滤
        #[arg(long)]
        filter: Option<String>,
        /// 等异步请求（≤10s，如 "5s"）
        #[arg(long)]
        wait: Option<String>,
        /// 响应体正则（≤512 字符）
        #[arg(long = "body-filter")]
        body_filter: Option<String>,
        /// 设为 false 关闭响应体预览（--bodies=false）
        #[arg(long, require_equals = true)]
        bodies: Option<bool>,
        /// 导出 HAR 到指定路径
        #[arg(long, value_name = "PATH")]
        har: Option<PathBuf>,
        #[arg(long)]
        budget: Option<u64>,
    },
    /// 上传本地文件；download-dir 固定报错
    Files(FilesArgs),
    /// 截图（落盘 / inline / annotate）
    Shot {
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 回图片内容（base64）而不是落盘
        #[arg(long)]
        inline: bool,
        /// 先快照再在图上画 ref 编号
        #[arg(long)]
        annotate: bool,
        /// png|jpeg
        #[arg(long)]
        format: Option<String>,
        /// 落盘路径（缺省进默认输出目录）
        #[arg(long = "out")]
        out: Option<PathBuf>,
    },
    /// 多标签并发 open/read/snapshot/js/wait
    Batch {
        /// `<tool>` 之后的所有 token：--tab-ids / --urls / --concurrency / --port
        /// 以及目标工具自己的 flags（如 read 的 --mode md）。
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        tokens: Vec<String>,
    },
    /// （内部）daemon 进程本体；由 `ema2 server` detached spawn
    #[command(hide = true)]
    Daemon,
}

#[derive(Subcommand)]
pub enum ServerAction {
    /// 干净关闭 daemon（POST /shutdown，PID 兜底）
    Stop,
}

#[derive(Args)]
pub struct TabsArgs {
    #[command(subcommand)]
    pub cmd: Option<TabsCommand>,
}

#[derive(Subcommand)]
pub enum TabsCommand {
    /// 列出标签页（默认动作）
    List {
        #[arg(long)]
        budget: Option<u64>,
        #[arg(long)]
        cursor: Option<u64>,
    },
    /// 当前活动标签页
    Current,
    /// 新建标签页
    New {
        url: String,
        /// 顺带激活
        #[arg(long)]
        active: bool,
    },
    /// 关闭标签页（位置参数 id ≤8 个，或 --session）
    Close {
        ids: Vec<u64>,
        /// 关掉本会话由 open/tabs new 打开的标签
        #[arg(long)]
        session: bool,
    },
    /// 激活标签页
    Activate { id: u64 },
}

#[derive(Args)]
pub struct FilesArgs {
    #[command(subcommand)]
    pub cmd: FilesCommand,
}

#[derive(Subcommand)]
pub enum FilesCommand {
    /// 上传本地文件（Windows 须 C:/... 绝对路径，≤20 个）
    Upload {
        paths: Vec<String>,
        #[arg(long = "ref")]
        r#ref: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
    /// Edge 不允许扩展改下载目录：固定返回 download_dir_unsupported
    DownloadDir,
}

// ---------------------------------------------------------------- 时长解析

/// "5s" / "500ms" / "5" → 秒（f64）。
pub fn parse_duration_secs(input: &str) -> Result<f64, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err(format!("invalid duration: {input:?}"));
    }
    let (digits, unit) = match text.chars().position(|c| !c.is_ascii_digit() && c != '.') {
        None => (text, ""),
        Some(index) => text.split_at(index),
    };
    let value: f64 = digits
        .parse()
        .map_err(|_| format!("invalid duration: {input:?}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("invalid duration: {input:?}"));
    }
    let seconds = match unit.trim() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => value,
        "ms" | "millis" | "milliseconds" => value / 1000.0,
        "m" | "min" | "minute" | "minutes" => value * 60.0,
        "h" | "hour" | "hours" => value * 3600.0,
        _ => return Err(format!("invalid duration unit in {input:?} (use ms/s/m/h)")),
    };
    Ok(seconds)
}

// ---------------------------------------------------------------- 各命令 → ToolCall

pub fn build_tabs(args: &TabsArgs) -> Result<ToolCall, String> {
    let arguments = match args.cmd.as_ref() {
        None => json!({ "action": "list" }),
        Some(TabsCommand::List { budget, cursor }) => {
            let mut arguments = json!({ "action": "list" });
            set_opt(&mut arguments, "budget", *budget);
            set_opt(&mut arguments, "cursor", *cursor);
            arguments
        }
        Some(TabsCommand::Current) => json!({ "action": "current" }),
        Some(TabsCommand::New { url, active }) => json!({
            "action": "new",
            "url": url,
            "active": *active,
        }),
        Some(TabsCommand::Close { ids, session }) => {
            if *session {
                if !ids.is_empty() {
                    return Err("tabs close: use either ids or --session, not both".into());
                }
                json!({ "action": "close", "scope": "session" })
            } else if ids.is_empty() {
                return Err("tabs close: pass ids or --session".into());
            } else if ids.len() > 8 {
                return Err("tabs close: at most 8 ids".into());
            } else {
                json!({ "action": "close", "tabIds": ids })
            }
        }
        Some(TabsCommand::Activate { id }) => json!({ "action": "activate", "tabId": id }),
    };
    Ok(ToolCall::new("tabs", arguments))
}

pub fn build_open(
    url: &str,
    tab: Option<u64>,
    replace: bool,
    reuse: &Option<String>,
    wait: &Option<String>,
    timeout: Option<f64>,
) -> Result<ToolCall, String> {
    let mut arguments = json!({ "url": url });
    let picks = [tab.is_some(), replace, reuse.is_some()]
        .iter()
        .filter(|flag| **flag)
        .count();
    if picks > 1 {
        return Err("open: use only one of --tab, --replace, --reuse session".into());
    }
    if let Some(id) = tab {
        arguments["tabId"] = json!(id);
    }
    if replace {
        arguments["replace"] = json!(true);
    }
    if let Some(reuse) = reuse {
        if reuse != "session" {
            return Err("open: --reuse only accepts \"session\"".into());
        }
        arguments["reuse"] = json!("session");
    }
    if let Some(wait) = wait {
        if !matches!(wait.as_str(), "load" | "dom" | "none") {
            return Err(format!("open: --wait must be load|dom|none, got {wait:?}"));
        }
        arguments["wait"] = json!(wait);
    }
    if let Some(seconds) = timeout {
        if !(1.0..=60.0).contains(&seconds) {
            return Err(format!(
                "open: --timeout must be 1..60 seconds, got {seconds}"
            ));
        }
        arguments["waitTimeout"] = json!(seconds);
    }
    Ok(ToolCall::new("open", arguments))
}

pub fn build_read(
    tab: Option<u64>,
    mode: &Option<String>,
    selector: &Option<String>,
    no_main: bool,
    budget: Option<u64>,
    cursor: Option<u64>,
) -> ToolCall {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "mode", mode.clone());
    set_opt(&mut arguments, "selector", selector.clone());
    if no_main {
        arguments["main"] = json!(false);
    }
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    ToolCall::new("read", arguments)
}

pub fn build_snapshot(
    tab: Option<u64>,
    filter: &Option<String>,
    near: &Option<String>,
    budget: Option<u64>,
    cursor: Option<u64>,
) -> ToolCall {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "filter", filter.clone());
    set_opt(&mut arguments, "near", near.clone());
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    ToolCall::new("snapshot", arguments)
}

pub fn build_wait(
    kind: &str,
    value: &Option<String>,
    timeout: &Option<String>,
    tab: Option<u64>,
) -> Result<ToolCall, String> {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    match kind {
        "text" | "textGone" | "selector" => {
            let Some(text) = value else {
                return Err(format!("wait {kind}: missing value"));
            };
            arguments[kind] = json!(text);
        }
        "time" => {
            let Some(text) = value else {
                return Err("wait time: missing duration (e.g. wait time 5s)".into());
            };
            let seconds =
                parse_duration_secs(text).map_err(|error| format!("wait time: {error}"))?;
            arguments["time"] = json!(seconds);
        }
        "idle" => {
            arguments["idle"] = json!(true);
        }
        _ => {
            return Err(format!(
                "wait: unknown kind {kind:?} (use text|textGone|selector|time|idle)"
            ));
        }
    }
    if let Some(timeout) = timeout {
        let seconds =
            parse_duration_secs(timeout).map_err(|error| format!("wait --timeout: {error}"))?;
        arguments["timeout"] = json!(seconds);
    }
    Ok(ToolCall::new("wait", arguments))
}

pub fn build_await(value: &Option<String>, tab: Option<u64>) -> Result<ToolCall, String> {
    let seconds = match value {
        Some(value) => parse_duration_secs(value).map_err(|error| format!("await: {error}"))?,
        None => 3.0,
    };
    if !(0.0..=300.0).contains(&seconds) {
        return Err("await: duration must be between 0 and 300 seconds".into());
    }
    let mut arguments = json!({ "await": true, "timeout": seconds });
    set_opt(&mut arguments, "tabId", tab);
    Ok(ToolCall::new("wait", arguments))
}

pub fn build_js(
    code: Option<&String>,
    file: &Option<PathBuf>,
    tab: Option<u64>,
    budget: Option<u64>,
) -> Result<ToolCall, String> {
    if code.is_none() == file.is_none() {
        return Err("js: pass the code inline or use --file, exactly one".into());
    }
    let source = match (code, file) {
        (Some(text), _) => text.clone(),
        _ => std::fs::read_to_string(file.as_ref().unwrap()).map_err(|error| {
            format!(
                "js: cannot read {}: {error}",
                file.as_ref().unwrap().display()
            )
        })?,
    };
    if source.trim().is_empty() {
        return Err("js: code must not be empty".into());
    }
    let mut arguments = json!({ "code": source });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "budget", budget);
    Ok(ToolCall::new("js", arguments))
}

#[allow(clippy::too_many_arguments)]
pub fn build_observe(
    kind: &str,
    tab: Option<u64>,
    since: Option<u64>,
    level: &Option<String>,
    filter: &Option<String>,
    wait: &Option<String>,
    body_filter: &Option<String>,
    bodies: Option<bool>,
    har: Option<PathBuf>,
    budget: Option<u64>,
) -> Result<ToolCall, String> {
    if !matches!(kind, "console" | "network") {
        return Err(format!(
            "observe: kind must be console|network, got {kind:?}"
        ));
    }
    let mut arguments = json!({ "kind": kind });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "since", since);
    set_opt(&mut arguments, "level", level.clone());
    set_opt(&mut arguments, "filter", filter.clone());
    set_opt(&mut arguments, "bodyFilter", body_filter.clone());
    set_opt(&mut arguments, "bodies", bodies);
    set_opt(&mut arguments, "budget", budget);
    if let Some(wait) = wait {
        let seconds =
            parse_duration_secs(wait).map_err(|error| format!("observe --wait: {error}"))?;
        arguments["wait"] = json!(seconds);
    }
    let mut call = ToolCall::new("observe", arguments);
    if let Some(path) = har {
        call.args["har"] = json!(true);
        call.out = Some(path);
    }
    Ok(call)
}

/// 上传载荷上限：daemon 的 /cmd body 上限是 8MB，base64 会膨胀 4/3。
const MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

fn guess_mime(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    }
}

pub fn build_files(args: &FilesArgs) -> Result<ToolCall, String> {
    match &args.cmd {
        FilesCommand::Upload {
            paths,
            r#ref,
            selector,
            target,
            tab,
        } => {
            if paths.is_empty() {
                return Err("files upload: pass at least one path".into());
            }
            if paths.len() > 20 {
                return Err("files upload: at most 20 paths".into());
            }
            let targets = [
                r#ref.is_some() as usize,
                selector.is_some() as usize,
                target.is_some() as usize,
            ]
            .iter()
            .sum::<usize>();
            if targets == 0 {
                return Err("files upload: needs one of --ref, --selector, --target".into());
            }
            if targets > 1 {
                return Err("files upload: use only one of --ref, --selector, --target".into());
            }
            // 把字节一并送给扩展：DataTransfer 路径不需要 debugger，
            // 另一个扩展占住 tab 时 upload 仍然可用。
            let mut payload: Vec<serde_json::Value> = Vec::new();
            let mut total_bytes = 0usize;
            for path in paths {
                let bytes = std::fs::read(path)
                    .map_err(|error| format!("files upload: cannot read {path}: {error}"))?;
                total_bytes += bytes.len();
                if total_bytes > MAX_UPLOAD_BYTES {
                    return Err(format!(
                        "files upload: payload exceeds {} MiB",
                        MAX_UPLOAD_BYTES / (1024 * 1024)
                    ));
                }
                payload.push(json!({
                    "name": std::path::Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone()),
                    "type": guess_mime(path),
                    "data": BASE64.encode(&bytes),
                }));
            }
            let mut arguments = json!({ "action": "upload", "paths": paths, "files": payload });
            if let Some(value) = r#ref {
                arguments["ref"] = json!(value);
            }
            if let Some(value) = selector {
                arguments["selector"] = json!(value);
            }
            if let Some(value) = target {
                arguments["target"] = json!(value);
            }
            set_opt(&mut arguments, "tabId", *tab);
            Ok(ToolCall::new("files", arguments))
        }
        FilesCommand::DownloadDir => {
            Ok(ToolCall::new("files", json!({ "action": "download-dir" })))
        }
    }
}

pub fn build_shot(
    tab: Option<u64>,
    inline: bool,
    annotate: bool,
    format: &Option<String>,
    out: Option<PathBuf>,
) -> Result<ToolCall, String> {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    if inline {
        arguments["inline"] = json!(true);
    }
    if annotate {
        arguments["annotate"] = json!(true);
    }
    if let Some(format) = format {
        if !matches!(format.as_str(), "png" | "jpeg") {
            return Err(format!("shot: --format must be png|jpeg, got {format:?}"));
        }
        arguments["format"] = json!(format);
    }
    let mut call = ToolCall::new("shot", arguments);
    call.out = out;
    Ok(call)
}

/// `ema2 batch` 的全部 token（`<tool>` 之后）都由这里解析：
/// 顶层识别 `--tab-ids/--urls/--concurrency/--port`，其余交给目标工具的
/// `parse_tool_flags`，最终放进 `args`（扩展的 batch 只从 `args` 读内层参数）。
pub fn build_batch(tokens: &[String]) -> Result<ToolCall, String> {
    let Some(tool) = tokens.first().map(String::as_str) else {
        return Err("batch: missing tool (open|read|snapshot|js|wait)".into());
    };
    if !matches!(tool, "open" | "read" | "snapshot" | "js" | "wait") {
        return Err(format!(
            "batch: tool must be open|read|snapshot|js|wait, got {tool:?}"
        ));
    }

    let mut tab_ids: Option<String> = None;
    let mut urls: Option<String> = None;
    let mut concurrency: Option<u32> = None;
    let mut port_override: Option<u16> = None;
    let mut flags: Vec<String> = Vec::new();
    let mut index = 1;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        let (flag, inline) = split_flag(token);
        match flag {
            "--tab-ids" | "--urls" | "--concurrency" | "--port" => {
                let value = take_flag_value(tokens, &mut index, inline, "batch", flag)?;
                match flag {
                    "--tab-ids" => tab_ids = Some(value),
                    "--urls" => urls = Some(value),
                    "--concurrency" => {
                        let parsed: u32 = value
                            .parse()
                            .map_err(|_| format!("batch: invalid --concurrency {value:?}"))?;
                        if !(1..=4).contains(&parsed) {
                            return Err("batch: --concurrency must be 1..4".into());
                        }
                        concurrency = Some(parsed);
                    }
                    _ => {
                        port_override = Some(
                            value
                                .parse::<u16>()
                                .map_err(|_| format!("batch: invalid --port {value:?}"))?,
                        );
                    }
                }
            }
            _ => {
                flags.push(token.to_owned());
                // 带值的普通 flag 原样留给目标工具解析；内联值不需要额外 token。
                if inline.is_none()
                    && matches!(
                        flag,
                        "--mode"
                            | "--selector"
                            | "--budget"
                            | "--cursor"
                            | "--filter"
                            | "--near"
                            | "--code"
                            | "--text"
                            | "--text-gone"
                            | "--time"
                            | "--timeout"
                            | "--wait"
                            | "--reuse"
                    )
                {
                    if let Some(value) = tokens.get(index + 1) {
                        flags.push(value.clone());
                        index += 1;
                    }
                }
                index += 1;
            }
        }
    }

    if tab_ids.is_some() && urls.is_some() {
        return Err("batch: --tab-ids and --urls are mutually exclusive".into());
    }
    if tab_ids.is_none() && urls.is_none() {
        return Err("batch: pass --tab-ids \"1,2\" or --urls \"u1,u2\"".into());
    }
    if tool == "open" && tab_ids.is_some() {
        return Err("batch: tool open only works with --urls".into());
    }

    let inner = parse_tool_flags(tool, &flags)?;
    let mut arguments = json!({ "tool": tool, "args": inner });
    if let Some(ids) = tab_ids {
        let parsed = parse_id_list(&ids)?;
        if parsed.is_empty() || parsed.len() > 8 {
            return Err("batch: --tab-ids must contain 1..8 ids".into());
        }
        arguments["tabIds"] = json!(parsed);
    }
    if let Some(list) = urls {
        let parsed: Vec<&str> = list
            .split(',')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect();
        if parsed.is_empty() || parsed.len() > 8 {
            return Err("batch: --urls must contain 1..8 urls".into());
        }
        arguments["urls"] = json!(parsed);
    }
    if let Some(concurrency) = concurrency {
        arguments["concurrency"] = json!(concurrency);
    }
    let mut call = ToolCall::new("batch", arguments);
    call.port = port_override;
    Ok(call)
}

fn split_flag(token: &str) -> (&str, Option<String>) {
    if let Some((flag, value)) = token.split_once('=')
        && flag.starts_with("--")
    {
        return (flag, Some(value.to_owned()));
    }
    (token, None)
}

fn take_flag_value(
    tokens: &[String],
    index: &mut usize,
    inline: Option<String>,
    context: &str,
    flag: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        *index += 1;
        if value.is_empty() {
            return Err(format!("{context}: {flag} needs a value"));
        }
        return Ok(value);
    }
    let value = tokens
        .get(*index + 1)
        .cloned()
        .ok_or_else(|| format!("{context}: {flag} needs a value"))?;
    *index += 2;
    Ok(value)
}

fn numeric_flag(context: &str, flag: &str, value: String) -> Result<Value, String> {
    let parsed: u64 = value
        .parse()
        .map_err(|_| format!("{context}: {flag} must be a non-negative integer, got {value:?}"))?;
    Ok(json!(parsed))
}

/// batch trailing flags → 目标工具的 args 对象（只接受该工具自己的 flag）。
fn parse_tool_flags(tool: &str, flags: &[String]) -> Result<Value, String> {
    let mut out = json!({});
    let mut index = 0;
    while index < flags.len() {
        let token = flags[index].as_str();
        let (flag, inline) = split_flag(token);
        // 宏而不是闭包：闭包若捕获 &mut index，分支里的 index += 1 会冲突。
        macro_rules! value {
            () => {
                take_flag_value(flags, &mut index, inline.clone(), "batch", flag)?
            };
        }
        match tool {
            "open" => match flag {
                "--wait" => out["wait"] = json!(value!()),
                "--replace" => {
                    out["replace"] = json!(true);
                    index += 1;
                }
                "--reuse" => out["reuse"] = json!(value!()),
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "read" => match flag {
                "--mode" => out["mode"] = json!(value!()),
                "--selector" => out["selector"] = json!(value!()),
                "--budget" => out["budget"] = numeric_flag("batch", flag, value!())?,
                "--cursor" => out["cursor"] = numeric_flag("batch", flag, value!())?,
                "--no-main" => {
                    out["main"] = json!(false);
                    index += 1;
                }
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "snapshot" => match flag {
                "--filter" => out["filter"] = json!(value!()),
                "--near" => out["near"] = json!(value!()),
                "--budget" => out["budget"] = numeric_flag("batch", flag, value!())?,
                "--cursor" => out["cursor"] = numeric_flag("batch", flag, value!())?,
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "js" => match flag {
                "--code" => out["code"] = json!(value!()),
                "--budget" => out["budget"] = numeric_flag("batch", flag, value!())?,
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "wait" => match flag {
                "--text" => out["text"] = json!(value!()),
                "--text-gone" => out["textGone"] = json!(value!()),
                "--selector" => out["selector"] = json!(value!()),
                "--time" | "--timeout" => {
                    let raw = value!();
                    let seconds =
                        parse_duration_secs(&raw).map_err(|error| format!("batch: {error}"))?;
                    let key = if flag == "--time" { "time" } else { "timeout" };
                    out[key] = json!(seconds);
                }
                "--idle" => {
                    out["idle"] = json!(true);
                    index += 1;
                }
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            _ => return Err(format!("batch: unsupported tool {tool:?}")),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- act 解析

const ACT_ACTIONS: &[&str] = &[
    "click", "type", "fill", "select", "press", "hover", "scroll", "submit", "back", "forward",
    "reload",
];

/// 位置参数里的 ref 只能是 `e12` / `e12@d1` / `[e12@d1]`；
/// 否则 `act click foo` 会把拼错的参数当成 ref，报成看不懂的 stale_ref。
fn check_ref_token(token: &str) -> Result<(), String> {
    let text = token.trim().trim_start_matches('[').trim_end_matches(']');
    let body = text.strip_prefix(['e', 'E']).unwrap_or(text);
    let ok = body.split_once('@').map_or_else(
        || body.parse::<u64>().is_ok(),
        |(seq, doc)| seq.parse::<u64>().is_ok() && doc.len() > 1 && doc.starts_with('d'),
    );
    if ok {
        return Ok(());
    }
    Err(format!(
        "act: {token:?} is not a ref (want e12 or e12@d1); use --selector or --target for CSS/role queries"
    ))
}

pub fn build_act(tokens: &[String]) -> Result<ToolCall, String> {
    let mut selector: Option<String> = None;
    let mut target: Option<String> = None;
    let mut key: Option<String> = None;
    let mut delta_y: Option<f64> = None;
    let mut submit_flag = false;
    let mut tab: Option<u64> = None;
    let mut wait_for: Option<String> = None;
    let mut steps_inline: Option<String> = None;
    let mut script_path: Option<String> = None;
    let mut port_override: Option<u16> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        // 值型 flag：取下一个 token；缺失时报错而不是 panic。
        let next_value = |name: &str| -> Result<String, String> {
            tokens
                .get(index + 1)
                .cloned()
                .ok_or_else(|| format!("act: {name} needs a value"))
        };
        match token {
            "--selector" => selector = Some(next_value("--selector")?),
            "--target" => target = Some(next_value("--target")?),
            "--key" => key = Some(next_value("--key")?),
            "--delta-y" => {
                let value = next_value("--delta-y")?;
                delta_y = Some(
                    value
                        .parse()
                        .map_err(|_| format!("act: invalid --delta-y {value:?}"))?,
                );
            }
            "--tab" => {
                let value = next_value("--tab")?;
                tab = Some(
                    value
                        .parse()
                        .map_err(|_| format!("act: invalid --tab {value:?}"))?,
                );
            }
            "--port" => {
                let value = next_value("--port")?;
                port_override = Some(
                    value
                        .parse()
                        .map_err(|_| format!("act: invalid --port {value:?}"))?,
                );
            }
            "--wait-for" => wait_for = Some(next_value("--wait-for")?),
            "--steps" => steps_inline = Some(next_value("--steps")?),
            "--script" => script_path = Some(next_value("--script")?),
            "--submit" => submit_flag = true,
            other if other.starts_with('-') => {
                // trailing_var_arg + allow_hyphen_values 会把拼错的 flag 变成位置参数；
                // 这里直接拒绝，别让它悄悄变成一个 ref。
                return Err(format!(
                    "act: unknown flag {other:?} (known: --selector --target --key --delta-y --submit                      --tab --wait-for --steps --script --port)"
                ));
            }
            other => {
                positional.push(other.to_owned());
                index += 1;
                continue;
            }
        }
        index += 2;
    }

    if let Some(path) = &script_path {
        if steps_inline.is_some() {
            return Err("act: use either --steps or --script, not both".into());
        }
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("act: cannot read script {path}: {error}"))?;
        steps_inline = Some(source);
    }

    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);

    if let Some(source) = steps_inline {
        // 多步模式：位置参数不该再出现（避免两套意图混用）。
        if !positional.is_empty() {
            return Err(format!(
                "act: --steps/--script cannot be combined with single-step arguments: {:?}",
                positional
            ));
        }
        let parsed = parse_act_script(&source)?;
        let mut steps = parsed["steps"].as_array().cloned().unwrap_or_default();
        if let Some(wait_for) = &wait_for {
            // 脚本里已经有 waitFor 时不再追加第二个；没有则挂在整次调用末尾。
            if steps
                .iter()
                .any(|step| step.get("action").and_then(Value::as_str) == Some("waitFor"))
            {
                return Err(
                    "act: only one waitFor per call (--wait-for or a script waitFor line)".into(),
                );
            }
            let mut step = parse_wait_for(wait_for)?;
            step["action"] = json!("waitFor");
            steps.push(step);
        }
        if steps.is_empty() || steps.len() > 20 {
            return Err("act: script must contain 1..20 steps".into());
        }
        arguments["steps"] = json!(steps);
    } else {
        let action = positional
            .first()
            .ok_or_else(|| "act: missing action (click|type|fill|select|press|hover|scroll|submit|back|forward|reload)".to_owned())?;
        if !ACT_ACTIONS.contains(&action.as_str()) {
            return Err(format!("act: unknown action {action:?}"));
        }
        if submit_flag && action != "type" {
            return Err(
                "act: --submit is only supported by type; use action=submit or click the submit button"
                    .into(),
            );
        }
        let rest: Vec<String> = positional[1..].to_vec();
        if let Some(wait_for) = &wait_for {
            arguments["waitFor"] = parse_wait_for(wait_for)?;
        }
        arguments["action"] = json!(action);
        set_opt(&mut arguments, "selector", selector.clone());
        set_opt(&mut arguments, "target", target.clone());
        if let Some(delta) = delta_y {
            arguments["deltaY"] = json!(delta);
        }
        if submit_flag {
            arguments["submit"] = json!(true);
        }
        if let Some(key) = &key {
            arguments["key"] = json!(key);
        }
        match action.as_str() {
            "click" | "hover" | "submit" => {
                if rest.len() > 1 {
                    return Err(format!(
                        "act {action}: unexpected extra argument {:?}; quote values with spaces",
                        rest[1]
                    ));
                }
                if let Some(r#ref) = rest.first() {
                    check_ref_token(r#ref)?;
                    arguments["ref"] = json!(r#ref);
                }
            }
            "type" | "fill" | "select" => {
                // 位置参数有两种形态：`[ref] <text>` 或（有 --selector/--target 时）`<text>`。
                let located = selector.is_some() || target.is_some();
                let (text, positional_ref) = if located {
                    if rest.len() != 1 {
                        return Err(format!(
                            "act {action}: expected <text> with --selector/--target (quote values with spaces)"
                        ));
                    }
                    (rest[0].clone(), None)
                } else {
                    if rest.len() != 2 {
                        return Err(format!(
                            "act {action}: expected [ref] <text> (quote values with spaces)"
                        ));
                    }
                    (rest[1].clone(), Some(rest[0].clone()))
                };
                if action == "fill" {
                    // 扩展的 fill 走 fields[]；CLI 保持 `fill e15 "hello"` 的行式形态。
                    let mut field = json!({ "value": text });
                    if let Some(r#ref) = positional_ref {
                        check_ref_token(&r#ref)?;
                        field["ref"] = json!(r#ref);
                    }
                    if let Some(selector) = arguments.get("selector").cloned() {
                        field["selector"] = selector;
                    }
                    if let Some(target) = arguments.get("target").cloned() {
                        field["target"] = target;
                    }
                    arguments["fields"] = json!([field]);
                    // fields 已携带定位信息，顶层不再重复。
                    if arguments["fields"][0].get("selector").is_some()
                        || arguments["fields"][0].get("target").is_some()
                    {
                        arguments.as_object_mut().unwrap().remove("selector");
                        arguments.as_object_mut().unwrap().remove("target");
                    }
                } else if action == "select" {
                    arguments["value"] = json!(text);
                    if let Some(r#ref) = positional_ref {
                        check_ref_token(&r#ref)?;
                        arguments["ref"] = json!(r#ref);
                    }
                } else {
                    arguments["text"] = json!(text);
                    if let Some(r#ref) = positional_ref {
                        check_ref_token(&r#ref)?;
                        arguments["ref"] = json!(r#ref);
                    }
                }
            }
            "press" => {
                // `press [ref] <key>` 或 `press [ref] --key <key>` 或 `press <key>`。
                if rest.len() > 2 || (key.is_some() && rest.len() > 1) {
                    return Err(
                        "act press: too many arguments (use `press [ref] <key>` or `press [ref] --key <key>`)"
                            .into(),
                    );
                }
                let positional_ref = if rest.len() >= 2 || (key.is_some() && rest.len() == 1) {
                    Some(rest[0].clone())
                } else {
                    None
                };
                let press_key = key
                    .clone()
                    .or_else(|| rest.get(1).cloned())
                    .or_else(|| rest.first().cloned())
                    .ok_or_else(|| "act press: missing key (act press [ref] <key>)".to_owned())?;
                arguments["key"] = json!(press_key);
                if let Some(r#ref) = positional_ref {
                    check_ref_token(&r#ref)?;
                    arguments["ref"] = json!(r#ref);
                }
            }
            "scroll" => {
                // scroll down 3 → text="down 3"（扩展按屏数滚动）；--delta-y 走像素。
                if rest.len() > 2 {
                    return Err(
                        "act scroll: too many arguments (use `scroll [down|up] [count]` or --delta-y)"
                            .into(),
                    );
                }
                let mut direction = rest.first().cloned();
                if let Some(dir) = &direction {
                    if dir != "down" && dir != "up" {
                        return Err(format!(
                            "act scroll: direction must be down|up, got {dir:?}"
                        ));
                    }
                } else if delta_y.is_none() {
                    direction = Some("down".to_owned());
                }
                if let Some(dir) = direction {
                    let count = rest.get(1).cloned().unwrap_or_else(|| "1".to_owned());
                    let count: f64 = count
                        .parse()
                        .map_err(|_| format!("act scroll: invalid count {:?}", rest.get(1)))?;
                    if count < 1.0 || count.fract() != 0.0 {
                        return Err("act scroll: count must be a positive integer".into());
                    }
                    arguments["text"] = json!(format!("{dir} {}", count as u64));
                }
            }
            "back" | "forward" | "reload" => {
                if !rest.is_empty() {
                    return Err(format!("act {action}: takes no arguments"));
                }
            }
            _ => unreachable!("validated above"),
        }
    }

    let mut call = ToolCall::new("act", arguments);
    call.port = port_override;
    Ok(call)
}

/// `--wait-for "url=api/save status=200 timeout=5s"` → waitFor 对象。
fn parse_wait_for(input: &str) -> Result<Value, String> {
    let tokens = tokenize_line(input)?;
    let mut out = json!({});
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!("act waitFor: expected key=value, got {token:?}"));
        };
        match key {
            "url" => out["url"] = json!(value),
            "status" => {
                let status: u16 = value
                    .parse()
                    .map_err(|_| format!("act waitFor: invalid status {value:?}"))?;
                out["status"] = json!(status);
            }
            "timeout" => {
                let seconds =
                    parse_duration_secs(value).map_err(|error| format!("act waitFor: {error}"))?;
                out["timeout"] = json!(seconds);
            }
            _ => {
                return Err(format!(
                    "act waitFor: unknown key {key:?} (url|status|timeout)"
                ));
            }
        }
    }
    if out.get("url").is_none() && out.get("status").is_none() {
        return Err("act waitFor: needs at least url= or status=".into());
    }
    Ok(out)
}

/// `.ema2` 脚本 / `--steps` 文本 → steps[] 数组。
///
/// 语法：每行一条；空行与 `#` 注释忽略；含空格的值加双引号（支持 `\"` 转义）；
/// `waitFor` 是独立步骤，保持它在脚本中的位置（不再挪到整次调用末尾）。
pub fn parse_act_script(source: &str) -> Result<Value, String> {
    let mut steps = Vec::new();
    for (number, raw) in source.lines().enumerate() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        for line in split_statements(raw) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let tokens = tokenize_line(line)
                .map_err(|error| format!("script line {}: {error}", number + 1))?;
            let action = tokens
                .first()
                .ok_or_else(|| format!("script line {}: empty statement", number + 1))?;
            if action == "waitFor" {
                let mut parsed = parse_wait_for(&tokens[1..].join(" "))
                    .map_err(|error| format!("script line {}: {error}", number + 1))?;
                parsed["action"] = json!("waitFor");
                steps.push(parsed);
                continue;
            }
            if action == "await" {
                if tokens.len() != 2 {
                    return Err(format!(
                        "script line {}: await needs exactly one duration, e.g. await 10s",
                        number + 1
                    ));
                }
                let raw = tokens[1]
                    .strip_prefix("timeout=")
                    .unwrap_or(tokens[1].as_str());
                let seconds = parse_duration_secs(raw)
                    .map_err(|error| format!("script line {}: {error}", number + 1))?;
                if !(0.0..=300.0).contains(&seconds) {
                    return Err(format!(
                        "script line {}: await duration must be between 0 and 300 seconds",
                        number + 1
                    ));
                }
                steps.push(json!({ "action": "await", "timeout": seconds }));
                continue;
            }
            if !ACT_ACTIONS.contains(&action.as_str()) {
                return Err(format!(
                    "script line {}: unknown action {action:?}",
                    number + 1
                ));
            }
            let mut step = json!({ "action": action });
            let mut rest: Vec<String> = tokens[1..].to_vec();
            // 只从位置参数开头摘 selector=/target=；文本值里的 `selector=` 仍然算文本。
            while let Some(first) = rest.first() {
                if let Some(value) = first.strip_prefix("selector=") {
                    step["selector"] = json!(value);
                    rest.remove(0);
                    continue;
                }
                if let Some(value) = first.strip_prefix("target=") {
                    step["target"] = json!(value);
                    rest.remove(0);
                    continue;
                }
                break;
            }
            let script_error = |message: String| format!("script line {}: {message}", number + 1);
            match action.as_str() {
                "click" | "hover" | "submit" => {
                    if rest.len() > 1 {
                        return Err(script_error(format!(
                            "{action}: unexpected extra argument {:?}; quote values with spaces",
                            rest[1]
                        )));
                    }
                    if let Some(r#ref) = rest.first() {
                        check_ref_token(r#ref).map_err(script_error)?;
                        step["ref"] = json!(r#ref);
                    }
                }
                "type" | "fill" | "select" => {
                    let located = step.get("selector").is_some() || step.get("target").is_some();
                    let (text, positional_ref) = if located {
                        if rest.len() != 1 {
                            return Err(script_error(format!(
                                "{action}: expected <text> with selector=/target= (quote values with spaces)"
                            )));
                        }
                        (rest[0].clone(), None)
                    } else {
                        if rest.len() != 2 {
                            return Err(script_error(format!(
                                "{action}: expected [ref] <text> (quote values with spaces)"
                            )));
                        }
                        (rest[1].clone(), Some(rest[0].clone()))
                    };
                    if let Some(r#ref) = &positional_ref {
                        check_ref_token(r#ref).map_err(script_error)?;
                    }
                    if action == "fill" {
                        let mut field = json!({ "value": text });
                        if let Some(r#ref) = positional_ref {
                            field["ref"] = json!(r#ref);
                        }
                        if let Some(selector) = step.get("selector").cloned() {
                            field["selector"] = selector;
                        }
                        if let Some(target) = step.get("target").cloned() {
                            field["target"] = target;
                        }
                        step["fields"] = json!([field]);
                        if step["fields"][0].get("selector").is_some()
                            || step["fields"][0].get("target").is_some()
                        {
                            step.as_object_mut().unwrap().remove("selector");
                            step.as_object_mut().unwrap().remove("target");
                        }
                    } else if action == "select" {
                        step["value"] = json!(text);
                        if let Some(r#ref) = positional_ref {
                            step["ref"] = json!(r#ref);
                        }
                    } else {
                        step["text"] = json!(text);
                        if let Some(r#ref) = positional_ref {
                            step["ref"] = json!(r#ref);
                        }
                    }
                }
                "press" => {
                    if rest.len() > 2 {
                        return Err(script_error("press: too many arguments".into()));
                    }
                    let key = rest
                        .get(1)
                        .or_else(|| rest.first())
                        .cloned()
                        .ok_or_else(|| script_error("press needs a key".into()))?;
                    step["key"] = json!(key);
                    if rest.len() >= 2 {
                        check_ref_token(&rest[0]).map_err(script_error)?;
                        step["ref"] = json!(rest[0]);
                    }
                }
                "scroll" => {
                    if rest.len() > 2 {
                        return Err(script_error("scroll: too many arguments".into()));
                    }
                    let direction = rest.first().cloned().unwrap_or_else(|| "down".to_owned());
                    if direction != "down" && direction != "up" {
                        return Err(script_error("scroll direction must be down|up".into()));
                    }
                    let count = rest.get(1).cloned().unwrap_or_else(|| "1".to_owned());
                    let count: f64 = count
                        .parse()
                        .map_err(|_| script_error("invalid scroll count".into()))?;
                    if count < 1.0 || count.fract() != 0.0 {
                        return Err(script_error(
                            "scroll count must be a positive integer".into(),
                        ));
                    }
                    step["text"] = json!(format!("{direction} {}", count as u64));
                }
                "back" | "forward" | "reload" => {
                    if !rest.is_empty() {
                        return Err(script_error(format!("{action}: takes no arguments")));
                    }
                }
                _ => unreachable!("validated above"),
            }
            steps.push(step);
        }
    }
    if steps.is_empty() {
        return Err("script: no steps found (only comments/blank lines?)".into());
    }
    Ok(json!({ "steps": steps }))
}

/// 按分号拆语句（引号内的分号不算），让 `--steps 'click e1; type e2 hi'`
/// 与 `.ema2` 脚本的逐行写法等价。
fn split_statements(line: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ';' if !in_quotes => {
                statements.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    statements.push(current);
    statements
}

/// 行式 tokenizer：空格分隔，双引号包裹的值保留空格，`\"` 转义引号。
fn tokenize_line(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => in_quotes = false,
            '"' => {
                in_quotes = true;
                has_token = true;
            }
            '\\' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_quotes {
        return Err("unterminated quote".into());
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

// ---------------------------------------------------------------- helpers

fn set_opt<T: Into<Value>>(object: &mut Value, key: &str, value: Option<T>) {
    if let Some(value) = value {
        object[key] = value.into();
    }
}

fn parse_id_list(input: &str) -> Result<Vec<u64>, String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.parse::<u64>()
                .map_err(|_| format!("invalid tab id {text:?} in --tab-ids"))
        })
        .collect()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(call: &ToolCall) -> &Value {
        &call.args
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration_secs("5s").unwrap(), 5.0);
        assert_eq!(parse_duration_secs("500ms").unwrap(), 0.5);
        assert_eq!(parse_duration_secs("5").unwrap(), 5.0);
        assert_eq!(parse_duration_secs("1m").unwrap(), 60.0);
        assert_eq!(parse_duration_secs("1.5s").unwrap(), 1.5);
        assert!(parse_duration_secs("abc").is_err());
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("5x").is_err());
    }

    #[test]
    fn act_single_step_forms() {
        let call = build_act(&["click".into(), "e12".into()]).unwrap();
        assert_eq!(args_of(&call)["action"], "click");
        assert_eq!(args_of(&call)["ref"], "e12");

        let call = build_act(&["type".into(), "e15".into(), "hello world".into()]).unwrap();
        assert_eq!(args_of(&call)["action"], "type");
        assert_eq!(args_of(&call)["ref"], "e15");
        assert_eq!(args_of(&call)["text"], "hello world");

        let call = build_act(&["click".into(), "--selector".into(), "#submit".into()]).unwrap();
        assert_eq!(args_of(&call)["selector"], "#submit");
        assert!(args_of(&call).get("ref").is_none());

        let call = build_act(&["press".into(), "Enter".into()]).unwrap();
        assert_eq!(args_of(&call)["key"], "Enter");
        assert!(args_of(&call).get("ref").is_none());

        let call = build_act(&["press".into(), "e15".into(), "Tab".into()]).unwrap();
        assert_eq!(args_of(&call)["key"], "Tab");
        assert_eq!(args_of(&call)["ref"], "e15");

        let call = build_act(&["scroll".into(), "down".into(), "3".into()]).unwrap();
        assert_eq!(args_of(&call)["text"], "down 3");

        let call = build_act(&["scroll".into(), "up".into()]).unwrap();
        assert_eq!(args_of(&call)["text"], "up 1");

        let call = build_act(&["back".into()]).unwrap();
        assert_eq!(args_of(&call)["action"], "back");

        // fill 单步 → fields[]（扩展只认 fields）。
        let call = build_act(&["fill".into(), "e15".into(), "user".into()]).unwrap();
        assert_eq!(args_of(&call)["fields"][0]["ref"], "e15");
        assert_eq!(args_of(&call)["fields"][0]["value"], "user");
        assert!(args_of(&call).get("text").is_none());

        // fill 用 selector 定位。
        let call = build_act(&[
            "fill".into(),
            "--selector".into(),
            "#user".into(),
            "bob".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["fields"][0]["selector"], "#user");
        assert_eq!(args_of(&call)["fields"][0]["value"], "bob");
        assert!(args_of(&call).get("selector").is_none());

        assert!(build_act(&["explode".into()]).is_err());
        assert!(build_act(&["type".into(), "e15".into()]).is_err());
        assert!(build_act(&["click".into(), "e12".into(), "extra".into()]).is_err());
        assert!(build_act(&["type".into(), "e15".into(), "hello".into(), "world".into()]).is_err());
        assert!(build_act(&["back".into(), "extra".into()]).is_err());
        assert!(build_act(&["click".into(), "e1".into(), "--submit".into()]).is_err());
    }

    #[test]
    fn act_wait_for_flag() {
        let call = build_act(&[
            "click".into(),
            "e12".into(),
            "--wait-for".into(),
            "url=api/save status=200 timeout=5s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["waitFor"]["url"], "api/save");
        assert_eq!(args_of(&call)["waitFor"]["status"], 200);
        assert_eq!(args_of(&call)["waitFor"]["timeout"], 5.0);

        assert!(
            build_act(&[
                "click".into(),
                "e1".into(),
                "--wait-for".into(),
                "bogus".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn act_script_parsing() {
        let script = "\
# 登录流程
open handled elsewhere
type e15 \"hello world\"
fill e16 pass
select e20 zh-CN
press e15 Tab
scroll down 3
click e30
waitFor url=api/save status=200 timeout=5s
";
        let script = script.replace("open handled elsewhere\n", "");
        let parsed = parse_act_script(&script).unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[0]["action"], "type");
        assert_eq!(steps[0]["ref"], "e15");
        assert_eq!(steps[0]["text"], "hello world");
        assert_eq!(steps[1]["fields"][0]["ref"], "e16");
        assert_eq!(steps[1]["fields"][0]["value"], "pass");
        assert_eq!(steps[2]["action"], "select");
        assert_eq!(steps[2]["value"], "zh-CN");
        assert_eq!(steps[3]["key"], "Tab");
        assert_eq!(steps[3]["ref"], "e15");
        assert_eq!(steps[4]["text"], "down 3");
        assert_eq!(steps[5]["action"], "click");
        assert_eq!(steps[6]["action"], "waitFor");
        assert_eq!(steps[6]["url"], "api/save");
        assert_eq!(steps[6]["status"], 200);
        assert_eq!(steps[6]["timeout"], 5.0);
    }

    #[test]
    fn act_script_selector_target_and_comments() {
        let script = "\
click selector=\"#submit\"
click target=\"button:提交\"
# 注释行忽略
";
        let parsed = parse_act_script(script).unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps[0]["selector"], "#submit");
        assert_eq!(steps[1]["target"], "button:提交");
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn act_script_wait_for_keeps_its_line_position() {
        let parsed =
            parse_act_script("click e1\nwaitFor url=api/first timeout=3s\nclick e2\n").unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1]["action"], "waitFor");
        assert_eq!(steps[1]["url"], "api/first");
        assert_eq!(steps[2]["action"], "click");

        // --steps 没有 waitFor 时，--wait-for 追加为最后一步。
        let call = build_act(&[
            "--steps".into(),
            "click e1".into(),
            "--wait-for".into(),
            "url=api/save".into(),
        ])
        .unwrap();
        let steps = call.args["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1]["action"], "waitFor");
    }

    #[test]
    fn act_script_keeps_selector_like_text_and_rejects_extra_tokens() {
        let parsed = parse_act_script("type e15 \"selector=not-a-param\"").unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps[0]["text"], "selector=not-a-param");
        assert!(parse_act_script("click e1 extra").is_err());
        assert!(parse_act_script("type e15 hello world").is_err());
        assert!(parse_act_script("back extra").is_err());
        assert!(parse_act_script("click not-a-ref").is_err());
    }

    #[test]
    fn act_steps_inline_compiles_to_step_array() {
        let call = build_act(&["--steps".into(), "click e12; type e15 hi".into()]).unwrap();
        let steps = call.args["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["action"], "click");
        assert_eq!(steps[0]["ref"], "e12");
        assert_eq!(steps[1]["action"], "type");
        assert_eq!(steps[1]["ref"], "e15");
        assert_eq!(steps[1]["text"], "hi");

        // 引号里的分号不能被当作语句分隔符。
        let call = build_act(&["--steps".into(), "type e12 \"a;b\"".into()]).unwrap();
        let steps = call.args["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["text"], "a;b");
    }

    #[test]
    fn tokenize_quotes_and_escapes() {
        assert_eq!(
            tokenize_line(r#"type e15 "hello \"world\"""#).unwrap(),
            vec!["type", "e15", "hello \"world\""]
        );
        assert_eq!(
            tokenize_line(r#"click target="button:提交""#).unwrap(),
            vec!["click", "target=button:提交"]
        );
        assert!(tokenize_line(r#"type e15 "unterminated"#).is_err());
    }

    #[test]
    fn wait_forms() {
        let call = build_wait("text", &Some("Done".into()), &Some("30s".into()), None).unwrap();
        assert_eq!(args_of(&call)["text"], "Done");
        assert_eq!(args_of(&call)["timeout"], 30.0);

        let call = build_wait("time", &Some("5s".into()), &None, Some(7)).unwrap();
        assert_eq!(args_of(&call)["time"], 5.0);
        assert_eq!(args_of(&call)["tabId"], 7);

        let call = build_wait("idle", &None, &None, None).unwrap();
        assert_eq!(args_of(&call)["idle"], true);

        let call = build_wait("textGone", &Some("Loading".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["textGone"], "Loading");

        assert!(build_wait("bogus", &None, &None, None).is_err());
        assert!(build_wait("text", &None, &None, None).is_err());
    }

    #[test]
    fn await_forms() {
        let call = build_await(&Some("10s".into()), Some(7)).unwrap();
        assert_eq!(call.tool, "wait");
        assert_eq!(args_of(&call)["await"], true);
        assert_eq!(args_of(&call)["timeout"], 10.0);
        assert_eq!(args_of(&call)["tabId"], 7);

        let call = build_await(&None, None).unwrap();
        assert_eq!(args_of(&call)["timeout"], 3.0);
        assert!(build_await(&Some("301s".into()), None).is_err());
        assert!(build_await(&Some("abc".into()), None).is_err());
    }

    #[test]
    fn act_script_await() {
        let parsed = parse_act_script("click e1\nawait 10s\nclick e2\n").unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1]["action"], "await");
        assert_eq!(steps[1]["timeout"], 10.0);

        let parsed = parse_act_script("await timeout=500ms").unwrap();
        assert_eq!(parsed["steps"][0]["timeout"], 0.5);
        assert!(parse_act_script("await").is_err());
        assert!(parse_act_script("await 10s extra").is_err());
        assert!(parse_act_script("await 301s").is_err());
    }

    #[test]
    fn batch_forms() {
        let call = build_batch(&[
            "read".into(),
            "--urls".into(),
            "https://a.com/,https://b.com/".into(),
            "--mode".into(),
            "md".into(),
            "--budget".into(),
            "8000".into(),
        ])
        .unwrap();
        assert_eq!(
            args_of(&call)["urls"],
            json!(["https://a.com/", "https://b.com/"])
        );
        assert_eq!(args_of(&call)["args"]["mode"], "md");
        assert_eq!(args_of(&call)["args"]["budget"], 8000);
        assert!(
            args_of(&call).get("mode").is_none(),
            "inner flags must live under args"
        );

        let call = build_batch(&[
            "wait".into(),
            "--tab-ids".into(),
            "1,2".into(),
            "--concurrency".into(),
            "2".into(),
            "--time".into(),
            "40".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["tabIds"], json!([1, 2]));
        assert_eq!(args_of(&call)["args"]["time"], 40.0);
        assert_eq!(args_of(&call)["concurrency"], 2);

        // 内联值 / 布尔 flag / 数字参数都要能解析。
        let call = build_batch(&[
            "snapshot".into(),
            "--tab-ids=3".into(),
            "--filter=form".into(),
            "--budget=2000".into(),
            "--cursor".into(),
            "0".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["tabIds"], json!([3]));
        assert_eq!(args_of(&call)["args"]["filter"], "form");
        assert_eq!(args_of(&call)["args"]["budget"], 2000);
        assert_eq!(args_of(&call)["args"]["cursor"], 0);

        let call = build_batch(&[
            "read".into(),
            "--tab-ids".into(),
            "1".into(),
            "--no-main".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["args"]["main"], false);

        let call = build_batch(&[
            "open".into(),
            "--urls".into(),
            "https://a.example/,https://b.example/".into(),
            "--wait".into(),
            "dom".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["args"]["wait"], "dom");

        assert!(build_batch(&["act".into(), "--tab-ids".into(), "1".into()]).is_err());
        assert!(
            build_batch(&[
                "read".into(),
                "--tab-ids".into(),
                "1".into(),
                "--urls".into(),
                "https://a/".into()
            ])
            .is_err()
        );
        assert!(build_batch(&["open".into(), "--tab-ids".into(), "1".into()]).is_err());
        assert!(build_batch(&["read".into()]).is_err());
        assert!(
            build_batch(&[
                "read".into(),
                "--tab-ids".into(),
                "1".into(),
                "--bogus".into()
            ])
            .is_err()
        );
        assert!(
            build_batch(&[
                "read".into(),
                "--tab-ids".into(),
                "1".into(),
                "--concurrency".into(),
                "9".into()
            ])
            .is_err()
        );
        assert!(build_batch(&[]).is_err());
    }

    #[test]
    fn batch_port_can_hide_in_trailing_tokens() {
        let call = build_batch(&[
            "read".into(),
            "--tab-ids".into(),
            "1".into(),
            "--mode".into(),
            "md".into(),
            "--port".into(),
            "7800".into(),
        ])
        .unwrap();
        assert_eq!(call.port, Some(7800));
        assert_eq!(args_of(&call)["args"]["mode"], "md");

        let call =
            build_batch(&["read".into(), "--tab-ids=1".into(), "--port=7801".into()]).unwrap();
        assert_eq!(call.port, Some(7801));
    }

    #[test]
    fn observe_forms() {
        let call = build_observe(
            "network",
            None,
            Some(12),
            &None,
            &Some("api".into()),
            &Some("5s".into()),
            &Some("error".into()),
            Some(false),
            Some(PathBuf::from("net.har")),
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["kind"], "network");
        assert_eq!(args_of(&call)["since"], 12);
        assert_eq!(args_of(&call)["filter"], "api");
        assert_eq!(args_of(&call)["wait"], 5.0);
        assert_eq!(args_of(&call)["bodyFilter"], "error");
        assert_eq!(args_of(&call)["bodies"], false);
        assert_eq!(args_of(&call)["har"], true);
        assert_eq!(call.out, Some(PathBuf::from("net.har")));

        assert!(
            build_observe(
                "dom", None, None, &None, &None, &None, &None, None, None, None
            )
            .is_err()
        );
    }

    #[test]
    fn open_navigation_modes_are_exclusive() {
        assert!(build_open("https://x.y", None, false, &None, &None, None).is_ok());
        assert!(build_open("https://x.y", Some(3), true, &None, &None, None).is_err());
        assert!(
            build_open(
                "https://x.y",
                Some(3),
                false,
                &Some("session".into()),
                &None,
                None
            )
            .is_err()
        );
        assert!(build_open("https://x.y", None, false, &Some("all".into()), &None, None).is_err());
        assert!(
            build_open(
                "https://x.y",
                None,
                false,
                &None,
                &Some("later".into()),
                None
            )
            .is_err()
        );
        assert!(build_open("https://x.y", None, false, &None, &None, Some(0.5)).is_err());
        let call = build_open("https://x.y", None, false, &None, &None, Some(20.0)).unwrap();
        assert_eq!(args_of(&call)["waitTimeout"], 20.0);
    }

    #[test]
    fn act_rejects_unknown_flags_instead_of_treating_them_as_refs() {
        let error = build_act(&["click".into(), "--at".into(), "593,380".into()])
            .err()
            .expect("unknown flag must fail");
        assert!(error.contains("unknown flag"), "{error}");
        let error = build_act(&["click".into(), "foo".into()])
            .err()
            .expect("non-ref positional must fail");
        assert!(error.contains("not a ref"), "{error}");
        assert!(build_act(&["click".into(), "e12@d1".into()]).is_ok());
        assert!(build_act(&["click".into(), "[e12@d1]".into()]).is_ok());
    }

    #[test]
    fn tabs_forms() {
        let call = build_tabs(&TabsArgs { cmd: None }).unwrap();
        assert_eq!(args_of(&call)["action"], "list");

        let call = build_tabs(&TabsArgs {
            cmd: Some(TabsCommand::List {
                budget: Some(200),
                cursor: Some(3),
            }),
        })
        .unwrap();
        assert_eq!(args_of(&call)["budget"], 200);
        assert_eq!(args_of(&call)["cursor"], 3);

        let call = build_tabs(&TabsArgs {
            cmd: Some(TabsCommand::Close {
                ids: vec![1, 2, 3],
                session: false,
            }),
        })
        .unwrap();
        assert_eq!(args_of(&call)["tabIds"], json!([1, 2, 3]));

        let call = build_tabs(&TabsArgs {
            cmd: Some(TabsCommand::Close {
                ids: vec![],
                session: true,
            }),
        })
        .unwrap();
        assert_eq!(args_of(&call)["scope"], "session");

        assert!(
            build_tabs(&TabsArgs {
                cmd: Some(TabsCommand::Close {
                    ids: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
                    session: false,
                }),
            })
            .is_err()
        );
    }

    #[test]
    fn files_forms() {
        let dir = std::env::temp_dir();
        let a = dir.join(format!("ema2-test-{}-a.pdf", std::process::id()));
        let b = dir.join(format!("ema2-test-{}-b.txt", std::process::id()));
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, b"world").unwrap();
        let call = build_files(&FilesArgs {
            cmd: FilesCommand::Upload {
                paths: vec![
                    a.to_string_lossy().to_string(),
                    b.to_string_lossy().to_string(),
                ],
                r#ref: Some("e3".into()),
                selector: None,
                target: None,
                tab: None,
            },
        })
        .unwrap();
        assert_eq!(args_of(&call)["action"], "upload");
        assert_eq!(args_of(&call)["ref"], "e3");
        assert_eq!(args_of(&call)["paths"][0], a.to_string_lossy().to_string());
        // 字节载荷随请求下发，upload 不再依赖 debugger。
        assert_eq!(args_of(&call)["files"][0]["type"], "application/pdf");
        assert_eq!(args_of(&call)["files"][1]["type"], "text/plain");
        assert_eq!(args_of(&call)["files"][0]["data"], "aGVsbG8=");
        assert_eq!(args_of(&call)["files"][1]["data"], "d29ybGQ=");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);

        let call = build_files(&FilesArgs {
            cmd: FilesCommand::DownloadDir,
        })
        .unwrap();
        assert_eq!(args_of(&call)["action"], "download-dir");

        assert!(
            build_files(&FilesArgs {
                cmd: FilesCommand::Upload {
                    paths: vec![],
                    r#ref: None,
                    selector: None,
                    target: None,
                    tab: None,
                },
            })
            .is_err()
        );
    }

    #[test]
    fn shot_forms() {
        let call = build_shot(None, false, false, &None, Some(PathBuf::from("out.png"))).unwrap();
        assert_eq!(call.out, Some(PathBuf::from("out.png")));
        assert!(args_of(&call).get("inline").is_none());

        let call = build_shot(Some(4), true, true, &Some("jpeg".into()), None).unwrap();
        assert_eq!(args_of(&call)["tabId"], 4);
        assert_eq!(args_of(&call)["inline"], true);
        assert_eq!(args_of(&call)["annotate"], true);
        assert_eq!(args_of(&call)["format"], "jpeg");

        assert!(build_shot(None, false, false, &Some("webp".into()), None).is_err());
    }

    #[test]
    fn js_forms() {
        let call = build_js(Some(&"1+1".to_owned()), &None, None, None).unwrap();
        assert_eq!(args_of(&call)["code"], "1+1");

        assert!(build_js(None, &None, None, None).is_err());
    }
}
