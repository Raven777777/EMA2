<p align="center">
  <img src="docs/img/ema2log_t.svg" alt="EMA2 — Edge MCP Agent" width="520">
</p>

# EMA2 — Edge MCP Agent 2 (Rust) · v1.12.1

让 AI 直接接管**你日常使用的 Edge**：真实指纹、真实登录态、不依赖 CDP 调试端口。

```
AI 客户端 ──MCP over HTTP──> ema2.exe (单进程) ──WebSocket──> Edge 扩展 ──> 真实浏览器
(opencode / Claude / Codex / Pi)   http://127.0.0.1:8765/mcp       ws://127.0.0.1:8765/
```

- **一次安装、一次授权**：扩展部署/更新、登录自启、AI 终端配置全部由 exe 完成；人只在首次加载扩展、点一次「授权」
- **配置无损合并**：JSONC 只插入自己的条目，注释、键顺序、排版原样保留；Codex 用 TOML 表段合并
- **面向 AI，不面向人**：11 个粗粒度工具、行式紧凑输出、`ref` 快照、`budget/cursor` 分页、错误自带 `hint`
- **单 exe、零依赖**：一个约 3.3 MB 的 Rust 进程同时承担 MCP 服务与浏览器桥接
- **深度 DOM**：shadow DOM、同源 iframe、跨标签并发（`batch`）
- **可观测**：真实网络状态码/响应体、console 增量、HAR 导出、标注截图
- **可自刷新**：`ema2.exe reload` 让扩展重读磁盘，改完扩展不用手点 Edge

> v1.4 起工具名与 v1.3 不兼容（18 → 11 个），协议改为行式文本。
> 历史版本见 [CHANGELOG](docs/CHANGELOG.md)。

## 快速开始（一次安装，之后只需授权）

```bash
bin\ema2.exe install
```

它把扩展部署到日常 Edge 的固定路径（`%LOCALAPPDATA%\EMA2\extension`；如果已经加载过，就原地更新）、
注册登录自启、启动中继并打开授权页。

首次只有一步需要手动：Edge 不允许程序静默安装解压扩展——在自动打开的 `edge://extensions` 里
开启“开发人员模式” → “加载解压缩的扩展”，选择 install 打印出的目录。**只此一次**，之后升级
都由 install 覆盖同一目录完成。

授权页会列出检测到的 AI 终端，勾选后点 **「授权此浏览器」**：

1. 中继生成 token 并落盘。
2. 按勾选写入配置：取消勾选会移除该终端里已有的 EMA2 条目；每次修改前把原内容写入
   `<配置文件>.ema2.bak`（文件原本不存在时是 0 字节备份）。
3. 重启 AI 客户端即可使用；中继随登录自启，扩展不再需要任何手工操作。

```bash
bin\ema2.exe install --no-autostart          # 不注册登录自启
bin\ema2.exe install --no-browser            # 只部署，不打开浏览器
bin\ema2.exe install --port 8766             # 自定义端口（授权页会记住）
bin\ema2.exe install --managed-profile       # 可选：独立 EMA2 Edge 窗口（全自动，但不是日常 profile）
bin\ema2.exe install --uninstall             # 卸载：自启 + 客户端条目 + 托管 profile
bin\ema2.exe install --uninstall --purge-output   # 连截图/HAR 一起删
```

> 卸载不会删除浏览器里的扩展（Edge 不允许程序卸载），需要在 `edge://extensions` 里点一次“移除”。

### 支持的 AI 终端

