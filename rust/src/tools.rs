//! v2.2 工具注册表（11 个粗粒度工具，面向 AI 的 CLI 子命令）。
//!
//! 约定：
//! - 输出行式紧凑文本（`key=value` / TSV / `[ref]`），不 pretty-print JSON
//! - 列表带 `total / shown / next`
//! - 错误统一 `error=<code> msg=... hint=...`
//! - 工具名白名单、参数校验、超时预算同源，都在这里定义一次

use std::time::Duration;

use serde_json::{Map, Value, json};

/// 11 个工具命令名（CLI 子命令与 /cmd 白名单共用）。
pub const TOOL_NAMES: &[&str] = &[
    "tabs", "open", "read", "snapshot", "act", "wait", "js", "observe", "files", "shot", "batch",
];

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const ACT_TIMEOUT: Duration = Duration::from_secs(60);
pub const LONG_TIMEOUT: Duration = Duration::from_secs(120);
pub const PAGE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次工具调用的产品总时长上限；超出时在调用前明确拒绝，而不是让 daemon 伪装成超时。
pub const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(600);
const RELAY_MARGIN_SECS: f64 = 15.0;
const MAX_BUDGET: u64 = 200_000;
const MAX_WAIT_SECS: f64 = 300.0;

pub fn contains(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// 中继等待扩展响应的超时（必须大于扩展侧自身的等待上限）。
pub fn timeout_for(name: &str, arguments: &Value) -> Duration {
    match name {
        "wait" => Duration::from_secs_f64(wait_seconds(arguments) + RELAY_MARGIN_SECS),
        "open" => {
            if arguments.get("wait").and_then(Value::as_str) == Some("none") {
                DEFAULT_TIMEOUT
            } else {
                // 扩展按 waitTimeout 等文档接位，再装 hook。默认 15s；
                // 预算必须跟着 waitTimeout 走，否则 --timeout 45 会先被 30s 中继超时打断。
                Duration::from_secs_f64(open_wait_seconds(arguments) + RELAY_MARGIN_SECS)
            }
        }
        "act" => Duration::from_secs_f64(act_budget_seconds(arguments)),
        // js 的 --timeout 是硬执行上限；中继预算必须跟着它走。
        "js" => Duration::from_secs_f64(js_timeout_seconds(arguments) + RELAY_MARGIN_SECS),
        "shot" | "files" => LONG_TIMEOUT,
        "batch" => batch_budget(arguments).min(MAX_CALL_TIMEOUT),
        _ => DEFAULT_TIMEOUT,
    }
}

/// js 执行上限（默认 10s，已由参数校验限制在 0.1..300）。
fn js_timeout_seconds(arguments: &Value) -> f64 {
    arguments
        .get("timeout")
        .and_then(Value::as_f64)
        .unwrap_or(10.0)
        .clamp(0.1, 300.0)
}

/// open 等待文档接位的秒数（默认 15，已由参数校验限制在 1..60）。
fn open_wait_seconds(arguments: &Value) -> f64 {
    arguments
        .get("waitTimeout")
        .and_then(Value::as_f64)
        .unwrap_or(15.0)
        .clamp(1.0, 60.0)
}

/// act 的脚本 await 与逐步骤 waitFor 时长总和。
fn act_wait_seconds(arguments: &Value) -> f64 {
    let top_level = if arguments
        .get("waitFor")
        .is_some_and(|value| !value.is_null())
    {
        arguments
            .pointer("/waitFor/timeout")
            .and_then(Value::as_f64)
            .unwrap_or(10.0)
            .clamp(0.0, 30.0)
    } else {
        0.0
    };
    let steps = arguments
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter(|step| step.get("action").and_then(Value::as_str) == Some("waitFor"))
                .map(|step| {
                    step.get("timeout")
                        .and_then(Value::as_f64)
                        .unwrap_or(10.0)
                        .clamp(0.0, 30.0)
                })
                .sum::<f64>()
        })
        .unwrap_or(0.0);
    top_level + steps
}

/// act 的完整预算：固定执行上限 + 每步最多 3s 的隐式 settle + 显式等待。
/// 20 步脚本光 settle 就可能 60s，不能只靠 ACT_TIMEOUT。
fn act_budget_seconds(arguments: &Value) -> f64 {
    let steps = arguments
        .get("steps")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(1);
    ACT_TIMEOUT.as_secs_f64()
        + steps as f64 * 3.0
        + act_wait_seconds(arguments)
        + act_await_seconds(arguments)
}

/// batch 的实际预算（未截断）：`timeout_for` 与参数校验共用这一个入口。
fn batch_budget(arguments: &Value) -> Duration {
    Duration::from_secs_f64(batch_budget_seconds(arguments))
}

/// 已校验/已应用默认值后的 wait 实际秒数：time 优先，否则 timeout（默认 30）。
fn wait_seconds(arguments: &Value) -> f64 {
    if let Some(time) = arguments.get("time").and_then(Value::as_f64) {
        return time.clamp(0.0, MAX_WAIT_SECS);
    }
    arguments
        .get("timeout")
        .and_then(Value::as_f64)
        .unwrap_or(PAGE_WAIT_TIMEOUT.as_secs_f64())
        .clamp(0.0, MAX_WAIT_SECS)
}

// act 脚本里 `await 10s` 的时长总和；这些等待会叠加到工具总预算。
fn act_await_seconds(arguments: &Value) -> f64 {
    arguments
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter(|step| step.get("action").and_then(Value::as_str) == Some("await"))
                .filter_map(|step| step.get("timeout").and_then(Value::as_f64))
                .map(|seconds| seconds.clamp(0.0, MAX_WAIT_SECS))
                .sum()
        })
        .unwrap_or(0.0)
}

