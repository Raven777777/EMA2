# EMA2 工具参考（v1.8.0）

所有工具都通过 MCP `tools/call` 调用。通用约定：

- 可选 `tabId`：缺省操作当前活动标签页；`open` 会返回新标签页的 `id`
- `budget`：返回文本的字符预算（默认 4000，范围 200–200000）
- `cursor`：从上次返回的 `next` 继续读
- 输出是**行式紧凑文本**，不是 JSON
- 工具错误是 `isError: true` 的文本结果，格式 `error=<code> msg=... hint=...`

## 输出协议

```text
# 头部：key=value（可能多行）
url=https://example.com/  title=Example Domain  scroll=0/1  refs=3  doc=t7-k3a9
# 数据行：[ref] role "name" extras，或 TSV
[e1@t7-k3a9] link "More information..." href=/help
[e2@t7-k3a9] textbox "q" name=q
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
tabs {action: list|current|new|close|activate, tabId?, url?, active?}
```
- `list` → TSV：`id<TAB>*或-<TAB>title<TAB>url` + `total/shown/next`
- `current` → `id/window/os/title/url`
- `new` → `id/url/window`；`close` → `closed=<id>`；`activate` → `active=<id>`

### open
```text
open {url, tabId?, wait: load|dom|none}
```
新标签打开（或导航指定 tab），等页面就绪，自动安装网络 hook。
返回 `id / url / title / net_hook=on|off`。

### read
```text
read {tabId?, mode: md|text|html, selector?, main?, budget?, cursor?}
```
- `main` 默认 `true`：按文本量/链接密度选正文容器，头部回 `main=<selector>`
- `selector` 优先于 `main`，支持 shadow DOM / 同源 iframe 内的元素
- Markdown 按 `childNodes` 遍历，根节点直接文本、p/h1/pre、文本+块混排都不会漏字或重复
- 头部 `url/title[/main]`，正文，尾部 `total/shown/next`

### snapshot
```text
snapshot {tabId?, filter: interactive|full|links|buttons|form, near?, budget?, cursor?}
```
返回可引用的交互元素：`[e12@t7-k3a9] button "提交"`；`filter=full` 额外带 `#1/#2` 标题；
`near=<ref>` 限定在该元素最近的 `form/section/article/dialog/table/ul/main/nav` 内。
头部带 `refs= roots= doc= visual=`；纯视觉页面会给 `hint` 建议用 `shot annotate`。
密码框即使有值，名称也只来自 label/ARIA/placeholder/alt/title 等元数据，绝不使用 value。

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
全局 debugger 队列串行执行；单任务 60s 执行上限，超时会 `Runtime.terminateExecution` 并 detach，
后续调用可继续。

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
  - 分页 `next` 只推进到已完整交付记录；环形缓冲溢出回 `dropped=`，导航回绕回 `reset=1`，
    预览未完成时回 `pending=1`
  - `har=true`：导出 HAR 1.2（采样/预览，不是完整抓包），中继落盘并回 `path=` + `entries=N`

### files
```text
files {action: upload|download-dir, tabId?, ref?/selector?/target?, paths?, dir?}
```
- `upload`：CDP `DOM.setFileInputFiles`；Windows 必须 `C:/...` 绝对路径（≤20 个）
- `download-dir`：Edge 不支持时返回 `download_dir_unsupported`

### shot
```text
shot {tabId?, inline?, annotate?, format?}
```
- 目标必须是所在窗口的活动 tab；否则 `tab_not_active`，不会返回前台页图片
- 默认：落盘 PNG，回 `path=` + `bytes=`
- `inline=true`：回图片内容
- `annotate=true`：先快照，再在图上画 ref 编号，回坐标列表 + 标注图
  （`[e3@t7-k3a9] button "提交" @439,275 43x26`）；截图前后复核 tab/document
- `format=png|jpeg`

### batch
```text
batch {tool: read|snapshot|js|wait, tabIds: [..], args?, concurrency?}
```
多标签并发（默认 2，最大 4），输出 `tab=<id>` 块，块之间 `---`；单块失败不影响其它块。
- `args` 会与每个 `tabId` 合并后复用子工具 validator：`batch(wait,args:{time:3600})` 会在执行前拒绝
- 预算按 `ceil(tabCount/concurrency) × 单次最长耗时 + 15s`；js 因全局 debugger 串行按有效并发 1 计算
- 估算超过 600s 时在调用前报 `invalid_argument`，请减少 tab、提高并发或拆分 batch

## 典型调用循环

```text
# 读取 / 抓数据
open(url) → wait(selector) → read(main=true) → 需要时 read(cursor=next)

# 操作
open(url) → snapshot() → act(ref) → 看 changed/new/console_errors
          → 页面变了就重新 snapshot（ref 会失效）

# 等接口
act(click, waitFor={url:"api/save", status:200}) → 直接拿 response 行

# 调试
observe(console, since=next) / observe(network, since=next, bodyFilter="error")

# 多标签
open(urlA) → open(urlB) → batch(read, tabIds=[a,b])
```

## HTTP 调用示例

```bash
# 1) 握手（响应头里拿 Mcp-Session-Id）
curl -i -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  http://127.0.0.1:8765/mcp

# 2) 打开页面
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open","arguments":{"url":"https://example.com"}}}' \
  http://127.0.0.1:8765/mcp

# 3) 快照 → 按 ref 点击
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"snapshot","arguments":{}}}' \
  http://127.0.0.1:8765/mcp
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"act","arguments":{"action":"click","ref":"e1@t7-k3a9"}}}' \
  http://127.0.0.1:8765/mcp
```

## 限制

- `js`（debugger 世界）直接发起的 fetch 可能只以 `status=-1` 的资源条目出现；这类条目不会被 `waitFor` 当作成功响应
- 网络观测的 body 只是 ≤800 字符预览，HAR 也只是采样；不是完整抓包代理
- 跨域 iframe 不可遍历；内置页不可脚本化
- 合成 `press` 不提供浏览器原生 Tab/组合键默认行为
- 截图需要目标 tab 是活动 tab 且窗口可见（最小化会失败）
- 视觉兜底只有编号 + 坐标，没有 OCR
