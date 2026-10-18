# EMA2 (Rust) 架构说明 · v1.7.5

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
当前为 **v1.7.5：11 个工具**（参数与输出协议见 [TOOLS.md](TOOLS.md)，
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
snapshot 交互元素 + heading → [eN] ref
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
- 非顶层元素在行尾标 `frame=f1`（仅供阅读；ref 全局唯一，直接传 ref 即可）
- `act` 的 `ref` / `selector` / `target` 在全部根里查找
- 跨域 iframe 无法访问，不计入；`snapshot` 头部 `roots=` 是实际遍历到的根数量

## 等待响应（v1.7）

`act` 支持 `waitFor: { url?, status?, timeout? }`：

- 动作**之前**记录网络序号基线（否则响应可能先到而漏掉）
- 动作后轮询 hook 里的新条目，命中返回 `response=<status> <method> <path> <ms>`
- 导航后 hook 会重建、序号归零，基线取 `min(baseline, current)` 避免永远等不到
- 超时返回 `error=response_timeout` + hint

## HAR 导出（v1.7）

`observe kind=network har=true` 把匹配到的条目转成 HAR 1.2 JSON；
中继（Rust）负责落盘到输出目录并只回 `path=... bytes=... entries=N`。

## 多标签并发（v1.7）

`batch { tool, tabIds[], args, concurrency }`：

- 只允许 `read / snapshot / js / wait`（扩展侧再次校验，防递归）
- 默认并发 2，上限 4；每个 tab 输出 `tab=<id>` 块，块之间 `---` 分隔
- 单块失败不影响其他块（失败块输出 `error=...`）

## 视觉兜底（v1.7）

`snapshot` 头部带 `visual=<canvas/video/img 数量>`；当 `refs=0 && visual>0` 时追加
`hint=page looks visual (canvas/img); try shot annotate=true`。
配合 `shot annotate=true` 的编号图和坐标列表，模型即使读不到 DOM 也能定位。

## 正文提取（v1.6）

`read` 默认 `main=true`：在 `main / article / [role=main] / #content / .post …` 候选中，
按 `text.length - linkLength * 1.5` 打分选正文容器，并在头部回 `main=<选择器>`；
Markdown 转换时跳过 `NAV/HEADER/FOOTER/ASIDE/FORM`，显著降低页面样板对上下文的污染。
需要整页时传 `main=false`，需要精确区域时传 `selector`。

## 网络观测（v1.6）

`observe kind=network` 不再读 performance entries，而是在 MAIN world hook `fetch` / `XMLHttpRequest`：

- 记录 method、url、**真实 status**、耗时、`seq`
- 默认截取响应体前 800 字符（`bodies=false` 关闭）
- `since` 用上次返回的 `next` 做增量；`wait` 给异步请求留时间（最多 10s）
- `open` / `act` 之后会自动安装 hook（`open` 输出里带 `net_hook=on|off`），
  避免“先发请求、后调 observe”漏掉数据
- 首次安装会用 `performance` 资源条目回填历史（`status=-1`，表示拿不到状态码）
- hook 在页面主世界；通过 `js`（debugger 上下文）直接发起的请求可能只以
  resource 条目出现（`status=-1`），页面自身代码的 fetch/XHR 不受影响

## 截图标注（v1.6）

`shot annotate=true` 先做一次 `snapshot` 分配 ref，再取当前视口内每个 ref 的
`getBoundingClientRect`，用 `OffscreenCanvas` 在截图上画框和编号，返回：

- 文本：`[e12] button "提交" @x,y wxh`（不依赖视觉模型也能用）
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

## act 的 target 与自动重试（v1.5）

- `target="button:提交"` 先按 role 精确匹配 name，再退化为包含匹配
- 目标找不到（SPA 未渲染）且给的是 `target`/`selector` 时，等 350ms + 重新注入
  helper 后自动重试一次；`stale_ref` 不重试（页面已变，必须重新 snapshot）
- 失败时附带 `candidates`（前 5 个可见交互元素），方便模型直接改用 ref

## 扩展自刷新（v1.5）

- `POST /reload`（回环 + 可选 token）→ 中继向扩展发 `bridge/reload`
- 扩展收到后 `chrome.runtime.reload()`，等价于在 `edge://extensions` 点“重新加载”
- CLI：`ema2.exe reload`，会先把 `extension/manifest.json` 的 patch 版本 +1，再 POST `/reload`
  （Edge 按 manifest 版本缓存扩展脚本，只发 reload 不会重读磁盘）
- 注意：扩展必须已经加载了带该 handler 的版本，之后才能自刷新

## ref 快照模型

`snapshot` 在页面里给交互元素打 `data-ema2-ref="eN"`（`window.__ema2_ref_seq`
单调递增，因此同一元素在多次快照间保持同一 ref），返回：

```
[e12] button "提交" disabled
[e13] textbox "搜索" name=q
```

`act` 用 `[data-ema2-ref="e12"]` 找回元素；页面导航后属性消失 →
`error=stale_ref hint=call snapshot again`；找不到但有快照时给出 `candidates=`
（前 5 个 ref），方便模型自我纠正。

`act` 执行后会：
- 比对 url/title，输出 `changed=url,title`
- 列出动作后新出现的 ref（`new=[e14] ...`）
- 若 console 钩子已安装，输出 `console_errors=<n>`

这三样让模型不必再 `read` 一次确认。

## 超时预算

| 场景 | 中继超时 | 扩展侧 |
|---|---|---|
| 普通工具 | 30s | chrome.* |
| `act` | 60s | 单步 + settle |
| `js` / `shot` | 90s | debugger / capture |
| `wait` 文本/选择器 | 45s | 30s 轮询（250ms） |
| `wait(time=N)` | N+10s | 精确 N 秒 |

中继超时必须大于扩展侧等待上限，否则模型只会看到 `extension timed out`。

## 会话与连接

- `initialize` 创建 `Mcp-Session-Id`；非 initialize 必须携带（否则 400/404）
- `GET /mcp` 服务端事件流按 `(session, stream_id)` 登记，旧流清理不误删新流
- legacy SSE：`GET /sse` + `POST /messages?sessionId=`
- 每 60s GC 10 分钟未活跃会话
- 扩展连接用 generation 区分；重连后旧响应被丢弃；pending 用 Drop guard
  保证客户端断开也不泄漏

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

```bash
cd rust && cargo test          # 42 tests
cargo clippy --all-targets
```

覆盖：协议版本协商、9 工具 schema/参数校验（含 null 与枚举）、超时预算、
pending 代际隔离与取消释放、token、事件流清理、会话 GC、截图落盘/失败、
Host/Origin/Content-Type 拦截、notification 202。
