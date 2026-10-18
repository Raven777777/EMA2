# EMA2 工具参考（v1.7.5）

所有工具都通过 MCP `tools/call` 调用。通用约定：

- 可选 `tabId`：缺省操作当前活动标签页；`open` 会返回新标签页的 `id`
- `budget`：返回文本的字符预算（默认 4000，范围 200–200000）
- `cursor`：从上次返回的 `next` 继续读
- 输出是**行式紧凑文本**，不是 JSON
- 工具错误是 `isError: true` 的文本结果，格式 `error=<code> msg=... hint=...`

## 输出协议

```text
# 头部：key=value（可能多行）
url=https://example.com/  title=Example Domain  scroll=0/1  refs=3
# 数据行：[ref] role "name" extras，或 TSV
[e1] link "More information..." href=/help
[e2] textbox "q" name=q
# 尾部：列表固定带 total / shown / next
total=2 shown=2 next=none
```

- `next=none` 表示没有更多；否则把 `next` 原样传给 `cursor`/`since`
- 文件类结果：`path=<绝对路径>` 单独一行，后跟 `bytes=` 等元信息
- 图片默认落盘；`inline=true` 才回图片内容

## 错误码

| code | 含义 | 建议 |
|---|---|---|
| `invalid_argument` | 参数不合法 | 看 `tools/list` schema |
| `extension_error` | 中继连不上扩展 | `ema2.exe status`，确认扩展已连接 |
| `stale_ref` | 页面已跳转，ref 失效 | 重新 `snapshot` |
| `ref_not_found` | 同页但 ref 不存在 | 重新 `snapshot` |
| `target_not_found` | ref/selector/target 都找不到 | 用 `snapshot` 的 ref，或 `target="role:name"` |
| `response_timeout` | `act waitFor` 未等到响应 | `observe network` 看实际请求，或调大 timeout |
| `wait_timeout` | `wait` 条件超时 | 检查条件或提高 timeout |
| `invalid_regex` | `bodyFilter` 不是合法正则 | 修正正则 |
| `debugger_attach_failed` | 该 tab 被 DevTools 占用 | 关掉 DevTools 重试 |
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
- 头部 `url/title[/main]`，正文，尾部 `total/shown/next`

### snapshot
```text
snapshot {tabId?, filter: interactive|full|links|buttons|form, near?, budget?, cursor?}
```
返回可引用的交互元素：`[e12] button "提交"`；`filter=full` 额外带 `#1/#2` 标题；
`near=<ref>` 限定在该元素最近的 `form/section/article/dialog/table/ul/main/nav` 内。
头部带 `refs= roots= visual=`；纯视觉页面会给 `hint` 建议用 `shot annotate`。

### act
```text
act {
  tabId?, action, ref?/selector?/target?, text?, value?, key?, deltaY?, submit?,
  fields?: [{ref?/selector?, value}], steps?: [ ... ], waitFor?: {url?, status?, timeout?}
}
```
- 动作：`click|type|fill|select|press|hover|scroll|submit|back|forward|reload`
- 定位：`ref`（snapshot 给的）优先，其次 `selector`，再 `target="button:提交"`
- 目标未渲染时自动重试 3 次（约 1s）；`stale_ref` 不重试
- `steps[]`：最多 20 步，可跨导航；返回每步结果 + `changed/url/title/new/console_errors`
- `waitFor`：动作后等匹配响应，命中回 `response=<status> <method> <path> <ms>`

### wait
```text
wait {tabId?, text|textGone|selector|time|idle（五选一）, timeout?}
```
返回 `ok=<条件> elapsed=...s`；`time` 最大 300s。

### js
```text
js {code, tabId?, budget?}
```
在页面主世界执行（debugger，`awaitPromise`）。字符串原样返回，其余值紧凑 JSON。

### observe
```text
observe {kind: console|network, tabId?, since?, level?, filter?, bodyFilter?, wait?, bodies?, har?, budget?}
```
- console：增量日志；`level=log|warn|error`；首次安装 hook（`open/act` 也会自动装）
- network：hook `fetch`/`XMLHttpRequest`，真实 `status` + 响应体前 800 字符
  - `filter`：URL 子串；`bodyFilter`：响应体正则
  - `wait`：等异步请求（≤10s）；`since` 用上次 `next`
  - `har=true`：导出 HAR 1.2，中继落盘并回 `path=` + `entries=N`

### files
```text
files {action: upload|download-dir, tabId?, ref?/selector?/target?, paths?, dir?}
```
- `upload`：CDP `DOM.setFileInputFiles`；Windows 必须 `C:/...` 绝对路径（≤20 个）
- `download-dir`：Edge 不支持时返回 `download_dir_unsupported`

### shot
```text
shot {tabId?, inline?, annotate?}
```
- 默认：落盘 PNG，回 `path=` + `bytes=`
- `inline=true`：回图片内容
- `annotate=true`：先快照，再在图上画 ref 编号，回坐标列表 + 标注图
  （`[e3] button "提交" @439,275 43x26`）

### batch
```text
batch {tool: read|snapshot|js|wait, tabIds: [..], args?, concurrency?}
```
多标签并发（默认 2，最大 4），输出 `tab=<id>` 块，块之间 `---`；单块失败不影响其它块。

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
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"act","arguments":{"action":"click","ref":"e1"}}}' \
  http://127.0.0.1:8765/mcp
```

## 限制

- `js`（debugger 世界）直接发起的 fetch 可能只以 `status=-1` 的资源条目出现
- 跨域 iframe 不可遍历；内置页不可脚本化
- 截图需要窗口可见（最小化会失败）
- 视觉兜底只有编号 + 坐标，没有 OCR
