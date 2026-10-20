//! 把已配对的 MCP 地址写进本机已安装的 AI 客户端配置。
//!
//! 目标：用户不需要手工编辑配置——授权页勾选终端，中继直接写入（改前备份）。
//! 只处理**已知**客户端和固定键名，不接受任意路径。
//!
//! 关键点：opencode 的配置是带注释的 `.jsonc`，Codex 是 TOML。这里全部采用
//! **文本级合并**（见 `jsonc` 模块），保留用户原有的注释、键顺序和排版。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use crate::jsonc;

pub const CLIENT_ID: &str = "edge-browser";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Jsonc,
    Toml,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Jsonc => "jsonc",
            Format::Toml => "toml",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub path: PathBuf,
    pub format: Format,
    /// 本机是否安装了该终端（配置目录存在）。
    pub detected: bool,
    /// 配置里是否已经有 `edge-browser` 条目。
    pub configured: bool,
    /// 给人看的补充信息（解析失败、未安装等）。
    pub note: Option<String>,
}

impl ClientSpec {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "path": self.path.display().to_string(),
            "format": self.format.as_str(),
            "detected": self.detected,
            "configured": self.configured,
            "note": self.note,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ClientResult {
    pub id: &'static str,
    pub label: &'static str,
    pub path: PathBuf,
    /// created / updated / unchanged / removed / skipped / error
    pub action: &'static str,
    pub error: Option<String>,
    pub note: Option<String>,
    /// 本次修改前的备份（新建文件时是 0 字节文件）。
    pub backup: Option<PathBuf>,
}

impl ClientResult {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "path": self.path.display().to_string(),
            "action": self.action,
            "error": self.error,
            "note": self.note,
            "backup": self.backup.as_ref().map(|path| path.display().to_string()),
        })
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Opencode,
    ClaudeDesktop,
    ClaudeCode,
    Pi,
    CodexToml,
}

struct Target {
    id: &'static str,
    label: &'static str,
    path: PathBuf,
    kind: Kind,
    detected: bool,
    note: Option<String>,
}

impl Target {
    fn format(&self) -> Format {
        match self.kind {
            Kind::CodexToml => Format::Toml,
            _ if self.path.extension().is_some_and(|ext| ext == "jsonc") => Format::Jsonc,
            _ => Format::Json,
        }
    }

    fn section(&self) -> &'static str {
        match self.kind {
            Kind::Opencode => "mcp",
            Kind::ClaudeDesktop | Kind::ClaudeCode | Kind::Pi => "mcpServers",
            Kind::CodexToml => "mcp_servers",
        }
    }
}

/// 全部已知终端（含未安装的），供授权页展示。
pub fn specs() -> Vec<ClientSpec> {
    targets()
        .into_iter()
        .map(|target| {
            let (configured, note) = if target.detected {
                match entry_state(&target) {
                    Ok(configured) => (configured, target.note.clone()),
                    Err(error) => (false, Some(error)),
                }
            } else {
                (false, target.note.clone())
            };
            ClientSpec {
                id: target.id,
                label: target.label,
                path: target.path.clone(),
                format: target.format(),
                detected: target.detected,
                configured,
                note,
            }
        })
        .collect()
}

fn targets() -> Vec<Target> {
    let mut targets = Vec::new();
    let home = home_dir();
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);

    if let Some(home) = &home {
        let dir = home.join(".config").join("opencode");
        targets.push(Target {
            id: "opencode",
            label: "opencode",
            path: opencode_path(&dir),
            kind: Kind::Opencode,
            detected: dir.is_dir(),
            note: None,
        });

        let claude = home.join(".claude.json");
        targets.push(Target {
            id: "claude-code",
            label: "Claude Code",
            path: claude.clone(),
            kind: Kind::ClaudeCode,
            detected: claude.is_file() || home.join(".claude").is_dir(),
            note: None,
        });

        let codex_dir = home.join(".codex");
        let codex = codex_dir.join("config.toml");
        targets.push(Target {
            id: "codex",
            label: "Codex",
            path: codex,
            kind: Kind::CodexToml,
            detected: codex_dir.is_dir(),
            note: None,
        });

        let pi_dir = home.join(".pi").join("agent");
        let pi = pi_dir.join("mcp.json");
        targets.push(Target {
            id: "pi",
            label: "Pi",
            path: pi,
            kind: Kind::Pi,
            detected: pi_dir.is_dir(),
            note: None,
        });
    } else {
        for (id, label, kind) in [
            ("opencode", "opencode", Kind::Opencode),
            ("claude-code", "Claude Code", Kind::ClaudeCode),
            ("codex", "Codex", Kind::CodexToml),
            ("pi", "Pi", Kind::Pi),
        ] {
            targets.push(Target {
                id,
                label,
                path: PathBuf::from(format!("<home unavailable>/{id}")),
                kind,
                detected: false,
                note: Some("cannot locate the user home directory".into()),
            });
        }
    }

    if let Some(appdata) = &appdata {
        let dir = appdata.join("Claude");
        targets.push(Target {
            id: "claude-desktop",
            label: "Claude Desktop",
            path: dir.join("claude_desktop_config.json"),
            kind: Kind::ClaudeDesktop,
            detected: dir.is_dir(),
            note: None,
        });
    } else {
        targets.push(Target {
            id: "claude-desktop",
            label: "Claude Desktop",
            path: PathBuf::from("<APPDATA unavailable>/Claude/claude_desktop_config.json"),
            kind: Kind::ClaudeDesktop,
            detected: false,
            note: Some("APPDATA is not set".into()),
        });
    }

    targets
}

