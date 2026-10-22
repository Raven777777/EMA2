# EMA2 命令参考

单文件 `ema2` CLI。AI 通过 shell 调用子命令操作真实 Edge。
无 MCP、无 token、无授权、无配置文件。默认端口 **7744**。

> 分发包 = `ema2.exe` + `extension/` + `ema2 init` 生成的 `AGENTS.md`；**exe 内的
> `ema2 <cmd> --help` 才是用户侧权威文档**，本文是仓库内参考。

## 命令总览

| 命令 | 一句话 |
|---|---|
| `ema2 server` | 后台启动中继（一次性，立即返回） |
| `ema2 server stop` | 关闭中继 |
| `ema2 status` | 中继/扩展连接状态 |
| `ema2 init` | 在当前目录写 AGENTS.md |
| `ema2 tabs` | 列出 / 当前 / 新建 / 关闭 / 激活标签页 |
| `ema2 open` | 打开 URL，回 `status=` / `net_hook=` |
| `ema2 read` | 读页面为 `md\|text\|html\|visible`，支持 selector/budget/cursor |
| `ema2 snapshot` | ref 快照 `[e12@d1] button "X"` |
| `ema2 act` | click/type/fill/... 单步或批量脚本 |
| `ema2 await` | 等上一个 act 动作的页面效果（导航/页面稳定） |
| `ema2 wait` | 等 text / textGone / selector / time / idle |
| `ema2 js` | 页面主世界执行 JS |
| `ema2 observe` | console 增量 / network（真实状态码+响应体，HAR） |
| `ema2 files` | 上传本地文件；`download-dir` 固定报错 |
| `ema2 shot` | 截图（落盘 / inline / annotate） |
| `ema2 batch` | 多标签并发 open/read/snapshot/js/wait |

## 全局约定

- **必须先 `ema2 server`**。命令发现 7744 不通时报 `relay_not_running`，不自动启动。
- `--port N`：覆盖默认 7744。server 与所有客户端命令都要用同一个端口。
- `--tab ID`：多数命令支持，缺省操作当前活动标签页。`open` 会返回目标标签页的 `id`。
- **输出是行式紧凑文本，不是 JSON**。HAR 导出是唯一例外（对外标准格式）。
- **退出码**：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错。
- **错误**：stderr 首行 `error=<code> msg=... hint=...`，`<code>` 见错误码表。
- `budget`：返回文本的字符预算，范围 200–200000；默认值按工具不同：
  read / snapshot / tabs / wait 为 4000，js 为 8000，observe 为 3000。
- `cursor`：把上次返回的 `next`（数字）传给 `--cursor` 继续读。

## 输出协议

```text
# 头部：key=value（可能多行）
url=https://example.com/  title=Example Domain  scroll=0/1  refs=3  doc=d3
# 数据行：[ref] role "name" extras，或 TSV
[e1@d3] link "More information..." href=/help
[e2@d3] textbox "q" name=q
# 尾部：列表固定带 total / shown / next
total=2 shown=2 next=none
```

- `next=none` 表示没有更多；否则 `next` 是**数字**，把它传给 `--cursor` / `--since`
  （例如 `--cursor 4000`）。CLI 不接受字面量 `next`。
- `ref` 内嵌 document epoch（`e<seq>@<doc>`）：同一 document 内稳定，导航/reload/跨 tab 后旧 ref 会 `stale_ref`。
- 文件类结果：`path=<绝对路径>` 单独一行，后跟 `bytes=` 等元信息。
- 图片默认落盘；`--inline` 才回图片内容（CLI 输出 `data:image/png;base64,...`）。
- `type` 按当前 caret/selection 插入，`fill` 覆盖整个值；两者只回写入状态和字符数，不回显输入值。
  密码框的 value，以及 textarea / contenteditable 的用户输入，都不参与 snapshot 名称。

## 错误码

> 这张表是 AI 自我纠正的主要依据：新增 `fail('code'` 时必须同步这里，
> `cargo test` 里有一条测试会扫描 `extension/modules/*.js` 并断言每个 code 都出现在本文中。

