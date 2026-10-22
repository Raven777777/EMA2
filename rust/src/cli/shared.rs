//! CLI 解析的公共 helper：端口/时长、token 游标、文件上传与 base64。

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

/// `--port 0` 会让 daemon 绑定随机端口，但 PID/日志文件仍按 0 生成，
/// 后续 `status`/`stop` 也找不到真实端口。端口必须在 1..=65535。
pub(super) fn parse_port(value: &str) -> Result<u16, String> {
    let port = value
        .parse::<u16>()
        .map_err(|_| format!("invalid port {value:?}: expected 1..65535"))?;
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    Ok(port)
}

/// "5s" / "500ms" / "5" → 秒（f64）。
pub(super) fn parse_duration_secs(input: &str) -> Result<f64, String> {
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

pub(super) fn set_opt<T: Into<Value>>(object: &mut Value, key: &str, value: Option<T>) {
    if let Some(value) = value {
        object[key] = value.into();
    }
}

/// `--flag=value` → `("--flag", Some("value"))`；普通 token 原样返回。
/// 只有以 `--` 开头的 token 才拆分，避免把 `selector=#x` 这类位置参数误当 flag。
pub(super) fn split_flag(token: &str) -> (&str, Option<String>) {
    if let Some((flag, value)) = token.split_once('=')
        && flag.starts_with("--")
    {
        return (flag, Some(value.to_owned()));
    }
    (token, None)
}

/// 顺序读取 token 的游标；越界返回 None，错误文案由调用方的 context/flag 决定。
///
/// act/batch 原来各自维护 `index` + `tokens.get(index)` 样板代码；行为完全一致：
/// - `required_next` 取紧跟当前 flag 的值（`--flag value`），不前进；
/// - `take_flag_value` 兼容 `--flag=value`，内联值只前进一格；
/// - 其余情况由调用方显式 `advance`。
pub(super) struct TokenCursor<'a> {
    tokens: &'a [String],
    index: usize,
}

impl<'a> TokenCursor<'a> {
    pub(super) fn new(tokens: &'a [String]) -> Self {
        Self { tokens, index: 0 }
    }

    /// 当前 token（不前进）。
    pub(super) fn peek(&self) -> Option<&'a str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    /// 当前位置之后第 `offset` 个 token。
    pub(super) fn peek_at(&self, offset: usize) -> Option<&'a str> {
        self.tokens.get(self.index + offset).map(String::as_str)
    }

    /// 紧跟当前 flag 的值；缺失时按 `<context>: <flag> needs a value` 报错。
    pub(super) fn required_next(&self, context: &str, flag: &str) -> Result<String, String> {
        self.tokens
            .get(self.index + 1)
            .cloned()
            .ok_or_else(|| format!("{context}: {flag} needs a value"))
    }

    /// 消费 `--flag value` / `--flag=value`：内联值前进一格，否则前进两格。
    pub(super) fn take_flag_value(
        &mut self,
        inline: Option<String>,
        context: &str,
        flag: &str,
    ) -> Result<String, String> {
        if let Some(value) = inline {
            self.index += 1;
            if value.is_empty() {
                return Err(format!("{context}: {flag} needs a value"));
            }
            return Ok(value);
        }
        let value = self.required_next(context, flag)?;
        self.index += 2;
        Ok(value)
    }

    pub(super) fn advance(&mut self) {
        self.index += 1;
    }
}

/// 上传载荷上限：daemon 的 /cmd body 上限是 8MB，base64 会膨胀 4/3。
pub(super) const MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

pub(super) fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

pub(super) fn guess_mime(path: &str) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::{parse_duration_secs, parse_port};

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
    fn port_parsing() {
        assert_eq!(parse_port("7744").unwrap(), 7744);
        assert!(parse_port("0").is_err());
        assert!(parse_port("65536").is_err());
    }
}