/// 优先沿用已有文件；两者都不存在时用 `.json`（新建）。
fn opencode_path(dir: &Path) -> PathBuf {
    let json = dir.join("opencode.json");
    let jsonc = dir.join("opencode.jsonc");
    if !json.exists() && jsonc.exists() {
        return jsonc;
    }
    json
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn skip_client_config() -> bool {
    std::env::var("EMA2_SKIP_CLIENT_CONFIG")
        .map(|value| crate::config::truthy(&value))
        .unwrap_or(false)
}

/// 把用户勾选的 id 收敛成“确实安装了的”终端；`None` 表示全选。
pub fn normalize(requested: Option<&[String]>) -> Vec<String> {
    let list = specs();
    normalize_from(&list, requested)
}

fn normalize_from(list: &[ClientSpec], requested: Option<&[String]>) -> Vec<String> {
    match requested {
        None => list
            .iter()
            .filter(|spec| spec.detected)
            .map(|spec| spec.id.to_owned())
            .collect(),
        Some(ids) => {
            let mut out = Vec::new();
            for id in ids {
                if !out.contains(id)
                    && list
                        .iter()
                        .any(|spec| spec.detected && spec.id == id.as_str())
                {
                    out.push(id.clone());
                }
            }
            out
        }
    }
}

/// 授权页的“保存选择”：写入勾选的，并移除未勾选里已有的 `edge-browser` 条目。
///
/// 这样“取消勾选”才是真正的还原，而不是只改 state.json 里的选择。
pub fn apply(ids: &[String], port: u16, token: &str) -> Vec<ClientResult> {
    if skip_client_config() {
        return Vec::new();
    }
    apply_to(targets(), ids, port, token)
}

fn apply_to(targets: Vec<Target>, ids: &[String], port: u16, token: &str) -> Vec<ClientResult> {
    let mut results = Vec::new();
    for target in targets {
        let selected = ids.iter().any(|id| id == target.id);
        if selected {
            results.push(configure_target(&target, port, token));
            continue;
        }
        if !target.detected || !target.path.is_file() {
            continue;
        }
        match entry_is_ours(&target) {
            Ok(Some(true)) => results.push(remove_target(&target)),
            Ok(Some(false)) => results.push(ClientResult {
                id: target.id,
                label: target.label,
                path: target.path.clone(),
                action: "skipped",
                error: None,
                note: Some("entry was not created by EMA2; left untouched".into()),
                backup: None,
            }),
            Ok(None) => {}
            Err(error) => results.push(ClientResult {
                id: target.id,
                label: target.label,
                path: target.path.clone(),
                action: "error",
                error: Some(error),
                note: None,
                backup: None,
            }),
        }
    }
    results
}

fn configure_target(target: &Target, port: u16, token: &str) -> ClientResult {
    if !target.detected {
        return ClientResult {
            id: target.id,
            label: target.label,
            path: target.path.clone(),
            action: "skipped",
            error: None,
            note: Some("not installed".into()),
            backup: None,
        };
    }
    let result = match target.kind {
        Kind::CodexToml => write_toml_target(&target.path, port, token),
        _ => write_json_target(
            &target.path,
            target.section(),
            &json_entry(target.kind, port, token),
            target.format(),
        ),
    };
    match result {
        Ok(action) => ClientResult {
            id: target.id,
            label: target.label,
            path: target.path.clone(),
            action,
            error: None,
            note: target.note.clone(),
            backup: if action == "unchanged" {
                // 首次接管一个早已存在、但本次无需改写的配置：补一份基线备份。
                ensure_baseline_backup(&target.path)
            } else {
                backup_for_action(&target.path, action)
            },
        },
        Err(error) => ClientResult {
            id: target.id,
            label: target.label,
            path: target.path.clone(),
            action: "error",
            error: Some(error),
            note: None,
            backup: None,
        },
    }
}

fn remove_target(target: &Target) -> ClientResult {
    let outcome = match target.kind {
        Kind::CodexToml => remove_toml_target(&target.path),
        _ => remove_json_target(&target.path, target.section()),
    };
    match outcome {
        Ok(action) => ClientResult {
            id: target.id,
            label: target.label,
            path: target.path.clone(),
            action,
            error: None,
            note: None,
            backup: backup_for_action(&target.path, action),
        },
        Err(error) => ClientResult {
            id: target.id,
            label: target.label,
            path: target.path.clone(),
            action: "error",
            error: Some(error),
            note: None,
            backup: None,
        },
    }
}

/// 配置没变但还没有备份时，补一份“当前内容”作为基线。
fn ensure_baseline_backup(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    let backup = backup_path(path);
    if backup.is_file() {
        return Some(backup);
    }
    fs::copy(path, &backup).ok()?;
    Some(backup)
}

fn backup_for_action(path: &Path, action: &str) -> Option<PathBuf> {
    if action == "unchanged" {
        return None;
    }
    let backup = backup_path(path);
    backup.is_file().then_some(backup)
}

/// 卸载：从所有已知终端里删掉 `edge-browser` 条目。
pub fn unconfigure_all() -> Vec<ClientResult> {
    targets()
        .into_iter()
        .filter(|target| target.detected && target.path.is_file())
        .map(|target| remove_target(&target))
        .filter(|result| result.action != "unchanged")
        .collect()
}

/// 配置里的条目是否像 EMA2 写的：url 指向本机 `/mcp`，或带 64 位 hex bearer token。
fn entry_is_ours(target: &Target) -> Result<Option<bool>, String> {
    if !target.path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&target.path).map_err(|error| format!("cannot read: {error}"))?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    match target.kind {
        Kind::CodexToml => {
            let value: toml::Value =
                toml::from_str(&text).map_err(|error| format!("invalid TOML: {error}"))?;
            let Some(server) = value
                .get("mcp_servers")
                .and_then(|servers| servers.get(CLIENT_ID))
            else {
                return Ok(None);
            };
            let url = server
                .get("url")
                .and_then(toml::Value::as_str)
                .unwrap_or("");
            let token = server
                .get("http_headers")
                .and_then(|headers| headers.get("Authorization"))
                .and_then(toml::Value::as_str)
                .unwrap_or("");
            Ok(Some(url_is_ours(url) || bearer_is_ours(token)))
        }
        _ => {
            let value =
                jsonc::validate(&text).map_err(|error| format!("invalid JSON/JSONC: {error}"))?;
            let Some(entry) = value
                .get(target.section())
                .and_then(|section| section.get(CLIENT_ID))
            else {
                return Ok(None);
            };
            let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
            let token = entry
                .get("headers")
                .and_then(|headers| headers.get("Authorization"))
                .and_then(Value::as_str)
                .unwrap_or("");
            Ok(Some(url_is_ours(url) || bearer_is_ours(token)))
        }
    }
}