| code | 含义 | 建议 |
|---|---|---|
| `relay_not_running` | 7744 不通，server 没起 | `ema2 server` |
| `extension_not_connected` | server 在，扩展没连 | 查扩展 popup 端口是否 7744 |
| `port_in_use` | 7744 被占用 | `ema2 server stop` 或换 `--port` |
| `relay_start_failed` | daemon 没起来 | 看提示里的 `%TEMP%\ema2.log` |
| `relay_error` | 已连上中继，但请求中途失败/响应不合法 | 检查 `ema2 status`；不要误以为 daemon 没起 |
| `payload_too_large` | 请求体超过中继上限（含上传） | 减小上传文件或拆分调用 |
| `shutdown_failed` | server 卡死且 PID 兜底失败 | 手动结束进程后重启 |
| `shutting_down` | daemon 正在关闭（HTTP 503） | 稍后重试；重试前先 `ema2 status` |
| `extension_error` | 中继连不上扩展（兜底） | `ema2 status` |
| `init_failed` | `ema2 init` 读写 AGENTS.md 失败 | 换可写目录，或检查文件权限 |
| `doc_handle_failed` | 文档句柄计数器无法读写 `chrome.storage.local` | 重载扩展；仍失败则检查浏览器存储/配置 |
| `invalid_argument` | 参数不合法（含脚本内层参数） | 看本节参数说明 |
| `invalid_arguments` | arguments 不是对象 | 用 `key=value` 形式传参 |
| `invalid_action` | 未支持的 act 动作 | 用 click/type/fill/select/press/hover/scroll/submit/back/forward/reload |
| `invalid_step` | `steps[]` 缺 action | 每步都要写动作名 |
| `invalid_tab` | tabId 不是非负整数 | `tabs list` 拿 id |
| `invalid_tool` | batch 不支持该工具 | batch 只跑 open/read/snapshot/js/wait |
| `invalid_url` | URL 不是 http(s) | 用完整 http(s) 地址 |
| `invalid_regex` | `bodyFilter` 不合法或有灾难性回溯风险 | 改写正则或改用子串 |
| `invalid_selector` | CSS selector 语法非法（`[`、未闭合括号等） | 修正 selector；或改用 snapshot ref/target |
| `unknown_tool` | 工具名不在白名单 | 11 个工具之一 |
| `missing_url` | `open` 缺 url | 传 URL |
| `missing_code` | `js` 缺 code | 传表达式或 `--file` |
| `missing_fields` | `fill` 缺 `fields[]` | `fill ref value` 或传 fields |
| `missing_files` | `files upload` 缺 paths | 传至少一个路径 |
| `missing_target` | 缺 ref/selector/target | 三选一 |
| `missing_key` | `press` 缺 key | 传键名 |
| `too_many_files` | 上传文件超过 20 个 | 分批传 |
| `page_changed` | 读取过程中页面已导航 | 等页面稳定后重试 |
| `too_many_steps` | act 步数超过 20 | 拆成多次调用 |
| `no_active_tab` | 没有活动标签页 | 先 `open` 或 `tabs new` |
| `tab_not_found` | 标签页不存在/已关闭 | `tabs list` |
| `stale_ref` | 页面已跳转/刷新/换 tab，ref 失效 | 重新 `snapshot` |
| `ref_not_found` | 同页但 ref 不存在 | 重新 `snapshot` |
| `target_not_found` | ref/selector/target 都找不到 | 用 `snapshot` 的 ref，或 `--target "role:name"` |
| `not_clickable` | 目标隐藏/空盒/不可聚焦/非交互容器 | 重新 snapshot，点真正可见的控件 |
| `obscured` | 目标中心点被 overlay/弹窗遮住 | 先关闭遮罩/弹窗，或点实际接收指针的元素 |
| `selector_not_found` | `read --selector` 没匹配到，且不是 `main`/`article`/`[role=main]` | 用 `snapshot` 的 ref 或换 selector；正文地标会自动回退 |
| `element_disabled` | 目标 disabled | 先启用控件或换目标 |
| `element_readonly` / `unsupported_control` | 控件不可写/类型不支持 | 用可编辑控件，或 `select` 对应控件 |
| `not_writable` / `not_select` | 目标不是可写控件/不是 select | 换目标或换动作 |
| `value_not_available` / `value_rejected` | select 无匹配 option / 控件拒绝写入 | 检查 value，或改用 `type` |
| `input_prevented` | 页面 cancel 了 `beforeinput`（自己接管输入） | 用 `js` 或 `press` 逐键输入 |
| `click_failed` | 点击触发时页面抛错 | 看 console，或换目标 |
| `action_failed` | 动作没有返回结果/执行失败 | 重新 `snapshot` 后重试 |
| `helpers_missing` | 页面 helper 未安装（注入丢失） | 重试 `snapshot`/`read`；持续失败则重载页面 |
| `no_form` | submit 目标不在 form 内 | 用 click 提交按钮，或 press Enter |
| `no_pending_action` | `await` 没有可等待的 act 上下文 | 先 `act`；或改用 `wait text/selector/time/idle` |
| `wait_timeout` | `wait` / `await` / `act waitFor` 页面条件超时 | 看最后 URL/文本，检查条件或提高 timeout |
| `response_timeout` | `act waitFor` 未等到响应/导航 | 看 `observation_gap`；在 `--wait-for` 里加 `timeout=5s`（≤30s） |
| `tool_timeout` | 扩展侧 RPC 超过硬上限（页面卡在对话框/长任务/注入不返回） | 关闭对话框、重载 tab，或缩小单次操作 |
| `screenshot_too_large` / `screenshot_decode_failed` / `screenshot_empty` / `screenshot_not_saved` | 截图编码/落盘失败 | 检查输出目录、窗口可见性，或改用 `--inline` |
| `har_not_saved` | HAR 落盘失败 | 检查 `--har` 路径与 `EMA2_OUTPUT_DIR` |
| `result_too_large` | 扩展返回超过 WS 消息上限 | 用 budget/cursor，或不要 `shot --inline` |
| `tab_not_active` | `shot` 目标不是活动 tab | 先 `tabs activate`；截图只截当前活动页 |
| `tab_changed` | 截图过程中目标 document 变化 | 页面稳定后重试 |
| `capture_failed` / `capture_empty` | `captureVisibleTab` 失败/空图 | 让窗口可见并保持前台 |
| `snapshot_failed` | 快照没有产出 | 重试，或用 `shot` 看渲染 |
| `upload_failed` | 文件注入失败 | 换 ref/selector，或确认目标是 `input[type=file]` |
| `js_exception` | 页面 JS 抛错（语法或运行时） | 看 `hint`，先 `read`/`snapshot` 再改代码 |
| `script_timeout` | `js` 超过 `--timeout`（默认 10s），已 `terminateExecution` | 简化表达式、加 `--timeout`，或改用 read/snapshot/wait |
| `debugger_attach_failed` | 无法挂 debugger（DevTools 或其他扩展占用该 tab） | `js` 会自动降级（看 `via=scripting`）；否则重载该 tab |
| `debugger_attach_timeout` / `debugger_detach_timeout` | attach/detach 在 10s/2s 内没有返回 | 关掉 DevTools/占用 debugger 的扩展，或重载 tab |
| `debugger_timeout` / `debugger_busy` | JS 执行或队列等待超期 | 终止长任务后重试 |
| `internal_page` | 内置页（edge:// 等）不可脚本化 | 换 http(s) 页面 |
| `empty_page` | `read` 的整页 md/text 一个字都没提取到 | 页面可能还在加载；`ema2 wait idle` 或 `wait selector` 后重试；canvas/PDF 类页面用 `shot --annotate` |
| `page_error` | 目标页面 HTTP 状态 ≥400（404/403/5xx 等） | 核对 url；拿不到正文就换页面 |
| `pdf_page` | 目标是 PDF 文档，read 无法抽文本 | 打开 html 版本；或让用户保存后用文件工具读 |
| `download_dir_unsupported` | Edge 不允许扩展改下载目录 | 文件会进默认下载目录 |

## 服务管理

### server

```bash
ema2 server [--port N]
```

后台启动中继，**daemon 化，立即返回**。中继在 7744 上同时监听 WS（扩展，看 `Upgrade` 头）和 HTTP（CLI 命令）。
扩展断线自动等重连，不需要重启 server。daemon 每 10s 发一次 ping；
连续 30s（3 个周期）收不到扩展任何消息即判定半开并摘除连接，`status` 随之回到
`extension=disconnected`，不会长期假在线。

- 7744 被占 → `port_in_use`，不自动换端口（避免和扩展 popup 配置错位）。
- 重复执行 `server` 时若已有实例在跑，输出 `relay=already_running pid=... port=...`（退出码 0）。
- 启动失败输出 `relay_start_failed`，并提示临时日志路径（`%TEMP%\ema2.log` 或 `%TEMP%\ema2-<port>.log`）。

### server stop

```bash
ema2 server stop [--port N]
```

干净关闭 daemon（`POST /shutdown`），server 卡死时用临时 PID 文件兜底。
`/shutdown` 会打断在途请求，并给 3s 硬期限强制退出。
命令会等到进程**真正退出**（PID 消失）后才输出 `relay=stopped`；端口已不监听但进程还活着时
报 `shutdown_failed` 并保留 PID 文件。
已停止时输出 `relay=not_running`，退出码 0（可重复执行）。

### status

```bash
ema2 status [--port N]
```

`GET /health`，输出：

```text
relay=running  pid=12345  port=7744  uptime=2h13m
extension=connected  version=<ext-version>  last_ping=2s
tabs=3  active_tab=42
```

扩展没连时输出 `extension=disconnected last_ping=never`、`tabs=unknown active_tab=unknown`，
并给一行 `hint=`：检查扩展 popup 端口（默认 7744）、点重连，仍失败就到 `edge://extensions` 重新加载。
`relay=not_running` 时退出码 2。

