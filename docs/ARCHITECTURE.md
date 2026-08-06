# EMA2 (Rust) 架构说明

## 总览

EMA2-Rust 用**一个常驻进程**同时向 AI 提供 MCP over HTTP 服务、向 Edge 扩展提供
WebSocket 桥接，让 AI 直接控制真实浏览器（真实指纹、真实登录态），
不依赖 Chrome DevTools 调试端口。

```
┌──────────────┐  MCP over HTTP (Streamable HTTP / SSE)
│  AI 客户端    │ ───────────────────────────────┐
│ (opencode /  │                                  ▼
│  Claude ...) │                          ┌────────────────────┐
└──────────────┘                          │   ema2.exe (Rust)  │  ← 单一常驻进程
      ▲                                  │                     │
      │  JSON-RPC 响应                     │  http.rs (axum)     │
      └──────────────────────────────────│   ├─ POST /mcp       │
                                         │   ├─ GET  /mcp (SSE) │
                                         │   ├─ GET  /sse       │
                                         │   ├─ POST /messages  │
                                         │   ├─ GET  /status    │
                                         │   └─ WS   / (扩展)   │
                                         │  relay.rs 路由+保活   │
                                         └──────────┬──────────┘
                                                    │ WebSocket + 10s keepalive ping
                                                    ▼
                                            ┌─────────────────┐
                                            │ Edge 扩展 (MV3)  │
                                            │ chrome.* API    │
                                            └────────┬─────────┘
                                                     ▼
                                             真实浏览器（页面/标签页/登录态）
```

## 单进程设计

旧版（Node）是两个进程：`shim`（stdio⇄WS 转发）+ `relay`（常驻桥接）。
Rust 版合并为**一个进程**：

- 对 AI：实现 MCP **Streamable HTTP**（`POST /mcp` 处理请求、`GET /mcp` 事件流、
  `Mcp-Session-Id` 会话）+ **legacy SSE**（`GET /sse` + `POST /messages`）双传输
- 对浏览器：`/` 根路径 WebSocket，扩展主动连出
- 内部：MCP 请求按 id 登记为待响应 → 转发给扩展 → 扩展响应按 id 回填 → 返回给 AI

```
POST /mcp (tools/call, id=3) ──> 登记 pending[3] ──> WS ──> 扩展
                                                                │ chrome.*
                                                                ▼
HTTP 响应 <───────────────────────── pending[3] 完成 <── WS <── 扩展
```

## 组件

| 文件 | 职责 |
|------|------|
| `main.rs` | clap 子命令：`run`（常驻）、`status`（健康检查） |
| `http.rs` | axum 路由：`/mcp`、`/sse`、`/messages`、`/status`、根路径 WS |
| `mcp.rs` | MCP 协议处理：initialize / tools/list / tools/call / ping |
| `relay.rs` | 扩展 WS 连接循环 + 10s 保活 ping |
| `state.rs` | 共享状态：扩展发送通道、pending 待响应映射、SSE 会话通道 |
| `tools.rs` | 18 个浏览器工具的 schema 定义 |
| `config.rs` | 端口（默认 8765）/ 保活间隔，`EDGE_MCP_PORT` 等 env 覆盖 |

## 依赖

`tokio`（异步运行时）、`axum`（HTTP + WS + SSE）、`serde/serde_json`、
`clap`（CLI）、`tracing`（日志到 stderr）、`futures-util`/`async-stream`。

## 扩展协议（与浏览器侧约定）

- 连接后先发 `{"method":"bridge/hello","role":"extension"}` 声明角色
- 中继每 10s 发 `{"method":"ping"}`，重置 MV3 service worker 闲置计时器
- 扩展对 MCP 请求回 `{jsonrpc, id, result}`，中继按 `id` 回填给等待的 AI 请求
- 扩展未连接时，AI 收到 `-32000 Edge extension is not connected`

## 安全

- `TcpListener` 只绑定 `127.0.0.1`
- 扩展权限含 `debugger` + `<all_urls>`，能力等于完全控制浏览器，仅限可信本机环境
- 用真实登录态高频自动化仍受目标网站风控约束

## 构建 / 体积

```bash
cd rust && cargo build --release
# 产物 rust/target/release/ema2.exe，约 1.5 MB（strip + lto）
# 复制到 bin/ema2.exe 即完成"分发"
```

## 与旧版 Node 实现对比（历史参考）

| | Node 旧版 | Rust 新版 |
|---|---|---|
| 进程数 | 2（shim + relay） | 1 |
| AI 传输 | stdio（需 spawn shim） | HTTP（remote 直连） |
| 依赖 | Node + ws | 仅一个 exe |
| 单文件体积 | ~数 MB + 依赖 | **1.5 MB** |
