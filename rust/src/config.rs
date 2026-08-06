//! 配置：端口 / 保活间隔，支持环境变量覆盖。

use std::path::PathBuf;
use std::time::Duration;

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
        let port = std::env::var("EDGE_MCP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8765);
        let keepalive_ms = std::env::var("EDGE_MCP_KEEPALIVE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10000)
            .max(1);
        let token = std::env::var("EDGE_MCP_TOKEN")
            .ok()
            .filter(|v| !v.is_empty());
        let output_dir = std::env::var("EDGE_MCP_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("output"));
        Config {
            port,
            keepalive: Duration::from_millis(keepalive_ms),
            token,
            output_dir,
        }
    }
}
