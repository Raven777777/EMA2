# EMA2 — Edge MCP Agent 2 (Rust) · v1.7

通过「Edge 扩展 + 单进程 Rust 中继」让 AI 助手直接控制你的**真实 Edge 浏览器**：
读取任意网页、操作标签页、点击/输入、执行 JS，并**带真实登录态**。

```
AI 客户端 ──MCP over HTTP──> ema2.exe (单进程) ──WebSocket──> Edge 扩展 ──> 真实浏览器
(opencode / Claude ...)   http://127.0.0.1:8765/mcp       ws://127.0.0.1:8765/
```

> **v1.4 是面向 AI 的重新设计，与 v1.3 及更早版本的工具名不兼容。**
> 工具从 18 个收敛为 9 个；输出统一为行式紧凑文本；引入 `ref` 快照、
> `budget/cursor` 分页和 `error=... hint=...` 自纠错；常驻 schema 从 ≈1200 tokens 降到 ≈800。
> 旧版本源码备份：`backup/EMA2-v1.3.0-src-*.tar.gz`。
>
> **v1.7（最终版）**：`act waitFor={url,status,timeout}` 点击后直接等指定响应；
> `observe network` 支持 `bodyFilter` 正则与 `har=true`（Rust 落盘 .har）；
> 新增 `batch`（多标签并发 read/snapshot/js/wait）；snapshot 头带 `visual=`，
> 纯视觉页面给 `hint`；schema 收敛到 ~4.2k 字符 / 11 个工具。
>
> **v1.6**：`read` 默认做正文提取（`main=true`，去掉导航/页脚）；`snapshot` 新增
> `filter=links|buttons|form` 与 `near=<ref>`（只在某个容器内找）；`observe network`
> 改为 hook fetch/XHR，能拿到**真实状态码和响应体**（`wait`/`bodies` 可调）；
> `shot annotate=true` 返回 set-of-marks 标注图和 `[ref] … @x,y` 坐标列表。
>
> **v1.5**：`snapshot`/`read`/`act` 现在会遍历 **shadow DOM 与同源 iframe**；
> 新增 `files`（文件上传 / 下载目录）；`act` 支持 `target="role:name"` 并在目标
> 未渲染时自动重试一次；中继新增 `POST /reload` 与 `ema2.exe reload`，
> 改完扩展可以直接让扩展 `chrome.runtime.reload()`，不用手动点 Edge。

## 设计原则（AI-first）

1. **状态化**：`snapshot` 返回可引用的 `[e12]`，`act` 按 ref 操作，不让模型猜 CSS 选择器
2. **增量**：`act` 返回变化摘要（url/title/新 ref/console 错误），`observe` 只回新增
3. **紧凑**：一行一条、`key=value`、TSV；绝不 pretty-print JSON
4. **可纠正**：错误统一 `error=<code> msg=... hint=...`，作为工具结果返回而不是协议错误
5. **可预算**：长输出一律 `total/shown/next`，用 `budget/cursor` 分页

## 目录结构

```
EMA2/
├── bin/ema2.exe              # release 构建的单 exe（直接分发）
├── rust/                     # Rust 中继（cargo 工程）
│   └── src/                  # main / http / mcp / relay / state / tools / config
├── extension/                # Edge MV3 扩展（background.js + manifest）
├── scripts/                  # 运维脚本：setup / start-bridge / stop-bridge
├── examples/                 # AI 侧示例：opencode 配置、weather.js、scrape_baike.py
│   └── output/               # 脚本产物（gitignored）
├── docs/ARCHITECTURE.md      # 架构与协议说明
├── backup/                   # 本地源码备份（gitignored）
├── README.md
└── LICENSE
```

## 工具（10 个）

