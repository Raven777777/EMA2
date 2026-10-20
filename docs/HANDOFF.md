# EMA2 交接文档

> 给下一位维护者 / AI：先读 [PHILOSOPHY.md](PHILOSOPHY.md)，再读本文。

## 当前版本

```text
exe        2.1.5
extension  2.1.5
helper     10
tools      11（tabs/open/read/snapshot/act/wait/js/observe/files/shot/batch）
```

`await` 是 CLI 命令，映射到 `wait` 工具，不增加第 12 个工具。

## 最短心智模型

```text
AI / shell
  → ema2 <command>  (短命 CLI)
  → POST 127.0.0.1:7744/cmd
  → daemon (同一 exe，detached)
  → WS 127.0.0.1:7744/ (Upgrade)
  → Edge MV3 extension
  → chrome.* / debugger
  → 真实 Edge
```

- `GET /health`：`ema2 status`
- `POST /shutdown`：`ema2 server stop`
- 默认端口 7744；扩展 popup 必须配同一个端口
- 无 MCP、无 token、无授权、无配置文件、无持久状态

## 仓库结构

| 路径 | 内容 |
|---|---|
| `rust/src/main.rs` | CLI 入口、daemon 启停、`status`、`init`、工具调用与退出码 |
| `rust/src/cli_args.rs` | clap 子命令、参数/脚本/batch 编译 |
| `rust/src/server.rs` | axum 单端口路由、`/cmd`、`/health`、`/shutdown` |
| `rust/src/relay.rs` | 扩展 WS、hello/welcome、keepalive |
| `rust/src/state.rs` | 扩展 generation、pending 请求、锁兜底 |
| `rust/src/tools.rs` | 11 工具白名单、参数校验、超时预算 |
| `rust/src/save.rs` | 截图/HAR 落盘 |
| `rust/src/client.rs` | 本机 HTTP 客户端 |
| `rust/src/paths.rs` | 端口、PID/日志、输出目录 |
| `extension/background.js` | 11 工具实现、ref、hook、settle/await、debugger 队列 |
| `extension/popup.*` | 端口配置与连接状态 |
| `docs/TOOLS.md` | 命令参考、输出协议、错误码 |
| `docs/TESTING.md` | 真机验收清单（禁止 Node 测试） |
| `docs/ARCHITECTURE.md` | 模块、生命周期、数据流 |
| `docs/PHILOSOPHY.md` | 项目哲学与边界 |
| `docs/CHANGELOG.md` | 版本历史 |
| `rust/assets/AGENTS.md` | `ema2 init` 模板 |

`bin/` 是本地构建产物，已 gitignore；`docs/archive/` 是历史规划稿。

## 构建与运行

```bash
# 构建
scripts\setup.cmd
# 或
cargo build --release --manifest-path rust/Cargo.toml

# 运行
bin\ema2.exe server
bin\ema2.exe status

# 停止
bin\ema2.exe server stop
```

Edge 侧：

1. `edge://extensions/` → 开发人员模式
2. 加载解压缩的扩展 → 选择 `extension/`
3. popup 端口设为 7744（或与 `--port` 一致）

## 测试

