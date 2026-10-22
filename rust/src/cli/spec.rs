//! clap 结构定义与帮助文本。
//!
//! 帮助文本是用户侧的权威文档（`ema2 act --help` 即脚本手册），改动时保持逐字稳定。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use super::shared::parse_port;

#[derive(Parser)]
#[command(
    name = "ema2",
    version = crate::paths::PRODUCT_VERSION,
    about = "AI-driven control of the user's real Edge browser (daemon + CLI, no MCP)",
    disable_help_subcommand = true,
    after_help = "\
AI 会话顺序:
  ema2 server
  ema2 status
  ema2 open <url>
  ema2 snapshot
  ema2 act click e12
  ema2 await 10s

完整参数与示例: ema2 <command> --help
.ema2 脚本语法: ema2 act --help
脚本只允许 act 动作 + await + waitFor；不能写 open/read/snapshot/wait。"
)]
pub struct Cli {
    /// 覆盖默认端口 7744；server 与所有客户端命令须用同一端口
    #[arg(long, global = true, value_parser = parse_port)]
    pub port: Option<u16>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 后台启动中继 daemon（一次性，立即返回）
    Server {
        #[command(subcommand)]
        action: Option<ServerAction>,
    },
    /// 中继/扩展连接状态
    Status,
    /// 在当前目录写 AGENTS.md（命令表 + 错误码 + 诊断）
    Init,
    /// 列出 / 关闭 / 激活标签页
    Tabs(TabsArgs),
    /// 打开 URL，等页面就绪，装网络 hook
    Open {
        /// URL（与 --urls 二选一）
        url: Option<String>,
        /// 为每个 URL 新开标签后打开（逗号分隔，可重复；与位置参数 URL 互斥）
        #[arg(long, value_name = "URL,URL", action = clap::ArgAction::Append)]
        urls: Vec<String>,
        /// 并发数（仅用于 --urls；1..4，默认 2）
        #[arg(long, value_name = "N")]
        concurrency: Option<u32>,
        /// 导航指定标签（不计入会话）
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 在当前活动标签里导航（不新建）
        #[arg(long)]
        replace: bool,
        /// 复用本会话最近标签并导航（last）；session 是旧别名
        #[arg(long)]
        reuse: Option<String>,
        /// 返回到什么程度：load|dom|none（--wait 是隐藏的旧别名）
        #[arg(long)]
        until: Option<String>,
        /// 隐藏的旧别名：--until
        #[arg(long, hide = true)]
        wait: Option<String>,
        /// 等文档接位的时长：20 / 20s / 500ms（默认 15，1..60）
        #[arg(long, value_name = "DURATION")]
        timeout: Option<String>,
        /// 在后台新建，不切换活动标签、不聚焦窗口
        #[arg(long)]
        background: bool,
    },
    /// 读页面为 md|text|html|visible
    Read {
        #[command(flatten)]
        fanout: FanoutArgs,
        /// 单个目标标签（与 --tabs/--urls/--url 互斥）
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 输出格式：md|text|html|visible（visible=innerText）
        #[arg(long)]
        mode: Option<String>,
        /// 聚焦元素（支持 shadow DOM / 同源 iframe）
        #[arg(long)]
        selector: Option<String>,
        /// 关闭正文提取（读整页）
        #[arg(long = "no-main")]
        no_main: bool,
        /// 返回字符预算（默认 4000，范围 200..200000）
        #[arg(long, value_name = "N")]
        budget: Option<u64>,
        /// 从上次输出的 next 续读（字符偏移）
        #[arg(long, value_name = "N")]
        cursor: Option<u64>,
    },
    /// ref 快照：[e12@d1] button "X"
    Snapshot {
        #[command(flatten)]
        fanout: FanoutArgs,
        /// 单个目标标签（与 --tabs/--urls/--url 互斥）
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// interactive|full|links|buttons|form
        #[arg(long)]
        filter: Option<String>,
        /// 限定在该 ref 最近的容器内
        #[arg(long)]
        near: Option<String>,
        /// 返回字符预算（默认 4000，范围 200..200000）
        #[arg(long, value_name = "N")]
        budget: Option<u64>,
        /// 从上次输出的 next 续读
        #[arg(long, value_name = "N")]
        cursor: Option<u64>,
        /// 附带 text/textarea/select/file/contenteditable 的当前值
        #[arg(long)]
        values: bool,
    },
    /// 单步或多步操作：act click e12 / act --steps '...' / act --script run.ema2
    #[command(after_help = r##"单步:
  ema2 act click e12
  ema2 act fill e15 "hello world"
  ema2 act press e15 Enter
  ema2 act click e12 --wait-for "url=api/save status=200 timeout=5s"
  ema2 act click e12 --wait-for "text=\"评论已提交\" timeout=5s"
  ema2 act click e12 --wait-for "selector=\"#comment-18\" timeout=5s"
  ema2 act click e12 --tab 42

多步:
  ema2 act --steps 'click e12; fill e15 "hi"; press e15 Enter'
  ema2 act --script flow.ema2
  ema2 act --script flow.ema2 --tab 42

.ema2 脚本（每行一条；空行和 # 注释忽略；含空格的值加双引号）:
  click e12
  click selector="#submit"
  click target="button:提交"
  type e15 "hello world"
  fill e15 "hello"
  select e20 "opt1"
  press Enter
  press e15 Tab
  hover e12
  scroll down 3
  submit
  back
  forward
  reload
  await 10s
  waitFor url=api/save status=200 timeout=5s
  waitFor text="保存成功" timeout=5s
  waitFor selector="#comment-18" timeout=5s

脚本规则:
- 只允许 11 个 act 动作 + await + waitFor；最多 20 步。
- 不能写 open/read/snapshot/wait/js/tabs 等 CLI 命令；这些在 shell 里单独调用。
- ref 裸写；selector= / target= 是命名参数，不能和裸 ref 同时用。
- waitFor 接受 url= / status= / timeout=（等响应或导航），
  或 text= / textGone= / selector=（等页面条件，和 ema2 wait 语义一致）。
  一次 waitFor 只写一个页面条件；页面条件不能和 url=/status= 混用。
  单纯等待用 await 1s。
- --selector/--target/--key/--delta-y/--submit 只用于单步；
  脚本里用 selector= / target= / press / submit。

常见错误:
  open https://...      → 先 ema2 open，再 ema2 act --script
  wait text "Done"      → 脚本用 waitFor text="Done"；或 shell 用 ema2 wait
  waitFor time 1s       → 用 await 1s

完整语法/示例: ema2 act --help（本页）"##)]
    Act {
        /// action [ref] [text] 与 --selector/--target/--key/--delta-y/--submit/--tab/
        /// --wait-for/--steps/--script/--port 混合
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        tokens: Vec<String>,
    },
    /// 等 text / textGone / selector / time / idle
    #[command(after_help = "\
这是 shell 命令，不能写进 .ema2 脚本。
脚本里用:
  await 10s                             等上一个 act 的导航/页面稳定
  waitFor url=... status=... timeout=5s 等响应或导航
  waitFor text=... timeout=5s           等页面文本/选择器条件
act 的 --wait-for 也接受同样的 text=/textGone=/selector=。")]
    Wait {
        #[command(flatten)]
        fanout: FanoutArgs,
        /// text|textGone|selector|time|idle
        kind: String,
        /// 条件值（文本 / CSS / 时长，如 "5s"）
        value: Option<String>,
        /// 兜底超时，如 30s（0..300s）
        #[arg(long, value_name = "DURATION")]
        timeout: Option<String>,
        /// 单个目标标签（与 --tabs/--urls 互斥）
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
    /// 等上一个 act 动作的页面效果完成（导航/页面稳定）
    Await {
        /// 最长等待时长，如 10s；缺省 3s
        #[arg(value_name = "DURATION")]
        value: Option<String>,
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
    /// 页面主世界执行 JS
    #[command(after_help = r##"调用范式:
  # 1) 两次调用（最省心）：注入会粘住，第二次直接调用
  ema2 js --file lib.js
  ema2 js "return __E.digest()"

  # 2) 一次投递（少一次往返）：--file 与内联 code 互斥，整段作为一次 code 传入
  ema2 js "$(cat lib.js)
  return __E.digest()"

求值规则:
  先按表达式编译；能编译就执行一次，否则按 async 函数体执行一次。
  多行代码必须显式 return，否则得到 undefined 并提示
  note=ran as an async function body。

注入生命周期:
  注入的全局活到该标签导航/关闭为止；重装同名全局不会自动替换旧实现，
  改库后必须在表达式里显式覆盖赋值。

分页:
  --budget 截断时回 truncated=1；--cursor 是输出字符偏移，
  续读时源输出必须不变（换 code 或换 budget 都会错位）。
"##)]
    Js {
        #[command(flatten)]
        fanout: FanoutArgs,
        /// JS 表达式或语句体（与 --file 互斥）
        code: Option<String>,
        #[arg(long = "file")]
        file: Option<PathBuf>,
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 返回字符预算（默认 8000，范围 200..200000）
        #[arg(long, value_name = "N")]
        budget: Option<u64>,
        /// 从上次输出的 next 续读（字符偏移；换 code 或 budget 会错位）
        #[arg(long)]
        cursor: Option<u64>,
        /// 执行上限：10 / 10s / 500ms（默认 10s，0.1..300）
        #[arg(long, value_name = "DURATION")]
        timeout: Option<String>,
    },
    /// console 增量 / network（真实状态码+响应体，HAR）
    Observe {
        /// console|network
        kind: String,
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 从上次输出的 next 续读
        #[arg(long)]
        since: Option<u64>,
        /// log|warn|error（console 最低级别；默认 error，log 含全部）
        #[arg(long, value_name = "LEVEL")]
        level: Option<String>,
        /// network：URL 子串过滤
        #[arg(long = "url-filter", value_name = "SUBSTR")]
        url_filter: Option<String>,
        /// 隐藏的旧别名：--url-filter（console 仍按文本过滤）
        #[arg(long, hide = true)]
        filter: Option<String>,
        /// 采集窗口，如 5s（≤10s；--wait 是隐藏的旧别名）
        #[arg(long, value_name = "DURATION")]
        collect: Option<String>,
        /// 隐藏的旧别名：--collect
        #[arg(long, hide = true)]
        wait: Option<String>,
        /// 响应体正则（≤512 字符）
        #[arg(long = "body-filter")]
        body_filter: Option<String>,
        /// 关闭响应体预览
        #[arg(long = "no-bodies")]
        no_bodies: bool,
        /// 隐藏的旧写法：--bodies=false
        #[arg(long, require_equals = true, hide = true)]
        bodies: Option<bool>,
        /// 导出 HAR 到指定路径
        #[arg(long, value_name = "PATH")]
        har: Option<PathBuf>,
        /// HAR 凭证头策略：redact（默认，脱敏）| all（保留原值）
        #[arg(long, value_name = "redact|all", default_value = "redact", value_parser = ["redact", "all"])]
        headers: String,
        /// 返回字符预算（默认 3000，范围 200..200000）
        #[arg(long, value_name = "N")]
        budget: Option<u64>,
    },
    /// 上传本地文件
    Files(FilesArgs),
    /// 截图（落盘 / inline / annotate）
    Shot {
        #[arg(long = "tab")]
        tab: Option<u64>,
        /// 回图片内容（base64）而不是落盘
        #[arg(long)]
        inline: bool,
        /// 先快照再在图上画 ref 编号
        #[arg(long)]
        annotate: bool,
        /// png|jpeg
        #[arg(long)]
        format: Option<String>,
        /// 落盘路径（相对路径按当前目录转绝对并清理 . / ..）
        #[arg(long = "out")]
        out: Option<PathBuf>,
    },
    /// （内部）daemon 进程本体；由 `ema2 server` detached spawn
    #[command(hide = true)]
    Daemon,
}

#[derive(Subcommand)]
pub enum ServerAction {
    /// 干净关闭 daemon（POST /shutdown，PID 兜底）
    Stop,
}

/// 多目标参数（read/snapshot/js/wait 共享）。
///
/// 带 `--tabs` / `--urls` / `--url` 时，命令不再直达工具，而是编译成一次
/// 内部 `batch` 工具调用；`batch` 只是执行器，CLI 没有它的子命令。
#[derive(Args, Debug, Clone, Default)]
pub struct FanoutArgs {
    /// 对已有标签并发执行（逗号分隔，可重复；≤8）
    #[arg(long, value_name = "ID,ID", action = clap::ArgAction::Append)]
    pub tabs: Vec<String>,
    /// 为每个 URL 新开标签后执行（逗号分隔，可重复；≤8）
    #[arg(long, value_name = "URL,URL", action = clap::ArgAction::Append)]
    pub urls: Vec<String>,
    /// 单 URL 形式：等价于 --urls "<URL>"
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// 并发数（1..4，默认 2；只能和 --tabs/--urls/--url 一起用）
    #[arg(long, value_name = "N")]
    pub concurrency: Option<u32>,
    /// 仅用于 --urls：url=按精确 URL 复用会话标签；session 是旧别名
    #[arg(long, value_name = "url|session")]
    pub reuse: Option<String>,
}

#[derive(Args)]
pub struct TabsArgs {
    #[command(subcommand)]
    pub cmd: Option<TabsCommand>,
}

#[derive(Subcommand)]
pub enum TabsCommand {
    /// 列出标签页（默认动作）
    List {
        /// 返回字符预算（默认 4000，范围 200..200000）
        #[arg(long, value_name = "N")]
        budget: Option<u64>,
        #[arg(long)]
        cursor: Option<u64>,
    },
    /// 关闭标签页（位置参数 id ≤8 个，或 --session）
    Close {
        ids: Vec<u64>,
        /// 关掉本会话由 open 打开的标签
        #[arg(long)]
        session: bool,
    },
    /// 激活标签页
    Activate { id: u64 },
}

#[derive(Args)]
pub struct FilesArgs {
    #[command(subcommand)]
    pub cmd: FilesCommand,
}

#[derive(Subcommand)]
pub enum FilesCommand {
    /// 上传本地文件（Windows 须 C:/... 绝对路径，≤20 个）
    #[command(after_help = "\
例:
  ema2 files upload C:/tmp/a.pdf C:/tmp/b.pdf --ref e8
路径是位置参数，所以 ref 必须用 --ref；act 的 ref 是位置参数。")]
    Upload {
        paths: Vec<String>,
        #[arg(long = "ref")]
        r#ref: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long = "tab")]
        tab: Option<u64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn help_is_the_self_contained_script_manual() {
        let mut root = Cli::command();
        let top = root.render_long_help().to_string();
        assert!(
            top.contains("ema2 act --help"),
            "top-level help must point to act help"
        );
        assert!(
            top.contains("脚本只允许 act 动作"),
            "top-level help must state the script boundary"
        );

        let act = root.find_subcommand_mut("act").expect("act subcommand");
        let help = act.render_long_help().to_string();
        for needle in [
            "fill e15 \"hello\"",
            "await 10s",
            "waitFor url=api/save status=200 timeout=5s",
            "waitFor text=\"保存成功\" timeout=5s",
            "waitFor selector=\"#comment-18\" timeout=5s",
            "不能写 open/read/snapshot/wait/js/tabs",
            "waitFor time 1s",
        ] {
            assert!(help.contains(needle), "act help missing {needle:?}");
        }

        let mut root = Cli::command();
        let wait = root.find_subcommand_mut("wait").expect("wait subcommand");
        let help = wait.render_long_help().to_string();
        assert!(
            help.contains("不能写进 .ema2 脚本"),
            "wait help must explain it is shell-only"
        );
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// 从 Markdown 表格里取出所有 `ema2 <command>` 的第一个词。
    fn table_commands(section: &str) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        for line in section.lines() {
            let line = line.trim();
            if !line.starts_with('|') {
                continue;
            }
            let mut rest = line;
            while let Some(index) = rest.find("ema2 ") {
                rest = &rest[index + "ema2 ".len()..];
                let name: String = rest
                    .chars()
                    .take_while(|ch| ch.is_ascii_lowercase())
                    .collect();
                if !name.is_empty() {
                    names.insert(name);
                }
            }
        }
        names
    }

    /// v2.4.3.21：旧面已从 CLI 删除；扩展的 tabs/files action 仍留给 /cmd。
    #[test]
    fn removed_commands_are_not_subcommands() {
        let root = Cli::command();
        let visible: Vec<&str> = root
            .get_subcommands()
            .filter(|command| !command.is_hide_set())
            .map(|command| command.get_name())
            .collect();
        assert_eq!(visible.len(), 14, "visible commands: {visible:?}");
        assert!(!visible.contains(&"batch"));
        assert!(root.find_subcommand("batch").is_none());

        let tabs = root.find_subcommand("tabs").expect("tabs");
        assert!(tabs.find_subcommand("new").is_none());
        assert!(tabs.find_subcommand("current").is_none());
        assert!(tabs.find_subcommand("list").is_some());
        let files = root.find_subcommand("files").expect("files");
        assert!(files.find_subcommand("download-dir").is_none());
        assert!(files.find_subcommand("upload").is_some());

        // 解析器已删除：这些输入现在是 clap 的用法错误（CLI 退出 1）。
        assert!(
            Cli::try_parse_from(["ema2", "batch", "read", "--urls", "https://a.example/"]).is_err()
        );
        assert!(Cli::try_parse_from(["ema2", "tabs", "new", "https://a.example/"]).is_err());
        assert!(Cli::try_parse_from(["ema2", "tabs", "current"]).is_err());
        assert!(Cli::try_parse_from(["ema2", "files", "download-dir"]).is_err());

        // 顶层 help 仍只列 14 个可见命令。
        let top = Cli::command().render_long_help().to_string();
        assert!(!top.contains("  batch"), "top-level help leaks batch");
    }

    /// 命令表是用户侧文档的第二份拷贝；它和 clap 可见子命令必须同步。
    ///
    /// 隐藏的子命令（`daemon`）不在表里，
    /// 所以这里只比较 `is_hide_set() == false` 的顶层命令。
    #[test]
    fn documented_command_tables_match_visible_subcommands() {
        let root = Cli::command();
        let visible: std::collections::BTreeSet<String> = root
            .get_subcommands()
            .filter(|command| !command.is_hide_set())
            .map(|command| command.get_name().to_owned())
            .collect();
        assert!(
            visible.len() >= 14,
            "unexpectedly few commands: {visible:?}"
        );

        let tools = include_str!("../../../docs/TOOLS.md");
        let overview = tools
            .split("## 命令总览")
            .nth(1)
            .and_then(|rest| rest.split("## 全局约定").next())
            .expect("docs/TOOLS.md must have a command overview table");
        assert_eq!(
            table_commands(overview),
            visible,
            "docs/TOOLS.md 命令总览与 clap 可见子命令不一致"
        );

        let agents = include_str!("../../assets/AGENTS.md");
        let signatures = agents
            .split("## 命令签名")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("rust/assets/AGENTS.md must have a command signature table");
        assert_eq!(
            table_commands(signatures),
            visible,
            "rust/assets/AGENTS.md 命令签名与 clap 可见子命令不一致"
        );
    }
}
