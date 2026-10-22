//! 普通命令（tabs/open/read/snapshot/wait/await/js/observe/files/shot）→ `ToolCall`。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::ToolCall;
use super::shared::{
    MAX_UPLOAD_BYTES, base64_encode, guess_mime, parse_duration_secs, parse_tab_ids,
    parse_url_list, parse_url_value, set_opt,
};
use super::spec::{FanoutArgs, FilesArgs, FilesCommand, TabsArgs, TabsCommand};

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

#[allow(clippy::too_many_arguments)]
pub fn build_open(
    url: &Option<String>,
    urls: &[String],
    concurrency: Option<u32>,
    tab: Option<u64>,
    replace: bool,
    reuse: &Option<String>,
    until: &Option<String>,
    wait_alias: &Option<String>,
    timeout: &Option<String>,
    background: bool,
) -> Result<ToolCall, String> {
    if until.is_some() && wait_alias.is_some() {
        return Err("open: use either --until or --wait, not both".into());
    }
    let wait = until.as_ref().or(wait_alias.as_ref());
    let wait = match wait {
        Some(value) if matches!(value.as_str(), "load" | "dom" | "none") => Some(value.clone()),
        Some(value) => {
            return Err(format!(
                "open: --until must be load|dom|none, got {value:?}"
            ));
        }
        None => None,
    };
    let wait_secs = match timeout {
        Some(text) => {
            let seconds =
                parse_duration_secs(text).map_err(|error| format!("open --timeout: {error}"))?;
            if !(1.0..=60.0).contains(&seconds) {
                return Err(format!(
                    "open: --timeout must be 1..60 seconds, got {text:?}"
                ));
            }
            Some(seconds)
        }
        None => None,
    };
    let concurrency = match concurrency {
        Some(value) if (1..=4).contains(&value) => Some(value),
        Some(value) => return Err(format!("open: --concurrency must be 1..4, got {value}")),
        None => None,
    };
    let reuse = match (reuse.as_deref(), urls.is_empty()) {
        (None, _) => None,
        (Some("last" | "session"), true) => Some("last"),
        (Some("url" | "session"), false) => Some("url"),
        (Some(value), true) => {
            return Err(format!(
                "open: --reuse must be last|session for a single URL, got {value:?}"
            ));
        }
        (Some(value), false) => {
            return Err(format!(
                "open: --reuse must be url|session with --urls, got {value:?}"
            ));
        }
    };

    // --urls：批量打开。内层 args 不含 url（由执行器按目标注入），
    // 也不允许 tab/replace —— 每个目标都是新标签。
    if !urls.is_empty() {
        if url.is_some() {
            return Err("open: use either <url> or --urls, not both".into());
        }
        if tab.is_some() || replace {
            return Err(
                "open: --urls always opens new tabs; --tab/--replace are not allowed".into(),
            );
        }
        let urls = parse_url_list("open", "--urls", urls)?;
        let mut inner = json!({});
        if let Some(wait) = &wait {
            inner["wait"] = json!(wait);
        }
        if let Some(seconds) = wait_secs {
            inner["waitTimeout"] = json!(seconds);
        }
        if background {
            inner["background"] = json!(true);
        }
        let mut outer = json!({ "tool": "open", "args": inner, "urls": urls });
        if let Some(reuse) = reuse {
            outer["reuse"] = json!(reuse);
        }
        if let Some(concurrency) = concurrency {
            outer["concurrency"] = json!(concurrency);
        }
        return Ok(ToolCall::new("batch", outer));
    }

    let Some(url) = url else {
        return Err("open: pass a URL or --urls \"u1,u2\"".into());
    };
    if concurrency.is_some() {
        return Err("open: --concurrency only applies to --urls".into());
    }
    let picks = [tab.is_some(), replace, reuse.is_some()]
        .iter()
        .filter(|flag| **flag)
        .count();
    if picks > 1 {
        return Err("open: use only one of --tab, --replace, --reuse".into());
    }
    if background && picks > 0 {
        return Err("open: --background only applies when opening a new tab".into());
    }
    let mut arguments = json!({ "url": url });
    if let Some(id) = tab {
        arguments["tabId"] = json!(id);
    }
    if replace {
        arguments["replace"] = json!(true);
    }
    if let Some(reuse) = reuse {
        arguments["reuse"] = json!(reuse);
    }
    if let Some(wait) = wait {
        arguments["wait"] = json!(wait);
    }
    if let Some(seconds) = wait_secs {
        arguments["waitTimeout"] = json!(seconds);
    }
    if background {
        arguments["background"] = json!(true);
    }
    Ok(ToolCall::new("open", arguments))
}

