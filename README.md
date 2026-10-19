# EMA2 — Edge MCP Agent 2 (Rust) · v1.8.0

让 AI 直接接管你**正在使用的真实 Edge 浏览器**：真实指纹、真实登录态、不依赖 CDP 调试端口。

```
AI 客户端 ──MCP over HTTP──> ema2.exe (单进程) ──WebSocket──> Edge 扩展 ──> 真实浏览器
(opencode / Claude ...)   http://127.0.0.1:8765/mcp       ws://127.0.0.1:8765/
```

- **面向 AI，不面向人**：11 个粗粒度工具、行式紧凑输出、`ref` 快照、`budget/cursor` 分页、错误自带 `hint`
- **单 exe、零依赖**：一个约 1.5 MB 的 Rust 进程同时承担 MCP 服务与浏览器桥接
- **深度 DOM**：shadow DOM、同源 iframe、跨标签并发（`batch`）
- **可观测**：真实网络状态码/响应体、console 增量、HAR 导出、标注截图
- **可自刷新**：`ema2.exe reload` 让扩展重读磁盘，改完扩展不用手点 Edge

> v1.4 起工具名与 v1.3 不兼容（18 → 11 个），协议改为行式文本。
> 历史版本见 [CHANGELOG](docs/CHANGELOG.md)。

## 快速开始

```bash
# 1) 启动中继（Windows）
scripts\setup.cmd                # 首次/更新源码后：重建 bin\ema2.exe
scripts\start-bridge.cmd          # 或 bin\ema2.exe run

# 2) 加载扩展
#    edge://extensions → 开发人员模式 → 加载解压缩的扩展 → 选择 extension/
bin\ema2.exe status               # 看到 "extension_connected": true 即成功

# 3) 配置 AI 客户端（opencode）
#    ~/.config/opencode/opencode.json
```

```json
{ "mcp": { "edge-browser": { "type": "remote", "url": "http://127.0.0.1:8765/mcp", "enabled": true } } }
```

Claude Desktop：

```json
{ "mcpServers": { "edge-browser": { "url": "http://127.0.0.1:8765/mcp" } } }
```

### 可选：token

```bash
set EDGE_MCP_TOKEN=change-me
scripts\start-bridge.cmd
```

扩展 service worker 控制台：

```js
chrome.storage.local.set({ token: 'change-me' })   // 扩展会自动追加 ?token=
```

## 工具一览（11 个）

| 工具 | 一句话 |
|---|---|
| `tabs` | 列出 / 当前 / 新建 / 关闭 / 激活标签页 |
| `open` | 新标签打开 URL 或让指定 tab 导航（返回 `net_hook=on`） |
| `read` | 读页面为 `md\|text\|html`，默认正文提取，支持 selector/budget/cursor |
| `snapshot` | ref 快照 `[e12@doc] button "X"`，支持 `filter` 与 `near=<ref>` |
| `act` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload，`steps[]` 批量，`waitFor` 等响应 |
| `wait` | 等 text / textGone / selector / time / idle |
| `js` | 页面主世界执行 JS（支持 await） |
| `observe` | console 增量 / network（真实状态码+响应体，`bodyFilter`、`har`） |
| `files` | 上传本地文件 / 设置下载目录 |
| `shot` | 截图（落盘回路径 / `inline` 回图 / `annotate` 回 ref 标注） |
| `batch` | 多标签并发 read/snapshot/js/wait |

完整参数、输出格式与错误码见 **[docs/TOOLS.md](docs/TOOLS.md)**。

## 目录结构

```text
bin/            ema2.exe（当前源码构建，v1.8.0）
extension/      Edge MV3 扩展（background.js + manifest）
rust/           Rust 中继源码（MCP HTTP + WebSocket bridge）
scripts/        setup / start / stop / bridge.ps1 进程管理
tests/          Node VM/jsdom / PowerShell 回归 + 真实 Edge 验收
examples/       调用示例（weather.js、scrape_baike.py）
docs/           TOOLS / ARCHITECTURE / CHANGELOG / TESTING
```

## 配置（环境变量）

| 变量 | 默认 | 说明 |
|---|---|---|
| `EDGE_MCP_PORT` | 8765 | 端口；`--port` > env > 8765 |
| `EDGE_MCP_KEEPALIVE_MS` | 10000 | 中继→扩展 ping 间隔 |
| `EDGE_MCP_TOKEN` | 无 | Bearer token；扩展 WS 用 `?token=` |
| `EDGE_MCP_OUTPUT_DIR` | `%LOCALAPPDATA%\EMA2\output` | 截图 / HAR 输出目录 |

## 命令行

```bash
bin\ema2.exe run [--port N]     # 启动常驻中继
bin\ema2.exe status             # 健康检查（自动带 token；异常退出码 1）
bin\ema2.exe reload             # 改完 extension/ 用：先校验服务，再 bump 版本 + 扩展重载
```

## 示例

- `examples/weather.js` — 控制真实浏览器查天气
- `examples/scrape_baike.py` — 抓百度百科输出 Markdown（纯标准库）
- `examples/opencode.json.example` — 客户端配置模板

详见 [examples/README.md](examples/README.md)。

## 安全

- 只监听 `127.0.0.1`
- Host/Origin 只接受回环与 `chrome-extension://`；强制 `Content-Type: application/json`
- 可选 token（常量时间比较）；扩展收到 `bridge/welcome` 前不执行任何指令
- 扩展含 `debugger` + `<all_urls>`，等价完全控制浏览器，仅限可信本机

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
# 扩展/脚本回归（19 项，Node 24 + jsdom devDependency）
npm install
npm run test:js

# Rust 中继（49 个单元测试）
cd rust && cargo test --locked --offline
cargo clippy --locked --all-targets -- -D warnings

# Windows 启动/停止脚本自检
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/bridge.ps1 -SelfTest

# 真实 Edge/MV3 验收（隔离 profile，约 2 分钟）
npm run test:edge
```

说明：Node VM/jsdom 与 Rust/PowerShell 是快速回归；`test:edge` 使用独立临时 profile
启动真实 Edge，16 个场景覆盖 F01–F13 主流程、files、shot 和扩展重连。
真实 Edge 验收已在 Edge 155.0.4283.62 上 16/16 通过。
`scripts\start-bridge.cmd` / `stop-bridge.cmd` 只管理当前项目 `bin\ema2.exe` 且精确匹配端口；
陌生进程占用端口时会报错退出，不会强杀。

## 已知限制

- 网络 body 是受限预览（≤800 字符），HAR 是采样；不是完整抓包代理
- `js`（debugger 上下文）直接发起的 fetch 可能只以 `status=-1` 的资源条目出现；页面自身请求正常抓取
- `act waitFor` 不把 hook 安装前的历史回填（`status=-1`）当作成功响应，会回 `observation_gap`
- 跨域 iframe 不可遍历；同源 iframe / shadow root 的截图坐标仍有待更多实机覆盖
- `download-dir` 在 Edge 上不可设（返回明确错误）
- `shot` 只截活动 tab；后台 tab 会返回 `tab_not_active`
- 合成 `press` 不提供浏览器原生 Tab/组合键默认行为
- 视觉兜底是"编号图 + 坐标"，不含 OCR
- Windows 上传路径必须是绝对路径（`C:/dir/a.pdf`）

## 许可

[MIT](./LICENSE)
