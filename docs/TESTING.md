# EMA2 测试说明

## 三条硬性规定

### 1. 真机、前台、可见

- 所有测试都必须在**真实、前台可见的 Edge 窗口**里执行；
  禁止无头浏览器、禁止后台隐藏窗口、禁止用模拟 DOM 代替。
- 扩展不提供 headless 模式；`shot`、`press`、密码管理器浮层都依赖前台窗口。
- `scripts/verify.py` 会 `tabs activate` 把窗口提到前台，并断言 `document.hasFocus()`；
  焦点断言失败即视为测试失败，不会静默跳过。

### 2. 禁止 Node.js 测试

- 禁止新增或运行 Node 测试（`node:test`、jest、jsdom、Playwright、Puppeteer、VM 模拟）。
- 禁止提交 `package.json`、`package-lock.json`、`node_modules/`。
- 原因：模拟测试慢、维护成本高，而且找不到真实浏览器差异——
  `type/select`、短 ref、`observe console --filter`、`files download-dir`
  都是真机验收才发现的问题。

### 3. 断言输出，而不是“看起来对”

- 每条用例都要检查 `key=value` 行（`status=` / `settle=` / `error=` / `changed=`），
  不要只看进程退出码。
- 负例必须断言**具体错误码**和退出码，否则“失败”可能是别的原因。

## 自动化

