//! `ema2 act`：单步参数、`.ema2` / `--steps` 脚本解析。

use serde_json::{Value, json};

use super::ToolCall;
use super::shared::{TokenCursor, parse_duration_secs, parse_port, set_opt, split_flag};

const ACT_ACTIONS: &[&str] = &[
    "click", "type", "fill", "select", "press", "hover", "scroll", "submit", "back", "forward",
    "reload",
];

/// 写进错误信息的脚本白名单，避免用户再猜哪些动作能出现在 `.ema2` 里。
const SCRIPT_ACTIONS_HINT: &str =
    "click/type/fill/select/press/hover/scroll/submit/back/forward/reload";

/// 脚本里误写 CLI 命令时，直接给出替代写法，而不是只说 unknown action。
fn script_action_hint(action: &str) -> &'static str {
    match action {
        "open" => "run `ema2 open <url>` in the shell, then start the script again",
        "read" => "run `ema2 read` outside the script",
        "snapshot" => {
            "run `ema2 snapshot` outside the script; refs from it can be used in the script"
        }
        "wait" => {
            "use `await <duration>` or `waitFor ...` in scripts; use `ema2 wait ...` in the shell"
        }
        "js" => "run `ema2 js` outside the script",
        "tabs" => "run `ema2 tabs` outside the script",
        "observe" => "run `ema2 observe` outside the script",
        "files" => "run `ema2 files upload` outside the script",
        "shot" => "run `ema2 shot` outside the script",
        "batch" => "run `ema2 batch` outside the script",
        "server" | "status" | "init" => "run it in the shell; scripts only contain act steps",
        _ => "run `ema2 act --help` for the script syntax and examples",
    }
}

/// 位置参数里的 ref 只能是 `e12` / `e12@d1` / `[e12@d1]`；
/// 否则 `act click foo` 会把拼错的参数当成 ref，报成看不懂的 stale_ref。
fn check_ref_token(token: &str) -> Result<(), String> {
    let text = token.trim().trim_start_matches('[').trim_end_matches(']');
    let body = text.strip_prefix(['e', 'E']).unwrap_or(text);
    let ok = body.split_once('@').map_or_else(
        || body.parse::<u64>().is_ok(),
        |(seq, doc)| {
            // 与页面 helper 的 /^e(\d+)(?:@(d\d+))?$/ 保持一致：
            // `e12@dfoo` 不是合法完整 ref，不能在 CLI 被放过后才报 stale_ref。
            seq.parse::<u64>().is_ok()
                && doc.strip_prefix('d').is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                })
        },
    );
    if ok {
        return Ok(());
    }
    Err(format!(
        "act: {token:?} is not a ref (want e12 or e12@d1); use --selector or --target for CSS/role queries"
    ))
}

/// 重复 flag 不能静默 last-wins：多写一个 `--tab` 会悄悄改变目标标签。
fn set_once<T>(slot: &mut Option<T>, value: T, context: &str, flag: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{context}: {flag} given twice"));
    }
    *slot = Some(value);
    Ok(())
}

/// 三个定位器（裸 ref / selector= / target=）只能出现一个，否则扩展按 ref 优先，
/// 其余参数会被静默忽略（`act click e12 --selector '#x'` 实际点 e12）。
fn ensure_single_locator(
    r#ref: Option<&str>,
    selector: Option<&str>,
    target: Option<&str>,
    context: &str,
) -> Result<(), String> {
    let locators = [
        r#ref
            .filter(|value| !value.trim().is_empty())
            .map(|_| "ref"),
        selector
            .filter(|value| !value.trim().is_empty())
            .map(|_| "selector"),
        target
            .filter(|value| !value.trim().is_empty())
            .map(|_| "target"),
    ];
    let used: Vec<&str> = locators.into_iter().flatten().collect();
    if used.len() > 1 {
        return Err(format!(
            "{context}: use only one of ref, selector=, target= (got {})",
            used.join(", ")
        ));
    }
    Ok(())
}