| 终端 | 配置文件 | 写入形状 |
|---|---|---|
| opencode | `~/.config/opencode/opencode.json[c]` | `mcp.edge-browser`（`type: remote` + headers） |
| Claude Code | `~/.claude.json` | `mcpServers.edge-browser`（`type: http`） |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json` | `mcpServers.edge-browser` |
| Codex | `~/.codex/config.toml` | `[mcp_servers.edge-browser]` + `http_headers` |
| Pi | `~/.pi/agent/mcp.json` | `mcpServers.edge-browser` |

只写已知终端的固定键名，不接受任意路径；没有安装的终端会被识别为“未安装”且不会创建配置。

### 手动 / 兼容用法

- 已有 `EDGE_MCP_TOKEN`（CI、服务器、多用户主机）：设置后中继直接进入已配对模式，不弹授权页。
- 开发者模式手动加载 `extension/`：仍然可用；扩展页点授权即可完成配对，`install` 会识别并原地更新。
- 免 token 运行（不推荐）：`bin\ema2.exe run --insecure-allow-no-token`，
  任何本地进程都能操控浏览器，启动日志会以 `ERROR` 级别提示。

## 工具一览（11 个）

| 工具 | 一句话 |
|---|---|
| `tabs` | 列出 / 当前 / 新建 / 关闭（`tabIds[]`、`scope=session`）/ 激活标签页 |
| `open` | 打开 URL：默认新标签，`replace`/`reuse=session` 复用标签；回 `status=`/`net_hook=` |
| `read` | 读页面为 `md\|text\|html`，默认正文提取，支持 selector/budget/cursor |
| `snapshot` | ref 快照 `[e12@d1] button "X"`；丢空节点、URL 截 40、支持 `filter`/`near=<ref>` |
| `act` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload，`steps[]` 批量，`waitFor` 等响应 |
| `wait` | 等 text / textGone / selector / time / idle |
| `js` | 页面主世界执行 JS（表达式或语句体，支持顶层 return/await） |
| `observe` | console 增量 / network（真实状态码+响应体，`bodyFilter`、`har`） |
| `files` | 上传本地文件；`download-dir` 在 Edge 上固定返回 `download_dir_unsupported` |
| `shot` | 截图（落盘回路径 / `inline` 回图 / `annotate` 回 ref 标注） |
| `batch` | 多标签并发 open/read/snapshot/js/wait；传 `urls` 可一次完成「打开→读取」 |

完整参数、输出格式与错误码见 **[docs/TOOLS.md](docs/TOOLS.md)**。

## 目录结构

```text
bin/            ema2.exe（当前源码构建，v1.12.1）
extension/      Edge MV3 扩展（background.js + manifest）
rust/           Rust 中继源码（MCP HTTP + WebSocket bridge）
scripts/        进程管理（setup / start / stop / bridge.ps1）+ 真机只读体检（live-check.py）
tests/          回归测试（extension / scripts）+ tests/integration 真实 Edge 验收
examples/       调用示例（weather.js、scrape_baike.py）
docs/           TOOLS / ARCHITECTURE / CHANGELOG / TESTING（docs/img 为品牌图）
```

## 配置（环境变量）

| 变量 | 默认 | 说明 |
|---|---|---|
| `EDGE_MCP_PORT` | 8765 | 端口；`--port` > env > 8765 |
| `EDGE_MCP_KEEPALIVE_MS` | 10000 | 中继→扩展 ping 间隔 |
| `EDGE_MCP_TOKEN` | 无 | 覆盖配对 token（CI/服务器）；设置后不再需要浏览器授权 |
| `EDGE_MCP_STATE_DIR` | `%LOCALAPPDATA%\EMA2` | 扩展部署、配对状态、默认输出目录 |
| `EDGE_MCP_ALLOW_ANONYMOUS` | 无 | 设为 `1` 才允许无 token 匿名运行（等价 `--insecure-allow-no-token`） |
| `EMA2_SKIP_CLIENT_CONFIG` | 无 | 设为 `1` 时不写入 AI 客户端配置 |
| `EMA2_EDGE_EXE` | 自动探测 | 指定 Edge 路径（`install` 使用） |
| `EMA2_EDGE_USER_DATA` | 自动探测 | 指定 Edge `User Data` 目录（检测已加载扩展；测试用） |
| `EDGE_MCP_OUTPUT_DIR` | `%LOCALAPPDATA%\EMA2\output` | 截图 / HAR 输出目录 |

## 命令行

```bash
bin\ema2.exe install [--port N] [--no-browser] [--no-autostart]
                     [--managed-profile] [--uninstall] [--purge-output]
bin\ema2.exe run [--port N] [--insecure-allow-no-token]           # 启动常驻中继
bin\ema2.exe status             # 健康/配对状态（未授权时读 /pair/status）
bin\ema2.exe pair [--reset]     # 查看配对状态 / 删除 token 后重新授权
bin\ema2.exe reload             # 改完 extension/ 用：先校验服务，再 bump 版本 + 扩展重载
```

## 示例

- `examples/weather.js` — 控制真实浏览器查天气
- `examples/scrape_baike.py` — 抓百度百科输出 Markdown（纯标准库）
- `examples/opencode.json.example` — 客户端配置模板

详见 [examples/README.md](examples/README.md)。

## 安全

- 只监听 `127.0.0.1`；Host/Origin 只接受回环与 `chrome-extension://`；强制 `Content-Type: application/json`
- **待授权态是关闭的**：没有 token 时中继只开放 `/pair/status` 与 `?pair=1` 配对通道，
  `/mcp` 一律 503，浏览器不可能被驱动
- 配对通道只接受 `chrome-extension://`（或无 Origin 的本地脚本），一次性：
  已有 token 后 `?pair=1` 直接 409，重新配对需 `ema2.exe pair --reset`
