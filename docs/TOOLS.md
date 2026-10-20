# EMA2 工具参考（v1.12.1）

所有工具都通过 MCP `tools/call` 调用。通用约定：

- 可选 `tabId`：缺省操作当前活动标签页；`open` 会返回目标标签页的 `id`
  （默认新开；`replace`/`reuse` 会复用已有标签，见下文）
- `budget`：返回文本的字符预算（默认 4000，范围 200–200000）
- `cursor`：从上次返回的 `next` 继续读
- 输出是**行式紧凑文本**，不是 JSON
- 工具错误是 `isError: true` 的文本结果，格式 `error=<code> msg=... hint=...`

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

- `next=none` 表示没有更多；否则把 `next` 原样传给 `cursor`/`since`
- `ref` 内嵌 document epoch（`e<seq>@<doc>`）：同一 document 内稳定，导航/reload/跨 tab 后旧 ref 会 `stale_ref`
- 文件类结果：`path=<绝对路径>` 单独一行，后跟 `bytes=` 等元信息
- 图片默认落盘；`inline=true` 才回图片内容
- `type`/`fill` 只回写入状态和字符数，不回显输入值；密码框的 value 不参与 snapshot 名称

## 错误码

| code | 含义 | 建议 |
|---|---|---|
| `invalid_argument` | 参数不合法（含 batch 内层参数） | 看 `tools/list` schema |
| `extension_error` | 中继连不上扩展 | `ema2.exe status`，确认扩展已连接 |
| `stale_ref` | 页面已跳转/刷新/换 tab，ref 失效 | 重新 `snapshot` |
| `ref_not_found` | 同页但 ref 不存在 | 重新 `snapshot` |
| `target_not_found` | ref/selector/target 都找不到 | 用 `snapshot` 的 ref，或 `target="role:name"` |
| `element_disabled` | 目标 disabled | 先启用控件或换目标 |
| `element_readonly` / `unsupported_control` | 控件不可写/类型不支持 | 用可编辑控件，或 `select` 对应控件 |
| `value_not_available` | select 没有匹配 option | 检查 option value |
| `no_form` | submit 目标不在 form 内 | 用 click 提交按钮，或 press Enter |
| `tab_not_active` | `shot` 目标不是活动 tab | 先 `tabs activate`；截图只截当前活动页 |
| `tab_changed` | 截图过程中目标 document 变化 | 页面稳定后重试 |
| `response_timeout` | `act waitFor` 未等到响应 | 看 `observation_gap`，或调大 timeout |
| `wait_timeout` | `wait` 条件超时 | 检查条件或提高 timeout |
| `invalid_regex` | `bodyFilter` 不是合法正则 | 修正正则 |
| `debugger_attach_failed` | 该 tab 被 DevTools 占用 | 关掉 DevTools 重试 |
| `debugger_timeout` / `debugger_busy` | JS 执行或队列等待超期 | 终止长任务后重试 |
| `internal_page` | 内置页（edge:// 等）不可脚本化 | 换 http(s) 页面 |
| `download_dir_unsupported` | Edge 不允许扩展改下载目录 | 文件会进默认下载目录 |

## 工具

### tabs
```text
tabs {action: list|current|new|close|activate, tabId?, tabIds?: [..], scope?: session, url?, active?}
```
- `list` → TSV：`id<TAB>*或-<TAB>title<TAB>url` + `total/shown/next`；
  本会话由 EMA2 打开的标签再追加一行 `session_tabs=<id,id>`
- `current` → `id/window/os/title/url`
- `new` → `id/url/window`（登记进会话）；`activate` → `active=<id>`
- `close` 三种写法：
  - `tabId` → `closed=<id>`
  - `tabIds: [..]`（≤8）→ `closed=<id,id>` + `count=<n>`，已不存在的 id 自动跳过
  - `scope: session` → 关掉本会话由 `open`/`tabs new` 打开的标签，返回 `closed=..|none` + `count=`
- 会话登记在 `chrome.storage.session`：service worker 被回收后仍在，浏览器重启即清空；
  用 `open tabId=`/`replace=true` 导航的用户标签**不**计入会话，`scope=session` 不会关掉它们