### init

```bash
ema2 init
```

在当前目录写/追加 `AGENTS.md` 片段：命令表 + 错误码提示 + "先 `ema2 server`" + 端口错配诊断。
片段由 `<!-- ema2:begin -->` / `<!-- ema2:end -->` 标记；重复执行会原地更新，不会产生多份。

---

## tabs

```bash
ema2 tabs                          # list
ema2 tabs list [--budget N] [--cursor N]
ema2 tabs current
ema2 tabs new <url> [--background]   # url 必须是 http(s)，与 open 一致
ema2 tabs close [<id>...]          # 或用 --session
ema2 tabs close --session
ema2 tabs activate <id>
```

- `list` → TSV：`id<TAB>*或-<TAB>title<TAB>url` + `total/shown/next`；
  `--budget`/`--cursor` 对标签行分页（默认 4000 字符），`next` 是下一条的行号。
  本会话由 `ema2 open` 打开的标签再追加一行 `session_tabs=<id,id>`（不计入 total）。
- `current` → `id/window/os/title/url`。
- `new` → `id/url/window`（登记进会话）；**默认激活并聚焦窗口**，`--background` 才在后台新建。
  这样 `tabs new` 后直接 `snapshot/act`（不带 `--tab`）不会误操作旧标签；`--active` 保留为兼容写法。
- `activate` → `active=<id> focused=1|0`：除了切 tab，还会 `chrome.windows.update{focused:true}`
  把窗口提到前台（截图/输入/密码管理器浮层都依赖前台）。
- `close`：
  - 位置参数 `id...`（≤8）→ `closed=<id,id>` + `count=<n>`，已不存在的 id 自动跳过。
  - `--session` → 关掉本会话由 `open`/`tabs new` 打开的标签，返回 `closed=..|none` + `count=`。
- 会话登记在 `chrome.storage.session`：service worker 被回收后仍在，浏览器重启即清空；
  用 `open --tab ID` / `--replace` 导航的用户标签**不**计入会话，`--session` 不会关掉它们。
  动作打开但无法关联 opener 的标签（例如 `noopener`）只用于定位结果，不自动进 session，避免误关用户手动开的标签。

## open

```bash
ema2 open <url> [--tab ID | --replace | --reuse session] [--wait load|dom|none] [--timeout 15]
```

等页面就绪、装网络 hook，返回 `id / url / title / status / reuse / net_hook`（有重定向时加 `redirects=`）。

- 加载期间有对话框时追加 `dialog_opened=<alert|confirm|prompt|print|beforeunload> handled=<dismissed|blocked|suppressed|passed> [msg=...]`（最多 3 条）；
  被拦截的 `window.open` 追加 `popup_blocked=1 url=...`。
- 输出包含 `focused=1|0`：`0` 表示浏览器窗口不在前台，会附 hint（先 `tabs activate`）。
- 默认新开标签，并登记进会话。
- `--replace`：在当前活动标签里导航（不新建），适合「同一个页面连续翻几页」。
- `--reuse session`：复用本会话**最近**打开的标签并在其中导航；目标与当前文档相同则
  不重载（保留滚动位置和表单状态）。标签已被关掉则退回新建。注意 `batch --reuse session`
  的规则不同：它按精确 URL 匹配会话标签，见下文。
- `--tab ID`：导航指定标签（显式意图，不计入会话）。
- `--timeout N`（单位：秒）：**整个 open 流程**的硬上限（1..60，默认 15）：打开标签 + 等文档接位 +
  装 hook + 读 timing/focus 都在这段预算内；慢站点/SPA 首屏可以调大。
- 三者互斥，同时给报 `invalid_argument`。
- `status=` 来自 `PerformanceNavigationTiming.responseStatus` 的真实主文档状态：
  `status=404` 这类错误页会额外给 `hint=http 404 ...`；`--wait none` 时是 `status=pending`。
- 返回前会确认**目标文档已经接位**（`location.href` + `readyState` 双条件），
  正常返回时 `url`/`title` 属于新页面；重定向到别的地址同样算到达。超时时明确输出
  `load=pending timeout=Ns timed_out=1`，并在截止前再做一次探测：如果页面刚好完成，
  就不会误报 pending。`status=unknown` 只在文档已就绪但拿不到 navigation timing 时出现。
- 新标签的 `about:blank` / `edge://newtab` 以及浏览器错误页不算到达；同 URL 的 `--replace` 会用
  `performance.timeOrigin` 确认 reload 后的新文档，不会被旧文档的 `complete` 提前骗过。
- 超时不会把旧的 hook/timing/focus 探测拖到后面：整个流程共用一个 deadline，
  中继预算也跟着 `waitTimeout` 走（默认 15s，最大 60s）。
- `open` 执行期间会临时注册 document_start 的 `page_guard.js`；页面加载期间触发的
  `alert/confirm/prompt/print` 被记录并自动关闭，不会让 open 卡死。`handled=dismissed/blocked/suppressed`
  只表示 EMA2 已关闭/拦截，**不代表用户确认了对话框**；命令结束后 guard 注销并恢复页面原生 API，
  不使用 EMA2 时不会注入或覆盖对话框。

## read

```bash
ema2 read [--tab ID] [--mode md|text|html|visible] [--selector X] [--no-main] [--budget N] [--cursor N]
```

- `--main` 默认开：按清洁后文本量/链接密度选正文容器，头部回 `main=<selector>`。
- 正文容器太短（<200 字符，或不到 body 的 5%）时回退 `main=body(fallback)`，并给
  `hint=main extraction too short (...); fell back to body`。
- `--selector` 优先于 `--main`，支持 shadow DOM / 同源 iframe 内的元素。
- `--selector main|article|[role=main]` 未命中时自动回退到默认 `main`/`body`，头部加
  `selector_fallback=1 selector=<原值>` 并给 hint；其他 selector 仍报 `selector_not_found`。
- `text`/`md` 会跳过 `script/style/noscript/template/textarea/input/select/option`，以及
  `display:none`/`visibility:hidden`/`content-visibility` 的隐藏元素；地标壳的跳过规则见下。
  `html` 仍是原始 HTML。
- `visible` 直接返回所选 root 的 `innerText`：不跳过导航/页脚/表单等页面壳，也不做
  Markdown/正文清洗；默认仍走 `main`，`--no-main` 读整页，`--selector` 读子树。
  它读的是**渲染文本**（包含滚动到视口外的已渲染内容），不是严格视口内文本；
  Edge 原生 PDF 查看器、canvas 和跨域 iframe 仍读不到。`text`/`md` 的内部遍历已合并，
  `visible` 和 `html` 是独立的提取路径。
- 正文遍历会进入 open shadow root（Web Component）；closed shadow root 仍不可见。

### 页面状态（login / challenge / pdf / 404 / 空页）

