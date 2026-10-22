# EMA2 仓库

维护者/继任者先读 [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) 与 [docs/HANDOFF.md](docs/HANDOFF.md)；
文档全貌见 [docs/README.md](docs/README.md)。

## 构建与测试（本仓库）

```bash
scripts\setup.cmd            # 或 cargo build --release --manifest-path rust/Cargo.toml
cd rust && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
python scripts/verify.py     # 真实、前台可见的 Edge 验收（禁止无头 / 禁止 Node）
```

- 改 `extension/` 后在 `edge://extensions` 重新加载；改页面 helper 要同步提升 `__ema2_helpers_version`。
- 动作类改动必须真机验证；不要提交 `bin/`、`package.json`、`node_modules`。
- 新的错误码必须写进 `docs/TOOLS.md`（`cargo test` 会检查）。
- 过时文档进 `docs/archive/`，不要留在 `docs/` 根下。

### 重新加载扩展（pywinauto）

```bash
pip install pywinauto
python scripts\reloadextension.py
```

- 需要已加载 EMA2 扩展的 Edge；Edge 没开时脚本会自动启动它。
- 脚本用 UI Automation 找 msedge.exe 窗口，先恢复并最大化（等价 Win+Up），
  再把 `edge://extensions` 填进地址栏；随后在 EMA2 卡片里按
  `Type=Button` + `Name=重新加载` 精确查找并点击，不依赖固定坐标，
  窗口缩放/主题/布局变化不受影响。
- 完成后 `ema2 status` 应显示 `extension=connected version=<新版本>`；daemon 没起先 `ema2 server`。

下面是给 AI 的最短操作手册（`ema2 init` 生成区块）：

<!-- ema2:begin -->
# EMA2 · 用 CLI 操作真实 Edge

本文件由 `ema2 init` 生成。EMA2 无 MCP、无 token、无授权页、无配置文件；AI 直接 shell 调用 `ema2`。
默认端口 **7744**。分发包只带 `ema2.exe` 和本文件；**`ema2 <cmd> --help` 是用户侧权威文档**。

## 最小流程

```bash
ema2 server                 # 每会话先起 daemon，后台立即返回
ema2 status                 # 期望 relay=running extension=connected
ema2 open <url>             # 或 tabs new；看 status= / net_hook= / focused=
ema2 snapshot               # 拿 ref，如 e12@d1
ema2 act click e12@d1       # 操作；默认 settle 3s
ema2 await 10s              # settle=pending 或需等导航/稳定时
ema2 read                   # 读正文；需要时再 snapshot
ema2 tabs close --session   # 收尾：关本会话打开的标签
ema2 server stop            # 收尾：关 daemon
```

执行顺序：`server` -> `status` 确认扩展连接 -> 操作 -> 失败按错误码动作 -> 收尾。
7744 不通不会自动拉起 daemon，只报 `relay_not_running`。
参数不确定先 `ema2 <cmd> --help`；脚本语法 `ema2 act --help`。

`server` 后首次 `status` 若为 `extension=disconnected last_ping=never`，等 2–3s 再查；
重连需人工点扩展图标 → 端口 7744 → 重连。

## 全局协议

- 输出：行式紧凑文本，**不是 JSON**；HAR 是唯一 JSON 例外。文件类结果 `path=` 单独一行。
- 错误：stderr 首行 `error=<code> msg=... hint=...`，按错误码表处理。
- 退出码：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错。
- 端口：默认 **7744**；`--port N` 时 server 与所有命令必须一致。
- 时间：`--timeout` 单位秒；`wait/await/waitFor` 可写 `5s` / `500ms`。
- `budget`：返回文本字符预算，默认 **4000**，范围 200–200000；`cursor` 用上次 `next` 续读。
- 列表尾部固定 `total/shown/next`；`next=none` 表示读完，否则原样传给 `--cursor`/`--since`。

## 命令签名

