## 最终架构共识

### 形态

```
ema2 server                     ← 一次性，daemon 化，立即返回
   ├── :7744 WS 服务端  ← 扩展 popup 配的端口，Upgrade 头分流
   ├── :7744 HTTP 端点  ← CLI 命令
   └── 保活：断线等重连、ping

ema2 act click e12              ← 短命客户端，POST :7744，打印，退出
ema2 server stop                ← 显式关闭 daemon
ema2 status                     ← GET :7744/health
ema2 init                       ← 写 AGENTS.md
```

单文件 exe，零外部依赖，无 token、无验签、无授权页、无状态文件、无 `install`/`uninstall`、无 `ema2.toml`、无 MCP。默认端口 **7744**，一个端口同时收 WS 和 HTTP，靠 `Upgrade` 头分发。

### 内核（Rust exe）

| 项 | 决策 |
|---|---|
| 子命令 | `server` / `server stop` / `status` / `init` + 11 个工具命令（`tabs` `open` `read` `snapshot` `act` `wait` `js` `observe` `files` `shot` `batch`） |
| 进程模型 | server daemon 化（detached、无窗口、父进程立即返回）；**手动 `server stop` 关闭**，不做 idle 超时 |
| 冷启动 | AI 必须先 `ema2 server`；`act` 等命令发现 7744 不通时**不自动 spawn**，直接报 `relay_not_running` |
| 安全 | 无 token、无验签、无授权，中继与扩展裸连 |
| 状态 | 无 `state.toml`；server 的关闭通过 `POST /shutdown` 实现（见实施细节） |
| 输出 | 行式紧凑文本；文件类结果 `path=` 单独行；HAR 导出是唯一保留的 JSON |
| 输入 | 位置参数 + `key=value`；批量用行式脚本，后缀 `.ema2`，`waitFor` 独立成行 |
| 错误 | 退出码粗粒度（0/1/2/3/4）；stderr 首行 `error=<code> msg=... hint=...`，TOOLS.md 细粒度码全保留 |
| 错误码新增 | `bridge_not_connected` 拆成 `relay_not_running`（7744 不通 → hint `ema2 server`）和 `extension_not_connected`（server 在、扩展没连 → hint 查 popup 端口） |
| 扩展侧逻辑 | 11 个工具实现、ref 快照、网络 hook **原样保留**；HMAC 验签移除 |
| 删除 | MCP HTTP/JSON-RPC/SSE、五类终端配置合并、`.ema2.bak`、授权页、状态页、`pair`、`state.toml`、`client_config` 模块 |

### 扩展（MV3）

- popup：端口输入框 + 保存 + 连接状态（`已连接 7744` / `未连接`），存 `chrome.storage.local`。
- 默认端口 **7744**（与 exe 一致）。
- 无授权页、无状态面板、无 token、无 HMAC challenge。
- 用户手动加载/更新/刷新，exe 不碰扩展目录。

### 文档层

1. `docs/TOOLS.md` → 改写成 man page 风格，错误码表**原样保留**，每个子命令一节。
2. `AGENTS.md` 模板（`ema2 init` 生成）：命令表 + 错误码提示 + 端口错配诊断 + **"先 `ema2 server`"** 作为第一条。
3. `scripts/keepalive.py` **不需要**，py 方案废弃。
4. `docs/ARCHITECTURE.md` 需重写：去掉 MCP 协议、ref 模型保留、网络 hook 保留、新增 daemon 生命周期与单端口分流。

### 实施细节（不阻塞，写代码时定）

- **`server stop` 的实现**：优先 `POST /shutdown`（干净），server 卡死时用 PID 文件兜底。PID 文件放临时目录（`%TEMP%\ema2.pid`），不是 `state.toml`，不算持久状态。
- **端口占用**：`ema2 server` 发现 7744 被占 → 报 `port_in_use` + hint，不自动换端口（避免和扩展配置错位）。
- **单端口分流**：hyper/axum 同时挂 WS upgrade 和 HTTP handler，按 `Upgrade: websocket` 头路由；两套 handler 共享同一个 `TcpListener`。
- **daemon 化**：Windows `DETACHED_PROCESS | CREATE_NO_WINDOW`，Unix `setsid` + fork；内部 `--daemon` 标志防止递归 spawn。
- **Rust 依赖**：`clap`（子命令）、`tokio` + `hyper`/`axum`（server + WS）、`serde`（仅 HAR 导出与内部结构）。TOML 依赖可以去掉，因为无 `ema2.toml`。

## 一句话总结

**单 exe 自带 server daemon，AI 先 `ema2 server` 再调 11 个子命令，扩展 popup 配 7744，无 token 无授权无配置文件，行式输入输出，细粒度错误码保留，批量脚本用 `.ema2`。**