read 头部现在总是带 `http=`（主文档响应状态）与 `content_type=`（主文档 MIME）。

| 情形 | 输出 | 退出码 |
|---|---|---|
| 一切正常读出正文 | 正文 + `total/shown/next`，头部按需要带 `main=` | 0 |
| 登录墙（可见密码框 + 登录语义的 title/h1，且不是弹层） | `state=login_required` + `hint=`，不给伪装的正文 | **0** |
| 人机验证（"Just a moment…" / `#challenge-form` / 验证文案） | `state=challenge` + `hint=` | **0** |
| HTTP ≥ 400（404/403/5xx；md/text/visible） | `error=page_error`，错误信息带 `http=` 与 `content_type=` | 4 |
| PDF 文档（MIME `application/pdf` 或 `.pdf` 文档） | `error=pdf_page`（不做 PDF 文本提取） | 4 |
| 整页 md/text 一个字都没有（`--no-main` 也一样） | `error=empty_page` | 4 |

- `state=login_required` / `state=challenge` 是**页面状态，不是错误**：按 exit 0 处理，
  这样 AI 可以继续 `snapshot`/`act` 处理登录或验证；不要把这当成一次成功的正文读取。
- **本期完全不检测 paywall**：不读 JSON-LD、不加订阅标记、不输出 `paywall=`。
  NYT/WSJ/FT 这类页面仍会按普通页面返回公开可见的薄内容，AI 需要自己判断。
- `--selector` 是显式子树请求：不做 state/empty 判定（子树允许为空），但 PDF 报错保留。
- `--mode html` 是原始视图：只有 pdf 报错，其余状态/空页判定不适用。

### 清洁遍历规则（md / text）

- **可见性（read 专用）**：`display:contents` 没有自己的盒，但子树照常渲染——
  只要它有自有文本或可见盒后代，就继续遍历（被它包裹的正文不再整段丢失）；
  actionability（snapshot/click 的可见性判断）不变，`display:contents` 在 click 侧仍无 box。
- **地标壳**：`nav`/`form` 永久跳过；`header`/`footer`/`aside` 在
  `role=banner/contentinfo/complementary` 时跳过，其余只有在「不在 `article`/`main`/
  `[role=main]` 内**且**没有 h1–h6 后代」时才跳过——`article > header > h1` 保留，
  Guardian 首页这类把内容放进页面级 header 的站点也不会只剩一个标题。
  `section` 不算正文容器语境。
- **块模型**：`<br>` 按块边界换行（md/text 都是两行）；H1–H6 全部按标题渲染；
  `center`、`dt`/`dd`（各成一段）、`details`/`summary`（summary 先单独一行）、
  自定义元素（tag 含 `-`）都当作块展开；`table.rows` 只取本表行（嵌套表的行不进入外层表）。
- **链接**：标签只用 `innerText`（textContent 会把 `<a><script>…</script></a>` 的
  脚本泄漏进正文）；空文本链接默认不输出、**不产生裸 URL**（需要链接结构用 snapshot）；
  只有 fragment-only 且空文本的链接才降级为空；带图链接用图片 `alt` 当标签。
  非空文本的 fragment 链接保持 `[text](绝对URL)` 不降级。
- **噪声**：`BUTTON`、`[role=button]` 与 1×1 / clip(0) 的 sr-only 元素不进入正文；
  这些过滤只影响 read，snapshot/click 的可见性判断不受影响。

- 正文容器太短时回退 body；Markdown 按 `childNodes` 遍历，根节点直接文本、p/h1/pre、
  文本+块混排都不会漏字或重复。
- `md` 是**有损**渲染：标题/列表/表格/代码块保留，链接写成 `[text](绝对URL)`，
  有序列表保留 `<ol start>` / `<li value>` / `reversed` 的编号；
  行内代码与 `strong/em` 保留；样式、合并单元格、脚本生成内容会丢，不要拿它当可回写的源文件。
  链接文字里的 `[`/`]` 会转义，URL 里的括号/空格会 percent-encode；`<img>` 输出 `![alt](src)`，
  `text` 模式输出 alt 文本。
- `read --selector "#input"` 会返回控件当前值（input/textarea/select），不再总是 0 字符。
- 尾部 `truncated=1` 明确表示还有内容；`--cursor` 是**字符偏移**，换 `--mode` 续读会错位，
  续读时保持同一个 mode（头部会回 `mode=`）。
- 头部 `url/title[/main][state?] http/? content_type?` 和 `mode=md|text|html|visible`，
  正文，尾部 `total/shown/next`；`total` 是全文提取后的字符数，`shown` 是本次返回的字符数。
- 四种 mode 共用 `--selector` / `--no-main` / 默认 `main` 的选区，以及 budget/cursor；
  换 mode 续读会错位，保持同一个 mode。
- **被截断时尾部多一行** `hint=truncated: returned 4000 of 12345 chars; call read with cursor=4000 to continue, or --budget 12345 --cursor 0 for all 12345 chars`：
  首次读取会同时给出全文长度和一次读完的 budget（`total <= 200000` 时）；不要把这 4000 字当成全文。

## snapshot

```bash
ema2 snapshot [--tab ID] [--filter interactive|full|links|buttons|form] \
              [--near REF] [--budget N] [--cursor N] [--values]
```

返回可引用的交互元素：`[e12@d1] button "提交"`——`d1` 是**本文档的短句柄**，由扩展分配，
导航或刷新后换号，旧 ref 会直接 `stale_ref` 而不是点到别的元素上。

- 完整 ref（`e12@d1`）带文档校验；手敲时也可以写短 ref（`e12`），表示“当前文档的第 12 个元素”。
  跨导航后短 ref 可能指向新文档的同号元素，想严格防止误点请使用 snapshot 原样返回的完整 ref。

- 无名称且不可操作的节点（`presentation ""`、`banner ""` 这类）默认不输出；`--filter full` 保持无损，仍会列出。
- `href` 使用完整绝对 URL（scheme/host/query/fragment 都保留），不再只给 path；
  value/accept/files 等 extra 在生成时已限制长度，不会再被二次截断成半截值。
- `--values` 额外输出 text/textarea/select/file/contenteditable 的当前值；`--filter form` 默认等价于
  开启 values。默认 interactive 不输出，密码框始终是 `value=<hidden>`，用户输入也不会进入 name。
  `select multiple` 的 value 用逗号连接。
- `role=search/banner/main/...` 这类**地标容器**不会被当成控件；搜索框以真正的
  `<input type=search>` / textbox ref 出现。
- `--filter full` 额外带 `#1/#2` 标题；`--near REF` 限定在该元素最近的 `form/section/article/dialog/table/ul/main/nav` 内。
- `--filter links` 只列真正可跳转的 `<a href>`，输出 role 统一为 `link`（Bing 的
  `<a role=button href="#">` 不会混成 button 行）；`#` / `javascript:` 伪链接被排除。
