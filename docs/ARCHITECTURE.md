# EMA2 架构说明

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
- 发往扩展的通道是**有界队列**（容量 64）。队列满只算拥塞：keepalive 丢弃并记日志，
  连接不会因此被判死；工具调用仍按 pending 表等响应或超时。
- keepalive 同时做**半开检测**：连续 30s（3 个 keepalive 周期）没有收到扩展任何消息，
  就主动摘除该连接，`/health` 回到 `extension=disconnected`，避免 `status` 长期假在线。
- `/shutdown` 触发 graceful shutdown 时会立即通知在途请求返回 `shutting_down`（503），
  并给 3s 硬期限；期限到仍未结束就强制退出，不再让一条卡住的命令拖住整个 daemon。
- `/shutdown` 或 Ctrl+C 触发 graceful shutdown，退出时删除自己写的 PID 文件。

### `ema2 server stop`

1. `POST /shutdown`；
2. 最多等 6s，确认端口关闭**且 PID 真的消失**，才输出 `relay=stopped`；
   端口已关闭但进程还活着时报 `shutdown_failed`，不谎报成功；
3. 卡死时读 PID 文件，确认进程名仍是 `ema2` 后 `taskkill /T /F`（Unix 用 `/proc/<pid>/exe` 校验 + `kill -TERM`）；
4. 已停止时输出 `relay=not_running`，退出 0。

### 短命 CLI

`ema2 <tool> ...` 不启动 runtime、不读写文件，只把参数编译成
`POST /cmd {"tool":..., "args":..., "out":...}`，然后打印结果并退出。

## 模块

| 文件 | 职责 |
|---|---|
| `rust/src/main.rs` | CLI 入口、daemon 启停、`status`、`init`、工具调用与退出码 |
| `rust/src/cli/` | clap 子命令 → `ToolCall`；`spec.rs` 定义语法，`build.rs`/`act.rs`/`batch.rs` 负责编译，`shared.rs` 放时长/令牌等共享解析 |
| `rust/src/client.rs` | 极简本机 HTTP/1.1 客户端（含 chunked） |
| `rust/src/server.rs` | axum 路由、Host/Origin 守卫、`/cmd`、`/health`、`/shutdown`、daemon 主循环 |
| `rust/src/relay.rs` | 扩展 WS 连接循环、hello/welcome、保活 |
| `rust/src/state.rs` | 扩展连接 generation、pending 请求表、锁中毒兜底 |
| `rust/src/tools.rs` | 11 个工具白名单 + 参数校验 + 超时预算；测试锁定 `contract/runtime.json` |
| `contract/runtime.json` | 版本与超时的单一来源；`scripts/sync_contract.py` 生成 `extension/contract.js` |
| `extension/contract.js` | 由 runtime.json 生成，SW 最先加载 |
| `rust/src/save.rs` | 截图/HAR 落盘，把 base64/JSON 换成 `path=` 行 |
| `rust/src/paths.rs` | 产品版本、默认端口、输出目录、临时 PID/日志路径 |
| `extension/background.js` | 38 行入口：按依赖顺序 `importScripts` 全部模块 |
| `extension/modules/` | 17 个经典脚本模块：core/tabs/network/open_wait/page/guard/injected_helpers/actions/hooks/act/files_shot/debugger/har/batch/observe/read_snapshot/dispatch；共享全局作用域是刻意设计，页面注入函数仍自包含 |
| `extension/page_guard.js` | 命令租约内按需启用：非阻塞记录 alert/confirm/prompt/print，解除后恢复原生 API |
| `extension/page_guard_armed.js` | document_start 动态 guard 的第二个脚本，在页面脚本前立即 arm |
| `extension/page_guard_bridge.js` | ISOLATED world bridge：把 arm 事件转给 SW，租约已释放时通知页面自行 disarm |
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
`call_extension` 统一分类：扩展未连接或连接中途丢失 → 503（`extension_not_connected` /
`relay_error`）；等响应超时 → 504（`extension_error`）；daemon 正在关闭 → 503 `shutting_down`。

`GET /health` 返回 daemon/扩展状态，`ema2 status` 再调用一次 `tabs list` 补
`tabs=/active_tab=` 摘要。

## 安全模型

v2 明确**只信任本机回环**，不做应用层鉴权：

- 只 bind `127.0.0.1`；
- 校验 `Host` 必须是 `127.0.0.1` / `localhost` / `[::1]`；
- `/cmd` 与 `/shutdown` 必须无 `Origin`、带 `X-EMA2-Client: ema2-cli`（阻止浏览器跨源页面借用户会话发命令）；
- `/health` 只允许无 `Origin` 或合法的 `chrome-extension://<32 位 a-p id>`；回环网页 Origin 也拒绝；
- 扩展 WS 必须完成 `bridge/hello`：`role=extension`、32 位扩展 ID、Origin 精确等于
  `chrome-extension://<id>`；未 hello 的连接不能收工具调用，5s 内不完成即断开；
