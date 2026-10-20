<p align="center">
  <img src="docs/img/ema2log_t.svg" alt="EMA2 — Edge Agent" width="520">
</p>

# EMA2 — Edge Agent 2

让 AI 通过 shell 直接操作**你日常使用的 Edge**：真实指纹、真实登录态，
不依赖 CDP 调试端口，也不需要 MCP 客户端配置。

> 当前版本见 [CHANGELOG](docs/CHANGELOG.md) 顶部；契约与已知弱点见 [HANDOFF](docs/HANDOFF.md)。

```text
AI / shell ──ema2 CLI──> :7744 ──daemon──> WebSocket ──> Edge 扩展 (MV3) ──> 真实浏览器
                        单端口：HTTP /cmd + WS upgrade
```

- **单 exe 自带 daemon**：`ema2 server` 一次，之后每个工具调用都是短命客户端（11 个工具 + `server/status/init/await`）
- **无 MCP、无 token、无授权页、无配置文件**：中继和扩展裸连本机回环端口
- **行式紧凑输出**：`key=value` / TSV / `[e12@d1]`，错误统一 `error=<code> msg=... hint=...`
- **ref 快照**：按 snapshot 返回的 ref 操作，不猜 CSS；文档句柄防止旧 ref 误点
- **深度 DOM**：shadow DOM、同源 iframe；多标签并发 `batch`
- **正文清洁**：`read` 跳过脚本/隐藏元素，main 太短自动回退 body 并给 hint
- **可观测**：真实网络状态码/响应预览、console 增量、HAR 导出、标注截图
- **零运行时依赖**：Rust 单文件 exe + MV3 扩展

> v2.0 与 v1.x 的 MCP/授权/`install` 完全不兼容。历史版本见 [CHANGELOG](docs/CHANGELOG.md)。

## 文档

- [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) — 项目哲学与边界
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — 架构与数据流
- [docs/TOOLS.md](docs/TOOLS.md) — 命令参考、输出协议、错误码
- [docs/TESTING.md](docs/TESTING.md) — 真实 Edge 验收（禁止 Node 测试）
- [docs/HANDOFF.md](docs/HANDOFF.md) — 交接文档与下一步计划
- [docs/README.md](docs/README.md) — 文档索引

## 仓库结构

| 路径 | 内容 |
|---|---|
| `rust/` | daemon + CLI（单 exe，Rust） |
| `extension/` | Edge MV3 扩展（无构建步骤，直接“加载解压缩的扩展”） |
| `docs/` | 契约文档；`docs/archive/` 只放已归档的历史规划 |
| `examples/` | `.ema2` 脚本、`js` 抽取脚本、批量 shell 示例 |
| `scripts/` | `setup.cmd` 构建、`verify.py` 真机验收 |
| `bin/` | 本地构建产物（gitignore，不提交） |

## 架构

```text
ema2 server                     ← 一次性，daemon 化，立即返回
   ├── :7744 WS 服务端          ← 扩展 popup 配的端口，Upgrade 头分流
   ├── :7744 HTTP 端点          ← CLI 的 /cmd、/health、/shutdown
   └── 保活：断线等重连、10s ping

ema2 act click e12              ← 短命客户端，POST :7744，打印，退出
ema2 server stop                ← 显式关闭 daemon
ema2 status                     ← GET /health + 标签页摘要
ema2 init                       ← 写 AGENTS.md
```

- 默认端口 **7744**；server 与所有命令必须用同一个端口（`--port N`）。
- daemon 的 PID/日志在临时目录：`%TEMP%\ema2.pid` / `%TEMP%\ema2.log`（自定义端口带端口号）。
- 截图/HAR 默认写到 `%LOCALAPPDATA%\EMA2\output`，可用 `EMA2_OUTPUT_DIR` 覆盖。
- 没有 `state.toml`、没有 `.ema2.bak`、没有终端配置合并。

## 快速开始

### 1. 构建

```bat
scripts\setup.cmd
```

产物是 `bin\ema2.exe`（单文件）。`bin/` 是本地构建产物，已加入 `.gitignore`，不要提交。也可以直接：

```bash
cargo build --release --manifest-path rust/Cargo.toml
```

### 2. 加载扩展（只需一次）

1. 打开 `edge://extensions/`，开启“开发人员模式”；
2. 点“加载解压缩的扩展”，选择仓库里的 `extension/` 目录；
3. 点工具栏 EMA2 图标，确认端口是 **7744**，点“保存并连接”。

升级扩展时，在 `edge://extensions/` 点一次“重新加载”，或在 popup 里重新保存端口。
exe 不会碰扩展目录。

### 3. 启动中继

```bash
ema2 server
ema2 status
```

`status` 看到 `relay=running` + `extension=connected` 即就绪。扩展没连时，
先确认 popup 端口与 `status` 的 `port=` 一致。

### 4. 用

```bash
ema2 open https://example.com/
ema2 read
ema2 snapshot
ema2 act click e1
ema2 shot --annotate
```

## 命令一览