| 命令 | 用途 / 关键副作用 |
|---|---|
| `ema2 server [--port N]` / `ema2 server stop` | 启停本机中继 daemon；`server` 后台化立即返回 |
| `ema2 status` | relay / extension / tabs 状态 |
| `ema2 init` | 在当前目录写/更新本文件 |
| `ema2 tabs [list\|current\|new\|close\|activate]` | 标签页；`tabs new` **默认激活并聚焦窗口**，`--background` 才后台 |
| `ema2 open <url> [--tab ID\|--replace\|--reuse session] [--wait load\|dom\|none] [--timeout 15]` | 打开/导航；输出 `id/url/title/status/reuse/net_hook/focused` |
| `ema2 read [--tab ID] [--mode md\|text\|html\|visible] [--selector X] [--no-main] [--budget N] [--cursor N]` | 读正文；默认 `--main` 开。登录/验证页输出 `state=login_required\|challenge`（exit 0），空页/PDF/HTTP 404 报错（exit 4） |
| `ema2 snapshot [--tab ID] [--filter interactive\|full\|links\|buttons\|form] [--near REF] [--budget N] [--cursor N] [--values]` | ref 快照；`[e12@d1] button "X"`，href 是完整 URL |
| `ema2 act <action> [ref] [text] [flags] [--tab ID]` / `--steps` / `--script x.ema2` | 单步或脚本；不强制切换活动标签 |
| `ema2 await [10s] [--tab ID]` | 等**上一个 act 所在标签**的导航/页面稳定；缺省 3s，范围 0–300s |
| `ema2 wait <text\|textGone\|selector\|time\|idle> ... [--timeout 30s] [--tab ID]` | 等页面条件；**shell 命令，不能写进 .ema2 脚本** |
| `ema2 js '<code>' [--tab ID] [--budget N] [--cursor N] [--timeout 10]` / `--file f.js` | 页面主世界执行 JS；默认 10s 上限 |
| `ema2 observe console\|network ... [--since SEQ] [--har x.har]` | console 默认 `--level error`；network 可 `--body-filter`/`--bodies=false` |
| `ema2 files upload <path...> --ref R` / `--selector S` / `--target T` | 上传本地文件；`files download-dir` 固定报错 |
| `ema2 shot [--tab ID] [--inline] [--annotate] [--format png\|jpeg] [--out PATH]` | 截图落盘；`--tab` 自动激活目标标签 |
| `ema2 batch <open\|read\|snapshot\|js\|wait> --urls "u1,u2"` / `--tab-ids "1,2"` | 多标签并发；默认新开，`--reuse session` 仅用于 `--urls` |

## 默认值 / 副作用 / 语义

- `ref`：形如 `e12@d1`；导航/刷新/换 tab 后旧完整 ref 报 `stale_ref`，重新 `snapshot`。
  手敲可用短 ref `e12`（当前文档）；严格防误点用 snapshot 原样完整 ref。
- `tabs new`：默认激活并聚焦窗口；`tabs activate` 也会 `windows.update{focused:true}`。
- `open`：默认新开并登记会话；`--replace` 当前标签导航；`--reuse session` 复用本会话最近标签；
  `--tab ID` 指定标签且不计入会话；三者互斥。`--timeout` 是整个 open 流程硬上限，1..60 默认 15。
- `act`：默认最多 settle 3s；`settle=pending` **不是失败**，接 `ema2 await 10s`。
  点链接没导航给 `nav=none`；可导航 `<a>` 无任何变化才自动重试一次，输出 `retried=1`。
  普通按钮最多 800ms；SPA 只改 DOM 时约 1s 后按 `nav=none` 结算。
- `act --wait-for`：动作 settle 后验证后置条件。`url=`/`status=` 等响应/导航；
  `text=`/`textGone=`/`selector=` 等页面条件（与 `ema2 wait` 同义，一次一个）。
  成功多一行 `expect=ok condition=... ms=...`；超时 `wait_timeout` + `expect=timeout`。
- `.ema2` 脚本：只允许 11 个 act 动作 + `await 10s` + `waitFor url=... status=... timeout=5s`
  或 `waitFor text=... timeout=5s`；
  **不能写** `open/read/snapshot/wait/js/tabs`。最多 **20 步**。pending 且下一步不是 await 时脚本停止。
- 输入：`fill` 覆盖整值；`type` 按 caret/selection 插入（选中会替换），优先 CDP 可信输入；
  `select` 位置参数是 option 的 `value`；`<select multiple>` 用逗号分隔。
