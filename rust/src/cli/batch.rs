//! `ema2 batch`：顶层 `--tab-ids/--urls/--concurrency/--port` 与内层目标工具 flag 解析。

use serde_json::{Value, json};

use super::ToolCall;
use super::shared::{TokenCursor, parse_duration_secs, parse_port, split_flag};

/// `ema2 batch` 的全部 token（`<tool>` 之后）都由这里解析：
/// 顶层识别 `--tab-ids/--urls/--concurrency/--port`，其余交给目标工具的
/// `parse_tool_flags`，最终放进 `args`（扩展的 batch 只从 `args` 读内层参数）。
pub fn build_batch(tokens: &[String]) -> Result<ToolCall, String> {
    let mut cursor = TokenCursor::new(tokens);
    let Some(tool) = cursor.peek() else {
        return Err("batch: missing tool (open|read|snapshot|js|wait)".into());
    };
    if !matches!(tool, "open" | "read" | "snapshot" | "js" | "wait") {
        return Err(format!(
            "batch: tool must be open|read|snapshot|js|wait, got {tool:?}"
        ));
    }
    cursor.advance();

    let mut tab_ids: Option<String> = None;
    // `--urls` 可重复：URL 自带逗号时，逗号拆分无法表达，只能分多次传入。
    let mut urls: Vec<String> = Vec::new();
    let mut reuse: Option<String> = None;
    let mut concurrency: Option<u32> = None;
    let mut port_override: Option<u16> = None;
    let mut flags: Vec<String> = Vec::new();
    while let Some(token) = cursor.peek() {
        let (flag, inline) = split_flag(token);
        match flag {
            "--tab-ids" | "--urls" | "--reuse" | "--concurrency" | "--port" => {
                let value = cursor.take_flag_value(inline, "batch", flag)?;
                match flag {
                    "--tab-ids" => tab_ids = Some(value),
                    "--urls" => urls.push(value),
                    "--reuse" => reuse = Some(value),
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
                        port_override = Some(parse_port(&value).map_err(|error| {
                            format!("batch: invalid --port {value:?}: {error}")
                        })?);
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
                            | "--file"
                            | "--text"
                            | "--text-gone"
                            | "--time"
                            | "--timeout"
                            | "--wait"
                            | "--reuse"
                    )
                {
                    if let Some(value) = cursor.peek_at(1) {
                        flags.push(value.to_owned());
                        cursor.advance();
                    }
                }
                cursor.advance();
            }
        }
    }

    if tab_ids.is_some() && !urls.is_empty() {
        return Err("batch: --tab-ids and --urls are mutually exclusive".into());
    }
    if reuse.is_some() && urls.is_empty() {
        return Err("batch: --reuse session only applies to --urls".into());
    }
    if tab_ids.is_none() && urls.is_empty() {
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
    if !urls.is_empty() {
        let mut parsed: Vec<&str> = Vec::new();
        for list in &urls {
            for part in list
                .split(',')
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                let lower = part.to_ascii_lowercase();
                if !lower.starts_with("http://") && !lower.starts_with("https://") {
                    return Err(format!(
                        "batch: --urls entry {part:?} is not http(s); commas are used as separators — pass a URL that contains a comma with a separate --urls flag"
                    ));
                }
                parsed.push(part);
            }
        }
        if parsed.is_empty() || parsed.len() > 8 {
            return Err("batch: --urls must contain 1..8 urls".into());
        }
        arguments["urls"] = json!(parsed);
    }
    if let Some(reuse) = reuse {
        if reuse != "session" {
            return Err(format!(
                "batch: --reuse only accepts \"session\", got {reuse:?}"
            ));
        }
        arguments["reuse"] = json!("session");
    }
    if let Some(concurrency) = concurrency {
        arguments["concurrency"] = json!(concurrency);
    }
    let mut call = ToolCall::new("batch", arguments);
    call.port = port_override;
    Ok(call)
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
    let mut cursor = TokenCursor::new(flags);
    // `batch js` 的 code 只能有一个来源：内联（--code/位置参数）或 --file。
    let mut js_source: Option<&'static str> = None;
    while let Some(token) = cursor.peek() {
        let (flag, inline) = split_flag(token);
        // 宏而不是闭包：闭包若捕获 &mut cursor，分支里的 cursor.advance() 会冲突。
        macro_rules! value {
            () => {
                cursor.take_flag_value(inline.clone(), "batch", flag)?
            };
        }
        match tool {
            "open" => match flag {
                "--wait" => out["wait"] = json!(value!()),
                "--timeout" => {
                    let raw = value!();
                    let seconds =
                        parse_duration_secs(&raw).map_err(|error| format!("batch: {error}"))?;
                    out["waitTimeout"] = json!(seconds);
                }
                "--replace" => {
                    out["replace"] = json!(true);
                    cursor.advance();
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
                    cursor.advance();
                }
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "snapshot" => match flag {
                "--filter" => out["filter"] = json!(value!()),
                "--near" => out["near"] = json!(value!()),
                "--budget" => out["budget"] = numeric_flag("batch", flag, value!())?,
                "--cursor" => out["cursor"] = numeric_flag("batch", flag, value!())?,
                "--values" => {
                    out["values"] = json!(true);
                    cursor.advance();
                }
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            "js" => match flag {
                "--code" => {
                    if js_source == Some("file") {
                        return Err("batch js: --code and --file are mutually exclusive".into());
                    }
                    js_source = Some("code");
                    out["code"] = json!(value!());
                }
                // 与 `js --file` 对齐：文件在客户端只读一次，再分发给每个标签。
                // 线上载荷仍是纯 code，扩展侧不需要知道来源是文件还是内联。
                "--file" => {
                    if js_source == Some("code") {
                        return Err("batch js: --code and --file are mutually exclusive".into());
                    }
                    js_source = Some("file");
                    let path = value!();
                    let text = std::fs::read_to_string(&path)
                        .map_err(|error| format!("batch js: cannot read {path}: {error}"))?;
                    if text.trim().is_empty() {
                        return Err(format!("batch js: {path} is empty"));
                    }
                    out["code"] = json!(text);
                }
                "--budget" => out["budget"] = numeric_flag("batch", flag, value!())?,
                "--cursor" => out["cursor"] = numeric_flag("batch", flag, value!())?,
                "--timeout" => {
                    let raw = value!();
                    let seconds =
                        parse_duration_secs(&raw).map_err(|error| format!("batch: {error}"))?;
                    out["timeout"] = json!(seconds);
                }
                // 兼容 `batch js --urls u 'return document.title'`：
                // 位置参数就是 code，不再报 unknown flag。
                _ if !flag.starts_with('-') => {
                    if js_source == Some("file") {
                        return Err(
                            "batch js: positional code and --file are mutually exclusive".into(),
                        );
                    }
                    js_source = Some("code");
                    out["code"] = json!(flag);
                    cursor.advance();
                }
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
                    cursor.advance();
                }
                // 位置参数 TEXT 等价于 --text。
                _ if !flag.starts_with('-') => {
                    out["text"] = json!(flag);
                    cursor.advance();
                }
                _ => return Err(format!("batch: unknown flag {flag:?} for tool {tool}")),
            },
            _ => return Err(format!("batch: unsupported tool {tool:?}")),
        }
    }
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(call: &ToolCall) -> &Value {
        &call.args
    }

    #[test]
    fn batch_forms() {
        let call = build_batch(&[
            "read".into(),
            "--urls".into(),
            "https://a.com/,https://b.com/".into(),
            "--reuse".into(),
            "session".into(),
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
        assert_eq!(args_of(&call)["reuse"], "session");
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

        // batch 的位置参数：js 的 code / wait 的 text 都允许直接写。
        let call = build_batch(&[
            "js".into(),
            "--urls".into(),
            "https://a.example/".into(),
            "return document.title".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["args"]["code"], "return document.title");
        let call =
            build_batch(&["wait".into(), "--tab-ids".into(), "1".into(), "Done".into()]).unwrap();
        assert_eq!(args_of(&call)["args"]["text"], "Done");
        let call = build_batch(&[
            "js".into(),
            "--tab-ids".into(),
            "1".into(),
            "--code".into(),
            "return 1".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["args"]["code"], "return 1");

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
                "--reuse".into(),
                "session".into(),
            ])
            .is_err()
        );
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
    fn batch_urls_can_repeat_and_explain_comma_splitting() {
        // 重复 --urls 追加而不是覆盖：URL 自带逗号时只能这样传。
        let call = build_batch(&[
            "read".into(),
            "--urls".into(),
            "https://a.example/".into(),
            "--urls".into(),
            "https://b.example/".into(),
        ])
        .unwrap();
        assert_eq!(
            args_of(&call)["urls"],
            json!(["https://a.example/", "https://b.example/"])
        );

        // 兼容逗号拆分；拆出的段不是 http(s) 时，错误必须点明逗号是分隔符。
        let call = build_batch(&[
            "read".into(),
            "--urls".into(),
            "https://a.example/,https://b.example/".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["urls"].as_array().unwrap().len(), 2);

        let error = build_batch(&[
            "read".into(),
            "--urls".into(),
            "https://a.example/,https://b.example/?x=1,2".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("commas are used as separators"), "{error}");
        assert!(error.contains("separate --urls flag"), "{error}");
    }

    #[test]
    fn batch_js_accepts_file() {
        let path = std::env::temp_dir().join(format!(
            "ema2-batch-js-{}-{}.js",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, "return document.title").unwrap();
        let call = build_batch(&[
            "js".into(),
            "--tab-ids".into(),
            "1".into(),
            "--file".into(),
            path.display().to_string(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["args"]["code"], "return document.title");
        assert!(
            args_of(&call)["args"].get("file").is_none(),
            "the file path must not leak into the wire payload"
        );

        // 与 `--code` / 位置参数互斥，且错误要点明是这两个 flag 冲突。
        let error = build_batch(&[
            "js".into(),
            "--tab-ids".into(),
            "1".into(),
            "--file".into(),
            path.display().to_string(),
            "--code".into(),
            "return 1".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("mutually exclusive"), "{error}");

        // 空文件必须报错，不能把空 code 发给每个标签。
        std::fs::write(&path, "  \n").unwrap();
        assert!(
            build_batch(&[
                "js".into(),
                "--tab-ids".into(),
                "1".into(),
                "--file".into(),
                path.display().to_string(),
            ])
            .is_err()
        );
        let _ = std::fs::remove_file(&path);
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

        assert!(
            build_batch(&["read".into(), "--tab-ids=1".into(), "--port=0".into()]).is_err(),
            "port 0 would bind a random port and break status/stop"
        );
    }
}