- **中继身份校验**：扩展每次连接带随机 challenge，中继回 `HMAC-SHA256(token, challenge)` 证明身份；
  扩展验签通过前不执行任何工具调用（堵住本地进程抢占 8765 冒充中继）
- token 存 `%LOCALAPPDATA%\EMA2\state.json`（用户级 ACL，原子写）；`EDGE_MCP_TOKEN` 优先级最高
- 扩展含 `debugger` + `<all_urls>`，等价完全控制浏览器，仅限可信本机

## 构建要求

Rust **1.85+**（`edition = "2024"`，已在 `Cargo.toml` 用 `rust-version` 声明）。
`scripts\setup.cmd` 需要 `cargo` 在 PATH 中，或用 `EMA2_CARGO` 指定完整路径。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/TOOLS.md](docs/TOOLS.md) | 工具参数、输出协议、错误码、典型调用循环 |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 协议、ref 模型、网络 hook、MV3 生命周期等内部实现 |
| [docs/CHANGELOG.md](docs/CHANGELOG.md) | 版本变更记录 |
| [docs/TESTING.md](docs/TESTING.md) | 测试矩阵、命令、真实 Edge 验收边界 |
| [examples/README.md](examples/README.md) | 示例脚本用法 |

## 测试

```bash
# 扩展/脚本回归（45 项，Node 22+ + jsdom devDependency）
npm install
npm run test:js

# Rust 中继（98 个单元测试）
cd rust && cargo test --locked --offline
cargo clippy --locked --all-targets -- -D warnings

# Windows 启动/停止脚本自检
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/bridge.ps1 -SelfTest

# 真实 Edge/MV3 验收（隔离 profile + 隔离 HOME，约 3 分钟）
npm run test:edge

# 真实登录态冒烟（连正在跑的中继与扩展，只读，约 2~3 分钟）
python scripts/live-check.py
```

说明：Node VM/jsdom 与 Rust/PowerShell 是快速回归；`test:edge` 使用独立临时 profile 与临时 HOME
启动真实 Edge，22 个场景覆盖 F01–F13 主流程、files、shot、扩展重连、SSE 通知、token 网关、
streaming 观测、首次配对与授权页渲染、五类终端的真实配置合并，以及 install 对已加载扩展的识别。
真实 Edge 验收已在 Edge 155.0.4283.62 上 22/22 通过。
`scripts\start-bridge.cmd` / `stop-bridge.cmd` 只管理当前项目 `bin\ema2.exe` 且精确匹配端口；
陌生进程占用端口时会报错退出，不会强杀。

## 已知限制

- **首次需要手动加载一次扩展**：Edge 不允许普通进程把解压扩展静默装进日常 profile（不依赖商店/管理员策略时只能如此）
- **卸载需要在 `edge://extensions` 里移除扩展**：浏览器不提供程序化卸载入口
- `install` 只支持 Windows（Startup 目录自启 + Edge profile 检测）
- 同一个目录下同时存在 `opencode.json` 与 `opencode.jsonc` 时优先写 `.json`；`.json` 必须是严格 JSON，
  `.jsonc` 支持注释与尾逗号，合并时除新增条目外不改动任何字节
- 网络 body 是受限预览（≤800 字符），HAR 是采样；不是完整抓包代理
- `js`（debugger 上下文）直接发起的 fetch 可能只以 `status=-1` 的资源条目出现；页面自身请求正常抓取
- `act waitFor` 不把 hook 安装前的历史回填（`status=-1`）当作成功响应，会回 `observation_gap`
- 跨域 iframe 不可遍历；同源 iframe / shadow root 的截图坐标仍有待更多实机覆盖
- `files action=download-dir` 在 Edge 上不可设（固定返回 `download_dir_unsupported`），
  schema 仍保留该动作以便客户端拿到明确错误；下载目录请在 `edge://settings/downloads` 手动设置
- `open status=` 取自主文档的 PerformanceNavigationTiming（Chromium 109+）；`wait=none` 时为 `status=pending`，
  Service Worker 直出缓存时可能是 `status=unknown`
- `js` 超时会用 `Runtime.terminateExecution` 终止该 tab 的**全部** JS 执行（不是只终止本次表达式），
  可能打断页面自身的并发逻辑；页面有长任务时优先用更短的表达式或 `wait`
- `shot` 只截活动 tab；后台 tab 会返回 `tab_not_active`
- 合成 `press` 不提供浏览器原生 Tab/组合键默认行为
- 视觉兜底是"编号图 + 坐标"，不含 OCR
- Windows 上传路径必须是绝对路径（`C:/dir/a.pdf`）

## 许可

[MIT](./LICENSE)