fn url_is_ours(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp")
}

fn bearer_is_ours(header: &str) -> bool {
    header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .is_some_and(|token| {
            token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn json_matches(current: Option<&Value>, entry: &Value) -> bool {
    let Some(current) = current else {
        return false;
    };
    let field = |value: &Value, key: &str| value.get(key).cloned();
    field(current, "url") == field(entry, "url")
        && current
            .get("headers")
            .and_then(|headers| headers.get("Authorization"))
            == entry
                .get("headers")
                .and_then(|headers| headers.get("Authorization"))
}

fn json_entry(kind: Kind, port: u16, token: &str) -> Value {
    let url = format!("http://127.0.0.1:{port}/mcp");
    let headers = json!({ "Authorization": format!("Bearer {token}") });
    match kind {
        Kind::Opencode => json!({
            "type": "remote",
            "url": url,
            "enabled": true,
            "headers": headers,
        }),
        Kind::ClaudeCode => json!({
            "type": "http",
            "url": url,
            "headers": headers,
        }),
        Kind::ClaudeDesktop | Kind::Pi => json!({
            "url": url,
            "headers": headers,
        }),
        Kind::CodexToml => json!(null),
    }
}

/// `.claude.json` 可能几十 MB；授权页每 2 秒轮询一次，不能每次完整解析。
/// 用 (mtime, size) 作为缓存键，文件变化后自动失效。
type StateStamp = (u128, u64, bool);

fn state_cache() -> &'static Mutex<HashMap<PathBuf, StateStamp>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, StateStamp>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_state(target: &Target) -> Result<bool, String> {
    if !target.path.is_file() {
        return Ok(false);
    }
    let meta = fs::metadata(&target.path).map_err(|error| format!("cannot stat: {error}"))?;
    let stamp = (
        meta.modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
        meta.len(),
    );
    if let Some((mtime, len, configured)) = state_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&target.path)
        && (*mtime, *len) == stamp
    {
        return Ok(*configured);
    }

    let text = fs::read_to_string(&target.path).map_err(|error| format!("cannot read: {error}"))?;
    if text.trim().is_empty() {
        return Ok(false);
    }
    let configured = match target.kind {
        Kind::CodexToml => {
            let value: toml::Value =
                toml::from_str(&text).map_err(|error| format!("invalid TOML: {error}"))?;
            value
                .get("mcp_servers")
                .and_then(|servers| servers.get(CLIENT_ID))
                .is_some()
        }
        _ => {
            let value =
                jsonc::validate(&text).map_err(|error| format!("invalid JSON/JSONC: {error}"))?;
            value
                .get(target.section())
                .and_then(|section| section.get(CLIENT_ID))
                .is_some()
        }
    };
    state_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(target.path.clone(), (stamp.0, stamp.1, configured));
    Ok(configured)
}

fn write_json_target(
    path: &Path,
    section: &str,
    entry: &Value,
    format: Format,
) -> Result<&'static str, String> {
    let existed = path.is_file();
    let raw = if existed {
        fs::read_to_string(path).map_err(|error| format!("cannot read: {error}"))?
    } else {
        String::new()
    };
    let text = if raw.trim().is_empty() {
        "{}".to_owned()
    } else {
        // 先完整校验，别在畸形文件上做文本手术。
        jsonc::validate(&raw).map_err(|error| format!("invalid JSON/JSONC: {error}"))?;
        raw
    };
    let entry_text = serde_json::to_string(entry).map_err(|error| error.to_string())?;
    let root = jsonc::root_object(&text)?;

    let updated = match jsonc::member_value(&text, root, section) {
        Some(span) => {
            let current = jsonc::validate(&text[span.start..span.end])
                .map_err(|error| format!("`{section}` is not valid JSON: {error}"))?;
            if !current.is_object() {
                return Err(format!("`{section}` is not a JSON object"));
            }
            if json_matches(current.get(CLIENT_ID), entry) {
                return Ok("unchanged");
            }
            jsonc::upsert_member(&text, span, CLIENT_ID, &entry_text)?
        }
        None => {
            let section_text = format!("{{ \"{CLIENT_ID}\": {entry_text} }}");
            jsonc::upsert_member(&text, root, section, &section_text)?
        }
    };

    let parsed = jsonc::validate(&updated)?;
    if parsed.get(section).and_then(|value| value.get(CLIENT_ID)) != Some(entry) {
        return Err("merge result does not contain the expected entry".into());
    }
    if format != Format::Jsonc {
        // 严格 JSON 配置：合并后必须仍是合法 JSON，不能让注释混进来。
        serde_json::from_str::<Value>(&updated)
            .map_err(|error| format!("merged config is not strict JSON: {error}"))?;
    }
    write_backup(path, existed)?;
    write_atomic(path, &updated)?;
    Ok(if existed { "updated" } else { "created" })
}