- 同一扩展 ID 重连会替换旧连接并清理 pending；不同 ID 不能顶掉当前扩展；
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

- 版本与超时基准在 `contract/runtime.json`；扩展侧 `contract.js` 由
  `python scripts/sync_contract.py` 生成，Rust 常量由 `cargo test` 对比同一份 JSON。
- 工具超时预算在 `tools.rs`：默认 80s（扩展 watchdog 70s）；wait = timeout+20s；
  open = waitTimeout+20s；act = 60s + 每步最多 3s 的隐式 settle + 脚本 waitFor/await；
  js = --timeout(默认 10s)+20s；shot/files=120s；
  batch 按目标数/并发/内层工具估算，超过 600s 在执行前拒绝。
- 扩展自己的 watchdog 必须早于 daemon：默认/act/wait/open/js 都是 70s 档；
  batch 例外地固定 580s，让 daemon 的 600s 产品上限先返回，避免两层同时超时。
- pending 表按扩展连接 generation 隔离；旧连接的迟到响应不会解决新请求。
- CLI 中途断开时 `PendingGuard` 清掉 pending 条目。

## 并发模型

扩展侧不再用一把全局锁，而是**按 tab 串行**：

- `executeTool` 先解析目标 tab（显式 `tabId`，否则当前活动标签），把调用挂到
  `tab:<id>` 的串行链上；同一 tab 上的 act/settle、read、snapshot、js、observe、shot
  按提交顺序执行，不同 tab 之间仍然并发。链上前一条失败不会阻断后一条。
- `tabs` 命令整体不排队：它的子命令操作标签页本身，不进页面执行链；`batch` **外层不排队**——
  它内部的每个子命令都会各自调用 `executeTool` 排队，外层再持锁会等自己（自锁）。
  单块之间因此仍可并行。
- `open` 不带 `tabId` 是新建标签，没有可争用的页面状态，不排队；显式给 `tabId` 时
  与该 tab 的其他命令互斥。
- `lastAct` 上下文按 tab 分槽保存，写入串行化且按 `updatedAt` 取较新值；
  `await` 只读对应 tab 的槽，不会被另一个 tab 的并发 act 覆盖。
- daemon 侧 pending 仍按扩展连接 generation 隔离，per-tab 串行只约束扩展里的执行顺序，
  不改变每个请求各自的超时预算。

## act settle / await

动作执行和页面稳定是两件事：

- `act` 执行后记录 `lastAct` 上下文（tab、expect、baseline URL/doc/title、耗时、新标签）。
- 导航意图（链接/表单提交/back/forward/reload）最多 settle 3s；普通按钮最多 800ms；
  其他动作不等待。页面提前稳定就提前返回。
- `settle=pending` 不是错误，退出码仍为 0；输出 `next_url=` 和
  `hint=call ema2 await 10s`。
- `ema2 await 10s` 读取 lastAct，继续等上一个动作的效果；不带 `--tab` 时使用动作所在 tab。
  显式 await 超时才返回 `wait_timeout`（退出 4）。
- 动作打开的新标签会返回 `new_tab=`；opener 明确的才自动登记进会话，`tabs close --session` 能清理。
  noopener 标签只用于定位，避免误关用户手动打开的 tab。
- `.ema2`/`--steps` 里 `await 10s` 是脚本专用步骤；如果上一步 pending 且下一步不是 await，
  脚本停止，避免在旧页面继续点击。
- `act` 的工具超时预算会叠加脚本中的 await + 每步 settle，超过 600s 在执行前拒绝。
- 脚本里的 `waitFor` 是独立步骤，在所在行执行；失败时已完成的 `stepN` 会随错误返回。
- `waitFor` 并行支持两类后置条件：`url`/`status` 等网络/导航，以及
  `text`/`textGone`/`selector` 页面条件。页面条件复用 `wait` 的判定；成功追加
  `expect=ok condition=... ms=...`，超时为 `wait_timeout` 并带最后 URL/文本。
  页面条件是单一条件，不能和 `url`/`status` 混用；核心不判断业务成功与否。

## command guard 生命周期（v5）

- manifest 不再永久注入页面 guard；普通浏览和 daemon 空闲时不覆盖任何页面 API。
- 页面命令开始时登记 guard lease；第一个 lease 通过
  `chrome.scripting.registerContentScripts` 动态注册 MAIN world、document_start、
  all-frames 的 `page_guard.js` + `page_guard_armed.js`，match 限定 `http(s)`，
  覆盖导航后的新文档，租约结束注销。
