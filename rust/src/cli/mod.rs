//! CLI 参数 → 工具 arguments 的映射。
//!
//! 职责：把 `ema2 act click e12 --wait-for "url=api/save"` 这类行式命令编译成
//! `POST /cmd` 的 JSON；参数错误在这里报（stderr `error=invalid_argument` + exit 1），
//! daemon 侧的 `tools::validate_arguments` 仍然兜底。
//!
//! 子模块：
//! - `spec`：clap 结构与帮助文本
//! - `build`：普通命令（tabs/open/read/snapshot/wait/await/js/observe/files/shot）
//! - `act`：`act` 单步/脚本解析
//! - `batch`：`batch` 顶层与内层 flag 解析
//! - `shared`：端口/时长/token 游标/文件与 base64 等公共 helper

mod act;
mod batch;
mod build;
mod shared;
mod spec;

use std::path::PathBuf;

use serde_json::Value;

pub use act::build_act;
pub use batch::build_batch;
pub use build::{
    build_await, build_files, build_js, build_observe, build_open, build_read, build_shot,
    build_snapshot, build_tabs, build_wait,
};
pub use spec::{Cli, Command, ServerAction};

/// 解析完成的一次工具调用（CLI 子命令的编译产物）。
pub struct ToolCall {
    pub tool: &'static str,
    pub args: Value,
    /// shot --out / observe --har 的显式落盘路径。
    pub out: Option<PathBuf>,
    /// act/batch 的 trailing tokens 里可能携带 --port；覆盖全局值。
    pub port: Option<u16>,
}

impl ToolCall {
    fn new(tool: &'static str, args: Value) -> Self {
        Self {
            tool,
            args,
            out: None,
            port: None,
        }
    }
}
