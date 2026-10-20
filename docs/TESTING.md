# EMA2 测试说明 · v2.1.4

> ## 硬性规定：禁止 Node.js 测试
>
> - **绝对禁止**新增或运行 Node.js 测试（`node:test`、`jest`、`jsdom`、Playwright、
>   Puppeteer、VM 模拟等都不允许）。
> - 禁止提交 `package.json`、`package-lock.json`、`node_modules/` 或任何 Node 测试依赖。
> - 所有功能、回归、验收测试必须在**真实 Edge 浏览器** + 真实 `ema2` CLI 上执行。
> - 原因：Node 模拟测试启动慢、维护成本高，且找不到真实浏览器行为差异。
>   v2.0 真机验收就发现了 `type`/`select`、短 ref、`observe console --filter`、
>   `files download-dir` 等 Node 模拟很难覆盖的问题。
> - Rust 单测只用于纯解析/校验的快速反馈，**不能替代真实浏览器验收**。

v2.0 的产品面是 **CLI + 真实 Edge**，测试以真机行为和 CLI 输出为准，
不依赖 MCP 客户端、token、授权页或终端配置。

## 自动化

```bash
cd rust
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Rust 单测覆盖：工具白名单/参数校验、超时预算、act 脚本/`--steps` 解析、
batch 内层参数、pending 代际、锁中毒、截图/HAR 落盘、HTTP 路由、health/shutdown。

扩展侧不依赖 VM/jsdom 模拟；关键 DOM 行为全部通过下面的真实 Edge 清单覆盖。

## 真机验收清单

建议用**隔离 profile**，不要动日常 Edge 的登录态：

```powershell
# 1) 启动中继
bin\ema2.exe server
bin\ema2.exe status          # relay=running, extension=disconnected

# 2) 隔离 Edge（另开一个窗口；端口默认 7744）
& 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe' `
  --user-data-dir="$env:TEMP\ema2-profile" `
  --load-extension="$PWD\extension" `
  --disable-extensions-except="$PWD\extension" `
  --no-first-run --no-default-browser-check about:blank

# 3) 点工具栏 EMA2 图标 → 端口 7744 → 保存并连接
bin\ema2.exe status          # extension=connected
```

逐项检查：