- `--tab ID`：可用于单步、`--steps`、`--script`；`--target "role:name"` 也匹配精确 HTML `name` 属性（如 `searchbox:q`）。
- `read`：默认清洁正文（跳脚本/隐藏/页面壳）；main 太短回退 `main=body(fallback)` 并给 hint。
  `--mode md` 有损（标题/列表/表格/链接保留，样式与合并单元格丢）。
  `--mode visible` 用 `innerText`（含导航壳），适合复杂 SPA；默认仍受 main/selector 约束。
  `--cursor` 是字符偏移，换 `--mode` 续读会错位。
- `read` 出现 `main=body(fallback)` 且混入“相关阅读/最新文章”链接时，改用 `--selector article`（无则 `--no-main`）；付费墙只读公开段落，勿重试。
- `snapshot`：默认 interactive；`--values` 输出当前值；`--filter form` 等价开启 values；
  密码框 value 始终 `<hidden>`，用户输入不进入 name。
- `click`：检查隐藏/遮罩；失败为 `not_clickable` / `obscured`。
- `press`：优先 CDP 可信按键（`trusted=1`），支持 Tab/Backspace/Enter/Ctrl+A 等默认行为；
  debugger 被占用回退合成事件（`trusted=0`），浏览器级默认行为不可用。
- 窗口：**必须前台**；`shot --tab` 自动激活目标标签；`open` 输出 `focused=0` 时先 `tabs activate <id>`。
- `files upload`：单次 ≤20 文件、≤5 MiB；走页面 `DataTransfer`，不依赖 debugger；
  Windows 路径必须 `C:/...` 绝对路径。
- `batch`：默认并发 2、最大 4；`--tab-ids` 与 `--urls` 互斥；`tool=open` 只能配 `--urls`；
  估算超过 600s 执行前报 `invalid_argument`。
- 多标签读正文用 `batch read --tab-ids "1,2,3"`，比逐个 `read --tab` 少往返。
- `js`：既可表达式也可 async 函数体；debugger 不可用降级 `via=scripting`，受 CSP 限制；
  `--timeout` 默认 10s（0.1..300s），超时会 `terminateExecution`，终止该 tab **所有** JS。
- `observe network`：body 只是 ≤800 字符预览；HAR 是采样，不是完整抓包代理；
  HAR 默认 redact `authorization/cookie/set-cookie`，`--headers all` 才导出原值。
- `shot`：默认落盘 PNG；`--inline` 回 base64；`--annotate` 先快照再画 ref 编号与坐标。

## Must / Never

Must：
- 每会话先 `ema2 server`，再 `ema2 status` 确认 `extension=connected`。
- 导航/刷新后重新 `snapshot` 再操作 ref。
- `settle=pending` 后接 `ema2 await 10s`。
- `open focused=0` 时先 `tabs activate <id>`。
- `read` 被截断时按 `--cursor next` 续读，保持同一 `--mode`。
- 收尾执行 `tabs close --session` 和 `server stop`。

Never：
- 不要假设 7744 不通会自动拉起 daemon。
- 不要在 `.ema2` 脚本里写 `open/read/snapshot/wait/js/tabs`。
- 不要复用导航/刷新后的旧完整 ref。
- 不要对隐藏元素或 overlay 下元素直接 click。
- 不要混用 server 与命令的 `--port`。
- 不要把 `wait` 写进脚本；脚本用 `await` / `waitFor`。
- 不要用 `batch --reuse session` 配 `--tab-ids`；它仅用于 `--urls`。

## 错误码 -> 动作