- 头部带 `refs= roots= doc= visual=`；纯视觉页面会给 `hint` 建议用 `shot --annotate`。
- ref 只存在页面内存的 WeakRef 表里，不再写 `data-ema2-ref` 属性；页面 MutationObserver/CSS
  不会被 EMA2 的 snapshot 污染。
- 密码框即使有值，名称也只来自 label/ARIA/placeholder/alt/title 等元数据；textarea / contenteditable 的用户输入同样不会进入名称。

实测（10 个结构差异大的站点，2500 预算）：压缩后一次调用能看到的可操作元素
wikipedia 32→49、jd 28→47、taobao 27→45、bing 33→46、hn 36→47，x.com 从 2 次翻页降到 1 次。

## act

```bash
ema2 act <action> [ref] [text] [flags...]     # 单步
ema2 act --steps 'click e12; type e15 hi'     # 内联多步
ema2 act --script run.ema2 [--tab ID]          # 脚本文件，指定目标标签
```

动作：`click | type | fill | select | press | hover | scroll | submit | back | forward | reload`

定位（三选一）：裸写 `ref`（snapshot 给的）/ `--selector` / `--target "button:提交"`。
三者**互斥**，同时给出两个及以上直接报 `invalid_argument`（单步与脚本同一规则），
不再静默以 ref 优先。
`--target "role:name"` 先匹配可访问名称，再精确匹配 HTML `name` 属性；例如没有 label、
但 `<input type="search" name="q">` 可用 `--target "searchbox:q"`。
`--selector/--target/--key/--delta-y/--submit` 只属于单步调用；`--steps`/`--script` 里改用脚本写法
（`selector=` / `target=` / `press <key>` / `submit` 步骤；像素滚动用 `js`）。
单步动作也会拒绝与该动作不匹配的 flag（例如 `back --selector`、`type --key`）。
flag 支持 `--flag=value` 写法；同一个 flag 重复出现会报 `invalid_argument`，不会取最后一个。
以 `-` 开头的文本用 `--` 分隔：`ema2 act type e1 -- -5`。

| 动作 | 位置参数 | flag |
|---|---|---|
| click | `[ref]` | `--selector` / `--target` |
| type | `[ref] <text>` | `--selector` / `--target`；在当前 caret/selection 插入；`--submit` 提交所在 form |
| fill | `[ref] <text>` | 同上；覆盖整个值 |
| select | `[ref] <value>` | 同上 |
| press | `[ref] <key>` | 同上 |
| hover | `[ref]` | 同上 |
| scroll | `down\|up <n>` | `--delta-y N` |
| submit | `[ref]` | — |
| back/forward/reload | — | `--tab ID` |

- 查找、控件校验、执行和结果检查在同一次页面注入内完成；目标被动态移除不会假成功。
- `--tab ID` 对单步、`--steps` 和 `--script` 都有效；目标标签无需处于活动状态，
  命令不会切换当前活动标签。`await` 继续跟随该动作所属标签。
- **隐式 settle**：点击链接/提交表单/back/forward/reload 后最多等 3s，普通按钮最多 800ms，
  其他动作不等待；页面提前稳定就提前返回。SPA 链接只改 DOM 时约 1s 后按 `nav=none` 结算，
  不再拖满 3s。settle 超时**不算动作失败**，退出码仍为 0。
- settle 结果会带 `settle=done|pending`、`expect=nav|page|none`、`ms=`；
  pending 时还会给 `next_url=` 和 `hint=... ema2 await 10s ...`。
- 动作打开新标签时会返回 `new_tab=<id>`，并自动登记进会话；`tabs close --session` 会清掉它。
- disabled/readOnly/不支持写值的控件返回明确错误；`type` 只回 `wrote=1 chars=N`，不回显输入。
- `type` 优先走 CDP `Input.insertText`，按当前 caret/selection 插入（选中文本会被替换），
  结果带 `trusted=1`；`fill` 才覆盖整个值。debugger 不可用时回退为页面合成输入，标 `trusted=0`。
- `select` 的位置参数是 option 的 `value`（不是显示文本）；`<select multiple>` 用逗号分隔多个 value
  （`act select e5 "m1,m2"`），失败会恢复原来的选中项。
- `fill`/`select` 失败是原子的：控件清洗值或 option 不存在时恢复原值，不会把表单改坏。
- `click` 会补齐指针事件序列（`pointerover/pointerdown/mousedown` → 聚焦 → `pointerup/mouseup`），
  最后用原生 `el.click()` 触发一次激活；靠 `focusin`/`pointerdown` 出浮层的密码管理器能看到这次点击。
  事件是合成的（`isTrusted=false`），校验 `isTrusted` 的站点仍会忽略。
- 可导航 `<a>` 点击后**完全没有变化**才安全重试一次，输出 `retried=1`；
  DOM/标题已经变化（SPA handler）时不重试，直接 `nav=none`；按钮不重试，避免提交/点赞执行两次。
- 导航判定同时看 URL、document 句柄和 `performance.timeOrigin`：同 URL 的 `act reload`、
  表单提交到当前 URL 会回 `changed=reload`，不再误报 `nav=none`。
- 只有确实有导航意图且没有任何变化时才给 `nav=none`；`back/forward` 无历史、HTML5 校验失败、
  下载、对话框、被拦截的 `window.open` 都会单独标记（`validation=failed` / `download=started` /
  `dialog_opened=` / `popup_blocked=1`），不再一律提示“重试”。
- `type`/`fill` 写值前会发 `beforeinput`；页面 cancel 掉它时返回 `input_prevented`，
  说明该控件自己接管输入，改用 `js` 或逐键 `press`。
- `press` 优先走 CDP `Input.dispatchKeyEvent`（可信按键）：`Tab` 切焦点、`Backspace/Delete` 编辑、
  `Enter` 提交、`Control+A/C/X/V/Z` 选区/剪贴板/撤销都由浏览器默认行为执行；结果带 `trusted=1`
  和 `focus_changed=1|0`。debugger 被占用时回退为合成 KeyboardEvent（`trusted=0`），此时浏览器级默认行为不可用。
- `click` 会补齐指针事件序列（`pointerover/pointerdown/mousedown` → 聚焦 → `pointerup/mouseup`），
  最后用原生 `el.click()` 触发一次激活；靠 `focusin`/`pointerdown` 出浮层的密码管理器能看到这次点击。
  点击前做 visibility/inert/pointer-events/`elementFromPoint` 检查：隐藏元素报 `not_clickable`，
  被 overlay 遮住报 `obscured`，不再穿透遮罩点到下面的链接。
- 目标未渲染时自动重试 3 次（约 1s）；`stale_ref` 不重试。
- `scroll down|up <n>` 的屏数是整数，范围 **1..1000**；`--delta-y` 只接受有限像素值，
  `nan` / `inf` 在参数解析阶段就报 `invalid_argument`。