/// 解析 `--tabs/--urls/--url`；三者只能出现一个，且不能和 `--tab` 同时用。
struct FanoutTargets {
    tab_ids: Option<Vec<u64>>,
    urls: Option<Vec<String>>,
    reuse: Option<String>,
    concurrency: Option<u32>,
}

fn fanout_targets(context: &str, fanout: &FanoutArgs) -> Result<Option<FanoutTargets>, String> {
    let has_url = fanout.url.is_some();
    let has_urls = !fanout.urls.is_empty();
    let has_tabs = !fanout.tabs.is_empty();
    if has_url && has_urls {
        return Err(format!("{context}: use either --url or --urls, not both"));
    }
    let url_mode = has_url || has_urls;
    if url_mode && has_tabs {
        return Err(format!("{context}: use only one of --tabs, --urls, --url"));
    }
    if !url_mode && !has_tabs {
        if fanout.concurrency.is_some() {
            return Err(format!(
                "{context}: --concurrency requires --tabs/--urls/--url"
            ));
        }
        if fanout.reuse.is_some() {
            return Err(format!("{context}: --reuse only applies to --urls"));
        }
        return Ok(None);
    }
    let tab_ids = if has_tabs {
        Some(parse_tab_ids(context, "--tabs", &fanout.tabs)?)
    } else {
        None
    };
    let urls = if has_tabs {
        None
    } else if let Some(url) = &fanout.url {
        Some(vec![parse_url_value(context, "--url", url)?])
    } else {
        Some(parse_url_list(context, "--urls", &fanout.urls)?)
    };
    let concurrency = match fanout.concurrency {
        Some(value) if (1..=4).contains(&value) => Some(value),
        Some(value) => {
            return Err(format!(
                "{context}: --concurrency must be 1..4, got {value}"
            ));
        }
        None => None,
    };
    let reuse = match fanout.reuse.as_deref() {
        None => None,
        Some("url" | "session") => {
            if urls.is_none() {
                return Err(format!("{context}: --reuse only applies to --urls"));
            }
            Some("url".to_owned())
        }
        Some(value) => {
            return Err(format!(
                "{context}: --reuse must be url|session, got {value:?}"
            ));
        }
    };
    Ok(Some(FanoutTargets {
        tab_ids,
        urls,
        reuse,
        concurrency,
    }))
}

/// 单目标 → 直接调工具；多目标 → 包成一次 `batch` 调用。
fn finish_fanout(
    tool: &'static str,
    mut args: Value,
    tab: Option<u64>,
    fanout: &FanoutArgs,
    context: &str,
) -> Result<ToolCall, String> {
    let Some(targets) = fanout_targets(context, fanout)? else {
        if let Some(id) = tab {
            args["tabId"] = json!(id);
        }
        return Ok(ToolCall::new(tool, args));
    };
    if tab.is_some() {
        return Err(format!(
            "{context}: use either --tab or --tabs/--urls, not both"
        ));
    }
    if let Some(object) = args.as_object_mut() {
        object.remove("tabId");
    }
    let mut outer = json!({ "tool": tool, "args": args });
    if let Some(ids) = targets.tab_ids {
        outer["tabIds"] = json!(ids);
    }
    if let Some(urls) = targets.urls {
        outer["urls"] = json!(urls);
    }
    if let Some(reuse) = targets.reuse {
        outer["reuse"] = json!(reuse);
    }
    if let Some(concurrency) = targets.concurrency {
        outer["concurrency"] = json!(concurrency);
    }
    Ok(ToolCall::new("batch", outer))
}

pub fn build_read(
    fanout: &FanoutArgs,
    tab: Option<u64>,
    mode: &Option<String>,
    selector: &Option<String>,
    no_main: bool,
    budget: Option<u64>,
    cursor: Option<u64>,
) -> Result<ToolCall, String> {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "mode", mode.clone());
    set_opt(&mut arguments, "selector", selector.clone());
    if no_main {
        arguments["main"] = json!(false);
    }
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    finish_fanout("read", arguments, tab, fanout, "read")
}

pub fn build_snapshot(
    fanout: &FanoutArgs,
    tab: Option<u64>,
    filter: &Option<String>,
    near: &Option<String>,
    budget: Option<u64>,
    cursor: Option<u64>,
    values: bool,
) -> Result<ToolCall, String> {
    let mut arguments = json!({});
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "filter", filter.clone());
    set_opt(&mut arguments, "near", near.clone());
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    if values {
        arguments["values"] = json!(true);
    }
    finish_fanout("snapshot", arguments, tab, fanout, "snapshot")
}