```bash
cd rust
cargo test                          # 87 项：参数校验、超时预算、脚本解析、路由、落盘
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

```bash
python scripts/verify.py                 # 静态契约 + 真实 Edge 冒烟（135 项）
python scripts/verify.py --skip-real-edge  # 只检查错误码文档漂移
```

`verify.py` 只用标准库，不引入 Node；它启动本机 fixture HTTP 服务，
用真实 `ema2` CLI 驱动真实 Edge 打开它，覆盖
open / snapshot / act（按钮+链接+SPA+scroll）/ read（md 链接·表格·列表·shadow DOM）/ js /
wait / observe console·network / shot / files upload（含 5 MiB 边界）/ batch / 负例错误码，
并静态断言 `extension/background.js` 的每个 `fail('code'` 都写进了 `docs/TOOLS.md`。

guard 回归还覆盖：未执行 EMA2 命令的页面没有 `window.__ema2_guard`；
`act` 返回后 guard 必须 `armed=0`；`role:name` 能匹配 HTML `name`，且隐藏的同名
控件不会被优先选中；非活动标签可由 `act --tab` 修改；`batch --reuse session` 复用会话标签；
`shot --out ./a/../b.png` 回显规范化路径。

## 真机验收

### 方式 A：日常 Edge（推荐）

扩展已加载、popup 端口是 7744 的情况下：

```bash
bin/ema2.exe server
bin/ema2.exe status          # relay=running + extension=connected
python scripts/verify.py
```

### 方式 B：隔离 profile（可选）

只在需要干净环境（不污染日常登录态）时用。**仍然是可见的前台窗口**，
不是无头；扩展要在这个 profile 里手动加载一次。

```powershell
bin\ema2.exe server
& 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe' `
  --user-data-dir="$env:TEMP\ema2-profile" `
  --load-extension="$PWD\extension" `
  --disable-extensions-except="$PWD\extension" `
  --no-first-run --no-default-browser-check about:blank
# 点工具栏 EMA2 图标 → 端口 7744 → 保存并连接
bin\ema2.exe status          # extension=connected
```

### 人工清单（真实站点/复杂交互）

`verify.py` 覆盖的是本机 fixture；下面这些仍需在真实站点上抽查。

| # | 命令 | 期望 |
|---|---|---|
| 0 | 普通浏览任意页面（不执行 EMA2 命令） | 无 `window.__ema2_guard`；alert/confirm/prompt/window.open 为原生行为 |
| 1 | `ema2 server` ×2 | 第二次 `relay=already_running pid=...`，退出 0 |
| 2 | `ema2 status` | `relay=running`、`extension=connected`、`tabs=N active_tab=ID` |
| 3 | `ema2 tabs current` | `id/window/os/title/url` |
| 4 | `ema2 open https://example.com/` | `status=200`、`net_hook=on`、`id=`、`focused=1` |
| 5 | `ema2 read --mode md` | 正文；链接是 `[text](url)`；截断时给 `--cursor N` |
| 5a | 百度 `read --mode text` | 不出现 `hotsearch_data` / `card_title` JSON / `/* eslint` |
| 5b | Bing 首页 `read --mode text` | 正文不是只有 8 字符；`main=body` 或 `body(fallback)` |
| 5c | GitHub dashboard `read --mode text` | 不出现 `{"props":` / `currentUserLogin` |
| 5d | 本地短 `<main>` + 长 body | `main=body(fallback)` + `hint=main extraction too short` |
| 5e | `read --mode html` | 仍是原始 HTML（允许 script/style） |
| 6 | `ema2 snapshot` | `[e1@d1] ...`，无密码 value |
| 7 | `ema2 act fill e2 "x"` / `type e2 "y"` | fill 覆盖、type 在 caret 处插入；`wrote=1 chars=N` |
| 8 | `ema2 act select e4 <value>` | select 的 value 生效 |
| 9 | `ema2 act click e1` | 页面变化；旧完整 ref 报 `stale_ref` |
| 9h | `ema2 act fill ... --tab ID` | 修改非活动 tab，活动 tab 和窗口焦点都不变 |
| 9a | 点击 5s 才响应的链接 | `settle=pending`、`next_url=`、退出 0 |
| 9b | 紧接 `ema2 await 10s` | `await=done elapsed=...s tab=... url=...` |
| 9c | 点击普通 fetch 按钮 | `settle=done ms<800 expect=page`，并提示用 `--wait-for` |
| 9d | 点击 `target=_blank` 链接 | `new_tab=ID`，`tabs list` 的 `session_tabs` 包含它 |
| 9e | `.ema2` 里 `click` 后写 `await 10s` | 每步 `ok`，最终仍在正确的新页面 |
| 9f | 脚本里 pending 后不写 `await` | `ok=1/N`、`stopped=1 reason=pending`，不继续点 |
| 9g | 点击一个不导航的 `<a href="#">` | `nav=none` + hint；`<a>` 只重试一次 |
| 10 | `ema2 act click e6 --wait-for "url=api/save status=200 timeout=5s"` | `response=200 GET http://.../api/save` |
| 10a | `ema2 act fill --target "searchbox:q" ...` | 无 label、`name=q` 的 searchbox 被命中 |
| 11 | `ema2 wait text/selector/time/idle` | `ok=... elapsed=...s` |
| 12 | `ema2 js '1+1'` / `ema2 js 'return 6*7'` / `--file` | `2` / `42` / 文件结果 |
| 12a | 弹过密码管理器浮层的 tab 里 `ema2 js` | 结果 + `via=scripting`，不因 `debugger_attach_failed` 丢失 |
| 13 | `ema2 observe network --since 0 --filter api` | 真实 status + body 预览 |
| 14 | `ema2 observe network --har out.har` | `path=... entries=N`，HAR 可解析 |
| 15 | `ema2 observe console --filter log-119` | 命中 console 行 |
| 16 | `ema2 files upload <abs> --ref e8` | `uploaded=1`；`js` 能看到文件名 |
| 17 | `ema2 files download-dir` | `download_dir_unsupported`，退出 4 |
| 18 | `ema2 shot --out out.png` | `path=` + `bytes=`，文件存在 |
| 19 | `ema2 shot --annotate --format jpeg` | 标注图落盘 |
| 20 | `ema2 shot --inline` | `data:image/png;base64,...` |
| 21 | `ema2 batch read --urls "u1,u2"` | 两块，`---` 分隔，各自 `tab=` |
| 21a | `ema2 batch read --urls "u1" --reuse session` | 复用本会话精确 URL 标签并回 `reused=1` |
| 22 | `ema2 batch open --urls "u1,u2"` | 并行开页，各自 `id=` |
| 23 | `ema2 act --steps 'fill e2 "a"; type e2 "b"'` | 两步；type 在 caret 处插入 |
| 24 | `ema2 act --script flow.ema2` | 逐行执行，`waitFor` 在所在行执行；失败保留已完成 steps |
| 25 | `ema2 open url --replace` / `--reuse session` / `--tab ID` | `reuse=replace/session/tabId` |
| 25a | `ema2 open url --timeout 30` | 慢页面也能等到文档接位；超时输出 `load=pending` |
| 26 | `ema2 tabs close --session` | 只关本会话 `open`/`tabs new` 的标签 |
| 27 | `ema2 init` | `path=... action=created`；重复执行 `action=updated`，标记只有一份 |
| 28 | `ema2 server stop` → `ema2 status` | `relay=stopped`；status 退出 2 |
| 29 | `ema2 act scroll up/down N` | 方向正确，`down N` 约 N 屏 |
| 30 | `ema2 read` on shadow DOM | 能看到 open shadow root 正文 |
| 31 | `ema2 files upload` 5 MiB | 不再被 2MB axum 默认上限截断 |
| 32 | 运行时 `SyntaxError` | `js_exception` 且副作用只执行一次 |
| 33 | 命令执行中的 `alert/print` | 非阻塞并回 `dialog_opened=` |
| 34 | 命令结束后定时检查 guard | `window.__ema2_guard.armed=0`，原生 API 已恢复 |

错误路径：

| 场景 | 期望 |
|---|---|
| 中继没起时 `ema2 tabs` | `error=relay_not_running`，退出 2 |
| 中继在、扩展没连时 `ema2 tabs` | `error=extension_not_connected`，退出 3 |
| 7744 被别的服务占用时 `ema2 server` | `error=port_in_use`，退出 2，不换端口 |
| `ema2 act type e15`（缺文本） | `error=invalid_argument`，退出 1 |
| `ema2 act click --at 1,2`（拼错/未知 flag） | `error=invalid_argument`，退出 1 |
| `ema2 act click foo`（不是 ref） | `error=invalid_argument`，退出 1 |
| `ema2 act click <disabled>` | `error=element_disabled`，退出 4 |
| `ema2 act click <旧完整 ref>` | `error=stale_ref`，退出 4 |
| `ema2 read --selector "#nope"` | `error=selector_not_found`，退出 4 |
| `ema2 files download-dir` | `error=download_dir_unsupported`，退出 4 |

## 开发注意

- 扩展是普通 MV3 service worker。改完 `extension/` 后在 `edge://extensions/`
  点“重新加载”；如果 Edge 仍用旧脚本，把 `extension/manifest.json` 的
  `version` 加一（Edge 按 manifest 版本缓存扩展脚本）。
- 页面 helper 有版本号（`__ema2_helpers_version`）。改 helper 行为时同步加一，
  否则已经打开的页面会继续用旧 helper。
- 截图要求目标 tab 是活动 tab 且窗口可见；连续截图注意 Chromium 的
  `MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND` 配额。
- 观察网络/hook 只能看到 hook 安装之后的请求；`act waitFor` 对安装前的历史请求
  会明确给 `observation_gap=status_unavailable`。
- 密码管理器等**别的扩展**的浮层只能被“召唤”，不能被 EMA2 点击
  （跨扩展隔离 + closed shadow root），真实登录仍需人手点一次浮层条目。

## 收尾

```bash
bin/ema2.exe tabs close --session
bin/ema2.exe server stop
```