fn batch_tab_count(arguments: &Value) -> usize {
    let tabs = arguments
        .get("tabIds")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if tabs > 0 {
        return tabs;
    }
    arguments
        .get("urls")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn batch_concurrency(arguments: &Value) -> f64 {
    arguments
        .get("concurrency")
        .and_then(Value::as_f64)
        .unwrap_or(2.0)
        .clamp(1.0, 4.0)
}

/// batch 的总预算 = 执行波次 × 单次最长耗时 + 余量。
///
/// js 走全局 debugger 串行队列，所以有效并发固定为 1，不能直接使用声明并发。
/// 传 urls 时每个目标多一次 open（等 load + 装 hook）。
fn batch_budget_seconds(arguments: &Value) -> f64 {
    let tool = arguments
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tabs = batch_tab_count(arguments) as f64;
    if tabs <= 0.0 {
        return LONG_TIMEOUT.as_secs_f64();
    }
    let declared = batch_concurrency(arguments);
    let effective = if tool == "js" { 1.0 } else { declared };
    let waves = (tabs / effective).ceil();
    let inner = arguments.get("args").unwrap_or(&Value::Null);
    let opens = batch_url_count(arguments) as f64;
    // `open` 本身没有“内层工具”：等待文档接位 + 装 hook/focus 的耗时就是单次预算。
    // 不能再用 PAGE_WAIT_TIMEOUT 固定值，否则 --waitTimeout 60 会先被中继超时打断。
    let (per_call, open_cost) = match tool {
        "wait" => (wait_seconds(inner), opens > 0.0),
        "open" => (open_wait_seconds(inner) + 12.0, false),
        "js" => (js_timeout_seconds(inner) + 8.0, opens > 0.0),
        _ => (DEFAULT_TIMEOUT.as_secs_f64(), opens > 0.0),
    };
    // 复合模式（urls + read/snapshot/js/wait）每个目标多一次 open；open 模式已计入 per_call。
    let open_cost = if open_cost {
        (opens / effective).ceil() * (PAGE_WAIT_TIMEOUT.as_secs_f64() + 2.0)
    } else {
        0.0
    };
    waves * per_call + open_cost + RELAY_MARGIN_SECS
}

fn batch_url_count(arguments: &Value) -> usize {
    arguments
        .get("urls")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

pub fn validate_arguments(name: &str, args: &Value) -> Result<(), String> {
    let Some(object) = args.as_object() else {
        return Err("arguments must be a JSON object".into());
    };
    if !contains(name) {
        return Err(format!("unknown tool: {name}"));
    }
    match name {
        "tabs" => {
            enum_arg(
                object,
                "action",
                &["list", "current", "new", "close", "activate"],
                false,
            )?;
            tab_id(object)?;
            budget_arg(object)?;
            cursor_arg(object)?;
            let has_many = tab_ids_arg(object)?;
            enum_arg(object, "scope", &["session"], false)?;
            if has_many && object.get("tabId").is_some_and(|v| !v.is_null()) {
                return Err("use either tabId or tabIds, not both".into());
            }
            let action = object
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let wants_close = has_many || object.get("scope").is_some_and(|v| !v.is_null());
            if wants_close && action != "close" {
                return Err("tabIds/scope only apply to tabs action=close".into());
            }
            if object.get("url").is_some_and(|v| !v.is_null()) {
                url_arg(object)?;
            }
            bool_arg(object, "active")?;
        }
        "open" => {
            url_arg(object)?;
            tab_id(object)?;
            enum_arg(object, "wait", &["load", "dom", "none"], false)?;
            if let Some(timeout) = number_arg(object, "waitTimeout", false)?
                && !(1.0..=60.0).contains(&timeout)
            {
                return Err("waitTimeout must be between 1 and 60 seconds".into());
            }
            bool_arg(object, "replace")?;
            enum_arg(object, "reuse", &["session"], false)?;
            let replace = object
                .get("replace")
                .is_some_and(|v| matches!(v, Value::Bool(true)));
            let has_reuse = object.get("reuse").is_some_and(|v| !v.is_null());
            let has_tab = object.get("tabId").is_some_and(|v| !v.is_null());
            let picks = [replace, has_reuse, has_tab]
                .iter()
                .filter(|flag| **flag)
                .count();
            if picks > 1 {
                return Err("use only one of tabId, replace=true, reuse=session".into());
            }
        }
        "read" => {
            tab_id(object)?;
            enum_arg(object, "mode", &["md", "text", "html", "visible"], false)?;
            string_arg(object, "selector", 2_048, false, true)?;
            bool_arg(object, "main")?;
            budget_arg(object)?;
            cursor_arg(object)?;
        }
        "snapshot" => {
            tab_id(object)?;
            enum_arg(
                object,
                "filter",
                &["interactive", "full", "links", "buttons", "form"],
                false,
            )?;
            string_arg(object, "near", 32, false, true)?;
            budget_arg(object)?;
            cursor_arg(object)?;
            bool_arg(object, "values")?;
        }
        "act" => {
            let has_steps = object
                .get("steps")
                .is_some_and(|v| !matches!(v, Value::Null));
            if has_steps {
                let steps = object
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or("steps must be an array")?;
                if steps.is_empty() || steps.len() > 20 {
                    return Err("steps must contain 1..20 actions".into());
                }
                for (index, step) in steps.iter().enumerate() {
                    let Some(step) = step.as_object() else {
                        return Err(format!("steps[{index}] must be an object"));
                    };
                    validate_action(step).map_err(|error| format!("steps[{index}]: {error}"))?;
                }
                let budget = act_budget_seconds(&Value::Object(object.clone()));
                if budget > MAX_CALL_TIMEOUT.as_secs_f64() {
                    return Err(format!(
                        "act budget {:.0}s exceeds the {}s call limit; reduce steps/await durations or split the script",
                        budget,
                        MAX_CALL_TIMEOUT.as_secs()
                    ));
                }
            } else {
                validate_action(object)?;
            }
            tab_id(object)?;
            if let Some(wait_for) = object.get("waitFor").filter(|v| !matches!(v, Value::Null)) {
                let Some(wait_for) = wait_for.as_object() else {
                    return Err("waitFor must be an object".into());
                };
                string_arg(wait_for, "url", 2_048, false, true)?;
                if let Some(status) = number_arg(wait_for, "status", false)?
                    && !(100.0..=599.0).contains(&status)
                {
                    return Err("waitFor.status must be between 100 and 599".into());
                }
                if let Some(timeout) = number_arg(wait_for, "timeout", false)?
                    && !(0.0..=30.0).contains(&timeout)
                {
                    return Err("waitFor.timeout must be between 0 and 30 seconds".into());
                }
            }
        }
        "wait" => {
            tab_id(object)?;
            let has_text = string_arg(object, "text", 100_000, false, true)?;
            let has_gone = string_arg(object, "textGone", 100_000, false, true)?;
            let has_selector = string_arg(object, "selector", 2_048, false, true)?;
            let time = number_arg(object, "time", false)?;
            let idle = object
                .get("idle")
                .is_some_and(|v| matches!(v, Value::Bool(true)));
            bool_arg(object, "await")?;
            let has_await = object
                .get("await")
                .is_some_and(|v| matches!(v, Value::Bool(true)));
            let conditions = [
                has_text,
                has_gone,
                has_selector,
                time.is_some(),
                idle,
                has_await,
            ]
            .iter()
            .filter(|flag| **flag)
            .count();
            if conditions != 1 {
                return Err(
                    "exactly one of text, textGone, selector, time, idle, await is required".into(),
                );
            }
            if let Some(seconds) = time
                && !(0.0..=300.0).contains(&seconds)
            {
                return Err("time must be between 0 and 300".into());
            }
            if let Some(timeout) = number_arg(object, "timeout", false)?
                && !(0.0..=300.0).contains(&timeout)
            {
                return Err("timeout must be between 0 and 300".into());
            }
        }
        "js" => {
            string_arg(object, "code", 100_000, true, false)?;
            tab_id(object)?;
            budget_arg(object)?;
            cursor_arg(object)?;
            if let Some(timeout) = number_arg(object, "timeout", false)?
                && !(0.1..=300.0).contains(&timeout)
            {
                return Err("js timeout must be between 0.1 and 300 seconds".into());
            }
        }
        "observe" => {
            enum_arg(object, "kind", &["console", "network"], true)?;
            tab_id(object)?;
            counter_arg(object, "since")?;
            enum_arg(object, "level", &["log", "warn", "error"], false)?;
            string_arg(object, "filter", 2_048, false, true)?;
            string_arg(object, "bodyFilter", 512, false, true)?;
            if let Some(wait) = number_arg(object, "wait", false)?
                && !(0.0..=10.0).contains(&wait)
            {
                return Err("wait must be between 0 and 10 seconds".into());
            }
            bool_arg(object, "bodies")?;
            bool_arg(object, "har")?;
            budget_arg(object)?;
        }
        "files" => {
            enum_arg(object, "action", &["upload", "download-dir"], true)?;
            tab_id(object)?;
            string_arg(object, "ref", 32, false, true)?;
            string_arg(object, "selector", 2_048, false, false)?;
            string_arg(object, "target", 200, false, false)?;
            let action = object
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if action == "upload" {
                let Some(paths) = object.get("paths") else {
                    return Err("paths is required for upload".into());
                };
                let Some(paths) = paths.as_array() else {
                    return Err("paths must be an array of absolute file paths".into());
                };
                if paths.is_empty() || paths.len() > 20 {
                    return Err("paths must contain 1..20 entries".into());
                }
                for (index, path) in paths.iter().enumerate() {
                    match path {
                        Value::String(value)
                            if !value.trim().is_empty() && value.len() <= 4_096 =>
                        {
                            if cfg!(windows) && !is_windows_absolute(value) {
                                return Err(format!(
                                    "paths[{index}] must be an absolute Windows path (e.g. C:/dir/file.pdf)"
                                ));
                            }
                        }
                        _ => return Err(format!("paths[{index}] must be a non-empty path")),
                    }
                }
                let has_target = ["ref", "selector", "target"].iter().any(|key| {
                    object.get(*key).is_some_and(|value| {
                        value.as_str().is_some_and(|text| !text.trim().is_empty())
                    })
                });
                if !has_target {
                    return Err("upload needs one of ref, selector, target".into());
                }
                // 字节载荷（DataTransfer 路径）：daemon body 上限 8MB，base64 会膨胀 4/3。
                if let Some(files) = object.get("files").filter(|value| !value.is_null()) {
                    let Some(files) = files.as_array() else {
                        return Err("files must be an array".into());
                    };
                    if files.len() > 20 {
                        return Err("files must contain at most 20 entries".into());
                    }
                    let mut total = 0usize;
                    for (index, file) in files.iter().enumerate() {
                        let Some(file) = file.as_object() else {
                            return Err(format!("files[{index}] must be an object"));
                        };
                        string_arg(file, "name", 255, true, true)
                            .map_err(|e| format!("files[{index}]: {e}"))?;
                        string_arg(file, "type", 100, false, true)
                            .map_err(|e| format!("files[{index}]: {e}"))?;
                        string_arg(file, "data", 7_000_000, true, true)
                            .map_err(|e| format!("files[{index}]: {e}"))?;
                        let data = file.get("data").and_then(Value::as_str).unwrap_or_default();
                        total += data.len();
                        if total > 7_000_000 {
                            return Err(
                                "files payload exceeds 7MB of base64; upload fewer/smaller files"
                                    .into(),
                            );
                        }
                    }
                }
            } else {
                // download-dir 只为返回明确的 download_dir_unsupported 错误，
                // dir 值本身无用（Edge 不允许扩展改下载目录）。
                string_arg(object, "dir", 4_096, false, false)?;
            }
        }
        "shot" => {
            tab_id(object)?;
            bool_arg(object, "inline")?;
            bool_arg(object, "annotate")?;
            // 扩展按 format 决定截图格式；CLI `--format png|jpeg` 与此对应。
            enum_arg(object, "format", &["png", "jpeg"], false)?;
        }
        "batch" => {
            enum_arg(
                object,
                "tool",
                &["open", "read", "snapshot", "js", "wait"],
                true,
            )?;
            let tool = object
                .get("tool")
                .and_then(Value::as_str)
                .expect("required above");
            let has_urls = urls_arg(object)?;
            enum_arg(object, "reuse", &["session"], false)?;
            if object.get("reuse").is_some_and(|value| !value.is_null()) && !has_urls {
                return Err("reuse=session only applies to urls[]".into());
            }
            let has_tabs = object
                .get("tabIds")
                .is_some_and(|v| !matches!(v, Value::Null));
            if has_urls && has_tabs {
                return Err("batch takes either tabIds or urls, not both".into());
            }
            if !has_urls {
                let Some(tab_ids) = object.get("tabIds").and_then(Value::as_array) else {
                    return Err("batch needs tabIds[] or urls[]".into());
                };
                if tab_ids.is_empty() || tab_ids.len() > 8 {
                    return Err("tabIds must contain 1..8 entries".into());
                }
                for (index, id) in tab_ids.iter().enumerate() {
                    match id.as_f64() {
                        Some(value) if value >= 0.0 && value.fract() == 0.0 => {}
                        _ => return Err(format!("tabIds[{index}] must be a non-negative integer")),
                    }
                }
                if tool == "open" {
                    return Err("batch open needs urls[], not tabIds[]".into());
                }
            }
            if let Some(args) = object.get("args")
                && !args.is_object()
            {
                return Err("args must be an object".into());
            }
            if let Some(concurrency) = number_arg(object, "concurrency", false)?
                && (!(1.0..=4.0).contains(&concurrency) || concurrency.fract() != 0.0)
            {
                return Err("concurrency must be an integer between 1 and 4".into());
            }
            // 内层参数复用子工具自己的验证器：batch 不能绕过 wait.time=300 等限制。
            if let Some(inner) = object.get("args") {
                let inner = inner.as_object().expect("checked above");
                if has_urls
                    && ["replace", "reuse", "tabId"]
                        .iter()
                        .any(|key| inner.get(*key).is_some_and(|value| !value.is_null()))
                {
                    return Err(
                        "urls mode always opens new tabs; replace/reuse/tabId are not allowed in batch args"
                            .into(),
                    );
                }
                let mut merged = inner.clone();
                if tool == "open" {
                    // urls 模式下 url 由 batch 逐个注入，这里只校验 wait 等其他参数。
                    merged.insert("url".into(), json!("https://placeholder.invalid/"));
                } else {
                    merged.insert("tabId".into(), json!(0));
                }
                validate_arguments(tool, &Value::Object(merged))
                    .map_err(|error| format!("args: {error}"))?;
            }
            let budget = batch_budget(&Value::Object(object.clone()));
            if budget > MAX_CALL_TIMEOUT {
                return Err(format!(
                    "batch estimated budget {:.0}s exceeds the {}s call limit; reduce tabIds/urls, raise concurrency, or split the batch",
                    budget.as_secs_f64(),
                    MAX_CALL_TIMEOUT.as_secs()
                ));
            }
        }
        _ => return Err(format!("unknown tool: {name}")),
    }
    Ok(())
}

fn validate_action(object: &Map<String, Value>) -> Result<(), String> {
    enum_arg(
        object,
        "action",
        &[
            "click", "type", "fill", "select", "press", "hover", "scroll", "submit", "back",
            "forward", "reload", "await", "waitFor",
        ],
        true,
    )?;
    string_arg(object, "ref", 32, false, true)?;
    string_arg(object, "selector", 2_048, false, false)?;
    string_arg(object, "target", 200, false, false)?;
    string_arg(object, "text", 100_000, false, true)?;
    string_arg(object, "value", 100_000, false, true)?;
    string_arg(object, "key", 100, false, false)?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if action == "await" {
        if let Some(timeout) = number_arg(object, "timeout", false)?
            && !(0.0..=300.0).contains(&timeout)
        {
            return Err("await timeout must be between 0 and 300 seconds".into());
        }
        return Ok(());
    }
    if action == "waitFor" {
        let url = string_arg(object, "url", 2_048, false, true)?;
        let status = number_arg(object, "status", false)?;
        if let Some(status) = status
            && !(100.0..=599.0).contains(&status)
        {
            return Err("waitFor.status must be between 100 and 599".into());
        }
        if let Some(timeout) = number_arg(object, "timeout", false)?
            && !(0.0..=30.0).contains(&timeout)
        {
            return Err("waitFor.timeout must be between 0 and 30 seconds".into());
        }
        if !url && status.is_none() {
            return Err("waitFor needs url= or status=".into());
        }
        return Ok(());
    }
    if action == "press"
        && !object
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.trim().is_empty())
    {
        return Err("key is required for press".into());
    }
    if action != "press" && object.get("key").is_some_and(|value| !value.is_null()) {
        return Err("key is only supported by press".into());
    }
    if action != "scroll" && object.get("deltaY").is_some_and(|value| !value.is_null()) {
        return Err("deltaY is only supported by scroll".into());
    }
    if matches!(action, "scroll" | "back" | "forward" | "reload")
        && ["selector", "target"]
            .iter()
            .any(|key| object.get(*key).is_some_and(|value| !value.is_null()))
    {
        return Err(format!(
            "{action} does not support selector/target locators"
        ));
    }
    if action == "fill"
        && object
            .get("fields")
            .is_none_or(|fields| matches!(fields, Value::Null))
    {
        return Err("fill requires fields[]".into());
    }
    if let Some(delta) = number_arg(object, "deltaY", false)?
        && delta.abs() > 100_000.0
    {
        return Err("deltaY must be within +/-100000".into());
    }
    if object
        .get("submit")
        .is_some_and(|v| !matches!(v, Value::Null | Value::Bool(_)))
    {
        return Err("submit must be a boolean".into());
    }
    if object
        .get("submit")
        .is_some_and(|v| matches!(v, Value::Bool(true)))
        && action != "type"
    {
        return Err(
            "submit=true is only supported by type; use action=submit or click the submit button"
                .into(),
        );
    }
    if let Some(fields) = object.get("fields").filter(|v| !matches!(v, Value::Null)) {
        let Some(fields) = fields.as_array() else {
            return Err("fields must be an array".into());
        };
        if fields.is_empty() || fields.len() > 100 {
            return Err("fields must contain 1..100 entries".into());
        }
        for (index, field) in fields.iter().enumerate() {
            let Some(field) = field.as_object() else {
                return Err(format!("fields[{index}] must be an object"));
            };
            string_arg(field, "ref", 32, false, true)
                .map_err(|e| format!("fields[{index}]: {e}"))?;
            string_arg(field, "selector", 2_048, false, false)
                .map_err(|e| format!("fields[{index}]: {e}"))?;
            string_arg(field, "value", 100_000, true, true)
                .map_err(|e| format!("fields[{index}]: {e}"))?;
        }
    }
    Ok(())
}

fn enum_arg(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
    required: bool,
) -> Result<(), String> {
    match object.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(format!("{key} is required"))
            } else {
                Ok(())
            }
        }
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => Ok(()),
        Some(Value::String(value)) => Err(format!(
            "{key} must be one of {}, got {value}",
            allowed.join("|")
        )),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn string_arg(
    object: &Map<String, Value>,
    key: &str,
    max: usize,
    required: bool,
    allow_empty: bool,
) -> Result<bool, String> {
    match object.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(format!("{key} is required"))
            } else {
                Ok(false)
            }
        }
        Some(Value::String(value)) => {
            if value.len() > max {
                return Err(format!("{key} is too long (max {max} bytes)"));
            }
            if !allow_empty && value.trim().is_empty() {
                return Err(format!("{key} must not be empty"));
            }
            Ok(true)
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn number_arg(
    object: &Map<String, Value>,
    key: &str,
    required: bool,
) -> Result<Option<f64>, String> {
    match object.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(format!("{key} is required"))
            } else {
                Ok(None)
            }
        }
        Some(value) => match value.as_f64() {
            Some(value) if value.is_finite() => Ok(Some(value)),
            _ => Err(format!("{key} must be a finite number")),
        },
    }
}