```bash
cd rust
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

当前 Rust 测试：**60 passed**。

真实浏览器验收按 [TESTING.md](TESTING.md)。  
**绝对禁止 Node.js 测试**，不要新增 `package.json` / `node_modules` / jsdom。

## 必须保持的契约

### 输出

- 行式紧凑文本；HAR 是唯一 JSON
- 列表尾部 `total/shown/next`；`next=none` 表示读完
- 文件结果 `path=` 单独一行，后跟 `bytes=` / `mime=`
- 错误 stderr 首行 `error=<code> msg=... hint=...`
- 退出码：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错

### ref

- `[e<seq>@<doc>]`，`doc` 是 `d1/d2…`
- 完整 ref 校验 document；短 ref 只在当前文档内解析
- 导航/reload/换 tab 后旧完整 ref 必须 `stale_ref`
- 密码框 value 不进入 snapshot 名称

### act / settle / await

- 导航意图最多 settle 3s；普通按钮最多 800ms；其他动作不等待
- 页面提前稳定就提前返回
- `settle=pending` 不是错误，退出码 0
- pending 时给 `expect=` / `next_url=` / `hint=ema2 await 10s`
- `ema2 await [duration]` 使用**上一个动作所在 tab**，不是当前活动 tab
- 显式 await 超时返回 `wait_timeout`，退出 4
- 动作打开新标签：`new_tab=` + 自动登记 session
- `.ema2` / `--steps` 里 `await 10s` 是脚本步骤；pending 且下一步不是 await 时脚本停止
- `open` 和新的 act 会清掉旧 lastAct 上下文

### read

- `text` / `md` 跳过 script/style/noscript/template/textarea/input/select/option、
  nav/header/footer/aside/form，以及 `display:none` / `visibility:hidden` / `content-visibility` 元素
- main 太短（<200 字符，或 body>2000 且候选 <body 5%）回退 `main=body(fallback)` 并给 hint
- `html` 保持原始输出
- 被截断必须给 `next` 和 cursor hint

### 其他

- `type` 追加，`fill` 覆盖
- `select` 的位置参数是 option 的 `value`
- `files download-dir` 固定返回 `download_dir_unsupported`
- `batch` 内层参数放在 `args`；内层校验复用子工具 validator
- `js` 全局 debugger 串行；attach 非超时失败自动重试一次
- `server` 重复启动输出 `relay=already_running`，退出 0
- `server stop` 已停止时输出 `relay=not_running`，退出 0

## 已知弱点与下一步优先级

### P1：act 结果可信度

HN 真机出现过一次 `act click` 返回 `ok=1/1` 但页面没有导航，换 selector 后成功。
可能是页面重绘导致元素被替换。下一步建议：

- 点击 `<a>` 后验证 URL / document / 新标签 / DOM 变化
- 无变化时只对 `<a>` 安全重试一次
- 普通按钮不自动重试（避免点赞/提交点两次）
- 明确返回 `nav=none` + hint，而不是假成功

这是当前核心最值得投入的方向。

### P2：动态页面可读时机

GitHub 首页在 `batch read` 时可能只有 `Loading`，等几秒才渲染。
现在 AI 需要自己 `wait time 3s`。可考虑：

- `read` / `snapshot` 增加可选 `--wait-stable 500ms`
- 只等 DOM 文本/节点短暂稳定，不猜网络
- 不改变默认快速路径

### P3：真实浏览器验收 harness

目前真机验收是人工清单 + 临时脚本。  
可以在 `scripts/` 下加 PowerShell/Python 验收脚本，但：

- **不能用 Node.js**
- 只做启动 Edge、打开页面、跑 CLI、断言输出
- 不替代人工检查复杂站点

### 明确不做

- ProseMirror/CKEditor 原生输入
- 携程/淘宝/Bing 专用下拉适配
- OCR、验证码、登录态管理
- 自动下单/发帖/业务工作流
- 第 12 个工具
- **驱动别的扩展自己的浮层 UI**（密码管理器下拉、翻译插件面板）：v2.1.5 起
  `click` 会补齐指针事件与 `focusin`，能把浮层**召唤**出来，但浮层内容在跨扩展
  隔离的 iframe / closed shadow root 里，`snapshot` 拿不到 ref，坐标点击也会被
  `debugger.attach` 的跨扩展限制挡住（真机实测：此时 `getTargets` 只剩一个 page target，
  `attach({tabId})` 和 `attach({targetId})` 都会被拒）。这一步仍需人手点一下。

这些用 `js`、`.ema2`、外部 Python 脚本完成。

## 开发陷阱

1. **修改页面 helper 必须提升 `__ema2_helpers_version`**
   否则已打开页面继续用旧 helper。

2. **Edge 会按 manifest 版本缓存扩展脚本**
   行为没更新时提升 `extension/manifest.json` 版本，或在 `edge://extensions` 重新加载；
   最稳的是换一个全新 `--user-data-dir` 测试。

3. **扩展 reload 会清 `chrome.storage.session`**
   `sessionTabs` 和 lastAct 可能丢失；这是可接受的会话边界。

4. **`open` 会清 lastAct**
   避免 `await` 等到过期页面。新增导航类命令时也要考虑是否该清。

5. **`.` 脚本 pending 停止**
   不要在 pending 后继续执行 ref 操作；先 `await`。

6. **`batch` 预算**
   内层工具仍走 validator；不要把 600s 上限绕过去。

7. **截图需要活动 tab 且窗口可见**
   连续截图注意 `MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND`。

8. **真实站点有反自动化**
   携程结果页、去哪儿首页可能不渲染；换站点或外部脚本，不要为单站加适配器。

## 下一位维护者的第一步

1. 读 [PHILOSOPHY.md](PHILOSOPHY.md)
2. 读 [ARCHITECTURE.md](ARCHITECTURE.md) 和 [TOOLS.md](TOOLS.md)
3. `cargo test` / `clippy` / `fmt`
4. `bin\ema2.exe server` + `status`，确认扩展连接
5. 打开一个真实网站，跑 `open → snapshot → act → read`
6. 修改行为前先问：这是通用原语，还是网站特例？
7. 任何动作相关改动，必须真机验证“成功就是真的成功”

## 一句话交接

**EMA2 的价值不在工具数量，而在动作结果可信、观察可读、错误可纠正。**  
守住这三点，它可以继续长大；丢掉任何一点，它就会退化成另一个不靠谱的浏览器脚本。