fn remove_json_target(path: &Path, section: &str) -> Result<&'static str, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("cannot read: {error}"))?;
    if text.trim().is_empty() {
        return Ok("unchanged");
    }
    jsonc::validate(&text).map_err(|error| format!("invalid JSON/JSONC: {error}"))?;
    let root = jsonc::root_object(&text)?;
    let Some(span) = jsonc::member_value(&text, root, section) else {
        return Ok("unchanged");
    };
    let Some(updated) = jsonc::remove_member(&text, span, CLIENT_ID)? else {
        return Ok("unchanged");
    };
    jsonc::validate(&updated)?;
    write_backup(path, true)?;
    write_atomic(path, &updated)?;
    Ok("removed")
}

// ---------------------------------------------------------------- Codex (TOML)

const CODEX_HEADER: &str = "[mcp_servers.edge-browser]";

fn toml_block(port: u16, token: &str, newline: &str) -> String {
    format!(
        "{CODEX_HEADER}{newline}url = \"{}\"{newline}http_headers = {{ Authorization = \"{}\" }}{newline}",
        toml_escape(&format!("http://127.0.0.1:{port}/mcp")),
        toml_escape(&format!("Bearer {token}")),
    )
}

fn toml_matches(server: Option<&toml::Value>, port: u16, token: &str) -> bool {
    let Some(server) = server else {
        return false;
    };
    let url = server.get("url").and_then(toml::Value::as_str);
    let header = server
        .get("http_headers")
        .and_then(|headers| headers.get("Authorization"))
        .and_then(toml::Value::as_str);
    url == Some(format!("http://127.0.0.1:{port}/mcp").as_str())
        && header == Some(format!("Bearer {token}").as_str())
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_toml_target(path: &Path, port: u16, token: &str) -> Result<&'static str, String> {
    let existed = path.is_file();
    let raw = if existed {
        fs::read_to_string(path).map_err(|error| format!("cannot read: {error}"))?
    } else {
        String::new()
    };
    if !raw.trim().is_empty() {
        toml::from_str::<toml::Value>(&raw).map_err(|error| format!("invalid TOML: {error}"))?;
    }
    let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let block = toml_block(port, token, newline);

    let current = toml::from_str::<toml::Value>(&raw).ok().and_then(|value| {
        value
            .get("mcp_servers")
            .and_then(|servers| servers.get(CLIENT_ID))
            .cloned()
    });
    // 只比较我们负责的两个字段：用户可能给这张表加了别的键，不能每次 install 都抹掉。
    if toml_matches(current.as_ref(), port, token) {
        return Ok("unchanged");
    }

    let updated = splice_toml(&raw, &block, newline);
    let parsed: toml::Value =
        toml::from_str(&updated).map_err(|error| format!("merged TOML is invalid: {error}"))?;
    if parsed
        .get("mcp_servers")
        .and_then(|servers| servers.get(CLIENT_ID))
        .and_then(|server| server.get("url"))
        .and_then(toml::Value::as_str)
        != Some(format!("http://127.0.0.1:{port}/mcp").as_str())
    {
        return Err("merge result does not contain the expected server".into());
    }
    write_backup(path, existed)?;
    write_atomic(path, &updated)?;
    Ok(if existed { "updated" } else { "created" })
}