| 工具 | 作用 | 关键参数 |
|---|---|---|
| `tabs` | 列出/当前/新建/关闭/切活标签页 | `action=list\|current\|new\|close\|activate`, `tabId`, `url` |
| `open` | 新标签打开 URL，或让指定 tab 导航（返回 `net_hook=on` 表示网络观测已就绪） | `url`, `tabId?`, `wait=load\|dom\|none` |
| `read` | 读页面为 `md\|text\|html`，默认正文提取（`main=true`） | `selector?`, `main`, `budget`, `cursor` |
| `snapshot` | ref 快照：`[e12] button "提交"` | `filter=interactive\|full\|links\|buttons\|form`, `near=<ref>`, `budget`, `cursor` |
| `act` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload | `action`, `ref?`/`selector?`/`target="role:name"`, `steps[]`, `waitFor={url,status,timeout}` |
| `wait` | 等 text / textGone / selector / time / idle | 五选一，`timeout` 默认 30s |
| `js` | 在页面主世界执行 JS（支持 await） | `code`, `tabId?`, `budget` |
| `observe` | console 增量 / network（fetch+XHR，真实状态码+响应体） | `kind`, `since`, `level`/`filter`, `bodyFilter`（正则）, `wait`, `bodies`, `har` |
| `files` | 上传本地文件到 file input / 设置下载目录 | `action=upload\|download-dir`, `paths[]`（Windows 用绝对路径如 `C:/dir/a.pdf`）, `dir`, `ref?`/`target?` |
| `shot` | 截图；默认落盘回路径，`inline=true` 回图片，`annotate=true` 回 ref 标注图+坐标 | `tabId?`, `inline`, `annotate` |
| `batch` | 多标签并发执行 read\|snapshot\|js\|wait | `tool`, `tabIds[]`, `args`, `concurrency=1..4` |

## 输出协议

```text
# tabs list（TSV）
1618179731	*	哔哩哔哩	https://www.bilibili.com/
total=4 shown=4 next=none

# snapshot
url=https://example.com/  title=Example Domain  scroll=0/1 refs=3
[e1] link "More information..." href=/help
[e2] textbox "q" name=q
total=2 shown=2 next=none

# act
step1 ok=click ref=e2
ok=1/1
url=https://example.com/search
title=Search
new=[e3] button "Go"
console_errors=1

# 错误（isError: true）
error=stale_ref msg=ref e2 is stale (page navigated) hint=call snapshot again
```

约定：

- 头部 `key=value`，数据行 `[ref] role "name" extras` 或 TSV
- 列表尾部固定 `total=<n> shown=<m> next=<cursor|none>`
- 图片默认落盘：`path=... bytes=... mime=...`
- 所有工具都接受可选 `tabId`；缺省操作当前活动标签页

## 快速开始

### 前置要求

- **Microsoft Edge**（新版）
- **Rust 工具链**（仅构建需要；运行只需 `bin/ema2.exe`）

### 1. 启动中继

```bash
scripts\start-bridge.cmd     # 或 bin\ema2.exe run
```

### 2. 加载扩展

1. Edge 打开 `edge://extensions/` → 开发人员模式 → 加载解压缩的扩展 → 选择 `extension/`
2. 确认：`bin\ema2.exe status` 显示 `"extension_connected": true`
3. 扩展会先发 `bridge/hello`，中继回 `bridge/welcome`；收到 welcome 前不执行任何工具

### 3.（可选，推荐）启用 token

```bash
set EDGE_MCP_TOKEN=change-me
scripts\start-bridge.cmd
```

在扩展 service worker 控制台执行：

```js
chrome.storage.local.set({ token: 'change-me' })
```

### 4. 配置 AI 客户端

**opencode**（`~/.config/opencode/opencode.json`）：

```json
{
  "mcp": {
    "edge-browser": {
      "type": "remote",
      "url": "http://127.0.0.1:8765/mcp",
      "enabled": true
    }
  }
}
```

**Claude Desktop**（`%APPDATA%\Claude\claude_desktop_config.json`）：

```json
{ "mcpServers": { "edge-browser": { "url": "http://127.0.0.1:8765/mcp" } } }
```

## 配置（环境变量）

