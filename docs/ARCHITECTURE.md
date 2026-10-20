# EMA2 架构说明 · v2.1.6

## 总览

EMA2 是**一个单文件 exe + 一个 MV3 扩展**。exe 既能以后台 daemon 运行，
也能作为短命 CLI 调用 daemon；扩展通过本机 WebSocket 连接 daemon，
把命令翻译成 `chrome.*` / debugger 操作，最终驱动用户真实在用的 Edge。

没有 MCP、没有 JSON-RPC 会话、没有 token、没有授权页、没有状态文件、没有
`ema2.toml`、没有 `install`/`uninstall`。默认端口 **7744**。

```text
┌──────────────┐   ema2 <tool> ...（短命进程）
│  AI / shell  │ ─────────────────────────────┐
└──────────────┘                              ▼
                                    ┌─────────────────────┐
                                    │  ema2.exe daemon    │
                                    │  127.0.0.1:7744     │
                                    │  HTTP /cmd /health  │
                                    │  WS  / (Upgrade)    │
                                    └──────────┬──────────┘
                                               │ WebSocket + 10s ping
                                               ▼
                                    ┌─────────────────────┐
                                    │ Edge 扩展 (MV3)      │  chrome.* / debugger
                                    │ 11 个工具实现        │
                                    └──────────┬──────────┘
                                               ▼
                                        真实 Edge（页面/登录态）
```

## 进程模型

### `ema2 server`

1. 先探测 `GET /health`：
   - 已是 ema2 → 输出 `relay=already_running pid=... port=...`，退出 0；
   - 端口上有别的服务 → `error=port_in_use`，退出 2，**不自动换端口**（避免和扩展 popup 错位）。
2. 以 detached 方式 spawn 自己：`ema2 daemon --port N`。
   - Windows：`DETACHED_PROCESS | CREATE_NO_WINDOW`；
   - Unix：新进程组。
   - stdout/stderr 追加到 `%TEMP%\ema2.log`（自定义端口为 `ema2-<port>.log`）。
3. 父进程轮询 `/health` 直到 `name=ema2`，打印 pid/port/log 后退出。

### daemon

- 绑定 `127.0.0.1:7744`，写入临时 PID 文件（`%TEMP%\ema2.pid`）；
- 同时挂 WS 升级和 HTTP handler，共享同一个 `TcpListener`；
- 扩展断线不会退出 daemon，扩展 watchdog 每 30s 重连；
- 每 10s 向扩展发 `ping`，防止 MV3 service worker 闲置回收；
- `/shutdown` 或 Ctrl+C 触发 graceful shutdown，退出时删除自己写的 PID 文件。

### `ema2 server stop`

1. `POST /shutdown`；
2. 最多等 6s 确认端口关闭；
3. 卡死时读 PID 文件，确认进程名仍是 `ema2` 后 `taskkill /T /F`（Unix 用 `/proc/<pid>/exe` 校验 + `kill -TERM`）；
4. 已停止时输出 `relay=not_running`，退出 0。

### 短命 CLI

`ema2 <tool> ...` 不启动 runtime、不读写文件，只把参数编译成
`POST /cmd {"tool":..., "args":..., "out":...}`，然后打印结果并退出。

## 模块

| 文件 | 职责 |
|---|---|
| `rust/src/main.rs` | CLI 入口、daemon 启停、`status`、`init`、工具调用与退出码 |
| `rust/src/cli_args.rs` | clap 子命令 → `ToolCall`；act 脚本/`--steps`、batch 参数、时长解析 |
| `rust/src/client.rs` | 极简本机 HTTP/1.1 客户端（含 chunked） |
| `rust/src/server.rs` | axum 路由、Host/Origin 守卫、`/cmd`、`/health`、`/shutdown`、daemon 主循环 |
| `rust/src/relay.rs` | 扩展 WS 连接循环、hello/welcome、保活 |
| `rust/src/state.rs` | 扩展连接 generation、pending 请求表、锁中毒兜底 |
| `rust/src/tools.rs` | 11 个工具白名单 + 参数校验 + 超时预算 |
| `rust/src/save.rs` | 截图/HAR 落盘，把 base64/JSON 换成 `path=` 行 |
| `rust/src/paths.rs` | 默认端口、输出目录、临时 PID/日志路径 |
| `extension/background.js` | 11 个工具实现、ref 快照、网络/console hook、debugger 队列、act settle/await 上下文 |
| `extension/popup.html/js` | 端口输入、保存/重连、连接状态 |

## 单端口分流

`GET /` 的 handler 以 `Result<WebSocketUpgrade, WebSocketUpgradeRejection>` 接收：

- 带 `Upgrade: websocket` → `relay::handle_extension_ws`；
- 普通 HTTP → 返回一行版本/端点说明。

CLI 全部走 `POST /cmd`：

```json
{ "tool": "act", "args": { "action": "click", "ref": "e12@d1" }, "out": null }
```

成功：HTTP 200 + `{"result": <扩展 result>}`；参数错 400，扩展未连 503，
扩展超时 504。工具业务错误仍是 200 + `result.isError=true`，由 CLI 转成 stderr + 退出码。

`GET /health` 返回 daemon/扩展状态，`ema2 status` 再调用一次 `tabs list` 补
`tabs=/active_tab=` 摘要。

## 安全模型

v2 明确**只信任本机回环**，不做应用层鉴权：

- 只 bind `127.0.0.1`；
- 校验 `Host` 必须是 `127.0.0.1` / `localhost` / `[::1]`；
- 校验 `Origin`：无 Origin（CLI/curl）、`chrome-extension://`、或回环 origin 才允许；
- 不做 token/HMAC。任何能访问本机 7744 的进程都能驱动浏览器——这是设计取舍。

