# EMA2 — Edge MCP Agent 2 (Rust)

通过「Edge 扩展 + 单进程 Rust 中继」让 AI 助手直接控制你的**真实 Edge 浏览器**：
读取任意网页、操作标签页、点击元素、执行 JS，并**带真实登录态**。

```
AI 客户端 ──MCP over HTTP──> ema2.exe (单进程) ──WebSocket──> Edge 扩展 ──> 真实浏览器
(opencode / Claude ...)   http://127.0.0.1:8765/mcp       ws://127.0.0.1:8765/
```

## 特性

- **单进程、单 exe（约 1.5 MB）**：Rust 实现，一个常驻进程同时承担 MCP 服务与浏览器桥接
- **MCP over HTTP**：标准 Streamable HTTP + legacy SSE 双传输，opencode `type: remote` 直连
- **控制真实浏览器**：真实指纹、真实登录态，无头检测失效，不依赖 CDP 调试端口
- **9 个工具**：读页面 / 读标签页 / 列标签页 / 新建关闭 / 导航 / 点击 / 执行 JS
- **MV3 保活**：定时 ping，防止 Edge 扩展 service worker 闲置回收断连
- **安全**：只绑定 `127.0.0.1`

## 目录结构

```
EMA2/
├── bin/ema2.exe                 # release 构建的单 exe（直接分发）
├── rust/                        # Rust 源码（cargo 工程）
│   └── src/
│       ├── main.rs              # 子命令：run / status
│       ├── config.rs            # 端口/保活（env 覆盖）
│       ├── http.rs              # axum：/mcp + /sse + /messages + /status + WS
│       ├── mcp.rs               # MCP 协议处理
│       ├── relay.rs             # 扩展 WS 连接 + 保活
│       ├── state.rs             # 共享状态（扩展连接/待响应映射）
│       └── tools.rs             # 9 个工具定义
├── extension/                   # Edge 浏览器扩展 (MV3)
├── scripts/
│   ├── setup.cmd                # cargo build --release → bin/
│   ├── start-bridge.cmd         # 启动常驻中继（隐藏后台）
│   └── stop-bridge.cmd          # 停止中继
├── examples/
│   ├── opencode.json.example    # opencode remote MCP 配置
│   └── claude_desktop_config.json.example
├── docs/ARCHITECTURE.md
├── README.md
└── LICENSE
```

## 快速开始

### 前置要求

- **Microsoft Edge**（新版）
- **Rust 工具链**（仅构建需要；运行只需 `bin/ema2.exe`）

### 1. 启动中继

```bash
scripts\start-bridge.cmd     # 或直接运行 bin\ema2.exe run
```

### 2. 加载扩展

1. Edge 打开 `edge://extensions/`
2. 打开右上角「开发人员模式」
3. 「加载解压缩的扩展」→ 选择 `extension/` 目录
4. 确认扩展已启用：`bin\ema2.exe status` 显示 `"extension_connected": true`

### 3. 配置 AI 客户端

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
{
  "mcpServers": {
    "edge-browser": {
      "url": "http://127.0.0.1:8765/mcp"
    }
  }
}
```

然后重启 AI 客户端。

## 工具列表

| 工具 | 说明 |
|------|------|
| `edge_get_tabs` | 列出所有标签页（id/title/url/active） |
| `edge_get_active_tab` | 当前标签页信息 |
| `edge_read_active_tab` | 提取当前标签页纯文本 |
| `edge_read_tab` | 按 tabId 提取指定标签页纯文本 |
| `edge_execute_js` | 在当前标签页执行任意 JS（chrome.debugger） |
| `edge_click_element` | 按 CSS 选择器点击元素 |
| `edge_new_tab` / `edge_close_tab` | 新建 / 关闭标签页 |
| `edge_navigate` | 当前标签页跳转 |
| `edge_wait_for` | 等文本出现/消失或固定时长（默认超时 30s） |
| `edge_take_screenshot` | 截图（保存 PNG 到输出目录，返回文件路径） |
| `edge_console_messages` | 读取页面 console 日志 |
| `edge_network_requests` | 列出页面发起的网络请求（performance entries） |
| `edge_type` | 向选择器元素输入文本（触发 input/change） |
| `edge_press_key` | 在当前聚焦元素按键（Enter/Escape 等） |
| `edge_fill_form` | 批量填充表单字段 |
| `edge_hover` | 悬停到指定元素 |
| `edge_scroll` | 页面滚动（默认一个视口高度） |

## 配置（环境变量）

| 变量 | 默认 | 说明 |
|------|------|------|
| `EDGE_MCP_PORT` | 8765 | 监听端口 |
| `EDGE_MCP_KEEPALIVE_MS` | 10000 | MV3 保活 ping 间隔 |
| `EDGE_MCP_TOKEN` | 无 | 启用后 MCP HTTP 需 `Authorization: Bearer <token>` |
| `EDGE_MCP_OUTPUT_DIR` | `./output` | 截图输出目录 |

## 命令行

```bash
bin\ema2.exe run --port 8765    # 启动常驻中继
bin\ema2.exe status             # 健康检查（含扩展连接状态）
```

## HTTP MCP 调用示例

不依赖任何 SDK，任何语言用 HTTP POST 即可调用（这也是 opencode remote 接入的方式）：

```bash
# 1. 握手
curl -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  http://127.0.0.1:8765/mcp

# 2. 工具列表
curl -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  http://127.0.0.1:8765/mcp

# 3. 调用工具（打开新标签页 / 读取标签页文本）
curl -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"edge_new_tab","arguments":{"url":"https://www.bing.com"}}}' \
  http://127.0.0.1:8765/mcp
```

Node.js 示例（读取网页正文）：

```js
const res = await fetch('http://127.0.0.1:8765/mcp', {
  method: 'POST', headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call',
    params: { name: 'edge_read_active_tab', arguments: {} } }),
});
const j = await res.json();
console.log(j.result.content[0].text);
```

## 构建

```bash
scripts\setup.cmd     # 或 cd rust && cargo build --release，产物复制到 bin/
```

## 常见问题

**扩展报 `ERR_CONNECTION_REFUSED`**
中继没在运行。先 `scripts\start-bridge.cmd`，扩展会自动重连（每 3s）。

**`edge_execute_js` 报错**
该工具走 `chrome.debugger`，attach 时 Edge 会短暂显示"正在调试"提示；不能在内置页面（edge:// 等）执行。若行为异常，到 `edge://extensions/` 刷新扩展。

**想换端口**
环境变量 `EDGE_MCP_PORT`，或 `ema2.exe run --port <n>`；扩展侧通过 `chrome.storage.local` 设置 `wsUrl` 同步修改。

**重启电脑后中继没了**
`scripts\start-bridge.cmd` 是手动常驻；如需开机自启，加入「启动」文件夹或任务计划。

## 安全提示

- 中继只绑定 `127.0.0.1`，不要对外网暴露
- 扩展含 `debugger` + `<all_urls>` 权限，等价于完全控制浏览器，**仅用于可信的本机环境**
- 用真实登录态做高频自动化时，遵守目标网站服务条款，账号风控依旧有效

## 许可

[MIT](./LICENSE)