| 命令 | 一句话 |
|---|---|
| `ema2 server` / `ema2 server stop` | 启动 / 关闭 daemon |
| `ema2 status` | 中继、扩展、标签页状态 |
| `ema2 init` | 在当前目录写/更新 `AGENTS.md` |
| `ema2 tabs` | 列出 / 当前 / 新建 / 关闭 / 激活标签页 |
| `ema2 open` | 打开 URL，回 `status=` / `net_hook=` |
| `ema2 read` | 读页面为 `md\|text\|html`，支持 selector/budget/cursor |
| `ema2 snapshot` | ref 快照 `[e12@d1] button "X"` |
| `ema2 act` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload |
| `ema2 await` | 等上一个 act 动作的导航/页面稳定 |
| `ema2 wait` | 等 text / textGone / selector / time / idle |
| `ema2 js` | 页面主世界执行 JS（表达式或 async 函数体） |
| `ema2 observe` | console 增量 / network（真实状态码+响应体，HAR） |
| `ema2 files` | 上传本地文件；`download-dir` 固定报错 |
| `ema2 shot` | 截图（落盘 / inline / annotate） |
| `ema2 batch` | 多标签并发 open/read/snapshot/js/wait |

完整参数、输出协议、错误码表见 **[docs/TOOLS.md](docs/TOOLS.md)**。

## 典型 AI 循环

```text
ema2 server
ema2 status

ema2 batch read --urls "https://a.com,https://b.com"
# 或串行：open → wait selector → read → 需要时 read --cursor next

# 同一页面连续翻页：不要每次开新标签
ema2 open url1 --replace → ema2 read → ema2 open url2 --replace → ema2 read

# 操作
ema2 open url → ema2 snapshot → ema2 act click e12
             → settle=pending 时接 ema2 await 10s
             → 页面变了就重新 snapshot（旧 ref 会 stale_ref）

# 等页面导航
ema2 act click e12
ema2 await 10s

# 等接口
ema2 act click e12 --wait-for "url=api/save status=200"

# 批量脚本
ema2 act --script flow.ema2
# flow.ema2:
#   fill e15 "user"
#   fill e16 "pass"
#   click e30
#   waitFor url=api/save status=200 timeout=5s

# 收尾
ema2 tabs close --session
ema2 server stop
```

## 端口与错误

| 场景 | 输出 / 处理 |
|---|---|
| 中继没起 | `error=relay_not_running hint=start it with \`ema2 server\``，退出码 2 |
| 中继在、扩展没连 | `error=extension_not_connected`，退出码 3；检查 popup 端口 |
| 7744 被占 | `error=port_in_use`；`ema2 server stop` 或换 `--port` |
| 参数错 | `error=invalid_argument`，退出码 1 |
| 工具执行错 | `error=<细粒度 code>`，退出码 4 |
| 成功 | 退出码 0；结果在 stdout |

`ema2 init` 生成的 `AGENTS.md` 里也带同一份诊断提示。

## 开发

> **硬性规定：禁止 Node.js 测试。**
> 不允许提交 `package.json` / `package-lock.json` / `node_modules`，
> 不允许用 `node:test`、jsdom、Playwright、Puppeteer 或 VM 模拟测试扩展。
> 所有功能与验收测试必须走**真实 Edge 浏览器 + 真实 ema2 CLI**。
> Rust 单测只做纯解析/校验的快速反馈，不能替代真机验收。
> 完整规则见 [CONTRIBUTING.md](CONTRIBUTING.md)。

```bash
cd rust
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

扩展是普通 MV3 service worker；改完 `extension/` 后在 `edge://extensions/` 重新加载。

**所有测试都在真实、前台可见的 Edge 里跑**：禁止无头浏览器、禁止后台隐藏窗口。
一键验收：

```bash
python scripts/verify.py                 # 静态契约 + 真实 Edge 冒烟（59 项）
python scripts/verify.py --skip-real-edge  # 只检查错误码文档漂移
```

真机检查清单见 **[docs/TESTING.md](docs/TESTING.md)**，模块与生命周期见
**[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**。

## 限制

- 网络 body 只是 ≤800 字符预览，HAR 是采样；不是完整抓包代理。
- 跨域 iframe 不可遍历；`edge://` 等内置页不可脚本化。
- 合成 `press` 不提供浏览器原生 Tab/组合键默认行为。
- 截图要求目标 tab 是活动 tab 且窗口可见（最小化会失败）。
- `js` 超时的 `Runtime.terminateExecution` 会终止目标 tab 的全部 JS 执行。
- `open status=` 依赖 `PerformanceNavigationTiming.responseStatus`（Chromium 109+）；
  Service Worker 缓存返回或 `--wait none` 时可能是 `unknown|pending`。
- `files download-dir` 固定返回 `download_dir_unsupported`，下载目录需在
  `edge://settings/downloads` 手动设置；`files upload` 单次 ≤20 个文件、载荷 ≤5 MiB。
- **前台要求**：窗口必须可见；`shot` 还要求目标 tab 是活动 tab。无头/后台窗口不支持。
- `click`/`press` 是合成事件（`isTrusted=false`），校验 `isTrusted` 的站点会忽略；
  浏览器级快捷键（`chrome.commands`）不可达。
- **别的扩展的浮层 UI 不可点击**（密码管理器下拉、翻译面板）：EMA2 只能把它召唤出来，
  浮层内容在跨扩展隔离的 iframe / closed shadow root 里。
- `read --mode md` 是有损渲染（标题/列表/表格/链接保留，样式与合并单元格丢失）。
- `js` 在 debugger 不可用时降级到 MAIN world 注入（输出 `via=scripting`）；
  严格 CSP 站点可能拒绝 eval，此时只能重载 tab 让 debugger 恢复。

## License

Just do what you dream / 去做你梦想的事 — 见 [LICENSE](LICENSE)。