fn bool_arg(object: &Map<String, Value>, key: &str) -> Result<(), String> {
    if object
        .get(key)
        .is_some_and(|value| !matches!(value, Value::Null | Value::Bool(_)))
    {
        return Err(format!("{key} must be a boolean"));
    }
    Ok(())
}

fn tab_id(object: &Map<String, Value>) -> Result<(), String> {
    if let Some(value) = number_arg(object, "tabId", false)?
        && (value < 0.0 || value.fract() != 0.0)
    {
        return Err("tabId must be a non-negative integer".into());
    }
    Ok(())
}

/// `tabIds`（批量关闭）：1..8 个非负整数；返回是否出现。
fn tab_ids_arg(object: &Map<String, Value>) -> Result<bool, String> {
    let Some(value) = object.get("tabIds").filter(|v| !matches!(v, Value::Null)) else {
        return Ok(false);
    };
    let Some(ids) = value.as_array() else {
        return Err("tabIds must be an array".into());
    };
    if ids.is_empty() || ids.len() > 8 {
        return Err("tabIds must contain 1..8 entries".into());
    }
    for (index, id) in ids.iter().enumerate() {
        match id.as_f64() {
            Some(value) if value >= 0.0 && value.fract() == 0.0 => {}
            _ => return Err(format!("tabIds[{index}] must be a non-negative integer")),
        }
    }
    Ok(true)
}