fn remove_toml_target(path: &Path) -> Result<&'static str, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("cannot read: {error}"))?;
    let lines = split_lines(&text);
    let Some(start) = lines.iter().position(|line| line.trim() == CODEX_HEADER) else {
        return Ok("unchanged");
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim_start().starts_with('['))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let mut kept: Vec<&str> = Vec::new();
    kept.extend_from_slice(&lines[..start]);
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    // 保留下一个表头前的一行空行，读起来和原来的 TOML 一致。
    if !kept.is_empty()
        && lines
            .get(end)
            .is_some_and(|line| line.trim_start().starts_with('['))
    {
        kept.push("");
    }
    kept.extend(lines[end..].iter());
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let updated = kept.join(newline);
    if !updated.trim().is_empty() {
        toml::from_str::<toml::Value>(&updated)
            .map_err(|error| format!("removing the server would corrupt the file: {error}"))?;
    }
    write_backup(path, true)?;
    write_atomic(path, &updated)?;
    Ok("removed")
}

/// 按行切分并去掉 CRLF 的 `\r`，保证重新拼接时不会出现双 CR。
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn splice_toml(text: &str, block: &str, newline: &str) -> String {
    let lines = split_lines(text);
    if let Some(start) = lines.iter().position(|line| line.trim() == CODEX_HEADER) {
        let end = lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, line)| line.trim_start().starts_with('['))
            .map(|(index, _)| index)
            .unwrap_or(lines.len());
        let mut kept: Vec<&str> = Vec::new();
        kept.extend_from_slice(&lines[..start]);
        kept.extend(block.trim_end_matches(['\r', '\n']).split(newline));
        kept.push("");
        kept.extend(lines[end..].iter());
        return kept.join(newline);
    }
    let body = text.trim_end_matches(['\r', '\n']);
    if body.is_empty() {
        block.to_owned()
    } else {
        format!("{body}{newline}{newline}{block}")
    }
}

