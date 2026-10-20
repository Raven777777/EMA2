# EMA2 (Rust) 架构说明 · v1.12.1

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
当前为 **v1.12.1：11 个工具**（参数与输出协议见 [TOOLS.md](TOOLS.md)，
版本历史见 [CHANGELOG.md](CHANGELOG.md)）。

## 组件

| 文件 | 职责 |
|---|---|
| `main.rs` | clap：`install`（日常 Edge 部署/自启/卸载）/ `run` / `pair` / `status` / `reload` |
| `http.rs` | axum 路由 + Host/Origin/Content-Type 守卫 + 会话/事件流管理 |
| `mcp.rs` | 协议：版本协商、tools、工具调用、截图落盘、工具错误封装 |
| `relay.rs` | 扩展 WS 连接、`hello`→`welcome` 握手、保活 |
| `state.rs` | 扩展连接（generation + sender）、pending 映射、会话/事件流、抗毒化的锁封装 |
| `tools.rs` | 11 个工具的**唯一**注册表（schema + 校验 + 超时） |
| `pair.rs` | token 生成/持久化、HMAC 挑战应答 |
| `clients.rs` | 五类终端检测与配置合并（opencode / Claude Code / Claude Desktop / Codex / Pi） |
| `jsonc.rs` | 文本级 JSONC 编辑：只增删一个成员，保留注释与排版 |
| `config.rs` | 端口/保活/token/状态目录/输出目录 |

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
- `since` 用上次返回的 `next` 做增量；环形缓冲溢出时回 `dropped=`，导航后序号回绕时回 `reset=1`
- 预览未完成的条目不会被截断丢弃：行尾标 `streaming=1`，尾部给 `pending=<n> seqs=...`；
  分页水位始终前进，不会因某条 body 一直 pending 而卡死
- `bodyFilter` 只对已就绪的 body 求值，并在编译期拒绝量词嵌套/量词组内分支这类
  会指数回溯的形态（JS 正则没有超时，阻塞 service worker 等于整个桥接停摆）
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

- 文本：`[e12@d1] button "提交" @x,y wxh`（不依赖视觉模型也能用）
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

- `POST /reload`（回环 + token）→ 扩展在线时发 `bridge/reload`（200）；离线返回 202，
  扩展下次启动 worker 时会直接从磁盘读新文件，不额外排队重载
- 扩展收到后 `chrome.runtime.reload()`，等价于在 `edge://extensions` 点“重新加载”
- CLI：`ema2.exe reload` 先 GET `/status` 确认中继可达且扩展已连接，再 bump
  `extension/manifest.json` 的 patch 版本并 POST `/reload`
  （Edge 按 manifest 版本缓存扩展脚本，只发 reload 不会重读磁盘）；
  服务不通或扩展未连接时不会改版本号；reload 失败时把 manifest 原样写回，不留版本漂移
- 注意：扩展必须已经加载了带该 handler 的版本，之后才能自刷新

## ref 快照模型（v1.8，v1.12 瘦身）

`snapshot` 在页面里给交互元素打 `data-ema2-ref="e12@d3"`。`window.__ema2_ref_seq` 在同一 document
内单调递增，因此同一元素在多次快照间保持同一 ref，而硬导航、同 URL reload、另一个 tab 都不会复用旧 ref。
`snapshot` 头部回 `doc=d3`。

### 为什么句柄由扩展分配（v1.12）

v1.8 的 epoch 是 `t<tabId>-<随机串>`，在页内生成。它解决了唯一性，但每行要背 18 个字符：
跨 12 个站点实测，文档标记占 snapshot 字符的 **28%**（x.com 这种短行页面高达 43%）。

改成短句柄 `d1、d2…` 时，**分配必须留在扩展侧**：
- 页面自己数序号会跨源重置（sessionStorage 按 origin 分区），同一 tab 换了域名后可能重号；
- 页面自己取随机短串（4 字符）在 500 个文档量级下碰撞率约 0.07%，而这正是防止「旧 ref 点错元素」的那把锁。

所以 `ensureHelpers` 只在「本次真的注入了 helper」时才从 `chrome.storage.local` 自增取号
（同步自增，两个文档不会撞号），再用一次注入 `setDocHandle()` 下发给页面。已装好的页面不消耗号，
号不会被空调用推高。页面侧 `refIsCurrent` 同时要求句柄非空且与当前文档一致。

计数器落 **local** 而不是 session：session 在扩展重载/更新时会被清空，归零后新文档重新拿到 d1、d2…，
会和仍然开着的旧页面撞号，旧 ref 就能在另一个文档上通过校验。

### 输出瘦身（v1.12）

跨 12 站点、942 行、73k 字符的字节构成：

| 成分 | 占比 | 处理 |
|---|---|---|
| `href=` 等额外属性 | 39% | 截到 40 字符（`filter=links` 不截） |
| 文档标记 | 28% | 短句柄 `@d1`，每行从 22 字符降到 5 |
| 名称 | 19% | 已在 60 字符封顶 |
| 角色词 | 7% | 保留 |
| 引号 | 2% | 保留（分隔 name 与额外属性） |