/// `urls`（batch 复合模式）：1..8 个 http(s) URL。
fn urls_arg(object: &Map<String, Value>) -> Result<bool, String> {
    let Some(value) = object.get("urls").filter(|v| !matches!(v, Value::Null)) else {
        return Ok(false);
    };
    let Some(urls) = value.as_array() else {
        return Err("urls must be an array".into());
    };
    if urls.is_empty() || urls.len() > 8 {
        return Err("urls must contain 1..8 entries".into());
    }
    for (index, url) in urls.iter().enumerate() {
        let Some(text) = url.as_str() else {
            return Err(format!("urls[{index}] must be a string"));
        };
        if text.len() > 8_192 || text.trim().is_empty() {
            return Err(format!("urls[{index}] must be a non-empty url (max 8192)"));
        }
        let lower = text.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return Err(format!("urls[{index}] must use http or https"));
        }
    }
    Ok(true)
}

fn url_arg(object: &Map<String, Value>) -> Result<(), String> {
    string_arg(object, "url", 8_192, true, false)?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("url must use http or https".into());
    }
    Ok(())
}

fn budget_arg(object: &Map<String, Value>) -> Result<(), String> {
    if let Some(budget) = number_arg(object, "budget", false)?
        && !(200.0..=MAX_BUDGET as f64).contains(&budget)
    {
        return Err(format!("budget must be between 200 and {MAX_BUDGET}"));
    }
    Ok(())
}