// ---------------------------------------------------------------- shared

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "{}.ema2.bak",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json")
    ))
}

/// 每次写入前刷新备份：`.ema2.bak` 永远是“本次修改前”的内容。
///
/// 文件原本不存在时写一个 0 字节备份——明确表示“没有旧配置可还原”，
/// 而不是让用户以为备份失败。
fn write_backup(path: &Path, existed: bool) -> Result<(), String> {
    let backup = backup_path(path);
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("cannot create dir: {error}"))?;
    }
    if existed {
        fs::copy(path, &backup).map_err(|error| format!("cannot back up: {error}"))?;
    } else if !backup.exists() {
        fs::write(&backup, b"").map_err(|error| format!("cannot create backup: {error}"))?;
    }
    Ok(())
}

fn invalidate_state_cache(path: &Path) {
    state_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path);
}

fn write_atomic(path: &Path, body: &str) -> Result<(), String> {
    invalidate_state_cache(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("cannot create dir: {error}"))?;
    }
    let tmp = path.with_extension("ema2.tmp");
    fs::write(&tmp, body).map_err(|error| format!("cannot write: {error}"))?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| format!("cannot replace: {error}"))?;
    }
    fs::rename(&tmp, path).map_err(|error| format!("cannot rename: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ema2-clients-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn spec(id: &'static str, detected: bool) -> ClientSpec {
        ClientSpec {
            id,
            label: id,
            path: PathBuf::from("x"),
            format: Format::Json,
            detected,
            configured: false,
            note: None,
        }
    }

    #[test]
    fn creates_opencode_config_with_remote_server() {
        let dir = temp_dir("opencode");
        let path = dir.join("opencode.json");
        assert_eq!(
            write_json_target(
                &path,
                "mcp",
                &json_entry(Kind::Opencode, 8765, "tok"),
                Format::Json
            )
            .unwrap(),
            "created"
        );
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["mcp"][CLIENT_ID]["type"], "remote");
        assert_eq!(value["mcp"][CLIENT_ID]["url"], "http://127.0.0.1:8765/mcp");
        assert_eq!(
            value["mcp"][CLIENT_ID]["headers"]["Authorization"],
            "Bearer tok"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn jsonc_merge_preserves_comments_and_formatting() {
        let dir = temp_dir("jsonc-merge");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.jsonc");
        let original = r#"{
  // 本地模型
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "lod": { "options": { "baseURL": "http://127.0.0.1:8080/v1" } }
  },
  /* 结尾注释 */
}
"#;
        fs::write(&path, original).unwrap();
        assert_eq!(
            write_json_target(
                &path,
                "mcp",
                &json_entry(Kind::Opencode, 8765, "tok"),
                Format::Jsonc
            )
            .unwrap(),
            "updated"
        );
        let updated = fs::read_to_string(&path).unwrap();
        assert!(
            updated.contains("// 本地模型"),
            "line comments must survive"
        );
        assert!(
            updated.contains("/* 结尾注释 */"),
            "block comments must survive"
        );
        assert!(updated.contains("\"baseURL\": \"http://127.0.0.1:8080/v1\""));
        let value = jsonc::validate(&updated).unwrap();
        assert_eq!(
            value["mcp"][CLIENT_ID]["headers"]["Authorization"],
            "Bearer tok"
        );
        assert_eq!(
            value["provider"]["lod"]["options"]["baseURL"],
            "http://127.0.0.1:8080/v1"
        );

        // 幂等：第二次写入不改文件。
        assert_eq!(
            write_json_target(
                &path,
                "mcp",
                &json_entry(Kind::Opencode, 8765, "tok"),
                Format::Jsonc
            )
            .unwrap(),
            "unchanged"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_the_section_when_missing_and_keeps_the_rest() {
        let dir = temp_dir("section");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.jsonc");
        fs::write(&path, "{\n  // only providers\n  \"model\": \"x\"\n}\n").unwrap();
        write_json_target(
            &path,
            "mcp",
            &json_entry(Kind::Opencode, 8765, "tok"),
            Format::Jsonc,
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("// only providers"));
        let value = jsonc::validate(&text).unwrap();
        assert_eq!(value["model"], "x");
        assert_eq!(value["mcp"][CLIENT_ID]["type"], "remote");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_code_uses_the_http_transport_shape() {
        let dir = temp_dir("claude-code");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        fs::write(&path, r#"{"installMethod":"native","mcpServers":{}}"#).unwrap();
        write_json_target(
            &path,
            "mcpServers",
            &json_entry(Kind::ClaudeCode, 8765, "tok"),
            Format::Json,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["installMethod"], "native");
        assert_eq!(value["mcpServers"][CLIENT_ID]["type"], "http");
        assert_eq!(
            value["mcpServers"][CLIENT_ID]["url"],
            "http://127.0.0.1:8765/mcp"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pi_gets_a_dedicated_mcp_json() {
        let dir = temp_dir("pi");
        let path = dir.join("mcp.json");
        assert_eq!(
            write_json_target(
                &path,
                "mcpServers",
                &json_entry(Kind::Pi, 8765, "tok"),
                Format::Json
            )
            .unwrap(),
            "created"
        );
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["mcpServers"][CLIENT_ID]["url"],
            "http://127.0.0.1:8765/mcp"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_json_instead_of_clobbering() {
        let dir = temp_dir("invalid");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        fs::write(&path, "{ not json").unwrap();
        let error = write_json_target(
            &path,
            "mcp",
            &json_entry(Kind::Opencode, 8765, "tok"),
            Format::Json,
        )
        .unwrap_err();
        assert!(error.contains("invalid JSON"), "unexpected error: {error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_toml_insert_is_surgical_and_idempotent() {
        let dir = temp_dir("codex");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "model = \"gpt-5\"\n\n[model_providers.custom]\nname = \"c\"\n\n[tui]\nnotifications = true\n",
        )
        .unwrap();
        assert_eq!(write_toml_target(&path, 8765, "tok").unwrap(), "updated");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("model = \"gpt-5\""));
        assert!(text.contains("[model_providers.custom]"));
        assert!(text.contains("[tui]"));
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            value["mcp_servers"]["edge-browser"]["url"].as_str(),
            Some("http://127.0.0.1:8765/mcp")
        );
        assert_eq!(
            value["mcp_servers"]["edge-browser"]["http_headers"]["Authorization"].as_str(),
            Some("Bearer tok")
        );
        assert_eq!(write_toml_target(&path, 8765, "tok").unwrap(), "unchanged");

        assert_eq!(write_toml_target(&path, 8765, "new").unwrap(), "updated");
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches(CODEX_HEADER).count(),
            1,
            "must replace, not duplicate"
        );
        assert!(text.contains("Bearer new"));
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value["tui"]["notifications"].as_bool(), Some(true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_toml_remove_drops_only_our_table() {
        let dir = temp_dir("codex-remove");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        write_toml_target(&path, 8765, "tok").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{text}\n[other]\nkey = 1\n")).unwrap();
        assert_eq!(remove_toml_target(&path).unwrap(), "removed");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(CODEX_HEADER));
        assert!(text.contains("[other]"));
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value["other"]["key"].as_integer(), Some(1));
        assert_eq!(remove_toml_target(&path).unwrap(), "unchanged");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unconfigure_removes_only_our_entry() {
        let dir = temp_dir("remove");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.jsonc");
        fs::write(
            &path,
            "{\n  \"mcp\": {\n    \"other\": { \"type\": \"remote\", \"url\": \"x\" },\n    \"edge-browser\": { \"type\": \"remote\" }\n  },\n  \"model\": \"m\"\n}\n",
        )
        .unwrap();
        assert_eq!(remove_json_target(&path, "mcp").unwrap(), "removed");
        let text = fs::read_to_string(&path).unwrap();
        let value = jsonc::validate(&text).unwrap();
        assert!(value["mcp"].get(CLIENT_ID).is_none());
        assert_eq!(value["mcp"]["other"]["url"], "x");
        assert_eq!(value["model"], "m");
        assert_eq!(remove_json_target(&path, "mcp").unwrap(), "unchanged");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn selection_only_keeps_installed_clients() {
        let list = vec![
            spec("opencode", true),
            spec("codex", true),
            spec("pi", false),
        ];
        assert_eq!(normalize_from(&list, None), vec!["opencode", "codex"]);
        assert_eq!(
            normalize_from(&list, Some(&["pi".into(), "codex".into(), "nope".into()])),
            vec!["codex"],
            "uninstalled and unknown ids are dropped"
        );
        assert!(normalize_from(&list, Some(&[])).is_empty());
    }

    fn target(id: &'static str, label: &'static str, path: PathBuf, kind: Kind) -> Target {
        Target {
            id,
            label,
            path,
            kind,
            detected: true,
            note: None,
        }
    }

    #[test]
    fn backup_always_matches_the_previous_content() {
        let dir = temp_dir("backup-refresh");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        fs::write(&path, "{\"model\": \"a\"}").unwrap();

        write_json_target(
            &path,
            "mcp",
            &json_entry(Kind::Opencode, 8765, "one"),
            Format::Json,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(backup_path(&path)).unwrap(),
            "{\"model\": \"a\"}",
            "backup must contain the pre-change content"
        );

        // 模拟用户手工改配置，再写一次：备份必须刷新成这次修改前的内容。
        fs::write(&path, "{\"model\": \"b\"}").unwrap();
        write_json_target(
            &path,
            "mcp",
            &json_entry(Kind::Opencode, 8765, "two"),
            Format::Json,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(backup_path(&path)).unwrap(),
            "{\"model\": \"b\"}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_a_file_leaves_an_empty_backup() {
        let dir = temp_dir("backup-empty");
        let path = dir.join("mcp.json");
        write_json_target(
            &path,
            "mcpServers",
            &json_entry(Kind::Pi, 8765, "tok"),
            Format::Json,
        )
        .unwrap();
        let backup = backup_path(&path);
        assert!(backup.is_file(), "new files still get a backup marker");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_write_creates_a_baseline_backup() {
        let dir = temp_dir("backup-baseline");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.json");
        write_json_target(
            &path,
            "mcpServers",
            &json_entry(Kind::Pi, 8765, "tok"),
            Format::Json,
        )
        .unwrap();
        let _ = fs::remove_file(backup_path(&path));
        let result = configure_target(&target("pi", "Pi", path.clone(), Kind::Pi), 8765, "tok");
        assert_eq!(result.action, "unchanged");
        assert!(
            result.backup.is_some(),
            "unchanged config should get a baseline backup"
        );
        assert_eq!(
            fs::read_to_string(backup_path(&path)).unwrap(),
            fs::read_to_string(&path).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_removes_unselected_entries_with_a_backup() {
        let dir = temp_dir("apply-remove");
        fs::create_dir_all(&dir).unwrap();
        let opencode = dir.join("opencode.json");
        let pi = dir.join("mcp.json");
        write_json_target(
            &opencode,
            "mcp",
            &json_entry(Kind::Opencode, 8765, "tok"),
            Format::Json,
        )
        .unwrap();
        write_json_target(
            &pi,
            "mcpServers",
            &json_entry(Kind::Pi, 8765, "tok"),
            Format::Json,
        )
        .unwrap();
        let _ = fs::remove_file(backup_path(&opencode));
        let _ = fs::remove_file(backup_path(&pi));

        let results = apply_to(
            vec![
                target("opencode", "opencode", opencode.clone(), Kind::Opencode),
                target("pi", "Pi", pi.clone(), Kind::Pi),
            ],
            &["opencode".to_owned()],
            8765,
            "tok",
        );
        assert!(results.iter().any(|result| {
            result.id == "pi" && result.action == "removed" && result.backup.is_some()
        }));
        assert!(!fs::read_to_string(&pi).unwrap().contains(CLIENT_ID));
        assert!(
            fs::read_to_string(backup_path(&pi))
                .unwrap()
                .contains(CLIENT_ID),
            "removal must back up the pre-removal content"
        );
        assert!(fs::read_to_string(&opencode).unwrap().contains(CLIENT_ID));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_leaves_foreign_entries_untouched() {
        let dir = temp_dir("apply-foreign");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            "{\"mcpServers\":{\"edge-browser\":{\"url\":\"https://example.com/mcp\"}}}",
        )
        .unwrap();
        let results = apply_to(
            vec![target("pi", "Pi", path.clone(), Kind::Pi)],
            &[],
            8765,
            "tok",
        );
        assert_eq!(results[0].action, "skipped");
        assert!(fs::read_to_string(&path).unwrap().contains("example.com"));
        let _ = fs::remove_dir_all(&dir);
    }
}