### open
```text
open {url, tabId? | replace?: true | reuse?: session, wait: load|dom|none}
```
等页面就绪、装网络 hook，返回 `id / url / title / status / reuse / net_hook`（有重定向时加 `redirects=`）。
- 默认新开标签，并登记进会话
- `replace: true`：在当前活动标签里导航（不新建），适合「同一个页面连续翻几页」
- `reuse: "session"`：复用本会话**最近**打开的标签；标签已被关掉则退回新建
- `tabId`：导航指定标签（显式意图，不计入会话）
- 三者互斥，同时给会返回 `invalid_argument`
- `status=` 来自 `PerformanceNavigationTiming.responseStatus` 的真实主文档状态：
  `status=404` 这类错误页会额外给 `hint=http 404 ...`；`wait=none` 时是 `status=pending`
- 返回前会确认**目标文档已经接位**（`location.href` + `readyState` 双条件），
  所以 `url`/`title` 一定属于新页面；重定向到别的地址同样算到达

### read
```text
read {tabId?, mode: md|text|html, selector?, main?, budget?, cursor?}
```
- `main` 默认 `true`：按文本量/链接密度选正文容器，头部回 `main=<selector>`
- `selector` 优先于 `main`，支持 shadow DOM / 同源 iframe 内的元素
- Markdown 按 `childNodes` 遍历，根节点直接文本、p/h1/pre、文本+块混排都不会漏字或重复
- 头部 `url/title[/main]`，正文，尾部 `total/shown/next`
- **被截断时尾部多一行** `hint=truncated: returned 4000 of 12345 chars; call read with cursor=4000 to continue`：
  不要把这 4000 字当成全文，按提示传 `cursor` 续读（长页面的 FAQ/页脚常在后半段）

### snapshot
```text
snapshot {tabId?, filter: interactive|full|links|buttons|form, near?, budget?, cursor?}
```
返回可引用的交互元素：`[e12@d1] button "提交"`——`d1` 是**本文档的短句柄**，由扩展分配，
导航或刷新后换号，旧 ref 会直接 `stale_ref` 而不是点到别的元素上。
- 无名称且不可操作的节点（`presentation ""`、`banner ""` 这类）默认不输出；`filter=full` 保持无损，仍会列出
- 额外属性里的 URL 截到 40 字符（`…` 结尾）；需要完整地址用 `filter=links`（不截断）或 `read`/`js`
- `filter=full` 额外带 `#1/#2` 标题；`near=<ref>` 限定在该元素最近的 `form/section/article/dialog/table/ul/main/nav` 内
- 头部带 `refs= roots= doc= visual=`；纯视觉页面会给 `hint` 建议用 `shot annotate`
- 密码框即使有值，名称也只来自 label/ARIA/placeholder/alt/title 等元数据，绝不使用 value

实测（10 个结构差异大的站点，2500 预算）：压缩后一次调用能看到的可操作元素
wikipedia 32→49、jd 28→47、taobao 27→45、bing 33→46、hn 36→47，x.com 从 2 次翻页降到 1 次。

### act
```text
act {
  tabId?, action, ref?/selector?/target?, text?, value?, key?, deltaY?, submit?,
  fields?: [{ref?/selector?, value}], steps?: [ ... ], waitFor?: {url?, status?, timeout?}
}
```
- 动作：`click|type|fill|select|press|hover|scroll|submit|back|forward|reload`
- 定位：`ref`（snapshot 给的）优先，其次 `selector`，再 `target="button:提交"`
- 查找、控件校验、执行和结果检查在同一次页面注入内完成；目标被动态移除不会假成功
- disabled/readOnly/不支持写值的控件返回明确错误；`type` 只回 `wrote=1 chars=N`，不回显输入
- `press` 有定位时先聚焦目标再发键；无定位时使用当前焦点。合成 KeyboardEvent 不能承诺浏览器原生 Tab/组合键行为
- 目标未渲染时自动重试 3 次（约 1s）；`stale_ref` 不重试
- `steps[]`：最多 20 步，可跨导航；返回每步结果 + `changed/url/title/new/console_errors`
- `waitFor`：动作后等匹配响应，命中回 `response=<status> <method> <path> <ms>`；
  跨导航时自动从新文档起点匹配；hook 安装前的历史请求只给 `observation_gap=status_unavailable`