/// CLI 与 daemon 共用 tools.rs 的 waitFor 校验，避免 timeout=60 / url= 只在
/// 扩展侧才失败。
fn validate_wait_for_value(value: &Value, context: &str) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err(format!("{context}: waitFor must be an object"));
    };
    crate::tools::validate_wait_for_spec(object).map_err(|error| format!("{context}: {error}"))
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
    let mut positional_only = false;

    let mut cursor = TokenCursor::new(tokens);
    while let Some(token) = cursor.peek() {
        // `--` 之后一律当位置参数：让 `type e1 -- -5` 能输入负号开头的文本。
        if positional_only {
            positional.push(token.to_owned());
            cursor.advance();
            continue;
        }
        if token == "--" {
            positional_only = true;
            cursor.advance();
            continue;
        }
        // 值型 flag：`--flag value` 与 `--flag=value` 都支持；缺失时报错而不是 panic。
        let (flag, inline) = split_flag(token);
        match flag {
            "--selector" => {
                let value = cursor.take_flag_value(inline, "act", "--selector")?;
                set_once(&mut selector, value, "act", "--selector")?;
            }
            "--target" => {
                let value = cursor.take_flag_value(inline, "act", "--target")?;
                set_once(&mut target, value, "act", "--target")?;
            }
            "--key" => {
                let value = cursor.take_flag_value(inline, "act", "--key")?;
                set_once(&mut key, value, "act", "--key")?;
            }
            "--delta-y" => {
                let value = cursor.take_flag_value(inline, "act", "--delta-y")?;
                let parsed: f64 = value
                    .parse()
                    .map_err(|_| format!("act: invalid --delta-y {value:?}"))?;
                if !parsed.is_finite() {
                    return Err(format!("act: invalid --delta-y {value:?}"));
                }
                set_once(&mut delta_y, parsed, "act", "--delta-y")?;
            }
            "--tab" => {
                let value = cursor.take_flag_value(inline, "act", "--tab")?;
                let parsed: u64 = value
                    .parse()
                    .map_err(|_| format!("act: invalid --tab {value:?}"))?;
                set_once(&mut tab, parsed, "act", "--tab")?;
            }
            "--port" => {
                let value = cursor.take_flag_value(inline, "act", "--port")?;
                let parsed = parse_port(&value)
                    .map_err(|error| format!("act: invalid --port {value:?}: {error}"))?;
                set_once(&mut port_override, parsed, "act", "--port")?;
            }
            "--wait-for" => {
                let value = cursor.take_flag_value(inline, "act", "--wait-for")?;
                set_once(&mut wait_for, value, "act", "--wait-for")?;
            }
            "--steps" => {
                let value = cursor.take_flag_value(inline, "act", "--steps")?;
                set_once(&mut steps_inline, value, "act", "--steps")?;
            }
            "--script" => {
                let value = cursor.take_flag_value(inline, "act", "--script")?;
                set_once(&mut script_path, value, "act", "--script")?;
            }
            "--submit" => {
                // 布尔 flag：不消耗下一个 token，否则 `--submit e1 hi` 会吞掉 ref。
                if inline.is_some() {
                    return Err("act: --submit does not take a value".into());
                }
                submit_flag = true;
                cursor.advance();
            }
            other if other.starts_with('-') => {
                // trailing_var_arg + allow_hyphen_values 会把拼错的 flag 变成位置参数；
                // 这里直接拒绝，别让它悄悄变成一个 ref。
                return Err(format!(
                    "act: unknown flag {other:?} (known: --selector --target --key --delta-y --submit --tab --wait-for --steps --script --port)"
                ));
            }
            other => {
                positional.push(other.to_owned());
                cursor.advance();
            }
        }
    }

    if let Some(path) = &script_path {
        if steps_inline.is_some() {
            return Err("act: use either --steps or --script, not both".into());
        }
        let source = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "act: cannot read script {path}: {error}; check the path or run `ema2 act --help`"
            )
        })?;
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
        // 这些 flag 只对单步调用有意义；静默忽略会让调用方以为已经提交/输入。
        let ignored = [
            selector.is_some().then_some("--selector"),
            target.is_some().then_some("--target"),
            key.is_some().then_some("--key"),
            delta_y.is_some().then_some("--delta-y"),
            submit_flag.then_some("--submit"),
        ];
        let ignored: Vec<&str> = ignored.into_iter().flatten().collect();
        if !ignored.is_empty() {
            return Err(format!(
                "act: {} cannot be used with --steps/--script; encode the equivalent step in the script (selector=/target=/press/submit)",
                ignored.join("/")
            ));
        }
        let parsed = parse_act_script(&source)?;
        let mut steps = parsed["steps"].as_array().cloned().unwrap_or_default();
        // 先校验脚本本身，再注入 --wait-for：20 步脚本 + waitFor 是合法的 21 步。
        if steps.is_empty() || steps.len() > 20 {
            return Err("act: script must contain 1..20 steps; run `ema2 act --help`".into());
        }
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
            let tokens =
                tokenize_line(wait_for).map_err(|error| format!("act waitFor: {error}"))?;
            let mut step = parse_wait_for(&tokens)?;
            step["action"] = json!("waitFor");
            validate_wait_for_value(&step, "act waitFor")?;
            steps.push(step);
            if steps.len() > 21 {
                return Err("act: --wait-for adds one step; scripts allow at most 20 + 1".into());
            }
        }
        arguments["steps"] = json!(steps);
    } else {
        let action = positional.first().ok_or_else(|| {
            format!("act: missing action ({SCRIPT_ACTIONS_HINT}); run `ema2 act --help`")
        })?;
        if !ACT_ACTIONS.contains(&action.as_str()) {
            return Err(format!(
                "act: unknown action {action:?}; use one of {SCRIPT_ACTIONS_HINT}, or run `ema2 act --help`"
            ));
        }
        if submit_flag && action != "type" {
            return Err(
                "act: --submit is only supported by type; use action=submit or click the submit button"
                    .into(),
            );
        }
        if key.is_some() && action != "press" {
            return Err("act: --key is only supported by press".into());
        }
        if delta_y.is_some() && action != "scroll" {
            return Err("act: --delta-y is only supported by scroll".into());
        }
        if matches!(action.as_str(), "scroll" | "back" | "forward" | "reload")
            && (selector.is_some() || target.is_some())
        {
            return Err(format!(
                "act {action}: --selector/--target are not supported by this action"
            ));
        }
        let rest: Vec<String> = positional[1..].to_vec();
        if let Some(wait_for) = &wait_for {
            let tokens =
                tokenize_line(wait_for).map_err(|error| format!("act waitFor: {error}"))?;
            let parsed = parse_wait_for(&tokens)?;
            validate_wait_for_value(&parsed, "act waitFor")?;
            arguments["waitFor"] = parsed;
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
                        "act {action}: takes at most one ref; use --selector/--target for CSS/role locators"
                    ));
                }
                let positional_ref = rest.first();
                if let Some(r#ref) = positional_ref {
                    check_ref_token(r#ref)?;
                }
                ensure_single_locator(
                    positional_ref.map(String::as_str),
                    selector.as_deref(),
                    target.as_deref(),
                    &format!("act {action}"),
                )?;
                if let Some(r#ref) = positional_ref {
                    arguments["ref"] = json!(r#ref);
                }
            }
            "type" | "fill" | "select" => {
                // 位置参数有两种形态：`[ref] <text>` 或（有 --selector/--target 时）`<text>`。
                let located = selector.is_some() || target.is_some();
                // 有命名定位器时，两个位置参数只可能是“ref + 忘记加引号的文本”；
                // 若首 token 是合法 ref，就按定位器冲突报，而不是误导成引号问题。
                let ref_candidate = if located {
                    rest.len()
                        .eq(&2)
                        .then(|| rest[0].as_str())
                        .filter(|token| check_ref_token(token).is_ok())
                } else {
                    rest.first().map(String::as_str)
                };
                ensure_single_locator(
                    ref_candidate,
                    selector.as_deref(),
                    target.as_deref(),
                    &format!("act {action}"),
                )?;
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
                ensure_single_locator(
                    positional_ref.as_deref(),
                    selector.as_deref(),
                    target.as_deref(),
                    "act press",
                )?;
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
                    let raw = rest.get(1).cloned().unwrap_or_else(|| "1".to_owned());
                    let count: u64 = raw
                        .parse()
                        .map_err(|_| format!("act scroll: invalid count {:?}", rest.get(1)))?;
                    if count == 0 {
                        return Err("act scroll: count must be a positive integer".into());
                    }
                    if count > 1000 {
                        return Err("act scroll: count must be at most 1000".into());
                    }
                    arguments["text"] = json!(format!("{dir} {count}"));
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

/// `--wait-for "url=api/save status=200 timeout=5s"` 或
/// `--wait-for "text=保存成功 timeout=5s"` → waitFor 对象。
///
/// 接收已经分好的 token（脚本入口直接传 `tokens[1..]`，flag 入口先 `tokenize_line`），
/// 避免把 `selector=#main a` 这样的单 token 再次按空格撕开。
///
/// 网络/导航条件（url/status）和页面条件（text/textGone/selector）是两种语义：
/// 前者可以 url+status 组合，后者与 `ema2 wait` 一样只接受一个条件。
fn parse_wait_for(tokens: &[String]) -> Result<Value, String> {
    let mut out = json!({});
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            let hint = if token == "time" || token.starts_with("time") {
                "use `await 1s` for a plain delay; waitFor accepts url=/status=/text=/textGone=/selector="
            } else {
                "waitFor accepts url= / status= / text= / textGone= / selector= / timeout=; use `await 1s` for a plain delay"
            };
            return Err(format!(
                "act waitFor: expected key=value, got {token:?}; {hint}"
            ));
        };
        match key {
            "url" => {
                if value.trim().is_empty() {
                    return Err("act waitFor: url= must not be empty".into());
                }
                out["url"] = json!(value);
            }
            "status" => {
                let status: u16 = value
                    .parse()
                    .map_err(|_| format!("act waitFor: invalid status {value:?}"))?;
                out["status"] = json!(status);
            }
            "text" | "textGone" | "selector" => {
                if value.trim().is_empty() {
                    return Err(format!("act waitFor: {key}= must not be empty"));
                }
                out[key] = json!(value);
            }
            "timeout" => {
                let seconds =
                    parse_duration_secs(value).map_err(|error| format!("act waitFor: {error}"))?;
                out["timeout"] = json!(seconds);
            }
            _ => {
                return Err(format!(
                    "act waitFor: unknown key {key:?}; only url= / status= / text= / textGone= / selector= / timeout= are supported"
                ));
            }
        }
    }
    let page_keys: Vec<&str> = ["text", "textGone", "selector"]
        .into_iter()
        .filter(|key| out.get(*key).is_some())
        .collect();
    let has_url = out.get("url").is_some();
    let has_status = out.get("status").is_some();
    if page_keys.len() > 1 {
        return Err(format!(
            "act waitFor: use only one of text= / textGone= / selector= (got {})",
            page_keys.join(", ")
        ));
    }
    if !page_keys.is_empty() && (has_url || has_status) {
        return Err(
            "act waitFor: page conditions (text=/textGone=/selector=) cannot be combined with url=/status=; use one condition per waitFor"
                .into(),
        );
    }
    if page_keys.is_empty() && !has_url && !has_status {
        return Err(
            "act waitFor: needs at least one of url= / status= / text= / textGone= / selector=; timeout alone is not a wait (use `await 1s`)"
                .into(),
        );
    }
    Ok(out)
}

