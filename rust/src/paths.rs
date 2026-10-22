//! 运行期路径：没有配置文件、没有 state.toml，只有临时 PID/日志与输出目录。
//!
//! - PID/日志放临时目录（`%TEMP%` / `/tmp`），不算持久状态。
//! - 截图/HAR 默认放用户数据目录，可用 `EMA2_OUTPUT_DIR` 覆盖。

use std::path::PathBuf;

/// 默认端口。扩展 popup 与所有 CLI 命令共用这个值。
pub const DEFAULT_PORT: u16 = 7744;
/// 对外产品版本。Cargo/SemVer 不能表达四位版本，因此 crate 版本仍为 2.4.2。
pub const PRODUCT_VERSION: &str = "2.4.3.1";

/// 截图 / HAR 输出目录。
pub fn output_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os("EMA2_OUTPUT_DIR") {
        return PathBuf::from(custom);
    }
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("EMA2").join("output");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(state).join("ema2").join("output");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("ema2")
                .join("output");
        }
    }
    std::env::temp_dir().join("ema2-output")
}

fn temp_name(port: u16, extension: &str) -> PathBuf {
    // 默认端口沿用 `ema2.pid`；自定义端口各自一份，避免多实例互相覆盖。
    let base = if port == DEFAULT_PORT {
        "ema2".to_owned()
    } else {
        format!("ema2-{port}")
    };
    std::env::temp_dir().join(format!("{base}.{extension}"))
}

/// daemon 的 PID 文件（`server stop` 的兜底通道）。
pub fn pid_file(port: u16) -> PathBuf {
    temp_name(port, "pid")
}

/// daemon 的日志文件（detached 进程的 stdout/stderr 都写这里）。
pub fn log_file(port: u16) -> PathBuf {
    temp_name(port, "log")
}