- 位置参数必须完整：`type e15 hello world` 会报 `invalid_argument`，不会静默只输入 `hello`；含空格请加引号。
- `waitFor` 有两种条件，动作 settle 后验证：
  - **响应/导航**：`url=` + 可选 `status=`；命中回 `response=<status> <method> <absolute-url> <ms>`，
    导航命中回 `nav=<url>`（只给 `url=` 时才算导航；带 `status=` 只匹配响应）。
    **只匹配本次动作之后发出的请求**：实况条目按动作前记录的 seq 水位过滤；
    `performance` 回填条目另加 `startedAt` 判定——早于动作起点或缺少时间的回填一律跳过，
    动作前已存在的历史资源不会被当成本次响应。
    动作前没有可用基线时（例如该 tab 从未装过 hook），从当前水位往后看，
    命中行可能带 `baseline_lost=1`，提示这次没有可信的起点。
    跨导航时自动从新文档起点匹配；确实拿不到状态的旧请求只给 `observation_gap=status_unavailable`。
  - **页面条件**：`text=` / `textGone=` / `selector=`，与 `ema2 wait` 语义一致；命中回
    `expect=ok condition=text:"评论已提交" ms=820`（`selector` / `textGone` 同样带引号）。
    超时返回 `error=wait_timeout`，消息以 `expect=timeout` 开头并带最后看到的 URL/文本。
  一次 `waitFor` 只写一个页面条件，页面条件不能和 `url=`/`status=` 混用；`timeout=5s`
  是可选兜底，**上限 30s**（daemon 与扩展同源校验，越界在动作执行前拒绝），不是 `waitFor.timeout` 字段。
  含空格的条件值必须在双引号内写：`--wait-for "text=\"保存 成功\" timeout=5s"`、
  `--wait-for "selector=\"#main a\" timeout=5s"`（脚本里同样写 `text="保存 成功"` /
  `selector="#main a"`）。
  单步示例：`ema2 act click e12 --wait-for "url=api/save status=200 timeout=5s"`、
  `ema2 act click e12 --wait-for "text=\"评论已提交\" timeout=5s"`。
  脚本里的 `waitFor` 是**独立步骤**，在它所在行执行；不再挪到整次调用末尾。
  `waitFor` 失败时已经执行过的 `stepN` 会保留在错误输出下面，避免 AI 重试整条脚本。

### act 脚本语法（`.ema2`）

```
# 每行一条；空行和 # 注释忽略（也允许行尾 # 注释）；含空格的值加双引号；key=value 是命名参数
# 文件可以是 UTF-8 带 BOM，首行 BOM 会被自动去掉
click e12
click selector="#submit"
click target="button:提交"
type e15 "hello world"
fill e15 "hello"
select e20 "opt1"
press Enter
press e15 Tab
hover e12
scroll down 3
submit
back
forward
reload
await 10s

# waitFor 是独立步骤，在所在行执行
click e12
waitFor url=api/save status=200 timeout=5s
waitFor text="保存成功" timeout=5s
waitFor selector="#comment-18" timeout=5s

# await 等上一个动作的导航/页面稳定
click e12
await 10s

# 多字段填表：连续 fill/select
fill e15 "user"
fill e16 "pass"
select e20 "zh-CN"
click e30
```

规则：
1. 首个 token 是 11 个动作之一，或脚本专用的 `await` / `waitFor`。
   **脚本里不能写 `open/read/snapshot/wait/js/tabs` 等 CLI 命令**；这些在 shell 里单独调用。
2. `ref` 裸写；`selector=` / `target=` 是命名参数；三者**只能三选一**，多写会报
   `invalid_argument`（与单步一致）。
3. 含空格的值用双引号包裹，引号内支持 `\"` 转义；`waitFor` 的值同样适用
   （`selector="#main a"`、`text="保存 成功"`）。
4. `waitFor` 是独立步骤，在所在行执行（通常放在触发动作之后）；接受
   `url=` / `status=`（响应/导航）或 `text=` / `textGone=` / `selector=`（页面条件），
   一次只写一个页面条件，且不能和 `url=`/`status=` 混用。`timeout=` 接受 `500ms` / `5s`
   （最大 30s）。单纯等待用 `await 1s`。
5. `await 10s` 等上一个动作的导航/页面稳定；如果上一步 `settle=pending` 且下一步不是 `await`，脚本会停止而不是在旧页面继续执行。
6. 脚本语法/示例/常见错误以 `ema2 act --help` 为准。

最多 20 步（1..20），可跨导航；`--steps`/`--script` 再加一个 `--wait-for` 时上限为 21 步。
返回每步结果 + `settle/expect/next_url/new_tab/changed/url/title/new/console_errors`。

## await

```bash
ema2 await [10s] [--tab ID]
```

等**上一个 act 动作**留下的页面效果完成。等待什么由动作决定，不需要 `await nav` / `await url` 之类的分级：

| 上一个动作 | 等什么 |
|---|---|
| 点击链接、form submit、back/forward/reload | URL / document epoch 变化，新文档 readyState 完成 |
| 普通 button click | 页面短暂稳定（最多 800ms，通常提前返回） |
| type/fill/select/hover/scroll | 没有待完成效果，立即返回 `await=done elapsed=0.0s` |
| 点击打开新标签 | 等新标签加载，返回 `tab=` / `new_tab=` |

- 缺省等待 3s，`ema2 await 10s` 显式 10s；范围 0–300s。
- 不带 `--tab` 时使用**上一个动作所在的标签页**，不是当前活动标签页。
- 动作已经 settle 完成时立即返回；`settle=pending` 时继续等。
- 显式 `await` 超时返回 `error=wait_timeout`，退出码 4；可以再 `await 10s` 或 `read --tab` 检查。
- 没有最近动作上下文时返回 `no_pending_action`；先 `act`。

示例：

```text
ema2 act click e12
# settle=pending expect=nav next_url=https://...
ema2 await 10s
# await=done elapsed=2.3s tab=42 url=https://...
```

## wait

```bash
ema2 wait text "<s>" [--timeout 30s] [--tab ID]
ema2 wait textGone "<s>" [...]
ema2 wait selector "<css>" [...]
ema2 wait time 5s
ema2 wait idle [--timeout 30s]
```

这是 shell 命令，**不能写进 `.ema2` 脚本**；脚本里用 `await` / `waitFor`。
`text` / `textGone` / `selector` 也是 `act --wait-for` 的页面条件语法：

```bash
ema2 act click e12 --wait-for "text=\"评论已提交\" timeout=5s"
ema2 act click e12 --wait-for "selector=\"#comment-18\" timeout=5s"
```

返回 `ok=<条件> elapsed=...s`；`time`/`timeout` 都在 0–300s。
`act --wait-for` 的页面条件成功时返回 `expect=ok condition=... ms=...`，超时为 `expect=timeout`。

## js

```bash
ema2 js '<code>' [--tab ID] [--budget N] [--cursor N] [--timeout 10]  # 秒
ema2 js --file script.js [...]
```

在页面主世界执行（debugger，`awaitPromise`）。字符串原样返回，其余值紧凑 JSON。