另外「无名称且不可操作」的节点（`presentation ""`、`banner ""`）默认不输出——它们在 x.com 占 20% 的行、
其余站点 0~1%；`filter=full` 保持无损视图。

```
[e12@d3] button "提交" disabled
[e13@d3] textbox "搜索" name=q
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

- `initialize` 返回 `capabilities.tools` 与 `capabilities.logging`，并创建 `Mcp-Session-Id`；
  非 initialize 必须携带（否则 400/404）
- `GET /mcp` 是真正的服务端事件流：扩展连接/断开时 relay 调 `broadcast_event`，
  向每个打开的流推送 `notifications/message`（`logger=ema2`），客户端无需等下一次
  `tools/call` 才知道扩展掉线
- 每个 session 只保留一条事件流：重复 `GET /mcp` 会替换旧流并立即关闭旧连接
  （日志记录被替换的 stream id），不会叠加注册
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

## 安装、配对与客户端配置（v1.10）

目标：AI 零配置直接用，人只点一次授权，扩展部署/更新由 exe 负责。

```text
ema2.exe install
  ├─ 在 Edge User Data 的 Preferences 里找已加载的 EMA2 扩展
  │    ├─ 找到（location=4 解包 / 8 命令行）→ 原地覆盖该目录（日常 Edge，升级即生效）
  │    └─ 没找到 → 部署到 %LOCALAPPDATA%\EMA2\extension，打开 edge://extensions 并打印指引
  │    （商店/策略安装的只读目录不会被覆盖）
  ├─ 注册登录自启（Startup 目录的 EMA2.vbs → 隐藏启动中继）
  └─ 中继以待授权态启动：只开 /pair/status 与 ?pair=1

扩展（未配对）→ ws://127.0.0.1:8765/?pair=1
  → bridge/hello → welcome{pairing:true} → 自动打开 pair.html
  → 授权页列出检测到的终端，人勾选后点“授权此浏览器”
  → bridge/pair-claim{clients:[...]}
  → 中继生成 32B token、写 state.json、按选择写终端配置
  → bridge/pair-granted{token, selection} → 扩展存 storage 并用 ?token= 重连
```

- 待授权态 `/mcp` 一律 503；配对通道不注册为工具连接，只接受 hello / pair-claim
- 配对一次性：已有 token 时 `?pair=1` → 409，`ema2.exe pair --reset` 后重来
- 每次连接扩展发随机 challenge，中继回 `HMAC-SHA256(token, "ema2-welcome|" + challenge)`；
  扩展验签失败立即断开且不置 ready（防止本地进程抢占 8765 冒充中继）
- token 优先级：`EDGE_MCP_TOKEN` > `state.json`；状态文件原子写
- `install --uninstall`：删除 Startup 自启、五类终端里的 `edge-browser` 条目、托管 profile、
  部署的扩展与状态文件；浏览器内的扩展只能由用户在 `edge://extensions` 移除

### 客户端配置合并

| 终端 | 键 | 形状 | 策略 |
|---|---|---|---|
| opencode | `mcp.edge-browser` | `type=remote` + `headers` | JSONC 文本级插入/替换 |
| Claude Code | `mcpServers.edge-browser` | `type=http` + `headers` | JSON 文本级插入/替换（必须仍是严格 JSON） |
| Claude Desktop | `mcpServers.edge-browser` | `url` + `headers` | 同上 |
| Codex | `[mcp_servers.edge-browser]` | `url` + `http_headers` | TOML 表段替换/追加，保留其他表 |
| Pi | `mcpServers.edge-browser` | `url` + `headers` | 写 `~/.pi/agent/mcp.json` |

- `jsonc.rs` 只扫描对象成员边界（字符串/注释内的括号不计数），在文本上插入或删除目标成员；
  除新增条目外不改动任何字节，注释、键顺序、缩进、CRLF 都保留
- 写入前先用 `validate()` 完整解析原文件，畸形配置直接报 `invalid JSON/JSONC`，不做文本手术
- 写入后再次解析并确认 `edge-browser` 的 url/token 正确；Codex 结果用 `toml` 解析校验
- 幂等：只比较 `url` 与 `Authorization`，用户给该条目加的额外键不会被反复覆盖
- **每次**改写/移除前都把当前内容写入 `<file>.ema2.bak`（覆盖旧备份）；文件原本不存在时写 0 字节备份；
  结果里带 `backup` 路径，授权页直接展示
- `apply(selection)`：写入勾选项，并移除未勾选项里的 `edge-browser`。只移除“像 EMA2 写的”条目
  （url 指向 `127.0.0.1:*/mcp`，或 Authorization 是 64 位 hex bearer）；别的条目原样保留
- `EMA2_SKIP_CLIENT_CONFIG=1` 可整体禁用