- **原生 API 只留在闭包里**：`original.alert/confirm/prompt/print/open` 不会被挂到
  `window.__ema2_guard`（旧版本暴露过后页面可以直接改写回去）。页面能看到的只有只读探针
  `probe()` 和受控方法；`armed` 是 getter，不可重定义。
- **`probe()` 三态**：没有 guard → `{guard:false}`（已干净）；有 guard 但没有
  `probe()`（v5 之前）→ `verified:false`，不能凭 `armed:false` 这种裸布尔判定；
  有 `probe()` → `verified:true`，只有 `armed===false && wrapped===false` 才算真正恢复。
  probe 自身抛异常时保守地当作仍被包装。
- **arm 租约 nonce**：每次 arm 生成随机 lease，页面只接受带同一 lease 的
  `ema2-guard-disarm` 事件；后台 release 也必须回显该 lease，伪造的布尔值无法通过验证。
- **guard 作用域收窄到本次命令的目标 tab**（`commandGuardTargetTabs`）：只有目标 tab
  上报的 `ema2/guard-armed` 会拿到非零租约并进入清理集合；长命令期间用户在别的标签
  打开的页面拿到 `lease=0`，bridge 会立即让它自行 disarm，不吞无关页面的对话框。
- 对已经打开的 tab，扩展立即注入并 arm；读取、快照、动作、等待、JS、观测、
  上传和截图都在租约内执行。`open` 先导航，再 arm 最终文档。
- 扩展只记录真正被 arm 过的 tab（`ensurePageGuard` + document_start 新文档上报），
  不再每条命令扫全部标签。该集合写入 `chrome.storage.session`
  （`ema2_command_guard_tabs`），service worker 重启后仍能继续清理。
- 最后一个 lease 结束时**先**注销动态脚本，**再**对记录过的 tab 广播
  `ema2-guard-disarm` 文档事件。
  disarm 分 top frame 和 all-frames 两次执行并分别验证：只有 top frame 与所有
  可脚本化 frame 都经 `probe()` 确认恢复，tab 才移出跟踪集合并持久化；受限/刚导航的
  frame 不能让 top frame 一起失败，也不能被成功结果掩盖。
- **验证失败保留 tab 跟踪**：未通过的 tab 留在 `commandGuardTabs` 里，下次 release/启动
  继续重试，并打一条内部诊断 `console.warn('[EMA2] guard_unverified tab=...')`。
  这个标签只出现在扩展控制台，不是 CLI 错误码，命令退出码不受影响。
- 每个 tab 的清理有单独时限，整批还有总上限；卡在原生对话框里的渲染进程不能拖死
  后续命令。释放窗口内仍在加载的新文档由 ISOLATED world bridge 询问租约，
  已归零就立即自行恢复原生 API。
- alert/confirm/prompt/print/window.open 以及赋值型 `onbeforeunload` 恢复原生行为。
- `page_guard.js` 默认 disarmed；从旧版本升级时先恢复原生 API，再用 `probe()` 复核；
  v1 的 own `onbeforeunload` accessor 不会被当成原生引用。
- `ema2_guard_protocol` 标记避免每次 service worker 唤醒都全量清理已升级页面；
  修改 guard 清理协议时与 `page_guard.js` 的 `GUARD_VERSION`（当前 5）一起提升。
- guard 注册失败不影响工具调用；页面若主动弹出同步对话框，仍由 watchdog
  返回 `tool_timeout`。

## ref 模型

- snapshot 给 `[e<seq>@<doc>]`，`doc` 是扩展分配的短句柄（`d1/d2…`），
  落 `chrome.storage.local`，浏览器重启后继续递增，避免新旧文档撞号；
- 完整 ref 校验 `doc` 必须等于当前文档；短 ref（`e12`）只在当前文档内解析；
- 导航/reload/换 tab 后旧完整 ref 返回 `stale_ref`；
- 密码框名称只来自 label/ARIA/placeholder/alt/title，绝不读 value；textarea/contenteditable 的用户输入同样不进入名称。

## read 正文清洁

- `text`/`md` 共用一次 DOM 遍历，产出 block/inline 中间结构，再由两个 renderer 输出；
  两者都跳过 script/style/noscript/template/textarea/input/select/option、
  nav/header/footer/aside/form 等页面壳，以及 `display:none`/`visibility:hidden`/`content-visibility`
  的隐藏元素。
- `html` 保持 `root.innerHTML`；`visible` 直接取 `root.innerText`，不跑清洁遍历，
  因此会包含导航/侧栏等渲染文本，但仍复用 main/selector/no-main 选区。