- **库 + 调用的两种投递方式**（`--file` 与内联 code 互斥）：

  ```bash
  # 两次调用：注入会粘住，第二次直接调用
  ema2 js --file lib.js
  ema2 js "return __E.digest()"

  # 一次投递：整段作为一个 code 传入，少一次往返
  ema2 js "$(cat lib.js)
  return __E.digest()"
  ```

- **注入即粘住**：注入的全局活到该标签导航/关闭为止；重装同名全局不会自动替换旧实现，
  改库后必须在表达式里显式覆盖赋值。改完 `lib.js` 却「没反应」，先怀疑页面里还留着旧实现。
- **既可传表达式也可传语句体**：先用 `Runtime.compileScript` 做 compile-only 探测；
  表达式能编译就只执行一次，否则按 async 函数体执行一次。因此顶层 `return`、`await`、
  `const a = 1; return a` 都能用；运行时抛出的 `SyntaxError` 不会重跑。
  函数体没写 `return` 时返回 `undefined` 并提示 `note=ran as an async function body`。
- 错误 `hint` 区分两类：`syntax error: ...` 与 `runtime error: ...`。除语法错误外的异常不会重跑，避免副作用执行两次。
- `debugger.attach` 被拒时先按 page target 的 `targetId` 重挂；仍失败则退到 MAIN world 注入，
  输出末尾多一行 `via=scripting`。降级路径的 `eval` 受页面 CSP 限制，严格 CSP 站点会返回
  `js_exception`；看到 `via=scripting` 说明本次没走 debugger。
- 全局 debugger 队列串行执行；默认执行上限 **10s**（`--timeout` 可改，0.1..300s），
  超时返回 `script_timeout` 并 `Runtime.terminateExecution` + detach，后续调用可继续。
  注意 `terminateExecution` 终止的是该 tab 里**所有** JS 执行（不只是本次表达式），
  页面自身的定时器/长任务也会被打断；能用 `wait` 或更短的表达式时不要依赖超时兜底。
- 返回 DOM 节点时给节点描述/outerHTML 前缀，不再变成 `{}`；语法错误保留完整的
  `SyntaxError: ...` 文本（不再只有 `msg=Uncaught`）。
- `--budget` 截断时回 `truncated=1`；字符串按字符切片，对象/数组会返回合法 JSON 分片信封
  `{"ema2_truncated":true,"total":N,"next":M,"chunk":"..."}`，用 `--cursor <next>` 续读。
- `--cursor` 是**输出字符偏移**：续读时源输出必须不变，换 code 或换 `--budget` 都会错位。
- debugger 不可用时的 `via=scripting` 降级路径**无法被 --timeout 中断**；无限循环只能重载 tab。

## observe

```bash
ema2 observe console [--tab ID] [--since SEQ] [--level log|warn|error] [--filter SUBSTR] [--wait 10s]
ema2 observe network [--tab ID] [--since SEQ] [--filter SUBSTR] [--body-filter RE] [--bodies=false] [--har PATH] [--headers redact|all]
```

- console：增量日志；首次安装 hook（`open`/`act` 也会自动装）。`--wait` 会先安装 hook 再等待，等待期间的新日志也会被记录；
  `--har` 只适用于 network（console 传 `--har` 会报 `invalid_argument`）。
- `--level` 是最低级别阈值，默认 `error`；`warn` 包含 warn/error，`log` 包含所有已采集级别。
  输出尾部明确带 `level=<阈值>`，避免把“没有输出”误认为“没有产生日志”。
- network：hook `fetch`/`XMLHttpRequest`，真实 `status` + **受限**响应预览；命令期间只对目标 tab 注册
  `chrome.webRequest`（`RequestFilter.tabId`），补齐页面 hook 安装前的首屏文档/资源
  status、headers、MIME，命令结束立即注销，不常驻观察其它标签。记录按 `startedAt` 排序匹配。
  - 列表行输出**完整绝对 URL**（跨域请求不再只剩 path）；`-1` 只用于确实拿不到状态的条目。
  - 预览在后台读取 `response.clone()`，最多 8KB/1.5s，不阻塞页面；事件流/二进制跳过。
  - `--bodies=false` 真正关闭新响应的 body clone/读取。
  - `--filter`：URL 子串；`--body-filter`：响应体正则。
  - `--wait`：等异步请求（≤10s）；`--since` 用上次 `next` 的数字。
  - 分页 `next` 只推进到已交付记录；环形缓冲溢出回 `dropped=`，导航回绕回 `reset=1`。
  - 响应体仍在读取（`preview=pending`）的条目不会被丢弃：记录行照常输出并带 `streaming=1`，
    尾部回 `pending=<n> seqs=<seq,...>`；需要补读时用 `--since <first seq - 1>`。
  - `--body-filter` 只匹配已就绪的 body；命中的 pending 条目记入 `seqs=` 而不参与匹配。
  - `--body-filter` 限 512 字符，拒绝量词嵌套（`(a+)+`）与量词组内分支（`(a|a)+`）这类
    会指数回溯的形态，返回 `invalid_regex`。
  - `--har PATH`：导出 HAR 1.2（采样/预览，不是完整抓包），`request.url` 是绝对 URL，
    `request/response.headers`、`queryString`、真实 `startedDateTime`、`content.mimeType/size` 来自
    webRequest + 页面 hook；JSON 不经过文本预算截断，落盘并回 `path=` + `entries=N`。
    `--har` 指向目录（或以 `/`、`\` 结尾）时自动拼 `har-<timestamp>.har`。
  - HAR 里的 `authorization`、`proxy-authorization`、`cookie`、`set-cookie` 默认替换为
    `<redacted>`，且在 webRequest 记录进入内存前就脱敏。需要原始值时显式加
    `--headers all`（只影响本次 network 观测的 HAR 导出）。

## files

```bash
ema2 files upload <path...> [--ref R | --selector S | --target T] [--tab ID]
ema2 files download-dir
```

- `upload` 走**页面侧 DataTransfer**：CLI 读文件字节随请求下发，扩展在页面里构造 `File`
  再赋给 `input.files`，因此不依赖 debugger（别的扩展占住 tab 时也能用）。
  路径是位置参数，所以 ref 必须用 `--ref`；`act` 的 ref 是位置参数。
  单次 ≤20 个文件、原始载荷 ≤5 MiB；Windows 路径必须是 `C:/...` 绝对路径。
- `download-dir`：只为返回明确错误——Edge 不允许扩展改下载目录，
  固定返回 `download_dir_unsupported`；请在 `edge://settings/downloads` 手动设置。

## shot

```bash
ema2 shot [--tab ID] [--inline] [--annotate] [--format png|jpeg] [--out PATH]
```

- `--tab` 会把目标标签激活并把窗口提到前台（成功时回 `activated=1`）；
  激活失败（窗口最小化等）才回 `tab_not_active`，不会返回前台页图片。