| # | 命令 | 期望 |
|---|---|---|
| 1 | `ema2 server` / `ema2 server` | 第二次 `relay=already_running pid=...`，退出 0 |
| 2 | `ema2 status` | `relay=running`、`extension=connected`、`tabs=N active_tab=ID` |
| 3 | `ema2 tabs current` | `id/window/os/title/url` |
| 4 | `ema2 open https://example.com/` | `status=200`、`net_hook=on`、`id=` |
| 5 | `ema2 read --mode md` | 正文；截断时 `hint=... --cursor N` |
| 5a | 对百度 `read --mode text` | 不出现 `hotsearch_data` / `card_title` JSON / `/* eslint` |
| 5b | 对 Bing 首页 `read --mode text` | 正文不是只有 8 字符；`main=body` 或 `body(fallback)` |
| 5c | 对 GitHub dashboard `read --mode text` | 不出现 `{"props":` / `currentUserLogin` |
| 5d | 本地短 `<main>` + 长 body | `main=body(fallback)` + `hint=main extraction too short` |
| 5e | `read --mode html` | 仍是原始 HTML（允许 script/style） |
| 5f | 本地含 `display:none` / `hidden` / `aria-hidden` 文本 | 隐藏文本不进入 read 正文 |
| 6 | `ema2 snapshot` | `[e1@d1] ...`，无密码 value |
| 7 | `ema2 act fill e2 "x"` / `type e2 "y"` | fill 覆盖、type 追加；`wrote=1 chars=N` |
| 8 | `ema2 act select e4 <value>` | select 的 value 生效 |
| 9 | `ema2 act click e1` | 页面变化；旧完整 ref 报 `stale_ref` |
| 9a | 点击一个 5s 才响应的链接 | `settle=pending`、`next_url=`、退出 0 |
| 9b | 紧接 `ema2 await 10s` | `await=done elapsed=...s tab=... url=...` |
| 9c | 点击普通 fetch 按钮 | `settle=done ms<800 expect=page`，并提示用 `--wait-for` |
| 9d | 点击 `target=_blank` 链接 | `new_tab=ID`，`tabs list` 的 `session_tabs` 包含它 |
| 9e | `.ema2` 里 `click` 后写 `await 10s` | 每步 `ok`，最终仍在正确的新页面 |
| 9f | 脚本里 pending 后不写 `await` | `ok=1/N`、`stopped=1 reason=pending`，不继续点 |
| 10 | `ema2 act click e6 --wait-for "url=api/save status=200 timeout=5s"` | `response=200 GET /api/save` |
| 11 | `ema2 wait text/selector/time/idle` | `ok=... elapsed=...s` |
| 12 | `ema2 js '1+1'` / `ema2 js 'return 6*7'` / `--file` | `2` / `42` / 文件结果 |
| 13 | `ema2 observe network --since 0 --filter api` | 真实 status + body 预览 |
| 14 | `ema2 observe network --har out.har` | `path=... entries=N`，HAR 可解析 |
| 15 | `ema2 observe console --filter log-119` | 命中 console 行 |
| 16 | `ema2 files upload <abs> --ref e8` | `uploaded=1`；`js` 能看到文件名 |
| 17 | `ema2 files download-dir` | `download_dir_unsupported`，退出 4 |
| 18 | `ema2 shot --out out.png` | `path=` + `bytes=`，文件存在 |
| 19 | `ema2 shot --annotate --format jpeg` | 标注图落盘 |
| 20 | `ema2 shot --inline` | `data:image/png;base64,...` |
| 21 | `ema2 batch read --urls "u1,u2"` | 两块，`---` 分隔，各自 `tab=` |
| 22 | `ema2 batch open --urls "u1,u2"` | 并行开页，各自 `id=` |
| 23 | `ema2 act --steps 'fill e2 "a"; type e2 "b"'` | 两步；type 追加 |
| 24 | `ema2 act --script flow.ema2` | 逐行执行，`waitFor` 挂在整体 |
| 25 | `ema2 open url --replace` / `--reuse session` / `--tab ID` | `reuse=replace/session/tabId` |
| 26 | `ema2 tabs close --session` | 只关本会话 `open`/`tabs new` 的标签 |
| 27 | `ema2 init` | `path=... action=created`；重复执行 `action=updated`，标记只有一份 |
| 28 | `ema2 server stop` → `ema2 status` | `relay=stopped`；status 退出 2 |

错误路径：

| 场景 | 期望 |
|---|---|
| 中继没起时 `ema2 tabs` | `error=relay_not_running`，退出 2 |
| 中继在、扩展没连时 `ema2 tabs` | `error=extension_not_connected`，退出 3 |
| 7744 被别的服务占用时 `ema2 server` | `error=port_in_use`，退出 2，不换端口 |
| `ema2 act type e15`（缺文本） | `error=invalid_argument`，退出 1 |
| `ema2 act click <disabled>` | `error=element_disabled`，退出 4 |
| `ema2 act click <旧完整 ref>` | `error=stale_ref`，退出 4 |
| `ema2 files download-dir` | `error=download_dir_unsupported`，退出 4 |

## 开发注意

- 扩展是普通 MV3 service worker。改完 `extension/` 后在 `edge://extensions/`
  点“重新加载”；如果 Edge 仍用旧脚本，把 `extension/manifest.json` 的
  `version` 加一，或换一个全新 `--user-data-dir`（Edge 按 manifest 版本缓存扩展脚本）。
- 页面 helper 有版本号（`__ema2_helpers_version`）。改 helper 行为时同步加一，
  否则已经打开的页面会继续用旧 helper。
- 截图要求目标 tab 是活动 tab 且窗口可见；连续截图注意 Chromium 的
  `MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND` 配额。
- 观察网络/hook 只能看到 hook 安装之后的请求；`act waitFor` 对安装前的历史请求
  会明确给 `observation_gap=status_unavailable`。
- 验收结束：

```powershell
bin\ema2.exe tabs close --session
bin\ema2.exe server stop
```
