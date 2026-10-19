# EMA2 (Rust) 架构说明 · v1.8.0

## 总览

EMA2 用**一个常驻进程**同时向 AI 提供 MCP over HTTP 服务、向 Edge 扩展提供
WebSocket 桥接，让 AI 直接控制真实浏览器（真实指纹、真实登录态），
不依赖 CDP 调试端口。

```
┌──────────────┐  MCP over HTTP (Streamable HTTP / SSE)
│  AI 客户端    │ ───────────────────────────────┐
└──────────────┘                                  ▼
                                         ┌────────────────────┐
                                         │   ema2.exe (Rust)  │  ← 单一常驻进程
                                         │  POST /mcp /status │
                                         │  WS / (扩展)        │
                                         └──────────┬─────────┘
                                                    │ WebSocket + 10s ping
                                                    ▼
                                            ┌─────────────────┐
                                            │ Edge 扩展 (MV3)  │  chrome.*
                                            └────────┬────────┘
                                                     ▼
                                             真实浏览器（页面/登录态）
```

v1.4 是**面向 AI 的重新设计**，与 v1.3 的工具名不兼容；
v1.5–v1.7 逐步补上深度 DOM、文件、网络、并发与视觉兜底。
当前为 **v1.8.0：11 个工具**（参数与输出协议见 [TOOLS.md](TOOLS.md)，
版本历史见 [CHANGELOG.md](CHANGELOG.md)）。

## 组件

| 文件 | 职责 |
|---|---|
| `main.rs` | clap：`run`（优雅退出）/ `status`（带 token） |
| `http.rs` | axum 路由 + Host/Origin/Content-Type 守卫 + 会话管理 |
| `mcp.rs` | 协议：版本协商、tools、工具调用、截图落盘、工具错误封装 |
| `relay.rs` | 扩展 WS 连接、`hello`→`welcome` 握手、保活 |
| `state.rs` | 扩展连接（generation + sender）、pending 映射、会话/事件流 |
| `tools.rs` | 11 个工具的**唯一**注册表（schema + 校验 + 超时） |
| `config.rs` | 端口/保活/token/输出目录 |

## 工具面（11 个）

```
tabs     list/current/new/close/activate
open     url → 新标签或指定 tab 导航
read     md|text|html，selector 限定，budget/cursor
snapshot 交互元素 + heading → [eN@doc] ref
act      click/type/fill/select/press/hover/scroll/submit/back/forward/reload
         + steps[] 批量（可跨导航）+ waitFor 等响应
wait     text / textGone / selector / time / idle
js       MAIN world 执行（debugger，await 支持）
observe  console/network 增量（since = 上次 next）
files    upload（DOM.setFileInputFiles）/ download-dir
shot     PNG；默认落盘回路径，inline=true 回图片
```

设计要点：

- **schema 只写一次**：`tools/list`、白名单、校验、超时都由 `TOOLS` 派生
- **工具名短**：模型每轮都要看这 11 个名字
- **粗粒度**：用 `action` 枚举合并同类操作，减少工具数量与选择成本

## 输出协议（AI 优先）

- 头部 `key=value`；数据行 `[ref] role "name" extras` 或 TSV
- 列表固定 `total=<n> shown=<m> next=<cursor|none>`
- 长文本按 `budget`（默认 4000）切，`cursor` 续读
- 截图默认只回 `path=... bytes=... mime=...`
- 工具错误作为 `isError: true` 的文本结果：`error=<code> msg=... hint=...`
  - 只有“工具不存在 / 协议非法”才走 JSON-RPC error

## 深度 DOM 遍历（v1.5）

`snapshot` / `read` / `act` / `wait` 不再只看顶层 `document`：

- 递归遍历 **open shadow roots**，以及**同源 iframe**（`contentDocument`）
- 每个根节点按文档顺序取 `h1,h2,h3` + 交互元素，跨根去重
- 非顶层元素在行尾标 `frame=f1`（仅供阅读；ref 在同一 document 内唯一，直接传 ref 即可）
- `act` 的 `ref` / `selector` / `target` 在全部根里查找
- 跨域 iframe 无法访问，不计入；`snapshot` 头部 `roots=` 是实际遍历到的根数量