- 默认：落盘 PNG（`--out` 指定路径，缺省进默认输出目录），回 `path=` + `bytes=`。
- `--out` 指向目录（已有目录，或以 `/` 结尾）时自动拼默认文件名 `shot-<timestamp>.png`；
  截图后缀只接受 `png` / `jpg` / `jpeg`，其他后缀报 `invalid_argument`（不会写出半截文件）。
- `--out` 的相对路径按 CLI 当前目录转为绝对路径，并清理 `.` / `..`；不解析符号链接。
- `--inline`：不落盘，CLI 输出 `data:image/png;base64,...`（或 jpeg）。
- `--annotate`：先快照，再在图上画 ref 编号，回坐标列表 + 标注图
  （`[e3@d3] button "提交" @439,275 43x26`）；截图前后复核 tab/document。
  只标注当前 document epoch、视口内的元素；同一元素只保留一个 ref，坐标裁剪为非负。
  同源 iframe 内的 ref 坐标会加上 iframe 偏移，不再画到页面左上角。

## batch

```bash
ema2 batch <tool> [tool flags...] --urls "u1,u2"
ema2 batch <tool> [tool flags...] --urls "u1,u2" --reuse session
ema2 batch <tool> [tool flags...] --tab-ids "1,2" [--concurrency 2]
```

多标签并发（默认 2，最大 4），块之间 `---`；单块失败不影响其它块。

- `--tab-ids`：对已有标签并发执行目标工具，每块头部 `tab=<id>`。
- `--urls`：先为每个 URL 打开新标签，再在该标签上执行目标工具，每块头部 `url=<url>` + `tab=<id>`；
  `tool=open` 时只打开（并行开多页）。**`open` + `read` 因此一次命令完成**，不再串行两次。
  默认始终新开标签；内层 `--replace`/`--reuse`/`tabId` 会被拒绝。
  `--urls` 可重复传入（`--urls u1 --urls u2`），逗号仍是分隔符；URL 本身带逗号时只能分多次传。
- `--reuse session`：仅用于 `--urls`。按规范化后的完整 URL 只匹配本会话由
  `open`/`tabs new` 打开的标签；命中时复用文档（同一 URL 不重载）并回
  `reused=1 tab=<id>`，未命中仍新开。
  不会复用任意用户标签，也不会改变默认的新开语义。
- `--tab-ids` 与 `--urls` 互斥；`tool=open` 只能配 `--urls`。
- 工具 flag 会与每个目标合并后复用子命令 validator：`ema2 batch wait --time 3600 --tab-ids 1,2`
  会在执行前拒绝；urls 模式下 `--wait`/`--timeout` 可以传给 open，内层 `--replace`/`--reuse` 会被拒绝。
- 内层参数：`read --mode/--selector/--no-main/--budget/--cursor`，
  `snapshot --filter/--near/--budget/--cursor/--values`，
  `js --code/--file/--budget/--cursor/--timeout`，`wait --text/--text-gone/--selector/--time/--timeout/--idle`。
  `js` 的位置参数可直接写 CODE，`wait` 的位置参数可直接写 TEXT；完整列表见 `ema2 batch --help`。
- `batch js --file` 与 `js --file` 对齐：文件在客户端只读一次，再分发给每个标签；
  `--file` 与 `--code`/位置参数互斥，空文件在执行前报 `invalid_argument`。
- 预算按 `ceil(目标数/并发) × 单次最长耗时 + open 成本 + 20s`；js 因全局 debugger 串行按有效并发 1 计算。
  `tool=open` 的单次耗时跟随内层 `waitTimeout`（默认 15s），不会再被固定 30s 提前掐断。
- 估算超过 600s 时在执行前报 `invalid_argument`，请减少目标、提高并发或拆分 batch。

## 典型调用循环

```text
# 起步（每个 AI 会话第一条）
ema2 server
ema2 status

# 读取 / 抓数据（单次命令就能拿到主内容）
ema2 batch read --urls "https://a.com,https://b.com"
# 或者串行：open → wait selector → read → 需要时把输出里的 next 数字传给 --cursor（如 --cursor 4000）

# 同一页面连续翻页：不要每次开新标签
ema2 open url1 --replace → ema2 read → ema2 open url2 --replace → ema2 read

# 操作
ema2 open url → ema2 snapshot → ema2 act click e12 → 看 changed/new/console_errors
             → 页面变了就重新 snapshot（ref 会失效）

# 等接口
ema2 act click e12 --wait-for "url=api/save status=200" → 直接拿 response 行

# 调试
ema2 observe console --since 0
ema2 observe network --since 0 --body-filter "error"
# 续读时把上一次输出里的 next 数字传给 --since（如 --since 12）

# 多标签
ema2 open urlA → ema2 open urlB → ema2 batch read --tab-ids "a,b"
# 收尾：关掉本次会话开的标签
ema2 tabs close --session

# 关服务
ema2 server stop
```

## 限制

- 网络观测的 body 只是 ≤800 字符预览，HAR 也只是采样（webRequest 只补 status/headers/MIME，
  正文仍来自页面 hook）；不是完整抓包代理。极早期或扩展自身发起的请求仍可能没有 status。
- 跨域 iframe 不可遍历；内置页不可脚本化。
- `press` 的浏览器默认行为（Tab/Backspace/Enter/快捷键）需要 debugger 可用；
  `via=scripting`/`trusted=0` 的降级路径只有页面合成事件，没有浏览器级默认行为。
- 截图会 `shot --tab` 自动激活目标标签；窗口最小化/不可见仍会失败。
- 只有 EMA2 页面命令执行期间，`page_guard.js` 才在 MAIN world 把
  `alert/confirm/prompt/print` 变成非阻塞记录：
  `confirm` 返回 false、`prompt` 返回 null、`print` 不打开预览；赋值型 `window.onbeforeunload`
  也会被吞掉（记录为 `dialog_opened=beforeunload handled=suppressed`），避免离开页面时被原生确认框卡住。
  动态 content script 无法按 tabId 限定，所以租约期间只 match `http(s)` 页面并在
  最后一个租约结束时注销；页面拥有同一个 JS realm，guard 只是防卡死优化，不是安全边界。
  需要用户确认的流程请先检查 `dialog_opened=`，不要假设页面拿到了“确认”。
  命令结束或失败后 guard 恢复原生 API；未调用 EMA2 时页面弹窗不会被扩展拦截。
- 视觉兜底只有编号 + 坐标，没有 OCR。
- `js` 超时的 `Runtime.terminateExecution` 会终止目标 tab 的全部 JS 执行。
- `open status=` 依赖 `PerformanceNavigationTiming.responseStatus`（Chromium 109+）；
  页面完全由 Service Worker 缓存返回、或 `--wait none` 时可能是 `status=unknown|pending`。
- `js` 的语句体回退只在**语法错误**时触发；如果代码本身在运行时抛 `SyntaxError`（如 `JSON.parse('{')`），
  会按原样报错，不会重复执行副作用。
