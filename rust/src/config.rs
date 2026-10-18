//! 配置：端口 / 保活间隔 / 鉴权 token / 输出目录，支持环境变量覆盖。

use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8765;
pub const DEFAULT_KEEPALIVE_MS: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub keepalive: Duration,
    /// MCP HTTP 鉴权 token（None = 不鉴权，仅本机使用）。
    pub token: Option<String>,
    /// 截图等输出文件的保存目录。
    pub output_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            port: env_port().unwrap_or(DEFAULT_PORT),
            keepalive: Duration::from_millis(env_keepalive_ms()),
            token: env_token(),
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
fn env_output_dir() -> PathBuf {
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
}