- `mainRoot` 用清洁后的文本和链接密度评分；候选 <200 字符，或 body>2000 字符且候选不到 body 5%
  时回退 `main=body(fallback)`，并输出 `hint=main extraction too short (...)`。
- `text`/`md` 遍历会进入 open shadow root；closed shadow root 仍不可见。
  `visible` 的 shadow/iframe 覆盖范围由浏览器 `innerText` 决定，不作为契约。
- `md` 是有损渲染：标题/列表/表格/代码块保留，链接写成 `[text](绝对URL)`，
  行内 code/strong/em 保留；样式、合并单元格、脚本生成内容会丢。
- `mainRoot` 先按 `textContent` 长度门槛过滤，最多对 12 个候选跑清洁评分，避免大页面 CPU 峰值。
- main 太短、百度 hidden textarea JSON、GitHub dashboard JSON、隐藏弹窗文本都不会再污染正文。

## 网络 / console hook

- `open` / `act` 会自动在 MAIN world 安装 hook；
- 网络 hook 包装 `fetch` 和 `XMLHttpRequest`，记录真实 status、耗时、URL；
- 响应预览通过 `response.clone()` 后台读取，最多 8KB / 1.5s，不阻塞页面；
- `observe network --bodies=false` 真正关闭 body clone；
- `bodyFilter` 限 512 字符，拒绝嵌套量词/量词组内分支等指数回溯形态；
- `--har` 导出 HAR 1.2 采样，不是完整抓包；JSON 不经过普通文本预算截断；
  `authorization`/`proxy-authorization`/`cookie`/`set-cookie` 默认脱敏为 `<redacted>`，
  `--headers all` 才导出原值；
- console hook 记录 log/info/warn/error/debug，支持增量 `--since` 和 `--filter`。

## js 的两条通道

`js` 默认走 debugger（`Runtime.evaluate`，`returnByValue` + `awaitPromise`），
先用 `Runtime.compileScript` 做 compile-only 探测：表达式能编译就只执行一次，
否则按 async 函数体执行；运行时 `SyntaxError` 绝不重跑（避免副作用执行两次）。
默认 10s 执行上限（`--timeout` 可调），超时返回 `script_timeout` 并 `Runtime.terminateExecution`。

当 `debugger.attach` 被拒（DevTools 占用，或 tab 里有**别的扩展**的页面目标）时：

1. 先按该 tab 的 page target `targetId` 重挂一次；
2. 仍失败则降级到 `chrome.scripting.executeScript({world:'MAIN'})`，在页面侧 eval，
   输出末尾多一行 `via=scripting`。

降级路径的 `eval` 受页面 CSP 约束（严格 CSP 站点会 `js_exception`），
且没有 `Runtime.terminateExecution` 兜底；看到 `via=scripting` 就知道本次没走 debugger。

## files upload

CLI 读本地文件 → base64 随 `/cmd` 请求下发 → 扩展在页面 MAIN world 里用
`DataTransfer` 构造 `File` 赋给 `input.files`，再派发 `input/change`。
因此上传**不依赖 debugger**，在“debugger 被别的扩展挡住”的 tab 上同样可用。
单次 ≤20 个文件、原始载荷 ≤5 MiB。axum `Json` 的 `DefaultBodyLimit`、
`RequestBodyLimitLayer`、扩展 WS 入站上限都放开到 8 MiB，base64 膨胀后仍有余量。

## 前台语义

扩展不提供 headless：`shot` 用 `captureVisibleTab`，只截窗口里**活动的 tab**；
`tabs activate` / `open`（新建标签）都会
`chrome.windows.update({focused:true})` 把窗口提到前台（`tabs new` 的 `/cmd` action 仍保留该行为）。
`open` 输出 `focused=1|0`：`0` 表示窗口不在前台，会附 hint。
测试脚本必须据此断言 `document.hasFocus()`（见 TESTING.md）。

## batch（/cmd 执行器）

CLI 没有 `batch` 子命令；`read/snapshot/js/wait --tabs/--urls` 与 `open --urls`
在 CLI 侧编译成一次 `batch` `/cmd` 调用。执行器本身：

- `--urls`：每个 URL 开新标签再执行目标工具；`tool=open` 只并行开页；
- `--reuse url`（旧名 `session`）：只按规范化完整 URL 复用本会话标签，未命中仍新开；
- `--tab-ids`：对已有标签并发执行；
- 并发默认 2，最大 4；`js` 因全局 debugger 串行，预算按有效并发 1 计算；
- 每块 `---` 分隔，单块失败不影响其它块；
- 内层参数复用子工具 validator，不能绕过 `wait time=300` 等上限；
  `--urls` 默认新开标签，内层 `--replace`/`--reuse`/`tabId` 会被拒绝。

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
