//! v1.8 工具注册表（11 个粗粒度工具，面向 AI）。
//!
//! 约定：
//! - 输出行式紧凑文本（`key=value` / TSV / `[ref]`），不 pretty-print JSON
//! - 列表带 `total / shown / next`
//! - 错误统一 `error=<code> msg=... hint=...`
//! - schema 只在这里定义一次（工具列表 / 白名单 / 校验 / 超时同源）

use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{Map, Value, json};

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const ACT_TIMEOUT: Duration = Duration::from_secs(60);
pub const LONG_TIMEOUT: Duration = Duration::from_secs(120);
pub const PAGE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次工具调用的产品总时长上限；超出时在调用前明确拒绝，而不是让中继伪装成超时。
pub const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(600);
const RELAY_MARGIN_SECS: f64 = 15.0;
/// 扩展侧 debugger 任务本身的执行上限（全局串行队列）。
const DEBUGGER_TASK_SECS: f64 = 60.0;
const MAX_BUDGET: u64 = 200_000;
const MAX_WAIT_SECS: f64 = 300.0;

pub static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        Tool {
            name: "tabs",
            description: "Tabs: action=list|current|new|close|activate. list = TSV id/active/title/url",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "current", "new", "close", "activate"] },
                    "tabId": { "type": "integer", "minimum": 0 },
                    "url": { "type": "string" },
                    "active": { "type": "boolean" }
                }
            }),
        },
        Tool {
            name: "open",
            description: "Open url in new tab, or navigate tabId. Returns id/url/title/net_hook.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "maxLength": 8192 },
                    "tabId": { "type": "integer", "minimum": 0 },
                    "wait": { "type": "string", "enum": ["load", "dom", "none"] }
                },
                "required": ["url"]
            }),
        },
        Tool {
            name: "read",
            description: "Read page as md|text|html. main=true (default) extracts main content; selector/budget/cursor supported.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "integer", "minimum": 0 },
                    "mode": { "type": "string", "enum": ["md", "text", "html"] },
                    "selector": { "type": "string", "maxLength": 2048 },
                    "main": { "type": "boolean" },
                    "budget": { "type": "integer", "minimum": 200, "maximum": 200000 },
                    "cursor": { "type": "integer", "minimum": 0 }
                }
            }),
        },
        Tool {
            name: "snapshot",
            description: "Ref snapshot `[e12@doc] button \"X\"`. filter/ near=<ref> scope; refs are document-scoped and go stale after navigation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "integer", "minimum": 0 },
                    "filter": { "type": "string", "enum": ["interactive", "full", "links", "buttons", "form"] },
                    "near": { "type": "string", "maxLength": 32 },
                    "budget": { "type": "integer", "minimum": 200, "maximum": 200000 },
                    "cursor": { "type": "integer", "minimum": 0 }
                }
            }),
        },
        Tool {
            name: "act",
            description: "click|type|fill|select|press|hover|scroll|submit|back|forward|reload by ref/selector/target; steps[] batch; waitFor response.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "integer", "minimum": 0 },
                    "action": { "type": "string", "enum": ["click", "type", "fill", "select", "press", "hover", "scroll", "submit", "back", "forward", "reload"] },
                    "ref": { "type": "string", "maxLength": 32 },
                    "selector": { "type": "string", "maxLength": 2048 },
                    "target": { "type": "string" },
                    "text": { "type": "string", "maxLength": 100000 },
                    "value": { "type": "string", "maxLength": 100000 },
                    "key": { "type": "string", "maxLength": 100 },
                    "deltaY": { "type": "number", "minimum": -100000, "maximum": 100000 },
                    "submit": { "type": "boolean" },
                    "fields": {
                        "type": "array", "minItems": 1, "maxItems": 100,
                        "items": {
                            "type": "object",
                            "properties": {
                                "ref": { "type": "string", "maxLength": 32 },
                                "selector": { "type": "string", "maxLength": 2048 },
                                "target": { "type": "string", "maxLength": 200 },
                                "value": { "type": "string", "maxLength": 100000 }
                            },
                            "required": ["value"]
                        }
                    },
                    "steps": {
                        "type": "array", "minItems": 1, "maxItems": 20,
                        "items": { "type": "object", "required": ["action"] }
                    },
                    "waitFor": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "maxLength": 2048 },
                            "status": { "type": "integer", "minimum": 100, "maximum": 599 },
                            "timeout": { "type": "number", "minimum": 0, "maximum": 30 }
                        }
                    }
                },
                "anyOf": [{ "required": ["action"] }, { "required": ["steps"] }]
            }),
        },
        Tool {
            name: "wait",
            description: "Wait for one of text|textGone|selector|time|idle (max 300s).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "integer", "minimum": 0 },
                    "text": { "type": "string", "maxLength": 100000 },
                    "textGone": { "type": "string" },
                    "selector": { "type": "string", "maxLength": 2048 },
                    "time": { "type": "number", "minimum": 0, "maximum": 300 },
                    "idle": { "type": "boolean" },
                    "timeout": { "type": "number", "minimum": 0, "maximum": 300 }
                }
            }),
        },
        Tool {
            name: "js",
            description: "Eval JS in tab (await ok); compact value, budget-capped.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "maxLength": 100000 },
                    "tabId": { "type": "integer", "minimum": 0 },
                    "budget": { "type": "integer", "minimum": 200, "maximum": 200000 }
                },
                "required": ["code"]
            }),
        },
        Tool {
            name: "observe",
            description: "console deltas or network (fetch/XHR with real status+body). since=last next; bodyFilter regex; har=true exports HAR.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["console", "network"] },
                    "tabId": { "type": "integer", "minimum": 0 },
                    "since": { "type": "integer", "minimum": 0 },
                    "level": { "type": "string", "enum": ["log", "warn", "error"] },
                    "filter": { "type": "string", "maxLength": 2048 },
                    "bodyFilter": { "type": "string", "maxLength": 512 },
                    "wait": { "type": "number", "minimum": 0, "maximum": 10 },
                    "bodies": { "type": "boolean" },
                    "har": { "type": "boolean" },
                    "budget": { "type": "integer", "minimum": 200, "maximum": 200000 }
                },
                "required": ["kind"]
            }),
        },
        Tool {
            name: "files",
            description: "Upload local files into a file input, or set the download directory.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["upload", "download-dir"] },
                    "tabId": { "type": "integer", "minimum": 0 },
                    "ref": { "type": "string", "maxLength": 32 },
                    "selector": { "type": "string", "maxLength": 2048 },
                    "target": { "type": "string", "maxLength": 200 },
                    "paths": { "type": "array", "minItems": 1, "maxItems": 20, "items": { "type": "string" } },
                    "dir": { "type": "string", "maxLength": 4096 }
                },
                "required": ["action"]
            }),
        },
        Tool {
            name: "shot",
            description: "Screenshot: default saves PNG and returns path; inline=true returns image; annotate=true adds ref boxes and a coordinate list.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "integer", "minimum": 0 },
                    "inline": { "type": "boolean" },
                    "annotate": { "type": "boolean" },
                    "format": { "type": "string", "enum": ["png", "jpeg"] }
                }
            }),
        },
        Tool {
            name: "batch",
            description: "Run read|snapshot|js|wait on several tabs concurrently. Returns one block per tab.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool": { "type": "string", "enum": ["read", "snapshot", "js", "wait"] },
                    "tabIds": {
                        "type": "array", "minItems": 1, "maxItems": 8,
                        "items": { "type": "integer", "minimum": 0 }
                    },
                    "args": { "type": "object" },
                    "concurrency": { "type": "integer", "minimum": 1, "maximum": 4 }
                },
                "required": ["tool", "tabIds"]
            }),
        },
    ]
});