| code | 动作 |
|---|---|
| `relay_not_running` | 运行 `ema2 server` |
| `extension_not_connected` | 打开 Edge 扩展 popup，端口填 7744，保存并重连；确认扩展已加载 |
| `port_in_use` | `ema2 server stop`，或换 `--port`（扩展端口也要改） |
| `relay_start_failed` | 看提示里的 `%TEMP%\ema2.log` |
| `relay_error` | 已连中继但请求失败；查 `ema2 status`，不要误判 daemon 没起 |
| `payload_too_large` | 减小上传或拆分调用 |
| `invalid_argument` / `invalid_arguments` | 看 `ema2 <cmd> --help`；脚本看 `ema2 act --help` |
| `invalid_action` / `invalid_step` / `too_many_steps` | 用支持的动作；每步写 action；≤20 步，拆多次 |
| `invalid_tab` / `tab_not_found` / `no_active_tab` | `tabs list`；先 `open` 或 `tabs new` |
| `invalid_url` | 用完整 http(s) 地址 |
| `invalid_path` | Windows 上传路径用 `C:/dir/file` 绝对路径 |
| `invalid_selector` | CSS 语法错；换 selector 或用 snapshot ref |
| `invalid_regex` | 改写 bodyFilter，避免灾难性回溯 |
| `missing_url` / `missing_code` / `missing_fields` / `missing_files` / `missing_target` / `missing_key` | 按提示补必填参数 |
| `too_many_files` | 分批上传，单次 ≤20 |
| `page_changed` | 等页面稳定后重试 |
| `stale_ref` / `ref_not_found` | 重新 `ema2 snapshot` |
| `target_not_found` | 用 snapshot ref，或 `--target "button:提交"` |
| `not_clickable` / `obscured` | 重新 snapshot；隐藏或 overlay 下元素不能点 |
| `element_disabled` / `element_readonly` / `unsupported_control` / `not_writable` / `not_select` | 换控件或先启用；select 用对应动作 |
| `value_not_available` / `value_rejected` | 检查 option value，或改用 `type` |
| `input_prevented` | 页面接管输入（cancel 了 `beforeinput`）；改用 `js` 或逐键 `press` |
| `click_failed` / `action_failed` | 看 console，重新 snapshot 后重试 |
| `helpers_missing` | 重试 `snapshot`/`read`；持续失败重载页面 |
| `no_form` | 用 click 提交按钮，或 press Enter |
| `no_pending_action` | 先 `act`；或改用 `wait text/selector/time/idle` |
| `wait_timeout` | 看最后 URL/文本，检查条件或提高 timeout |
| `response_timeout` | 看 `observation_gap`；`--wait-for` 加 `timeout=5s`（≤30s） |
| `tool_timeout` | 关对话框/重载 tab，或缩小单次操作 |
| `screenshot_too_large` / `screenshot_decode_failed` / `screenshot_empty` / `screenshot_not_saved` | 检查输出目录/窗口可见性，或 `--inline` |
| `har_not_saved` | 检查 `--har` 路径与 `EMA2_OUTPUT_DIR` |
| `result_too_large` | 用 budget/cursor，避免 `shot --inline` |
| `tab_not_active` | 先 `tabs activate <id>`；`shot --tab` 自动激活 |
| `tab_changed` / `capture_failed` / `capture_empty` | 页面稳定后重试；保持窗口前台可见 |
| `snapshot_failed` | 重试，或用 `shot` 看渲染 |
| `upload_failed` | 换 ref/selector，确认目标是 `input[type=file]` |
| `js_exception` | 看 hint，先 `read`/`snapshot` 再改代码 |
| `script_timeout` | `js` 超时；简化表达式、加 `--timeout`，或改用 read/snapshot/wait |
| `debugger_attach_failed` | 先重载该 tab；`ema2 js` 自动降级（看 `via=scripting`） |
| `debugger_attach_timeout` / `debugger_detach_timeout` | 关掉 DevTools/占用 debugger 的扩展，或重载 tab |
| `debugger_timeout` / `debugger_busy` | 终止长任务后重试 |
| `internal_page` | `edge://` 等内置页不可脚本化；换 http(s) |
| `empty_page` | `read` 整页 md/text 0 字符；`wait idle`/`wait selector` 后重试，或 `shot --annotate` |
| `page_error` | HTTP ≥400（404/403/5xx）；核对 url 或换页面 |
| `pdf_page` | PDF 不做文本提取；开 html 版本，或让用户保存后用文件工具读 |
| `download_dir_unsupported` | Edge 不允许扩展改下载目录；文件进默认下载目录 |

## 端口错配诊断

```text
ema2 status
# relay=running ... extension=disconnected
```

1. 点 Edge 工具栏 EMA2 图标；
2. 端口必须与 `ema2 status` 的 `port=` 相同（默认 7744）；
3. 保存后点“重新连接”；
4. 仍不连：`edge://extensions` 重新加载扩展。

## 收尾

```bash
ema2 tabs close --session   # 关本会话由 open/tabs new 打开的标签
ema2 server stop            # 显式关闭 daemon
```
<!-- ema2:end -->
