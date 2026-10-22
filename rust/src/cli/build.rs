//! 普通命令（tabs/open/read/snapshot/wait/await/js/observe/files/shot）→ `ToolCall`。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::ToolCall;
use super::shared::{MAX_UPLOAD_BYTES, base64_encode, guess_mime, parse_duration_secs, set_opt};
use super::spec::{FilesArgs, FilesCommand, TabsArgs, TabsCommand};

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
        Some(TabsCommand::New {
            url,
            active,
            background,
        }) => json!({
            "action": "new",
            "url": url,
            // 默认激活：AI 执行 `tabs new` 后紧接着 snapshot/act 时，
            // 不应默默操作旧标签。只有 --background 才保持后台。
            "active": *active || !*background,
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
    values: bool,
) -> ToolCall {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "filter", filter.clone());
    set_opt(&mut arguments, "near", near.clone());
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    if values {
        arguments["values"] = json!(true);
    }
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
            if let Some(value) = value {
                // `wait idle 10s` 里的位置参数就是兜底超时，不能静默丢弃。
                if timeout.is_some() {
                    return Err(
                        "wait idle: use either a positional duration or --timeout, not both".into(),
                    );
                }
                let seconds =
                    parse_duration_secs(value).map_err(|error| format!("wait idle: {error}"))?;
                if !(0.0..=300.0).contains(&seconds) {
                    return Err("wait idle: duration must be between 0 and 300 seconds".into());
                }
                arguments["timeout"] = json!(seconds);
            }
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
    cursor: Option<u64>,
    timeout: Option<f64>,
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
    if let Some(seconds) = timeout
        && !(0.1..=300.0).contains(&seconds)
    {
        return Err(format!(
            "js: --timeout must be 0.1..300 seconds, got {seconds}"
        ));
    }
    let mut arguments = json!({ "code": source });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    set_opt(&mut arguments, "timeout", timeout);
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
    headers: &str,
    budget: Option<u64>,
) -> Result<ToolCall, String> {
    if !matches!(kind, "console" | "network") {
        return Err(format!(
            "observe: kind must be console|network, got {kind:?}"
        ));
    }
    if !matches!(headers, "redact" | "all") {
        return Err(format!(
            "observe --headers must be redact|all, got {headers:?}"
        ));
    }
    if headers == "all" && kind != "network" {
        return Err("observe --headers only applies to network".into());
    }
    let mut arguments = json!({ "kind": kind });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "since", since);
    set_opt(&mut arguments, "level", level.clone());
    set_opt(&mut arguments, "filter", filter.clone());
    set_opt(&mut arguments, "bodyFilter", body_filter.clone());
    set_opt(&mut arguments, "bodies", bodies);
    set_opt(&mut arguments, "budget", budget);
    if headers == "all" {
        arguments["harHeaders"] = json!("all");
    }
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
            let mut payload: Vec<Value> = Vec::new();
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
                    "data": base64_encode(&bytes),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(call: &ToolCall) -> &Value {
        &call.args
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

        let call = build_wait("idle", &Some("10s".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["idle"], true);
        assert_eq!(args_of(&call)["timeout"], 10.0);
        // 位置参数与 --timeout 不能同时静默覆盖。
        assert!(build_wait("idle", &Some("10s".into()), &Some("20s".into()), None).is_err());

        let call = build_wait("textGone", &Some("Loading".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["textGone"], "Loading");

        assert!(build_wait("bogus", &None, &None, None).is_err());
        assert!(build_wait("text", &None, &None, None).is_err());
    }

    #[test]
    fn read_visible_mode_passes_through_with_the_same_root_flags() {
        let call = build_read(
            Some(7),
            &Some("visible".into()),
            &Some("main".into()),
            false,
            Some(4000),
            Some(120),
        );
        assert_eq!(call.tool, "read");
        assert_eq!(args_of(&call)["mode"], "visible");
        assert_eq!(args_of(&call)["selector"], "main");
        assert_eq!(args_of(&call)["tabId"], 7);
        assert_eq!(args_of(&call)["budget"], 4000);
        assert_eq!(args_of(&call)["cursor"], 120);

        let call = build_read(None, &Some("visible".into()), &None, true, None, None);
        assert_eq!(args_of(&call)["main"], false);
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
            "redact",
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
        assert_eq!(args_of(&call)["harHeaders"], Value::Null);
        assert_eq!(call.out, Some(PathBuf::from("net.har")));

        let call = build_observe(
            "network",
            None,
            None,
            &None,
            &None,
            &None,
            &None,
            None,
            Some(PathBuf::from("net.har")),
            "all",
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["harHeaders"], "all");
        assert!(
            build_observe(
                "console", None, None, &None, &None, &None, &None, None, None, "all", None
            )
            .is_err()
        );

        assert!(
            build_observe(
                "dom", None, None, &None, &None, &None, &None, None, None, "redact", None
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
            cmd: Some(TabsCommand::New {
                url: "https://x.y".into(),
                active: false,
                background: false,
            }),
        })
        .unwrap();
        assert_eq!(
            args_of(&call)["active"],
            true,
            "tabs new must activate by default"
        );
        let call = build_tabs(&TabsArgs {
            cmd: Some(TabsCommand::New {
                url: "https://x.y".into(),
                active: false,
                background: true,
            }),
        })
        .unwrap();
        assert_eq!(args_of(&call)["active"], false);

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
        let call = build_js(Some(&"1+1".to_owned()), &None, None, None, None, None).unwrap();
        assert_eq!(args_of(&call)["code"], "1+1");

        let call = build_js(
            Some(&"1+1".to_owned()),
            &None,
            None,
            Some(500),
            Some(200),
            Some(5.0),
        )
        .unwrap();
        assert_eq!(args_of(&call)["budget"], 500);
        assert_eq!(args_of(&call)["cursor"], 200);
        assert_eq!(args_of(&call)["timeout"], 5.0);
        assert!(build_js(None, &None, None, None, None, None).is_err());
        assert!(build_js(Some(&"1".to_owned()), &None, None, None, None, Some(0.0)).is_err());
    }
}