### wait
```text
wait {tabId?, text|textGone|selector|time|idle（五选一）, timeout?}
```
返回 `ok=<条件> elapsed=...s`；`time`/`timeout` 都在 0–300s，文本/选择器 wait 使用 `timeout`（默认 30s）。

### js
```text
js {code, tabId?, budget?}
```
在页面主世界执行（debugger，`awaitPromise`）。字符串原样返回，其余值紧凑 JSON。
- **既可传表达式也可传语句体**：先按表达式执行；若报语法错误，再按 async 函数体执行一次，
  因此顶层 `return`、`await`、`const a = 1; return a` 都能用（与常见 codemode 沙箱一致）。
  函数体没写 `return` 时返回 `undefined` 并提示 `note=ran as an async function body`
- 错误 `hint` 区分两类：`syntax error: ...`（表达式/语句体写法问题）与 `runtime error: ...`（代码本身抛错）
- 除语法错误外的异常不会重跑，避免副作用执行两次
全局 debugger 队列串行执行；单任务 60s 执行上限，超时会 `Runtime.terminateExecution` 并 detach，
后续调用可继续。注意 `terminateExecution` 终止的是该 tab 里**所有** JS 执行（不只是本次表达式），
页面自身的定时器/长任务也会被打断；能用 `wait` 或更短的表达式时不要依赖超时兜底。

### observe
```text
observe {kind: console|network, tabId?, since?, level?, filter?, bodyFilter?, wait?, bodies?, har?, budget?}
```
- console：增量日志；`level=log|warn|error`；首次安装 hook（`open/act` 也会自动装）
- network：hook `fetch`/`XMLHttpRequest`，真实 `status` + **受限**响应预览
  - 预览在后台读取 `response.clone()`，最多 8KB/1.5s，不阻塞页面；事件流/二进制跳过
  - `bodies=false` 真正关闭新响应的 body clone/读取
  - `filter`：URL 子串；`bodyFilter`：响应体正则
  - `wait`：等异步请求（≤10s）；`since` 用上次 `next`
  - 分页 `next` 只推进到已交付记录；环形缓冲溢出回 `dropped=`，导航回绕回 `reset=1`
  - 响应体仍在读取（`preview=pending`）的条目不会被丢弃：记录行照常输出并带 `streaming=1`，
    尾部回 `pending=<n> seqs=<seq,...>`；需要补读时用 `since=<first seq - 1>`
  - `bodyFilter` 只匹配已就绪的 body；命中的 pending 条目记入 `seqs=` 而不参与匹配
  - `bodyFilter` 限 512 字符，拒绝量词嵌套（`(a+)+`）与量词组内分支（`(a|a)+`）这类
    会指数回溯的形态，返回 `invalid_regex`
  - `har=true`：导出 HAR 1.2（采样/预览，不是完整抓包），中继落盘并回 `path=` + `entries=N`

### files
```text
files {action: upload|download-dir, tabId?, ref?/selector?/target?, paths?, dir?}
```
- `upload`：CDP `DOM.setFileInputFiles`；Windows 必须 `C:/...` 绝对路径（≤20 个）
- `download-dir`：schema 保留该动作只为返回明确错误——Edge 不允许扩展改下载目录，
  固定返回 `download_dir_unsupported`；请在 `edge://settings/downloads` 手动设置

### shot
```text
shot {tabId?, inline?, annotate?, format?}
```
- 目标必须是所在窗口的活动 tab；否则 `tab_not_active`，不会返回前台页图片
- 默认：落盘 PNG，回 `path=` + `bytes=`
- `inline=true`：回图片内容
- `annotate=true`：先快照，再在图上画 ref 编号，回坐标列表 + 标注图
  （`[e3@d3] button "提交" @439,275 43x26`）；截图前后复核 tab/document
