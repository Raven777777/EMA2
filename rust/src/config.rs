//! 配置：端口 / 保活间隔 / 鉴权 token / 输出目录，支持环境变量覆盖。

use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8765;
pub const DEFAULT_KEEPALIVE_MS: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub keepalive: Duration,
    /// MCP HTTP 鉴权 token。`None` 只在显式声明匿名运行时成立（见 `allow_anonymous`）。
    pub token: Option<String>,
    /// 是否显式允许无 token 运行（`--insecure-allow-no-token` / `EDGE_MCP_ALLOW_ANONYMOUS=1`）。
    pub allow_anonymous: bool,
    /// 截图等输出文件的保存目录。
    pub output_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            port: env_port().unwrap_or(DEFAULT_PORT),
            keepalive: Duration::from_millis(env_keepalive_ms()),
            token: env_token(),
            allow_anonymous: env_allow_anonymous(),
            output_dir: env_output_dir(),
        }
    }
}

/// `EDGE_MCP_PORT`（非法值忽略，回落到默认端口）。
pub fn env_port() -> Option<u16> {
    std::env::var("EDGE_MCP_PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

/// `EDGE_MCP_TOKEN`（空值视为未设置）。
pub fn env_token() -> Option<String> {
    std::env::var("EDGE_MCP_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// `EDGE_MCP_ALLOW_ANONYMOUS=1|true|yes|on`：显式声明“免 token 运行”。
///
/// 默认关闭。未设置 token 时中继进入待授权态（只开放配对通道）。
pub fn env_allow_anonymous() -> bool {
    std::env::var("EDGE_MCP_ALLOW_ANONYMOUS")
        .map(|value| truthy(&value))
        .unwrap_or(false)
}

pub fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 安装/配对状态目录：`EDGE_MCP_STATE_DIR` > `%LOCALAPPDATA%\EMA2` > XDG > `~/.local/state/ema2`。
pub fn env_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EDGE_MCP_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("EMA2");
    }
    if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(base).join("ema2");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("ema2");
    }
    std::env::temp_dir().join("ema2-state")
}

pub fn state_file() -> PathBuf {
    crate::pair::state_path(&env_state_dir())
}

/// token 优先级：显式环境变量 > 已配对状态文件。
pub fn resolve_token(env: Option<String>, stored: Option<String>) -> Option<String> {
    env.filter(|value| !value.trim().is_empty()).or(stored)
}

fn env_keepalive_ms() -> u64 {
    std::env::var("EDGE_MCP_KEEPALIVE_MS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_KEEPALIVE_MS)
        .max(1)
}

/// 输出目录：`EDGE_MCP_OUTPUT_DIR` > `%LOCALAPPDATA%\EMA2\output` > `$XDG_STATE_HOME` > `~/.local/state`。
///
/// 不再使用相对当前工作目录的 `./output`：中继经常由脚本以任意 CWD 后台启动，
/// 相对路径会让截图落到不可预期的位置。
pub fn env_output_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EDGE_MCP_OUTPUT_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("EMA2").join("output");
    }
    if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(base).join("ema2").join("output");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("ema2")
            .join("output");
    }
    std::env::temp_dir().join("ema2-output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_falls_back_to_default() {
        // 该函数只读环境变量；未设置时应返回默认值。
        if std::env::var_os("EDGE_MCP_KEEPALIVE_MS").is_none() {
            assert_eq!(env_keepalive_ms(), DEFAULT_KEEPALIVE_MS);
        }
    }

    #[test]
    fn explicit_output_dir_wins() {
        if let Some(dir) = std::env::var_os("EDGE_MCP_OUTPUT_DIR") {
            assert_eq!(env_output_dir(), PathBuf::from(dir));
        }
    }

    #[test]
    fn anonymous_switch_parsing_is_strict() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(truthy(value), "{value} should enable anonymous mode");
        }
        for value in ["0", "false", "", "no", "enabled"] {
            assert!(!truthy(value), "{value} should not enable anonymous mode");
        }
    }

    #[test]
    fn env_token_wins_over_the_stored_token() {
        assert_eq!(
            resolve_token(Some("env".into()), Some("file".into())).as_deref(),
            Some("env")
        );
        assert_eq!(
            resolve_token(None, Some("file".into())).as_deref(),
            Some("file")
        );
        assert_eq!(resolve_token(None, None), None);
        assert_eq!(resolve_token(Some("  ".into()), None), None);
    }
}
