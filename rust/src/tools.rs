//! v1.7 工具注册表（11 个粗粒度工具，面向 AI）。
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
pub const LONG_TIMEOUT: Duration = Duration::from_secs(90);
pub const PAGE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BUDGET: u64 = 200_000;

pub static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        Tool {
            name: "tabs",
            description: "Tabs: action=list|current|new|close|activate. list = TSV id/active/title/url",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "current", "new", "close", "activate"] },
                    "tabId": { "type": "number" },
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
                    "url": { "type": "string" },
                    "tabId": { "type": "number" },
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
                    "tabId": { "type": "number" },
                    "mode": { "type": "string", "enum": ["md", "text", "html"] },
                    "selector": { "type": "string" },
                    "main": { "type": "boolean" },
                    "budget": { "type": "number" },
                    "cursor": { "type": "number" }
                }
            }),
        },
        Tool {
            name: "snapshot",
            description: "Ref snapshot `[e12] button \"X\"`. filter/ near=<ref> scope; refs go stale after navigation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "number" },
                    "filter": { "type": "string", "enum": ["interactive", "full", "links", "buttons", "form"] },
                    "near": { "type": "string" },
                    "budget": { "type": "number" },
                    "cursor": { "type": "number" }
                }
            }),
        },
        Tool {
            name: "act",
            description: "click|type|fill|select|press|hover|scroll|submit|back|forward|reload by ref/selector/target; steps[] batch; waitFor response.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "number" },
                    "action": { "type": "string", "enum": ["click", "type", "fill", "select", "press", "hover", "scroll", "submit", "back", "forward", "reload"] },
                    "ref": { "type": "string" },
                    "selector": { "type": "string" },
                    "target": { "type": "string" },
                    "text": { "type": "string" },
                    "value": { "type": "string" },
                    "key": { "type": "string" },
                    "deltaY": { "type": "number" },
                    "fields": { "type": "array", "items": { "type": "object" } },
                    "steps": { "type": "array", "items": { "type": "object" } },
                    "waitFor": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string" },
                            "status": { "type": "number" },
                            "timeout": { "type": "number" }
                        }
                    }
                }
            }),
        },
        Tool {
            name: "wait",
            description: "Wait for one of text|textGone|selector|time|idle (max 300s).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tabId": { "type": "number" },
                    "text": { "type": "string" },
                    "textGone": { "type": "string" },
                    "selector": { "type": "string" },
                    "time": { "type": "number" },
                    "idle": { "type": "boolean" },
                    "timeout": { "type": "number" }
                }
            }),
        },
        Tool {
            name: "js",
            description: "Eval JS in tab (await ok); compact value, budget-capped.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "tabId": { "type": "number" },
                    "budget": { "type": "number" }
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
                    "tabId": { "type": "number" },
                    "since": { "type": "number" },
                    "level": { "type": "string", "enum": ["log", "warn", "error"] },
                    "filter": { "type": "string" },
                    "bodyFilter": { "type": "string" },
                    "wait": { "type": "number" },
                    "bodies": { "type": "boolean" },
                    "har": { "type": "boolean" },
                    "budget": { "type": "number" }
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
                    "tabId": { "type": "number" },
                    "ref": { "type": "string" },
                    "selector": { "type": "string" },
                    "target": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "dir": { "type": "string" }
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
                    "tabId": { "type": "number" },
                    "inline": { "type": "boolean" },
                    "annotate": { "type": "boolean" }
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
                    "tabIds": { "type": "array", "items": { "type": "number" } },
                    "args": { "type": "object" },
                    "concurrency": { "type": "number" }
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
        "wait" => {
            let secs = arguments
                .get("time")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 300.0);
            if secs > 0.0 {
                Duration::from_secs_f64(secs + 10.0)
            } else {
                PAGE_WAIT_TIMEOUT + Duration::from_secs(15)
            }
        }
        "act" => {
            let wait = arguments
                .pointer("/waitFor/timeout")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 30.0);
            ACT_TIMEOUT + Duration::from_secs_f64(wait)
        }
        "js" | "shot" | "files" => LONG_TIMEOUT,
        "batch" => {
            // batch 里跑 wait(time=N) 时，预算要覆盖 N 秒
            if arguments.get("tool").and_then(Value::as_str) == Some("wait") {
                let secs = arguments
                    .pointer("/args/time")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .clamp(0.0, 300.0);
                if secs > 0.0 {
                    return Duration::from_secs_f64(secs + 15.0);
                }
            }
            LONG_TIMEOUT
        }
        _ => DEFAULT_TIMEOUT,
    }
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
                && !(1.0..=4.0).contains(&concurrency)
            {
                return Err("concurrency must be between 1 and 4".into());
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
            schema_chars < 5_000,
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
    }

    #[test]
    fn timeout_budget() {
        assert_eq!(
            timeout_for("wait", &json!({ "text": "x" })),
            PAGE_WAIT_TIMEOUT + Duration::from_secs(15)
        );
        assert_eq!(
            timeout_for("wait", &json!({ "time": 120 })),
            Duration::from_secs(130)
        );
        assert_eq!(timeout_for("act", &json!({})), ACT_TIMEOUT);
        assert_eq!(
            timeout_for("act", &json!({ "waitFor": { "timeout": 10 } })),
            ACT_TIMEOUT + Duration::from_secs(10)
        );
        assert_eq!(timeout_for("js", &json!({})), LONG_TIMEOUT);
        assert_eq!(timeout_for("batch", &json!({})), LONG_TIMEOUT);
        assert_eq!(
            timeout_for("batch", &json!({ "tool": "wait", "args": { "time": 40 } })),
            Duration::from_secs(55)
        );
        assert_eq!(timeout_for("tabs", &json!({})), DEFAULT_TIMEOUT);
    }
}