| 变量 | 默认 | 说明 |
|---|---|---|
| `EDGE_MCP_PORT` | 8765 | 端口；优先级 `--port` > env > 8765 |
| `EDGE_MCP_KEEPALIVE_MS` | 10000 | 中继→扩展 ping 间隔 |
| `EDGE_MCP_TOKEN` | 无 | Bearer token；扩展 WS 用 `?token=`（扩展自动追加） |
| `EDGE_MCP_OUTPUT_DIR` | `%LOCALAPPDATA%\EMA2\output` | 截图输出目录 |

## 命令行

```bash
bin\ema2.exe run --port 8765    # 启动常驻中继
bin\ema2.exe status             # 健康检查（自动带 EDGE_MCP_TOKEN；异常退出码 1）
bin\ema2.exe reload             # 改完 extension/ 用这个：bump manifest 版本 + 让扩展 reload（Edge 才会重读文件）
```

## 典型 AI 调用循环

```text
open(url) → wait(selector/text) → snapshot() → act(ref) → 读 act 的变化摘要 → 需要时 read()/observe()
```

- 页面结构变了就重新 `snapshot`（ref 会失效，返回 `error=stale_ref`）
- 元素在 shadow DOM / 同源 iframe 里也能被 `snapshot` 看到（行尾带 `frame=f1`）
- 知道按钮文字时可以直接 `act {action:"click", target:"button:提交"}`，省一次快照
- 长页面用 `read(selector=...)` 只读结果区，再用 `cursor` 续读
- 调试用 `observe(kind=console|network, since=上次next)`，只看新增

## HTTP 调用示例

```bash
# 1. 握手（响应头里拿 Mcp-Session-Id）
curl -i -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  http://127.0.0.1:8765/mcp

# 2. 打开页面
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open","arguments":{"url":"https://example.com"}}}' \
  http://127.0.0.1:8765/mcp

# 3. 快照 + 点击
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"snapshot","arguments":{}}}' \
  http://127.0.0.1:8765/mcp
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"act","arguments":{"action":"click","ref":"e1"}}}' \
  http://127.0.0.1:8765/mcp
```

## 构建 / 测试

```bash
cd rust
cargo test              # 47 个测试：协议、11 工具校验、超时、截图（含标注）、HAR、文件、会话、安全头
cargo clippy --all-targets
cargo build --release
cd .. && node --check extension\background.js
```

`scripts\setup.cmd` 会自动 release 构建并复制到 `bin\`。

## 示例脚本

- `examples/weather.js`：查天气（`node examples/weather.js "Tokyo"`）
- `examples/scrape_baike.py`：用 EMA2 抓百度百科输出 Markdown（纯标准库）

两者都已更新到 v1.4 工具面（`open / wait / read / js / tabs`）。

## 安全模型

- 只监听 `127.0.0.1`
- Host/Origin 只接受回环与 `chrome-extension://`；强制 `Content-Type: application/json`
- 可选 token（常量时间比较），扩展走 `?token=`，中继回 `bridge/welcome` 才执行
- 扩展含 `debugger` + `<all_urls>`，等价完全控制浏览器，仅限可信本机

## 常见问题

**扩展报 `ERR_CONNECTION_REFUSED`**：中继没起。先 `scripts\start-bridge.cmd`，扩展会 1s→30s 退避重连。

**`status` 输出空 / 401**：中继启用了 token，运行 `status` 的终端也要设 `EDGE_MCP_TOKEN`。

**`403` / `415`**：Host/Origin 不是本机 / 缺 `Content-Type: application/json`。

**`error=stale_ref`**：页面变了，重新 `snapshot` 再用新 ref。

**`error=debugger_attach_failed`**：该 tab 开着 DevTools 或别的调试器，关掉重试。

**换端口**：`--port` 或 `EDGE_MCP_PORT`；扩展侧 `chrome.storage.local.set({ wsUrl: 'ws://127.0.0.1:<n>' })`。

## 许可

[MIT](./LICENSE)