/// 行尾 `#` 注释：引号外、且位于行首或空白之后才算注释；
/// 引号内的 `#`（`fill e1 "#hashtag"`）与 `e1#x` 这类紧贴 token 的 `#` 保留。
fn strip_trailing_comment(line: &str) -> &str {
    let mut in_quotes = false;
    let mut escaped = false;
    let mut previous: Option<char> = None;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            previous = Some(ch);
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes && previous.is_none_or(char::is_whitespace) => {
                return &line[..index];
            }
            _ => {}
        }
        previous = Some(ch);
    }
    line
}

/// `.ema2` 脚本 / `--steps` 文本 → steps[] 数组。
///
/// 语法：每行一条；空行与 `#` 注释（行首或行尾）忽略；含空格的值加双引号
/// （支持 `\"` 转义）；`waitFor` 是独立步骤，保持它在脚本中的位置
/// （不再挪到整次调用末尾）。
fn parse_act_script(source: &str) -> Result<Value, String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut steps = Vec::new();
    for (number, raw) in source.lines().enumerate() {
        // 注释在拆语句之前剥离：`click e1 # note; click e2` 的整段注释都不会成为语句。
        let raw = strip_trailing_comment(raw);
        if raw.trim().is_empty() {
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
                let mut parsed = parse_wait_for(&tokens[1..])
                    .map_err(|error| format!("script line {}: {error}", number + 1))?;
                crate::tools::validate_wait_for_spec(
                    parsed.as_object().expect("waitFor is always an object"),
                )
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
                    "script line {}: unknown action {action:?}; scripts only support {SCRIPT_ACTIONS_HINT} + await/waitFor. {}",
                    number + 1,
                    script_action_hint(action)
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
            if matches!(action.as_str(), "scroll" | "back" | "forward" | "reload")
                && (step.get("selector").is_some() || step.get("target").is_some())
            {
                return Err(script_error(format!(
                    "{action}: selector=/target= are not supported by this action"
                )));
            }
            match action.as_str() {
                "click" | "hover" | "submit" => {
                    if rest.len() > 1 {
                        return Err(script_error(format!(
                            "{action}: takes at most one ref; use selector=/target= for CSS/role locators"
                        )));
                    }
                    let positional_ref = rest.first();
                    if let Some(r#ref) = positional_ref {
                        check_ref_token(r#ref).map_err(script_error)?;
                    }
                    ensure_single_locator(
                        positional_ref.map(String::as_str),
                        step.get("selector").and_then(Value::as_str),
                        step.get("target").and_then(Value::as_str),
                        &script_error(action.to_string()),
                    )?;
                    if let Some(r#ref) = positional_ref {
                        step["ref"] = json!(r#ref);
                    }
                }
                "type" | "fill" | "select" => {
                    let located = step.get("selector").is_some() || step.get("target").is_some();
                    let ref_candidate = if located {
                        rest.len()
                            .eq(&2)
                            .then(|| rest[0].as_str())
                            .filter(|token| check_ref_token(token).is_ok())
                    } else {
                        rest.first().map(String::as_str)
                    };
                    ensure_single_locator(
                        ref_candidate,
                        step.get("selector").and_then(Value::as_str),
                        step.get("target").and_then(Value::as_str),
                        &script_error(action.to_string()),
                    )?;
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
                    let positional_ref = (rest.len() >= 2).then(|| rest[0].as_str());
                    ensure_single_locator(
                        positional_ref,
                        step.get("selector").and_then(Value::as_str),
                        step.get("target").and_then(Value::as_str),
                        &script_error("press".into()),
                    )?;
                    step["key"] = json!(key);
                    if let Some(r#ref) = positional_ref {
                        check_ref_token(r#ref).map_err(script_error)?;
                        step["ref"] = json!(r#ref);
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
                    let raw = rest.get(1).cloned().unwrap_or_else(|| "1".to_owned());
                    let count: u64 = raw
                        .parse()
                        .map_err(|_| script_error("invalid scroll count".into()))?;
                    if count == 0 {
                        return Err(script_error(
                            "scroll count must be a positive integer".into(),
                        ));
                    }
                    if count > 1000 {
                        return Err(script_error("scroll count must be at most 1000".into()));
                    }
                    step["text"] = json!(format!("{direction} {count}"));
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
        return Err(
            "script: no steps found (only comments/blank lines?); run `ema2 act --help`".into(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(call: &ToolCall) -> &Value {
        &call.args
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
    fn act_wait_for_page_conditions() {
        let call = build_act(&[
            "click".into(),
            "e12".into(),
            "--wait-for".into(),
            "text=评论已提交 timeout=5s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["waitFor"]["text"], "评论已提交");
        assert_eq!(args_of(&call)["waitFor"]["timeout"], 5.0);

        let call = build_act(&[
            "click".into(),
            "e12".into(),
            "--wait-for".into(),
            "selector=#comment-18 timeout=5s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["waitFor"]["selector"], "#comment-18");

        let call = build_act(&[
            "click".into(),
            "e12".into(),
            "--wait-for".into(),
            "textGone=\"正在提交\" timeout=2s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["waitFor"]["textGone"], "正在提交");

        // 页面条件和网络条件语义不同，不能混在一次 waitFor 里。
        for input in [
            "text=done selector=#done",
            "text=done url=api/save",
            "textGone=loading status=200",
            "text=",
            "timeout=5s",
        ] {
            let error = build_act(&[
                "click".into(),
                "e1".into(),
                "--wait-for".into(),
                input.into(),
            ])
            .err()
            .expect("invalid waitFor must fail");
            assert!(error.contains("waitFor"), "{input}: {error}");
        }
    }

    #[test]
    fn act_script_wait_for_page_condition() {
        let parsed =
            parse_act_script("click e1\nwaitFor text=\"保存成功\" timeout=3s\nclick e2\n").unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps[1]["action"], "waitFor");
        assert_eq!(steps[1]["text"], "保存成功");
        assert_eq!(steps[1]["timeout"], 3.0);
        assert_eq!(steps[2]["action"], "click");
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
    fn script_errors_point_to_the_cli_alternative() {
        let open = parse_act_script("open https://example.com").unwrap_err();
        assert!(open.contains("unknown action"), "{open}");
        assert!(open.contains("ema2 open"), "{open}");
        assert!(open.contains(SCRIPT_ACTIONS_HINT), "{open}");

        let wait = parse_act_script("wait text Done").unwrap_err();
        assert!(wait.contains("ema2 wait"), "{wait}");

        let wait_for = parse_act_script("waitFor time 1s").unwrap_err();
        assert!(wait_for.contains("await 1s"), "{wait_for}");

        let empty = parse_act_script("# only a comment").unwrap_err();
        assert!(empty.contains("ema2 act --help"), "{empty}");
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

        // selector=/target= 只在能定位的动作上有意义。
        assert!(parse_act_script("back selector=#x").is_err());
        assert!(parse_act_script("scroll target=button:x").is_err());
        assert!(parse_act_script("click selector=#x").is_ok());
    }

    #[test]
    fn act_port_rejects_zero() {
        // parse_port 本身在 shared::tests 覆盖；这里确认 act 的 --port 走同一校验。
        assert!(build_act(&["click".into(), "e1".into(), "--port".into(), "0".into(),]).is_err());
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
        // 文档后缀必须是 d<digits>，与页面 helper 的 parseRef 一致。
        assert!(build_act(&["click".into(), "e12@dfoo".into()]).is_err());
        assert!(build_act(&["click".into(), "e12@d1x".into()]).is_err());
        assert!(build_act(&["click".into(), "e12@d".into()]).is_err());
    }

    #[test]
    fn act_and_script_accept_an_explicit_tab() {
        let call = build_act(&["click".into(), "e12".into(), "--tab".into(), "7".into()]).unwrap();
        assert_eq!(args_of(&call)["tabId"], 7);

        let call = build_act(&[
            "--script".into(),
            {
                let path = std::env::temp_dir().join(format!(
                    "ema2-act-tab-{}-{}.ema2",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::write(&path, "click e1\n").unwrap();
                path.to_string_lossy().into_owned()
            },
            "--tab".into(),
            "8".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["tabId"], 8);
    }

    #[test]
    fn act_double_dash_allows_hyphen_text() {
        let call = build_act(&["type".into(), "e1".into(), "--".into(), "-5".into()]).unwrap();
        assert_eq!(args_of(&call)["text"], "-5");
        assert_eq!(args_of(&call)["ref"], "e1");

        let call = build_act(&[
            "fill".into(),
            "--selector".into(),
            "#x".into(),
            "--".into(),
            "--literal".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["fields"][0]["value"], "--literal");

        // 还是不能把拼错的 flag 当 ref。
        assert!(build_act(&["click".into(), "--at".into(), "1,2".into()]).is_err());
    }

    #[test]
    fn act_submit_is_a_flag_even_before_positionals() {
        // `--submit` 是布尔 flag，不能在 `type` 前吞掉 ref/text。
        let call = build_act(&[
            "type".into(),
            "--submit".into(),
            "e1".into(),
            "hello".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["ref"], "e1");
        assert_eq!(args_of(&call)["text"], "hello");
        assert_eq!(args_of(&call)["submit"], true);

        let call = build_act(&[
            "--submit".into(),
            "type".into(),
            "e1".into(),
            "hello".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["ref"], "e1");
        assert_eq!(args_of(&call)["text"], "hello");
        assert_eq!(args_of(&call)["submit"], true);
    }

    #[test]
    fn act_rejects_flags_that_cannot_apply() {
        // 单步里放错动作的 flag：不能静默忽略。
        assert!(build_act(&["click".into(), "e1".into(), "--key".into(), "Enter".into()]).is_err());
        assert!(
            build_act(&["click".into(), "e1".into(), "--delta-y".into(), "10".into()]).is_err()
        );
        assert!(build_act(&["back".into(), "--selector".into(), "#x".into()]).is_err());
        assert!(build_act(&["scroll".into(), "--target".into(), "button:x".into()]).is_err());

        // 多步脚本里的 flag 必须写在脚本行上，否则调用方会误以为已提交/定位。
        assert!(build_act(&["--steps".into(), "type e1 hi".into(), "--submit".into()]).is_err());
        assert!(
            build_act(&[
                "--script".into(),
                "x.ema2".into(),
                "--selector".into(),
                "#x".into()
            ])
            .is_err()
        );

        // 合法组合仍然可用。
        assert!(build_act(&["press".into(), "e1".into(), "--key".into(), "Enter".into()]).is_ok());
        assert!(build_act(&["scroll".into(), "--delta-y".into(), "-100".into()]).is_ok());
    }

    /// P1-2：脚本里的 waitFor 不能把已经分好的 token 再按空格撕开。
    #[test]
    fn script_wait_for_keeps_quoted_spaces() {
        let parsed = parse_act_script("waitFor selector=\"#main a\" timeout=3s").unwrap();
        assert_eq!(parsed["steps"][0]["selector"], "#main a");
        assert_eq!(parsed["steps"][0]["timeout"], 3.0);

        let parsed = parse_act_script("waitFor text=\"保存 成功\" timeout=3s").unwrap();
        assert_eq!(parsed["steps"][0]["text"], "保存 成功");

        let call = build_act(&["--steps".into(), "waitFor selector=\"#main a\"".into()]).unwrap();
        assert_eq!(call.args["steps"][0]["selector"], "#main a");

        // 未加引号时仍然报 key=value，而不是把第二个词当条件。
        let error = parse_act_script("waitFor selector=#main a").unwrap_err();
        assert!(error.contains("expected key=value"), "{error}");

        // --wait-for 字符串入口先 tokenize 一次，再走同一个函数。
        let call = build_act(&[
            "click".into(),
            "e1".into(),
            "--wait-for".into(),
            "selector=\"#main a\" timeout=3s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["waitFor"]["selector"], "#main a");
    }

    /// P1-3：裸 ref 与 selector=/target= 不能同时出现（扩展按 ref 优先，其余会被忽略）。
    #[test]
    fn locators_are_mutually_exclusive() {
        let error = build_act(&[
            "click".into(),
            "e12".into(),
            "--selector".into(),
            "#x".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("use only one of ref"), "{error}");

        let error = build_act(&[
            "click".into(),
            "e12".into(),
            "--selector".into(),
            "#x".into(),
            "--target".into(),
            "button:y".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("use only one of ref"), "{error}");

        let error = build_act(&[
            "press".into(),
            "e1".into(),
            "Enter".into(),
            "--selector".into(),
            "#x".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("use only one of ref"), "{error}");

        // 脚本分支同样互斥。
        for script in [
            "click selector=#x e1",
            "fill selector=#x e1 hello",
            "press selector=#x e1 Enter",
            "type target=button:x e1 hello",
        ] {
            let error = parse_act_script(script).unwrap_err();
            assert!(error.contains("use only one of ref"), "{script}: {error}");
        }
    }

    /// 多参数报错要区分动作：click 没有“加引号”这种说法。
    #[test]
    fn extra_argument_hint_depends_on_action() {
        let error = build_act(&["click".into(), "e1".into(), "extra".into()])
            .err()
            .expect("expected an error");
        assert!(error.contains("takes at most one ref"), "{error}");
        assert!(!error.contains("quote values with spaces"), "{error}");

        let error = build_act(&["type".into(), "e1".into(), "hello".into(), "world".into()])
            .err()
            .expect("expected an error");
        assert!(error.contains("quote values with spaces"), "{error}");

        let error = parse_act_script("click e1 extra").unwrap_err();
        assert!(error.contains("takes at most one ref"), "{error}");
    }

    #[test]
    fn flags_accept_inline_values_and_reject_duplicates() {
        let call = build_act(&[
            "click".into(),
            "e1".into(),
            "--tab=3".into(),
            "--wait-for=url=api/save timeout=2s".into(),
        ])
        .unwrap();
        assert_eq!(args_of(&call)["tabId"], 3);
        assert_eq!(args_of(&call)["waitFor"]["url"], "api/save");

        let call = build_act(&["press".into(), "e1".into(), "--key=Enter".into()]).unwrap();
        assert_eq!(args_of(&call)["key"], "Enter");

        let error = build_act(&[
            "click".into(),
            "e1".into(),
            "--tab".into(),
            "1".into(),
            "--tab".into(),
            "2".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("--tab given twice"), "{error}");

        let error = build_act(&[
            "click".into(),
            "--selector=#a".into(),
            "--selector=#b".into(),
        ])
        .err()
        .expect("expected an error");
        assert!(error.contains("--selector given twice"), "{error}");

        assert!(build_act(&["click".into(), "e1".into(), "--tab=".into()]).is_err());
        assert!(build_act(&["click".into(), "e1".into(), "--submit=true".into()]).is_err());
    }

    #[test]
    fn numeric_flags_reject_non_finite_and_oversized_values() {
        assert!(build_act(&["scroll".into(), "--delta-y".into(), "nan".into()]).is_err());
        assert!(build_act(&["scroll".into(), "--delta-y=inf".into()]).is_err());
        assert!(build_act(&["scroll".into(), "--delta-y".into(), "-100".into()]).is_ok());

        // 1e30 以前会被 saturating cast 成 u64::MAX。
        assert!(build_act(&["scroll".into(), "down".into(), "1e30".into()]).is_err());
        assert!(build_act(&["scroll".into(), "down".into(), "1001".into()]).is_err());
        assert!(build_act(&["scroll".into(), "down".into(), "1000".into()]).is_ok());
        let error = build_act(&["scroll".into(), "down".into(), "0".into()])
            .err()
            .expect("expected an error");
        assert!(error.contains("positive integer"), "{error}");
        assert!(parse_act_script("scroll down 1e30").is_err());
        assert!(parse_act_script("scroll down 0").is_err());
    }

    #[test]
    fn script_strips_bom_and_trailing_comments() {
        let parsed = parse_act_script(
            "\u{feff}click e1\ntype e2 \"#hashtag\" # 行尾注释\nfill e3 \"a#b\"\n",
        )
        .unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["action"], "click");
        assert_eq!(steps[1]["text"], "#hashtag");
        assert_eq!(steps[2]["fields"][0]["value"], "a#b");

        // 注释吞掉同一行后面的语句；紧贴 token 的 `#` 不算注释。
        let parsed = parse_act_script("click e1 # note; click e2").unwrap();
        assert_eq!(parsed["steps"].as_array().unwrap().len(), 1);
        let parsed = parse_act_script("click e1#frag").unwrap_err();
        assert!(parsed.contains("not a ref"), "{parsed}");
    }

    #[test]
    fn steps_limit_checks_script_before_adding_wait_for() {
        let twenty = "click e1\n".repeat(20);
        let call = build_act(&[
            "--steps".into(),
            twenty.clone(),
            "--wait-for".into(),
            "url=api/save".into(),
        ])
        .unwrap();
        assert_eq!(call.args["steps"].as_array().unwrap().len(), 21);

        let twenty_one = "click e1\n".repeat(21);
        let error = build_act(&["--steps".into(), twenty_one])
            .err()
            .expect("expected an error");
        assert!(error.contains("script must contain 1..20 steps"), "{error}");
    }
}