pub fn tool_list() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect(),
    )
}

pub fn contains(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// 中继等待扩展响应的超时（必须大于扩展侧自身的等待上限）。
pub fn timeout_for(name: &str, arguments: &Value) -> Duration {
    match name {
        "wait" => Duration::from_secs_f64(wait_seconds(arguments) + RELAY_MARGIN_SECS),
        "act" => {
            let wait = arguments
                .pointer("/waitFor/timeout")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 30.0);
            ACT_TIMEOUT + Duration::from_secs_f64(wait)
        }
        "js" | "shot" | "files" => LONG_TIMEOUT,
        "batch" => Duration::from_secs_f64(
            batch_budget_seconds(arguments).min(MAX_CALL_TIMEOUT.as_secs_f64()),
        ),
        _ => DEFAULT_TIMEOUT,
    }
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

fn batch_tab_count(arguments: &Value) -> usize {
    arguments
        .get("tabIds")
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
    let per_call = match tool {
        "wait" => wait_seconds(arguments.get("args").unwrap_or(&Value::Null)),
        "js" => DEBUGGER_TASK_SECS,
        _ => DEFAULT_TIMEOUT.as_secs_f64(),
    };
    waves * per_call + RELAY_MARGIN_SECS
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
            if object.get("url").is_some_and(|v| !v.is_null()) {
                url_arg(object)?;
            }
            bool_arg(object, "active")?;
        }
        "open" => {
            url_arg(object)?;
            tab_id(object)?;
            enum_arg(object, "wait", &["load", "dom", "none"], false)?;
        }
        "read" => {
            tab_id(object)?;
            enum_arg(object, "mode", &["md", "text", "html"], false)?;
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
            let conditions = [has_text, has_gone, has_selector, time.is_some(), idle]
                .iter()
                .filter(|flag| **flag)
                .count();
            if conditions != 1 {
                return Err(
                    "exactly one of text, textGone, selector, time, idle is required".into(),
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
        }
        "observe" => {
            enum_arg(object, "kind", &["console", "network"], true)?;
            tab_id(object)?;
            if let Some(since) = number_arg(object, "since", false)?
                && since < 0.0
            {
                return Err("since must be >= 0".into());
            }
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
            } else {
                string_arg(object, "dir", 4_096, true, false)?;
            }
        }
        "shot" => {
            tab_id(object)?;
            bool_arg(object, "inline")?;
            bool_arg(object, "annotate")?;
        }
        "batch" => {
            enum_arg(object, "tool", &["read", "snapshot", "js", "wait"], true)?;
            let Some(tab_ids) = object.get("tabIds").and_then(Value::as_array) else {
                return Err("tabIds must be an array".into());
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
                let tool = object
                    .get("tool")
                    .and_then(Value::as_str)
                    .expect("required above");
                let mut merged = inner.clone();
                merged.insert("tabId".into(), json!(0));
                validate_arguments(tool, &Value::Object(merged))
                    .map_err(|error| format!("args: {error}"))?;
            }
            let budget = batch_budget_seconds(&Value::Object(object.clone()));
            if budget > MAX_CALL_TIMEOUT.as_secs_f64() {
                return Err(format!(
                    "batch estimated budget {:.0}s exceeds the {}s call limit; reduce tabIds, raise concurrency, or split the batch",
                    budget,
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
            "forward", "reload",
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
    if action == "press"
        && !object
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.trim().is_empty())
    {
        return Err("key is required for press".into());
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

fn cursor_arg(object: &Map<String, Value>) -> Result<(), String> {
    if let Some(cursor) = number_arg(object, "cursor", false)?
        && (cursor < 0.0 || cursor.fract() != 0.0)
    {
        return Err("cursor must be a non-negative integer".into());
    }
    Ok(())
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
        assert_eq!(TOOLS.len(), 11);
        assert_eq!(tool_list().as_array().unwrap().len(), 11);
        let schema_chars = tool_list().to_string().len();
        assert!(
            schema_chars < 6_000,
            "tool schema grew to {schema_chars} chars; keep it compact"
        );
        for tool in TOOLS.iter() {
            assert!(
                !tool.description.is_empty(),
                "{} has no description",
                tool.name
            );
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        }
    }

    #[test]
    fn every_tool_is_reachable() {
        for tool in TOOLS.iter() {
            if let Err(error) = validate_arguments(tool.name, &json!({})) {
                assert!(
                    !error.contains("unknown tool"),
                    "{} is not handled",
                    tool.name
                );
            }
            assert!(contains(tool.name));
        }
        assert!(!contains("edge_get_tabs"));
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
        assert!(validate_arguments("read", &json!({ "budget": 100 })).is_err());
        assert!(validate_arguments("read", &json!({ "cursor": -1 })).is_err());
        assert!(validate_arguments("read", &json!({ "main": false })).is_ok());
        assert!(validate_arguments("read", &json!({ "main": "yes" })).is_err());
        assert!(validate_arguments("snapshot", &json!({ "filter": "all" })).is_err());
        assert!(validate_arguments("snapshot", &json!({ "filter": "links" })).is_ok());
        assert!(validate_arguments("snapshot", &json!({ "filter": "form", "near": "e3" })).is_ok());
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
    }

    #[test]
    fn js_and_observe_validation() {
        assert!(validate_arguments("js", &json!({ "code": "1+1" })).is_ok());
        assert!(validate_arguments("js", &json!({ "code": "  " })).is_err());
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
        assert_eq!(timeout_for("act", &json!({})), ACT_TIMEOUT);
        assert_eq!(
            timeout_for("act", &json!({ "waitFor": { "timeout": 10 } })),
            ACT_TIMEOUT + Duration::from_secs(10)
        );
        assert_eq!(timeout_for("js", &json!({})), LONG_TIMEOUT);
        assert_eq!(timeout_for("shot", &json!({})), LONG_TIMEOUT);
        assert_eq!(timeout_for("batch", &json!({})), LONG_TIMEOUT);
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
        // js 受全局 debugger 串行队列限制：声明并发 4 时仍按 1 计算波次。
        assert_eq!(
            timeout_for(
                "batch",
                &json!({
                    "tool": "js", "tabIds": [1, 2], "concurrency": 4,
                    "args": { "code": "1+1" }
                })
            ),
            Duration::from_secs(135)
        );
        assert_eq!(timeout_for("tabs", &json!({})), DEFAULT_TIMEOUT);
    }
}