## 等待响应（v1.8）

`act` 支持 `waitFor: { url?, status?, timeout? }`：

- 动作**之前**记录网络水位 `{documentEpoch, seq}`，不同文档的 seq 不可比较
- 动作后轮询 hook 里的新条目，命中返回 `response=<status> <method> <path> <ms>`
- 导航后 hook 重建、epoch 改变：从新文档 seq=0 开始匹配，不再跨序列取 `min`
- hook 安装前已发生的请求只有 `performance` 回填（`status=-1`），**不能**作为本次动作成功证据；
  超时会明确带 `observation_gap=status_unavailable`
- 超时返回 `error=response_timeout` + hint

## HAR 导出（v1.7）

`observe kind=network har=true` 把本次匹配到的条目转成 HAR 1.2 JSON；
中继（Rust）负责落盘到输出目录并只回 `path=... bytes=... entries=N`。
HAR 是**采样/预览**：`startedDateTime` 使用导出时间，响应体只有受限预览，
历史回填条目的 `status=-1` 不会被伪装成完整响应。

## 多标签并发（v1.7）

`batch { tool, tabIds[], args, concurrency }`：

- 只允许 `read / snapshot / js / wait`（Rust 与扩展各校验一次，防递归）
- 默认并发 2，上限 4；每个 tab 输出 `tab=<id>` 块，块之间 `---` 分隔
- 单块失败不影响其他块（失败块输出 `error=...`）
- batch 内层参数复用子工具 validator，不能绕过 `wait.time ≤ 300` 等限制
- 中继预算按 `ceil(tabCount / 有效并发) × 单次最长耗时 + 15s` 计算；
  `js` 受全局 debugger 串行队列限制，有效并发按 1 计算。估算超过 600s 时在调用前拒绝

## 视觉兜底（v1.7）

`snapshot` 头部带 `visual=<canvas/video/img 数量>`；当 `refs=0 && visual>0` 时追加
`hint=page looks visual (canvas/img); try shot annotate=true`。
配合 `shot annotate=true` 的编号图和坐标列表，模型即使读不到 DOM 也能定位。

## 正文提取（v1.6）

`read` 默认 `main=true`：在 `main / article / [role=main] / #content / .post …` 候选中，
按 `text.length - linkLength * 1.5` 打分选正文容器，并在头部回 `main=<选择器>`；
Markdown 转换时跳过 `NAV/HEADER/FOOTER/ASIDE/FORM`，显著降低页面样板对上下文的污染。
转换按 `childNodes` 遍历，根节点直接文本和“文本 + 块元素”混排都不会漏字或重复。
需要整页时传 `main=false`，需要精确区域时传 `selector`。

## 网络观测（v1.8）

`observe kind=network` 在 MAIN world hook `fetch` / `XMLHttpRequest`：

- 记录 method、url、**真实 status**、耗时、`seq`
- **先返回原始 Response**，再在独立异步任务里读取 `response.clone()` 的有限前缀：
  最多 8KB / 1.5s，读完主动 cancel；事件流和二进制类型默认跳过
- `bodies=false` 会真正关闭新响应的 clone/读取，而不只是隐藏已采集正文
- `since` 用上次返回的 `next` 做增量；`next` 只推进到**已完整交付**的记录，
  环形缓冲溢出时回 `dropped=`，导航后序号回绕时回 `reset=1`
- `wait` 给异步请求留时间（最多 10s）
- `open` / `act` 之后会自动安装 hook（`open` 输出里带 `net_hook=on|off`）
- 首次安装会用 `performance` 资源条目回填历史（`status=-1`，表示拿不到状态码）
- hook 在页面主世界；通过 `js`（debugger 上下文）直接发起的请求可能只以
  resource 条目出现（`status=-1`），页面自身代码的 fetch/XHR 不受影响

## 截图标注（v1.8）

`shot` 只允许目标 tab 是所在窗口的**活动 tab**；`captureVisibleTab` 永远截当前活动页。
非活动目标返回 `error=tab_not_active`，不会静默换成前台页图片。
截图前后都会复核活动 tab；`annotate=true` 还会复核 document epoch，变化时返回 `tab_changed`。