- `format=png|jpeg`

### batch
```text
batch {tool: open|read|snapshot|js|wait, tabIds?: [..] | urls?: [..], args?, concurrency?}
```
多标签并发（默认 2，最大 4），块之间 `---`；单块失败不影响其它块。
- `tabIds`：对已有标签并发执行目标工具，每块头部 `tab=<id>`
- `urls`：先为每个 URL 打开新标签，再在该标签上执行目标工具，每块头部 `url=<url>` + `tab=<id>`；
  `tool=open` 时只打开（并行开多页）。**`open` + `read` 因此一次调用完成**，不再串行两次
- `tabIds` 与 `urls` 互斥；`tool=open` 只能配 `urls`
- `args` 会与每个目标合并后复用子工具 validator：`batch(wait,args:{time:3600})` 会在执行前拒绝；
  `urls` 模式下的 `wait/replace/reuse` 属于 open，不会被当成目标工具参数
- 预算按 `ceil(目标数/并发) × 单次最长耗时 + open 成本 + 15s`；js 因全局 debugger 串行按有效并发 1 计算
- 估算超过 600s 时在调用前报 `invalid_argument`，请减少目标、提高并发或拆分 batch

## 典型调用循环

```text
# 读取 / 抓数据（单次调用就能拿到主内容）
batch(tool=read, urls=[...])  → 每块：url/tab + 正文
# 或者串行：open(url) → wait(selector) → read(main=true) → 需要时 read(cursor=next)

# 同一页面连续翻页：不要每次开新标签
open(url1, replace=true) → read() → open(url2, replace=true) → read()

# 操作
open(url) → snapshot() → act(ref) → 看 changed/new/console_errors
          → 页面变了就重新 snapshot（ref 会失效）

# 等接口
act(click, waitFor={url:"api/save", status:200}) → 直接拿 response 行

# 调试
observe(console, since=next) / observe(network, since=next, bodyFilter="error")

# 多标签
open(urlA) → open(urlB) → batch(read, tabIds=[a,b])
# 收尾：关掉本次会话开的标签，别在用户桌面上留 tab
tabs(close, scope=session)
```

## HTTP 调用示例

中继默认要求 token，每个请求都要带 `-H "Authorization: Bearer $EDGE_MCP_TOKEN"`；
只有以 `--insecure-allow-no-token` 启动时才能省略。

```bash
# 1) 握手（响应头里拿 Mcp-Session-Id）
curl -i -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  http://127.0.0.1:8765/mcp

# 2) 打开页面
curl -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open","arguments":{"url":"https://example.com"}}}' \
  http://127.0.0.1:8765/mcp

# 3) 快照 → 按 ref 点击
curl -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"snapshot","arguments":{}}}' \
  http://127.0.0.1:8765/mcp
curl -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"act","arguments":{"action":"click","ref":"e1@d3"}}}' \
  http://127.0.0.1:8765/mcp
```

## 限制

- `js`（debugger 世界）直接发起的 fetch 可能只以 `status=-1` 的资源条目出现；这类条目不会被 `waitFor` 当作成功响应
- 网络观测的 body 只是 ≤800 字符预览，HAR 也只是采样；不是完整抓包代理
- 跨域 iframe 不可遍历；内置页不可脚本化
- 合成 `press` 不提供浏览器原生 Tab/组合键默认行为
- 截图需要目标 tab 是活动 tab 且窗口可见（最小化会失败）
- 视觉兜底只有编号 + 坐标，没有 OCR
- `js` 超时的 `Runtime.terminateExecution` 会终止目标 tab 的全部 JS 执行
- `open status=` 依赖 `PerformanceNavigationTiming.responseStatus`（Chromium 109+）；
  页面完全由 Service Worker 缓存返回、或 `wait=none` 时可能是 `status=unknown|pending`
- `js` 的语句体回退只在**语法错误**时触发；如果代码本身在运行时抛 `SyntaxError`（如 `JSON.parse('{')`），
  会按原样报错，不会重复执行副作用