pub fn build_wait(
    fanout: &FanoutArgs,
    kind: &str,
    value: &Option<String>,
    timeout: &Option<String>,
    tab: Option<u64>,
) -> Result<ToolCall, String> {
    let mut arguments = json!({});
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
    finish_fanout("wait", arguments, tab, fanout, "wait")
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
    fanout: &FanoutArgs,
    code: Option<&String>,
    file: &Option<PathBuf>,
    tab: Option<u64>,
    budget: Option<u64>,
    cursor: Option<u64>,
    timeout: &Option<String>,
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
    let timeout = match timeout {
        Some(text) => {
            let seconds =
                parse_duration_secs(text).map_err(|error| format!("js --timeout: {error}"))?;
            if !(0.1..=300.0).contains(&seconds) {
                return Err(format!(
                    "js: --timeout must be 0.1..300 seconds, got {text:?}"
                ));
            }
            Some(seconds)
        }
        None => None,
    };
    let mut arguments = json!({ "code": source });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "budget", budget);
    set_opt(&mut arguments, "cursor", cursor);
    set_opt(&mut arguments, "timeout", timeout);
    finish_fanout("js", arguments, tab, fanout, "js")
}

#[allow(clippy::too_many_arguments)]
pub fn build_observe(
    kind: &str,
    tab: Option<u64>,
    since: Option<u64>,
    level: &Option<String>,
    url_filter: &Option<String>,
    filter_alias: &Option<String>,
    collect: &Option<String>,
    wait_alias: &Option<String>,
    body_filter: &Option<String>,
    no_bodies: bool,
    bodies_alias: Option<bool>,
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
    if url_filter.is_some() && filter_alias.is_some() {
        return Err("observe: use either --url-filter or --filter, not both".into());
    }
    if url_filter.is_some() && kind != "network" {
        return Err("observe: --url-filter only applies to network".into());
    }
    if collect.is_some() && wait_alias.is_some() {
        return Err("observe: use either --collect or --wait, not both".into());
    }
    if no_bodies && bodies_alias.is_some() {
        return Err("observe: use either --no-bodies or --bodies=false, not both".into());
    }
    let filter = url_filter.clone().or_else(|| filter_alias.clone());
    let wait = collect.as_ref().or(wait_alias.as_ref());
    let bodies = if no_bodies { Some(false) } else { bodies_alias };
    let mut arguments = json!({ "kind": kind });
    set_opt(&mut arguments, "tabId", tab);
    set_opt(&mut arguments, "since", since);
    set_opt(&mut arguments, "level", level.clone());
    set_opt(&mut arguments, "filter", filter);
    set_opt(&mut arguments, "bodyFilter", body_filter.clone());
    set_opt(&mut arguments, "bodies", bodies);
    set_opt(&mut arguments, "budget", budget);
    if headers == "all" {
        arguments["harHeaders"] = json!("all");
    }
    if let Some(wait) = wait {
        let seconds =
            parse_duration_secs(wait).map_err(|error| format!("observe --collect: {error}"))?;
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

    #[allow(clippy::too_many_arguments)]
    fn open(
        url: Option<&str>,
        urls: &[&str],
        concurrency: Option<u32>,
        tab: Option<u64>,
        replace: bool,
        reuse: Option<&str>,
        until: Option<&str>,
        wait_alias: Option<&str>,
        timeout: Option<&str>,
        background: bool,
    ) -> Result<ToolCall, String> {
        build_open(
            &url.map(str::to_owned),
            &urls
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>(),
            concurrency,
            tab,
            replace,
            &reuse.map(str::to_owned),
            &until.map(str::to_owned),
            &wait_alias.map(str::to_owned),
            &timeout.map(str::to_owned),
            background,
        )
    }

    #[test]
    fn wait_forms() {
        let fanout = FanoutArgs::default();
        let call = build_wait(
            &fanout,
            "text",
            &Some("Done".into()),
            &Some("30s".into()),
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["text"], "Done");
        assert_eq!(args_of(&call)["timeout"], 30.0);

        let call = build_wait(&fanout, "time", &Some("5s".into()), &None, Some(7)).unwrap();
        assert_eq!(args_of(&call)["time"], 5.0);
        assert_eq!(args_of(&call)["tabId"], 7);

        let call = build_wait(&fanout, "idle", &None, &None, None).unwrap();
        assert_eq!(args_of(&call)["idle"], true);

        let call = build_wait(&fanout, "idle", &Some("10s".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["idle"], true);
        assert_eq!(args_of(&call)["timeout"], 10.0);
        // 位置参数与 --timeout 不能同时静默覆盖。
        assert!(
            build_wait(
                &fanout,
                "idle",
                &Some("10s".into()),
                &Some("20s".into()),
                None
            )
            .is_err()
        );

        let call = build_wait(&fanout, "textGone", &Some("Loading".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["textGone"], "Loading");

        assert!(build_wait(&fanout, "bogus", &None, &None, None).is_err());
        assert!(build_wait(&fanout, "text", &None, &None, None).is_err());
    }

    #[test]
    fn read_visible_mode_passes_through_with_the_same_root_flags() {
        let fanout = FanoutArgs::default();
        let call = build_read(
            &fanout,
            Some(7),
            &Some("visible".into()),
            &Some("main".into()),
            false,
            Some(4000),
            Some(120),
        )
        .unwrap();
        assert_eq!(call.tool, "read");
        assert_eq!(args_of(&call)["mode"], "visible");
        assert_eq!(args_of(&call)["selector"], "main");
        assert_eq!(args_of(&call)["tabId"], 7);
        assert_eq!(args_of(&call)["budget"], 4000);
        assert_eq!(args_of(&call)["cursor"], 120);

        let call = build_read(
            &fanout,
            None,
            &Some("visible".into()),
            &None,
            true,
            None,
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["main"], false);
    }

    #[test]
    fn fanout_compiles_to_a_single_batch_call() {
        let fanout = FanoutArgs {
            tabs: vec!["1,2".into()],
            ..Default::default()
        };
        let call = build_read(
            &fanout,
            None,
            &Some("md".into()),
            &None,
            false,
            Some(2000),
            None,
        )
        .unwrap();
        assert_eq!(call.tool, "batch");
        assert_eq!(args_of(&call)["tool"], "read");
        assert_eq!(args_of(&call)["tabIds"], json!([1, 2]));
        assert_eq!(args_of(&call)["args"]["mode"], "md");
        assert_eq!(args_of(&call)["args"]["budget"], 2000);
        assert!(
            args_of(&call)["args"].get("tabId").is_none(),
            "fan-out must not leak a single tabId into the inner args"
        );

        let fanout = FanoutArgs {
            urls: vec!["https://a.example/".into(), "https://b.example/".into()],
            concurrency: Some(3),
            reuse: Some("url".into()),
            ..Default::default()
        };
        let call = build_snapshot(
            &fanout,
            None,
            &Some("links".into()),
            &None,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(call.tool, "batch");
        assert_eq!(args_of(&call)["tool"], "snapshot");
        assert_eq!(
            args_of(&call)["urls"],
            json!(["https://a.example/", "https://b.example/"])
        );
        assert_eq!(args_of(&call)["concurrency"], 3);
        assert_eq!(args_of(&call)["reuse"], "url");
        assert_eq!(args_of(&call)["args"]["filter"], "links");

        // --url 是 --urls 的单数形式。
        let fanout = FanoutArgs {
            url: Some("https://one.example/".into()),
            ..Default::default()
        };
        let call = build_js(
            &fanout,
            Some(&"1+1".to_owned()),
            &None,
            None,
            None,
            None,
            &Some("5s".into()),
        )
        .unwrap();
        assert_eq!(args_of(&call)["urls"], json!(["https://one.example/"]));
        assert_eq!(args_of(&call)["args"]["timeout"], 5.0);
        assert!(args_of(&call)["args"].get("url").is_none());

        // --url 是单数参数，不按逗号切分（查询串里的逗号必须保留）。
        let fanout = FanoutArgs {
            url: Some("https://one.example/?q=1,2".into()),
            ..Default::default()
        };
        let call = build_read(&fanout, None, &None, &None, false, None, None).unwrap();
        assert_eq!(
            args_of(&call)["urls"],
            json!(["https://one.example/?q=1,2"])
        );

        // 旧别名 --reuse session 映射到精确 URL 复用。
        let fanout = FanoutArgs {
            urls: vec!["https://a.example/".into()],
            reuse: Some("session".into()),
            ..Default::default()
        };
        let call = build_wait(&fanout, "text", &Some("Done".into()), &None, None).unwrap();
        assert_eq!(args_of(&call)["reuse"], "url");
    }

    #[test]
    fn fanout_rejects_conflicting_targets() {
        let both = FanoutArgs {
            tabs: vec!["1".into()],
            urls: vec!["https://a.example/".into()],
            ..Default::default()
        };
        let error = build_read(&both, None, &None, &None, false, None, None)
            .err()
            .expect("expected an error");
        assert!(error.contains("only one of --tabs"), "{error}");

        let both = FanoutArgs {
            url: Some("https://a.example/".into()),
            urls: vec!["https://b.example/".into()],
            ..Default::default()
        };
        assert!(build_read(&both, None, &None, &None, false, None, None).is_err());

        let tab_and_tabs = FanoutArgs {
            tabs: vec!["1".into()],
            ..Default::default()
        };
        let error = build_read(&tab_and_tabs, Some(2), &None, &None, false, None, None)
            .err()
            .expect("expected an error");
        assert!(error.contains("--tab or --tabs"), "{error}");

        // --concurrency/--reuse 没有目标时不能静默忽略。
        let orphan_concurrency = FanoutArgs {
            concurrency: Some(2),
            ..Default::default()
        };
        assert!(build_read(&orphan_concurrency, None, &None, &None, false, None, None).is_err());
        let orphan_reuse = FanoutArgs {
            reuse: Some("url".into()),
            ..Default::default()
        };
        assert!(build_snapshot(&orphan_reuse, None, &None, &None, None, None, false).is_err());

        // --reuse url 只对 --urls 有意义；--tabs 不行。
        let reuse_with_tabs = FanoutArgs {
            tabs: vec!["1".into()],
            reuse: Some("url".into()),
            ..Default::default()
        };
        assert!(build_wait(&reuse_with_tabs, "idle", &None, &None, None).is_err());

        // 8 个目标上限与 batch 一致。
        let too_many = FanoutArgs {
            tabs: vec!["1,2,3,4,5,6,7,8,9".into()],
            ..Default::default()
        };
        assert!(build_read(&too_many, None, &None, &None, false, None, None).is_err());
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
            &None,
            &Some("5s".into()),
            &None,
            &Some("error".into()),
            true,
            None,
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

        // 旧别名仍可用，且与旧线协议字段一致。
        let call = build_observe(
            "network",
            None,
            None,
            &None,
            &None,
            &Some("api".into()),
            &None,
            &Some("5s".into()),
            &None,
            false,
            Some(false),
            None,
            "redact",
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["filter"], "api");
        assert_eq!(args_of(&call)["wait"], 5.0);
        assert_eq!(args_of(&call)["bodies"], false);

        // --url-filter 只属于 network；console 用旧的 --filter 过滤文本。
        assert!(
            build_observe(
                "console",
                None,
                None,
                &None,
                &Some("api".into()),
                &None,
                &None,
                &None,
                &None,
                false,
                None,
                None,
                "redact",
                None
            )
            .is_err()
        );
        let call = build_observe(
            "console",
            None,
            None,
            &None,
            &None,
            &Some("boom".into()),
            &None,
            &None,
            &None,
            false,
            None,
            None,
            "redact",
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["filter"], "boom");

        let call = build_observe(
            "network",
            None,
            None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            false,
            None,
            Some(PathBuf::from("net.har")),
            "all",
            None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["harHeaders"], "all");
        assert!(
            build_observe(
                "console", None, None, &None, &None, &None, &None, &None, &None, false, None, None,
                "all", None
            )
            .is_err()
        );

        assert!(
            build_observe(
                "dom", None, None, &None, &None, &None, &None, &None, &None, false, None, None,
                "redact", None
            )
            .is_err()
        );
    }

    #[test]
    fn open_navigation_modes_are_exclusive() {
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                None,
                None,
                None,
                None,
                false
            )
            .is_ok()
        );
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                Some(3),
                true,
                None,
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                Some(3),
                false,
                Some("last"),
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                Some("all"),
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                None,
                Some("later"),
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(open(None, &[], None, None, false, None, None, None, None, false).is_err());
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                None,
                None,
                None,
                Some("0.5"),
                false
            )
            .is_err()
        );
        let call = open(
            Some("https://x.y"),
            &[],
            None,
            None,
            false,
            None,
            None,
            None,
            Some("20"),
            false,
        )
        .unwrap();
        assert_eq!(args_of(&call)["waitTimeout"], 20.0);

        // 时长串与裸数字等价。
        let call = open(
            Some("https://x.y"),
            &[],
            None,
            None,
            false,
            None,
            Some("none"),
            None,
            Some("2s"),
            false,
        )
        .unwrap();
        assert_eq!(args_of(&call)["wait"], "none");

        // 旧别名 --wait 仍可用，但不能与 --until 同时给。
        let call = open(
            Some("https://x.y"),
            &[],
            None,
            None,
            false,
            None,
            None,
            Some("dom"),
            None,
            false,
        )
        .unwrap();
        assert_eq!(args_of(&call)["wait"], "dom");
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                None,
                Some("dom"),
                Some("load"),
                None,
                false
            )
            .is_err()
        );

        // --background 只对新建标签生效。
        let call = open(
            Some("https://x.y"),
            &[],
            None,
            None,
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(args_of(&call)["background"], true);
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                Some(3),
                false,
                None,
                None,
                None,
                None,
                true
            )
            .is_err()
        );

        // --reuse last（session 是旧别名）映射为规范值 last。
        for reuse in ["last", "session"] {
            let call = open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                Some(reuse),
                None,
                None,
                None,
                false,
            )
            .unwrap();
            assert_eq!(args_of(&call)["reuse"], "last");
        }
        assert!(
            open(
                Some("https://x.y"),
                &[],
                None,
                None,
                false,
                Some("url"),
                None,
                None,
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn open_urls_fans_out_through_batch() {
        let call = open(
            None,
            &["https://a.example/", "https://b.example/"],
            Some(3),
            None,
            false,
            Some("url"),
            Some("dom"),
            None,
            Some("20s"),
            true,
        )
        .unwrap();
        assert_eq!(call.tool, "batch");
        assert_eq!(args_of(&call)["tool"], "open");
        assert_eq!(
            args_of(&call)["urls"],
            json!(["https://a.example/", "https://b.example/"])
        );
        assert_eq!(args_of(&call)["reuse"], "url");
        assert_eq!(args_of(&call)["concurrency"], 3);
        assert_eq!(args_of(&call)["args"]["wait"], "dom");
        assert_eq!(args_of(&call)["args"]["waitTimeout"], 20.0);
        assert_eq!(args_of(&call)["args"]["background"], true);
        assert!(args_of(&call)["args"].get("url").is_none());

        // --urls 与位置 URL 互斥；也不接受 --tab/--replace。
        assert!(
            open(
                Some("https://x.y"),
                &["https://a.example/"],
                None,
                None,
                false,
                None,
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(
            open(
                None,
                &["https://a.example/"],
                None,
                Some(3),
                false,
                None,
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        assert!(
            open(
                None,
                &["https://a.example/"],
                None,
                None,
                true,
                None,
                None,
                None,
                None,
                false
            )
            .is_err()
        );
        // 单 URL 不接受批量专用的 --reuse url / --concurrency。
        assert!(
            open(
                Some("https://x.y"),
                &[],
                Some(2),
                None,
                false,
                None,
                None,
                None,
                None,
                false
            )
            .is_err()
        );
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
        let fanout = FanoutArgs::default();
        let call = build_js(
            &fanout,
            Some(&"1+1".to_owned()),
            &None,
            None,
            None,
            None,
            &None,
        )
        .unwrap();
        assert_eq!(args_of(&call)["code"], "1+1");

        let call = build_js(
            &fanout,
            Some(&"1+1".to_owned()),
            &None,
            None,
            Some(500),
            Some(200),
            &Some("5s".into()),
        )
        .unwrap();
        assert_eq!(args_of(&call)["budget"], 500);
        assert_eq!(args_of(&call)["cursor"], 200);
        assert_eq!(args_of(&call)["timeout"], 5.0);

        // --timeout 接受裸数字 / 时长串，并保持 0.1..300 范围。
        let call = build_js(
            &fanout,
            Some(&"1".to_owned()),
            &None,
            None,
            None,
            None,
            &Some("500ms".into()),
        )
        .unwrap();
        assert_eq!(args_of(&call)["timeout"], 0.5);
        assert!(build_js(&fanout, None, &None, None, None, None, &None).is_err());
        assert!(
            build_js(
                &fanout,
                Some(&"1".to_owned()),
                &None,
                None,
                None,
                None,
                &Some("0".into())
            )
            .is_err()
        );
        assert!(
            build_js(
                &fanout,
                Some(&"1".to_owned()),
                &None,
                None,
                None,
                None,
                &Some("abc".into())
            )
            .is_err()
        );
    }
}