`shot annotate=true` 先做一次 `snapshot` 分配 ref，再取当前视口内每个 ref 的
`getBoundingClientRect`，用 `OffscreenCanvas` 在截图上画框和编号，返回：

- 文本：`[e12@doc] button "提交" @x,y wxh`（不依赖视觉模型也能用）
- 图片：标注后的 PNG（Rust 默认落盘，只回 path；`inline=true` 回图片）

## snapshot 过滤与局部（v1.6）

- `filter=links|buttons|form`：只返回链接 / 按钮 / 表单控件
- `near=<ref>`：先找该 ref，取最近的 `form/section/article/dialog/table/ul/main/nav/header/footer`
  作为容器，只在该容器内快照（避开整页噪声）
- 头部回 `near=<container>`，方便模型确认范围

## 文件能力（v1.5）

`files` 工具通过 debugger：

- `upload`：在 MAIN world 里深度找到 `input[type=file]`（或任意元素），
  `Runtime.evaluate` 拿 objectId → `DOM.requestNode` → `DOM.setFileInputFiles`
- `download-dir`：先试 `Page.setDownloadBehavior`，再试 `Browser.setDownloadBehavior`；
  两者都不可用时返回 `download_dir_unsupported`（Edge 不允许扩展改下载目录）
- 上传路径必须是平台绝对路径（Windows 下 `C:/dir/a.pdf`），否则 CDP 可能把渲染进程搞崩；
  Rust 与扩展各校验一层，非法路径直接 `error=invalid_argument`

## act 的 target、执行与按键（v1.8）

- `target="button:提交"` 先按 role 精确匹配 name，再退化为包含匹配
- 查找、控件校验、动作执行、结果检查在**同一次页面注入**内完成；目标被动态移除时不会假成功
- disabled / readOnly / 控件类型不支持会返回明确错误（`element_disabled`、`element_readonly`、
  `unsupported_control`、`value_not_available`、`no_form`）
- 目标找不到（SPA 未渲染）且给的是 `target`/`selector` 时，等 350ms + 重新注入
  helper 后自动重试一次；`stale_ref` 不重试（页面已变，必须重新 snapshot）
- `press`：给了 ref/selector/target 就先聚焦该元素再发键；完全没给定位条件才使用当前焦点。
  按键通过页面 `KeyboardEvent` 派发，Enter 在未被取消时会尝试 `requestSubmit()`；
  需要浏览器原生 Tab/组合键行为时应使用 `js` 或后续真实输入接口，不能把合成事件当原生默认行为
- `type`/`fill` 反馈只返回是否写入与字符数（`wrote=1 chars=N`），绝不回显输入值
- 失败时附带 `candidates`（前 5 个可见交互元素），方便模型直接改用 ref

## 扩展自刷新（v1.5）

- `POST /reload`（回环 + 可选 token）→ 中继向扩展发 `bridge/reload`
- 扩展收到后 `chrome.runtime.reload()`，等价于在 `edge://extensions` 点“重新加载”
- CLI：`ema2.exe reload` 先 GET `/status` 确认中继可达且扩展已连接，再 bump
  `extension/manifest.json` 的 patch 版本并 POST `/reload`
  （Edge 按 manifest 版本缓存扩展脚本，只发 reload 不会重读磁盘）；
  服务不通或扩展未连接时不会改版本号
- 注意：扩展必须已经加载了带该 handler 的版本，之后才能自刷新

## ref 快照模型（v1.8）

`snapshot` 在页面里给交互元素打 `data-ema2-ref="e12@<documentEpoch>"`。epoch 由
tabId + 随机串组成、每个 document 一份；`window.__ema2_ref_seq` 在同一 document 内单调递增，
因此同一元素在多次快照间保持同一 ref，而硬导航、同 URL reload、另一个 tab 都不会复用旧 ref。
`snapshot` 头部回 `doc=<epoch>`。

```
[e12@t7-k3a9] button "提交" disabled
[e13@t7-k3a9] textbox "搜索" name=q
```