## MCP 自我描述（v1.10.1）

`initialize` 的结果决定模型怎么理解这组工具：

- `instructions`：明确“操作的是用户真实、可见的 Microsoft Edge（真实 cookie/登录态），
  不是无头、远程或沙箱”，并明确要求模型不得声称“没有控制浏览器的工具”
- `serverInfo.name = "ema2"`，`title = "EMA2 — user's real Microsoft Edge browser"`
- 11 个工具都带 `title`，描述统一以 “On the user's real, visible Microsoft Edge browser...” 开头
- `tools/list` 的 schema 体积上限相应放宽到 7KB

没有这段说明时，实测模型会把 `tabs/open/read` 当成通用网页或无头工具，甚至一边否认
一边成功调用；这些字段就是给模型的“身份说明”。

## 标签复用与会话收尾（v1.11）

长会话里“每次 open 都新开一个标签 + 手工记 tabId”是主要摩擦来源，因此：

- 扩展维护 `sessionTabs`（有序 id 列表），落在 `chrome.storage.session`：
  MV3 service worker 随时会被回收，只放内存会丢；放 `local` 又会跨浏览器重启残留。
  `session` 的生命周期正好是“本次浏览器会话”
- 只有真正**新建**的标签（`open` 默认分支、`tabs new`）进入会话；
  `open tabId=`/`replace=true` 导航的是用户已有的标签，登记会造成
  `scope=session` 误关用户页面，所以明确排除
- `tabs close scope=session` 逐个 `tabs.remove`，对已不存在的 id 静默跳过（用户可能已手动关掉）
- `open` 的导航目标三选一（`tabId` / `replace` / `reuse=session`），校验在中继与扩展两侧同源，
  避免“看起来指定了但实际导航了别的标签”

## 真实状态码（v1.11）

`open` 在页面就绪后读 `performance.getEntriesByType('navigation')[0]`：
`responseStatus` 给出主文档真实 HTTP 状态、`redirectCount` 给出重定向次数。
选它而不选 `Network.responseReceived` 的原因：后者要求常驻 debugger 会话，
每次 `open` 都会在浏览器顶部留一条“正在调试此浏览器”的横幅，且与 `js`/`shot` 的全局
串行 debugger 队列相互阻塞。`wait=none` 时导航尚未完成，返回 `status=pending`。

同一个坑还有一处：`tabs.update` 之后旧文档依旧是 `complete`，只看 `readyState` 会立刻判「已就绪」，
于是 `replace`/`reuse`/`tabId` 导航返回的是上一页的 `url/title`。所以 `open` 的就绪判定改成
`waitForDocument()`：同时读页内 `location.href` 与 `readyState`，必须确认**已离开旧文档**
（重定向到别的地址同样算离开）且 `readyState` 达标才返回。

## 安全

- 只绑 `127.0.0.1`；Host/Origin 只允许回环与 `chrome-extension://`
- 强制 `Content-Type: application/json`（阻止简单请求绕过 CORS 预检）
- **token 默认必填**：未设 `EDGE_MCP_TOKEN` 且未显式 `--insecure-allow-no-token`
  （或 `EDGE_MCP_ALLOW_ANONYMOUS=1`）时拒绝启动，退出码 2；匿名启动会打 `ERROR` 日志
- token 传输：HTTP `Authorization: Bearer`，扩展 WS `?token=`；常量时间比较
- `bridge/welcome` 握手：扩展不对未知服务执行指令
- `panic = "unwind"` + `state::lock` 忽略锁中毒：单个任务 panic 不会带走整个中继

## 测试

完整测试矩阵、覆盖映射和真实 Edge 验收边界见 [TESTING.md](TESTING.md)。

```bash
# Rust：98 个单元测试（含 JSONC/TOML 合并、选择/移除、备份、MCP 元数据、SSE、配对/HMAC）
cd rust && cargo test --locked --offline
cargo clippy --locked --all-targets -- -D warnings

# 扩展/脚本回归：45 项（Node 22+ + jsdom devDependency）
npm install
npm run test:js

# 真实 Edge/MV3 验收：22 场景（隔离 profile + 隔离 HOME）
npm run test:edge

# Windows 进程管理自检与外部分占用者保护
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/bridge.ps1 -SelfTest
```

覆盖：协议版本协商、11 工具 schema/参数校验（含 null/枚举/范围）、
wait 与 batch 超时预算、pending 代际隔离与取消释放、token 必需性、事件流投递/替换与 Drop、
锁中毒后仍可用、会话 GC、截图落盘/失败、Host/Origin/Content-Type 拦截、notification 202；
扩展侧覆盖 F01–F13 的网络非阻塞、敏感信息、文档作用域 ref、debugger deadline、
截图活动页、观测分页、Markdown、动作结果、press 目标、跨文档网络水位和参数边界。
真实 Edge 验收已在 Edge 155.0.4283.62 上 16/16 通过（独立临时 profile）；
详见 [TESTING.md](TESTING.md)。