/// 游标类参数（cursor / since）从工具输出里抄回来，模型常把它写成字符串（"400"）。
/// 输出里是裸数字，两种形式都收；只接受非负整数。
fn counter_arg(object: &Map<String, Value>, key: &str) -> Result<(), String> {
    let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let parsed = value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    });
    match parsed {
        Some(counter) if counter.is_finite() && counter >= 0.0 && counter.fract() == 0.0 => Ok(()),
        _ => Err(format!(
            "{key} must be a non-negative integer; use the numeric value from the previous output"
        )),
    }
}

fn cursor_arg(object: &Map<String, Value>) -> Result<(), String> {
    counter_arg(object, "cursor")
}

/// Windows 绝对路径：`C:\...`、`C:/...` 或 UNC `\\server\share`。
fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == 92u8 || bytes[2] == b'/')
    {
        return true;
    }
    path.as_bytes().starts_with(&[92, 92])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_compact_and_complete() {
        assert_eq!(TOOL_NAMES.len(), 11);
        // CLI 子命令与 /cmd 白名单共用这一份名单。
        for name in TOOL_NAMES {
            assert!(contains(name), "{name} missing from contains()");
        }
        assert!(!contains("edge_get_tabs"));
        assert!(!contains("pair"));
        assert!(!contains("install"));
    }

    #[test]
    fn every_tool_is_reachable() {
        for name in TOOL_NAMES {
            if let Err(error) = validate_arguments(name, &json!({})) {
                assert!(!error.contains("unknown tool"), "{} is not handled", name);
            }
        }
    }

    #[test]
    fn tabs_validates_action() {
        assert!(validate_arguments("tabs", &json!({})).is_ok());
        assert!(validate_arguments("tabs", &json!({ "action": "list" })).is_ok());
        assert!(
            validate_arguments("tabs", &json!({ "action": "new", "url": "https://a.b" })).is_ok()
        );
        assert!(validate_arguments("tabs", &json!({ "action": "explode" })).is_err());
        assert!(validate_arguments("tabs", &json!({ "tabId": 1.5 })).is_err());
    }

    #[test]
    fn open_requires_http_url() {
        assert!(validate_arguments("open", &json!({ "url": "https://x.y" })).is_ok());
        assert!(validate_arguments("open", &json!({ "url": "file:///etc/passwd" })).is_err());
        assert!(
            validate_arguments("open", &json!({ "url": "HTTPS://x.y", "wait": "dom" })).is_ok()
        );
        assert!(
            validate_arguments("open", &json!({ "url": "https://x.y", "wait": "later" })).is_err()
        );
        assert!(validate_arguments("open", &json!({})).is_err());
    }

    #[test]
    fn read_and_snapshot_bounds() {
        assert!(validate_arguments("read", &json!({})).is_ok());
        assert!(validate_arguments("read", &json!({ "mode": "raw" })).is_err());
        assert!(validate_arguments("read", &json!({ "mode": "visible" })).is_ok());
        assert!(validate_arguments("read", &json!({ "budget": 100 })).is_err());
        assert!(validate_arguments("read", &json!({ "cursor": -1 })).is_err());
        assert!(validate_arguments("read", &json!({ "main": false })).is_ok());
        assert!(validate_arguments("read", &json!({ "main": "yes" })).is_err());
        assert!(validate_arguments("snapshot", &json!({ "filter": "all" })).is_err());
        assert!(validate_arguments("snapshot", &json!({ "filter": "links" })).is_ok());
        assert!(validate_arguments("snapshot", &json!({ "filter": "form", "near": "e3" })).is_ok());
    }

    /// 游标是从工具输出里抄回来的（`next=400`），模型常带引号；两种形式都该收。
    #[test]
    fn counters_accept_the_numeric_string_form() {
        assert!(validate_arguments("read", &json!({ "cursor": "400" })).is_ok());
        assert!(validate_arguments("read", &json!({ "cursor": " 400 " })).is_ok());
        assert!(validate_arguments("snapshot", &json!({ "cursor": "12" })).is_ok());
        assert!(validate_arguments("observe", &json!({ "kind": "console", "since": "7" })).is_ok());
        assert!(validate_arguments("read", &json!({ "cursor": "next" })).is_err());
        assert!(validate_arguments("read", &json!({ "cursor": "1.5" })).is_err());
        assert!(validate_arguments("read", &json!({ "cursor": -1 })).is_err());
    }

    #[test]
    fn act_supports_single_batch_target_and_waitfor() {
        assert!(validate_arguments("act", &json!({ "action": "click", "ref": "e3" })).is_ok());
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "type", "selector": "#q", "text": "" })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "target": "button:提交" })
            )
            .is_ok()
        );
        assert!(validate_arguments("act", &json!({ "action": "teleport" })).is_err());
        assert!(validate_arguments("act", &json!({ "action": "fill", "fields": [] })).is_err());
        assert!(validate_arguments("act", &json!({ "action": "fill" })).is_err());
        assert!(validate_arguments("act", &json!({ "action": "press" })).is_err());
        assert!(validate_arguments("act", &json!({ "action": "press", "key": "Enter" })).is_ok());
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "ref": "e1", "key": "Enter" })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "ref": "e1", "deltaY": 10 })
            )
            .is_err()
        );
        assert!(validate_arguments("act", &json!({ "action": "back", "selector": "#x" })).is_err());
        assert!(
            validate_arguments("act", &json!({ "action": "scroll", "target": "button:x" }))
                .is_err()
        );
        assert!(validate_arguments("act", &json!({ "action": "scroll", "deltaY": -10 })).is_ok());
        assert!(
            validate_arguments(
                "act",
                &json!({ "steps": [{ "action": "click", "ref": "e1" }] })
            )
            .is_ok()
        );
        assert!(validate_arguments("act", &json!({ "steps": [] })).is_err());
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "ref": "e1", "waitFor": { "url": "api", "status": 200, "timeout": 5 } })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "waitFor": { "status": 42 } })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "waitFor": { "timeout": 60 } })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({
                    "steps": [
                        { "action": "click", "ref": "e1" },
                        { "action": "await", "timeout": 10 }
                    ]
                })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "steps": [{ "action": "await", "timeout": 301 }] })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "act",
                &json!({ "steps": [{ "action": "waitFor", "url": "api", "timeout": 5 }] })
            )
            .is_ok()
        );
        assert!(validate_arguments("act", &json!({ "steps": [{ "action": "waitFor" }] })).is_err());
        assert!(
            validate_arguments(
                "act",
                &json!({ "action": "click", "ref": "e1", "submit": true })
            )
            .is_err()
        );
        // 脚本 await 总量超过 600s 预算：执行前拒绝。
        let steps: Vec<Value> = (0..8)
            .map(|_| json!({ "action": "await", "timeout": 300 }))
            .collect();
        assert!(validate_arguments("act", &json!({ "steps": steps })).is_err());
    }

    #[test]
    fn wait_needs_exactly_one_condition() {
        assert!(validate_arguments("wait", &json!({ "text": "Done" })).is_ok());
        assert!(validate_arguments("wait", &json!({ "selector": "#x" })).is_ok());
        assert!(validate_arguments("wait", &json!({ "time": 2 })).is_ok());
        assert!(validate_arguments("wait", &json!({ "idle": true })).is_ok());
        assert!(validate_arguments("wait", &json!({})).is_err());
        assert!(validate_arguments("wait", &json!({ "text": "a", "time": 1 })).is_err());
        assert!(validate_arguments("wait", &json!({ "time": 301 })).is_err());
        assert!(validate_arguments("wait", &json!({ "text": "x", "timeout": 60 })).is_ok());
        assert!(validate_arguments("wait", &json!({ "text": "x", "timeout": 999 })).is_err());
        assert!(validate_arguments("wait", &json!({ "await": true, "timeout": 10 })).is_ok());
        assert!(validate_arguments("wait", &json!({ "await": true })).is_ok());
        assert!(validate_arguments("wait", &json!({ "await": true, "text": "x" })).is_err());
        assert!(validate_arguments("wait", &json!({ "await": "yes" })).is_err());
    }

    #[test]
    fn js_and_observe_validation() {
        assert!(validate_arguments("js", &json!({ "code": "1+1" })).is_ok());
        assert!(validate_arguments("js", &json!({ "code": "  " })).is_err());
        assert!(
            validate_arguments("js", &json!({ "code": "1", "cursor": 10, "timeout": 5 })).is_ok()
        );
        assert!(validate_arguments("js", &json!({ "code": "1", "timeout": 0 })).is_err());
        assert!(validate_arguments("js", &json!({ "code": "1", "timeout": 301 })).is_err());
        assert!(validate_arguments("observe", &json!({ "kind": "console" })).is_ok());
        assert!(validate_arguments("observe", &json!({ "kind": "dom" })).is_err());
        assert!(validate_arguments("observe", &json!({ "kind": "network", "since": -2 })).is_err());
        assert!(
            validate_arguments("observe", &json!({ "kind": "console", "level": "trace" })).is_err()
        );
        assert!(
            validate_arguments(
                "observe",
                &json!({ "kind": "network", "bodyFilter": "ok", "har": true })
            )
            .is_ok()
        );
        assert!(validate_arguments("observe", &json!({ "kind": "network", "wait": 11 })).is_err());
        assert!(
            validate_arguments("observe", &json!({ "kind": "network", "bodies": "no" })).is_err()
        );
    }

    #[test]
    fn files_validation() {
        assert!(validate_arguments("files", &json!({})).is_err());
        assert!(validate_arguments("files", &json!({ "action": "upload" })).is_err());
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "upload", "ref": "e3", "paths": ["C:/tmp/a.pdf"] })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "upload", "target": "file:上传", "paths": [] })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "upload", "paths": ["C:/tmp/a.pdf"] })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "download-dir", "dir": "C:/downloads" })
            )
            .is_ok()
        );
        // CLI `ema2 files download-dir` 不带 dir：也要能通过校验（扩展固定报错）。
        assert!(validate_arguments("files", &json!({ "action": "download-dir" })).is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn windows_upload_paths_must_be_absolute() {
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "upload", "ref": "e1", "paths": ["C:/tmp/a.pdf"] })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "files",
                &json!({ "action": "upload", "ref": "e1", "paths": ["/tmp/a.pdf"] })
            )
            .is_err()
        );
    }

    #[test]
    fn shot_validation() {
        assert!(validate_arguments("shot", &json!({})).is_ok());
        assert!(validate_arguments("shot", &json!({ "inline": true, "annotate": true })).is_ok());
        assert!(validate_arguments("shot", &json!({ "inline": "yes" })).is_err());
        assert!(validate_arguments("shot", &json!({ "annotate": 1 })).is_err());
        assert!(validate_arguments("shot", &json!({ "format": "jpeg" })).is_ok());
        assert!(validate_arguments("shot", &json!({ "format": "webp" })).is_err());
    }

    #[test]
    fn batch_validation() {
        assert!(validate_arguments("batch", &json!({})).is_err());
        assert!(validate_arguments("batch", &json!({ "tool": "read", "tabIds": [1, 2] })).is_ok());
        assert!(validate_arguments("batch", &json!({ "tool": "act", "tabIds": [1] })).is_err());
        assert!(validate_arguments("batch", &json!({ "tool": "read", "tabIds": [] })).is_err());
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "read", "tabIds": [1], "concurrency": 9 })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "read", "tabIds": [1], "args": [] })
            )
            .is_err()
        );
        // 内层参数必须复用子工具校验，不能绕过公开范围。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1], "args": { "time": 40 }
                })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1], "args": { "time": 3600 }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1], "args": { "time": -1 }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1], "args": { "time": 1, "text": "x" }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "js", "tabIds": [1], "args": {}
                })
            )
            .is_err()
        );
        // 8 tab × 300s 超过单次调用总上限：执行前拒绝。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1,2,3,4,5,6,7,8], "concurrency": 1,
                    "args": { "time": 300 }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1,2,3,4,5,6,7,8], "concurrency": 1,
                    "args": { "time": 40 }
                })
            )
            .is_ok()
        );
    }

    #[test]
    fn open_reuse_modes_are_exclusive() {
        assert!(validate_arguments("open", &json!({ "url": "https://x.y" })).is_ok());
        assert!(
            validate_arguments(
                "open",
                &json!({ "url": "https://x.y", "replace": true, "wait": "dom" })
            )
            .is_ok()
        );
        assert!(
            validate_arguments("open", &json!({ "url": "https://x.y", "reuse": "session" }))
                .is_ok()
        );
        // tabId / replace / reuse 三者互斥：同时给会让「哪个 tab 被导航」变得不可预测。
        assert!(
            validate_arguments(
                "open",
                &json!({ "url": "https://x.y", "replace": true, "tabId": 3 })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "open",
                &json!({ "url": "https://x.y", "reuse": "session", "tabId": 3 })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "open",
                &json!({ "url": "https://x.y", "replace": true, "reuse": "session" })
            )
            .is_err()
        );
        assert!(
            validate_arguments("open", &json!({ "url": "https://x.y", "reuse": "all" })).is_err()
        );
        assert!(
            validate_arguments("open", &json!({ "url": "https://x.y", "replace": "yes" })).is_err()
        );
    }

    #[test]
    fn tabs_close_scopes() {
        assert!(
            validate_arguments("tabs", &json!({ "action": "close", "scope": "session" })).is_ok()
        );
        assert!(
            validate_arguments("tabs", &json!({ "action": "close", "tabIds": [1, 2, 3] })).is_ok()
        );
        assert!(validate_arguments("tabs", &json!({ "action": "close", "tabIds": [] })).is_err());
        assert!(
            validate_arguments(
                "tabs",
                &json!({ "action": "close", "tabIds": [1, 2, 3, 4, 5, 6, 7, 8, 9] })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "tabs",
                &json!({ "action": "close", "tabId": 1, "tabIds": [2] })
            )
            .is_err()
        );
        assert!(
            validate_arguments("tabs", &json!({ "action": "close", "scope": "window" })).is_err()
        );
        // 只有 close 接受批量/会话语义，其他动作传了就是参数错。
        assert!(
            validate_arguments("tabs", &json!({ "action": "list", "scope": "session" })).is_err()
        );
        assert!(
            validate_arguments("tabs", &json!({ "action": "activate", "tabIds": [1] })).is_err()
        );
    }

    #[test]
    fn batch_urls_open_and_composite() {
        // 打开多个 URL：不需要 tabIds。
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "open", "urls": ["https://a.example/", "https://b.example/"] })
            )
            .is_ok()
        );
        // 复合：打开后读取。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "read", "urls": ["https://a.example/"],
                    "args": { "mode": "md", "budget": 8000 }
                })
            )
            .is_ok()
        );
        // open 的内层参数走 open 的校验器。
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "open", "urls": ["https://a.example/"], "args": { "wait": "dom" } })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "open", "urls": ["https://a.example/"], "args": { "wait": "later" } })
            )
            .is_err()
        );
        // 8 个慢页面顺序 open：预算跟随 waitTimeout 后仍在 600s 内。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "open",
                    "urls": ["https://a/","https://b/","https://c/","https://d/","https://e/","https://f/","https://g/","https://h/"],
                    "concurrency": 1,
                    "args": { "waitTimeout": 60 }
                })
            )
            .is_ok()
        );
        // tabIds 与 urls 不能混用；open 不能只给 tabIds。
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "open", "urls": ["https://a.example/"], "tabIds": [1] })
            )
            .is_err()
        );
        assert!(validate_arguments("batch", &json!({ "tool": "open", "tabIds": [1] })).is_err());
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "open", "urls": ["https://a.example/"], "args": { "replace": true }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "open", "urls": ["https://a.example/"], "args": { "reuse": "session" }
                })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "read", "urls": ["https://a.example/"], "reuse": "session" })
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "read", "tabIds": [1], "reuse": "session" })
            )
            .is_err()
        );
        assert!(
            validate_arguments(
                "batch",
                &json!({ "tool": "read", "urls": ["file:///etc/passwd"] })
            )
            .is_err()
        );
        assert!(validate_arguments("batch", &json!({ "tool": "read", "urls": [] })).is_err());
        assert!(validate_arguments("batch", &json!({ "tool": "read" })).is_err());
        // 复合模式仍然受内层工具上限约束。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "read", "urls": ["https://a.example/"],
                    "args": { "budget": 100 }
                })
            )
            .is_err()
        );
        // 8 个 URL 复合读取：open 成本计入预算，仍在 600s 内。
        assert!(
            validate_arguments(
                "batch",
                &json!({
                    "tool": "read",
                    "urls": ["https://a/","https://b/","https://c/","https://d/","https://e/","https://f/","https://g/","https://h/"],
                    "concurrency": 2
                })
            )
            .is_ok()
        );
    }

    #[test]
    fn timeout_budget() {
        assert_eq!(
            timeout_for("wait", &json!({ "text": "x" })),
            Duration::from_secs(45)
        );
        assert_eq!(
            timeout_for("wait", &json!({ "text": "x", "timeout": 60 })),
            Duration::from_secs(75)
        );
        assert_eq!(
            timeout_for("wait", &json!({ "time": 120 })),
            Duration::from_secs(135)
        );
        assert_eq!(
            timeout_for("wait", &json!({ "time": 0 })),
            Duration::from_secs(15)
        );
        assert_eq!(
            timeout_for("act", &json!({})),
            ACT_TIMEOUT + Duration::from_secs(3)
        );
        assert_eq!(
            timeout_for("act", &json!({ "waitFor": { "timeout": 10 } })),
            ACT_TIMEOUT + Duration::from_secs(13)
        );
        assert_eq!(timeout_for("open", &json!({})), Duration::from_secs(30));
        assert_eq!(
            timeout_for("open", &json!({ "waitTimeout": 45 })),
            Duration::from_secs(60)
        );
        assert_eq!(
            timeout_for("open", &json!({ "wait": "none", "waitTimeout": 60 })),
            DEFAULT_TIMEOUT
        );
        assert_eq!(timeout_for("js", &json!({})), Duration::from_secs(25));
        assert_eq!(
            timeout_for("js", &json!({ "code": "1", "timeout": 30 })),
            Duration::from_secs(45)
        );
        assert_eq!(timeout_for("shot", &json!({})), LONG_TIMEOUT);
        assert_eq!(timeout_for("batch", &json!({})), LONG_TIMEOUT);
        // batch open 的单次预算必须跟随内层 waitTimeout，而不是固定 30s。
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "open", "urls": ["https://a/"],
                    "args": { "waitTimeout": 60 }
                })
            ),
            Duration::from_secs(87)
        );
        assert_eq!(
            timeout_for(
                "batch",
                &json!({ "tool": "wait", "tabIds": [1, 2], "args": { "time": 40 } })
            ),
            Duration::from_secs(55)
        );
        // 8 tab / 并发 1/2/4：波次 × 单次 40s + 15s 余量。
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1,2,3,4,5,6,7,8], "concurrency": 1,
                    "args": { "time": 40 }
                })
            ),
            Duration::from_secs(335)
        );
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1,2,3,4,5,6,7,8], "concurrency": 2,
                    "args": { "time": 40 }
                })
            ),
            Duration::from_secs(175)
        );
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "wait", "tabIds": [1,2,3,4,5,6,7,8], "concurrency": 4,
                    "args": { "time": 40 }
                })
            ),
            Duration::from_secs(95)
        );
        // js 受全局 debugger 串行队列限制：声明并发 4 时仍按 1 计算波次；
        // 单次预算跟随 args.timeout（默认 10s + 8s）。
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "js", "tabIds": [1, 2], "concurrency": 4,
                    "args": { "code": "1+1" }
                })
            ),
            Duration::from_secs(51)
        );
        assert_eq!(timeout_for("tabs", &json!({})), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout_for("wait", &json!({ "await": true, "timeout": 10 })),
            Duration::from_secs(25)
        );
        assert_eq!(
            timeout_for(
                "act",
                &json!({
                    "steps": [
                        { "action": "click", "ref": "e1" },
                        { "action": "await", "timeout": 10 },
                        { "action": "waitFor", "url": "api", "timeout": 5 }
                    ]
                })
            ),
            ACT_TIMEOUT + Duration::from_secs(24)
        );
    }
    /// 错误码表是 AI 自我纠正的依据；新增 `fail('code'` 时文档必须同步。
    #[test]
    fn documented_error_codes_cover_extension_codes() {
        let js = include_str!("../../extension/background.js");
        let docs = include_str!("../../docs/TOOLS.md");
        let mut codes: Vec<String> = Vec::new();
        for marker in ["fail('", "code: '"] {
            let mut rest = js;
            while let Some(index) = rest.find(marker) {
                rest = &rest[index + marker.len()..];
                let end = rest.find("'").unwrap_or(0);
                let code = &rest[..end];
                let looks_like_error_code =
                    !code.is_empty() && code.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_');
                if looks_like_error_code && !codes.iter().any(|known| known == code) {
                    codes.push(code.to_owned());
                }
            }
        }
        assert!(
            codes.len() > 20,
            "expected to find fail('code') calls, found {codes:?}"
        );
        let missing: Vec<&String> = codes
            .iter()
            .filter(|code| !docs.contains(&format!("`{code}`")))
            .collect();
        assert!(missing.is_empty(), "docs/TOOLS.md 缺少错误码: {missing:?}");
    }
}