扩展只连 `ws://127.0.0.1:<popup 端口>`，任何占用该端口的本机服务都能冒充 daemon；
因此端口错配时必须先 `ema2 status`，不要盲目授权/重连。

## 数据流

```text
CLI 参数
  → build_*() 生成 args
  → POST /cmd
  → tools::validate_arguments（daemon 兜底）
  → RelayState::call_extension 登记 pending + 发送 tools/call
  → 扩展 executeTool
  → 返回 content[]
  → shot / observe har 在 daemon 落盘
  → CLI 渲染文本（图片 --inline 输出 data URI）
```

- 工具超时预算在 `tools.rs`：wait = timeout+15s，act = 60s+waitFor，js/shot/files=120s，
  batch 按目标数/并发/内层工具估算，超过 600s 在执行前拒绝。
- pending 表按扩展连接 generation 隔离；旧连接的迟到响应不会解决新请求。
- CLI 中途断开时 `PendingGuard` 清掉 pending 条目。

## act settle / await

动作执行和页面稳定是两件事：

- `act` 执行后记录 `lastAct` 上下文（tab、expect、baseline URL/doc/title、耗时、新标签）。
- 导航意图（链接/表单提交/back/forward/reload）最多 settle 3s；普通按钮最多 800ms；
  其他动作不等待。页面提前稳定就提前返回。
- `settle=pending` 不是错误，退出码仍为 0；输出 `next_url=` 和
  `hint=call ema2 await 10s`。
- `ema2 await 10s` 读取 lastAct，继续等上一个动作的效果；不带 `--tab` 时使用动作所在 tab。
  显式 await 超时才返回 `wait_timeout`（退出 4）。
- 动作打开的新标签会返回 `new_tab=` 并登记进会话，`tabs close --session` 能清理。
- `.ema2`/`--steps` 里 `await 10s` 是脚本专用步骤；如果上一步 pending 且下一步不是 await，
  脚本停止，避免在旧页面继续点击。
- `act` 的工具超时预算会叠加脚本中的 await 时长，超过 600s 在执行前拒绝。

## ref 模型

- snapshot 给 `[e<seq>@<doc>]`，`doc` 是扩展分配的短句柄（`d1/d2…`），
  落 `chrome.storage.local`，浏览器重启后继续递增，避免新旧文档撞号；
- 完整 ref 校验 `doc` 必须等于当前文档；短 ref（`e12`）只在当前文档内解析；
- 导航/reload/换 tab 后旧完整 ref 返回 `stale_ref`；
- 密码框名称只来自 label/ARIA/placeholder/alt/title，绝不读 value。

## read 正文清洁

- `text`/`md` 用 DOM 遍历取文本，跳过 script/style/noscript/template/textarea/input/select/option、
  nav/header/footer/aside/form 等页面壳，以及 `display:none`/`visibility:hidden`/`content-visibility`
  的隐藏元素；`html` 保持原始输出。
- `mainRoot` 用清洁后的文本和链接密度评分；候选 <200 字符，或 body>2000 字符且候选不到 body 5%
  时回退 `main=body(fallback)`，并输出 `hint=main extraction too short (...)`。
- main 太短、百度 hidden textarea JSON、GitHub dashboard JSON、隐藏弹窗文本都不会再污染正文。

## 网络 / console hook

- `open` / `act` 会自动在 MAIN world 安装 hook；
- 网络 hook 包装 `fetch` 和 `XMLHttpRequest`，记录真实 status、耗时、URL；
- 响应预览通过 `response.clone()` 后台读取，最多 8KB / 1.5s，不阻塞页面；
- `observe network --bodies=false` 真正关闭 body clone；
- `bodyFilter` 限 512 字符，拒绝嵌套量词/量词组内分支等指数回溯形态；
- `--har` 导出 HAR 1.2 采样，不是完整抓包；
- console hook 记录 log/info/warn/error/debug，支持增量 `--since` 和 `--filter`。

## batch

- `--urls`：每个 URL 开新标签再执行目标工具；`tool=open` 只并行开页；
- `--tab-ids`：对已有标签并发执行；
- 并发默认 2，最大 4；`js` 因全局 debugger 串行，预算按有效并发 1 计算；
- 每块 `---` 分隔，单块失败不影响其它块；
- 内层参数复用子工具 validator，不能绕过 `wait time=300` 等上限。

## 输出与退出码

- 输出是行式紧凑文本；HAR 是唯一 JSON；
- 列表尾部 `total/shown/next`；`next=none` 表示读完；
- 文件类结果 `path=` 单独一行，后跟 `bytes=`/`mime=`；
- 错误 stderr 首行 `error=<code> msg=... hint=...`；
- 退出码：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错。

细粒度错误码见 [TOOLS.md](TOOLS.md)。

## 与 v1.x 的差异

删除：MCP HTTP/JSON-RPC/SSE、token 与 HMAC 配对、授权页、状态页、
`state.toml`、`.ema2.bak`、五类终端配置合并、`install`/`uninstall`、
`pair`、`client_config`/`jsonc`/`clients` 模块、`ema2.toml`。

保留：11 个工具的扩展侧实现、ref 快照、网络/console hook、debugger 队列、
HAR/截图落盘、batch 并发。

新增：daemon 生命周期、单端口分流、PID/日志兜底、`ema2 server/stop/status/init`、
popup 端口配置、CLI 行式参数与 `.ema2` 脚本、`relay_not_running` 与
`extension_not_connected` 拆分。