`act`/`files` 用 `[data-ema2-ref="..."]` 找回元素，并先校验 ref 内嵌 epoch 是否属于当前 document；
导航/过期/跨 tab 使用旧 ref 会返回 `error=stale_ref hint=call snapshot again`，不会命中新目标。
找不到时给出 `candidates=`（前 5 个 ref），方便模型自我纠正。公开 ref 长度仍受 Rust 32 字节上限约束。

`act` 执行后会：
- 比对 url/title，输出 `changed=url,title`
- 列出动作后新出现的 ref（`new=[e14] ...`）
- 若 console 钩子已安装，输出 `console_errors=<n>`

这三样让模型不必再 `read` 一次确认。

## 超时预算（v1.8）

| 场景 | 中继超时 | 扩展侧 |
|---|---|---|
| 普通工具 | 30s | chrome.* |
| `act` | 60s + `waitFor.timeout` | 单步 + settle |
| `js` / `shot` / `files` | 120s | debugger / capture |
| `wait` 文本/选择器/idle | `timeout`（默认 30s）+ 15s | 同一 timeout，250ms 轮询 |
| `wait(time=N)` | N + 15s | 精确 N 秒 |
| `batch` | 波次 × 单次最长耗时 + 15s | 子工具并发 |
| debugger 单任务 | 受 `js`/`files` 120s 中继上限 | 60s 执行上限 + 600s 排队上限 |

中继超时必须大于扩展侧等待上限，否则模型只会看到 `extension timed out`。
batch 预算会把 tab 数、并发和子工具参数一起计算；`js` 按全局串行队列的有效并发 1 计算。
估算超过 600s 时，Rust 在调用前拒绝并提示拆分 batch。扩展侧 debugger 超时会先发
`Runtime.terminateExecution`，再 detach；队列继续执行后续任务。

## 会话与连接

- `initialize` 创建 `Mcp-Session-Id`；非 initialize 必须携带（否则 400/404）
- `GET /mcp` 服务端事件流按 `(session, stream_id)` 登记，旧流清理不误删新流
- legacy SSE：`GET /sse` + `POST /messages?sessionId=`
- 每 60s GC 10 分钟未活跃会话
- 扩展连接用 generation 区分；重连后旧响应被丢弃；pending 用 Drop guard
  保证客户端断开也不泄漏
- SSE 生成器内也有 Drop guard：客户端直接丢弃流时，事件流/legacy session 仍会被清理

## MV3 生命周期

- 连接期间 20s 心跳（`chrome.runtime.getPlatformInfo`）重置空闲计时器
- 每个工具调用内部再套一层心跳
- 30s watchdog alarm 兜底重连（1s→30s 指数退避）
- `act` 的 `steps` 每步重新注入 helper，因此可跨导航执行

## 安全

- 只绑 `127.0.0.1`；Host/Origin 只允许回环与 `chrome-extension://`
- 强制 `Content-Type: application/json`（阻止简单请求绕过 CORS 预检）
- 可选 token：HTTP `Authorization: Bearer`，扩展 WS `?token=`；常量时间比较
- `bridge/welcome` 握手：扩展不对未知服务执行指令

## 测试

完整测试矩阵、覆盖映射和真实 Edge 验收边界见 [TESTING.md](TESTING.md)。

```bash
# Rust：49 个单元测试（含超时预算、batch 内层校验、SSE Drop 清理）
cd rust && cargo test --locked --offline
cargo clippy --locked --all-targets -- -D warnings

# 扩展/脚本回归：19 项（Node 24 + jsdom devDependency）
npm install
npm run test:js

# Windows 进程管理自检与外部分占用者保护
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/bridge.ps1 -SelfTest
```

覆盖：协议版本协商、11 工具 schema/参数校验（含 null/枚举/范围）、
wait 与 batch 超时预算、pending 代际隔离与取消释放、token、事件流清理与 Drop、
会话 GC、截图落盘/失败、Host/Origin/Content-Type 拦截、notification 202；
扩展侧覆盖 F01–F13 的网络非阻塞、敏感信息、文档作用域 ref、debugger deadline、
截图活动页、观测分页、Markdown、动作结果、press 目标、跨文档网络水位和参数边界。
这些是 Node VM/jsdom 回归，不等于真实 Edge 集成测试。
