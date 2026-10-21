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

### 重新加载扩展（AutoHotkey）

```bash
"C:\Program Files\AutoHotkey\v2\AutoHotkey64.exe" scripts\reloadextension.ahk
```

- 需要 AutoHotkey v2 和已加载 EMA2 扩展的 Edge。
- 脚本会激活/启动 Edge，打开 `edge://extensions`，点击 `(600,815)` 处的“重新加载”；
  窗口缩放/布局不同时改 `scripts/reloadextension.ahk` 顶部的 `clickX` / `clickY`。
- 完成后 `ema2 status` 应显示 `extension=connected version=<新版本>`；daemon 没起先 `ema2 server`。

下面是给 AI 的最短操作手册（`ema2 init` 生成区块）：

<!-- ema2:begin -->
# EMA2 · 用 CLI 操作真实 Edge

本文件由 `ema2 init` 生成。EMA2 没有 MCP、没有 token、没有授权页；
AI 直接通过 shell 调用 `ema2` 子命令。

## 第一条命令

```bash
ema2 server      # 后台 daemon，立即返回
ema2 status      # relay=running / extension=connected
```

**每个会话先 `ema2 server`。** 命令发现 7744 不通时不会自动拉起 daemon，
只会报 `relay_not_running`。

参数不确定时先跑 `ema2 <cmd> --help`；脚本语法见 `ema2 act --help`。

## 命令表

| 命令 | 用途 |
|---|---|
| `ema2 server` / `ema2 server stop` | 启动 / 关闭本机中继 daemon |
| `ema2 status` | 中继、扩展、标签页状态 |
| `ema2 init` | 在当前目录写/更新本文件 |
| `ema2 tabs [list\|current\|new\|close\|activate]` | 标签页；`tabs new` 默认激活 |
| `ema2 open <url> [--replace\|--reuse session\|--tab ID] [--timeout N]` | 打开 URL，回 `status=` / `net_hook=` / `focused=` |
| `ema2 read [--mode md\|text\|html] [--selector X] [--budget N] [--cursor N]` | 读页面正文 |
| `ema2 snapshot [--filter ...] [--values]` | ref 快照 `[e12@d1] button "X"`；href 是完整 URL |
| `ema2 act <action> ...` / `--steps` / `--script x.ema2` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload |
| `ema2 await 10s` | 等上一个 act 动作的导航/页面稳定 |
| `ema2 wait <text\|textGone\|selector\|time\|idle> ...` | 等页面条件 |
| `ema2 js '<code>' [--file f.js] [--timeout N]` | 页面主世界执行 JS（默认 10s 上限） |
| `ema2 observe console\|network ... [--har x.har]` | console / network 增量 |
| `ema2 files upload <path...> --ref R` | 上传本地文件 |
| `ema2 shot [--annotate] [--format png\|jpeg] [--out path]` | 截图落盘 |
| `ema2 batch <open\|read\|snapshot\|js\|wait> --urls "u1,u2"` | 多标签并发 |

常用细节：

- 输出是行式紧凑文本；HAR 是唯一 JSON。`path=` 单独一行。
- `ref` 形如 `e12@d1`；导航/刷新后旧完整 ref 报 `stale_ref`，重新 `snapshot`。
  手敲可用短 ref `e12`（当前文档）；要严格防止误点就用 snapshot 原样返回的完整 ref。
- 单步写 `ema2 act fill <ref> "<text>"`；`type` 按当前 caret/selection 插入（选中会替换），`fill` 覆盖整个值；`select` 的位置参数是 option 的 `value`，`<select multiple>` 可用逗号分隔多个 value。
- `read` 默认清洁正文（跳过脚本/隐藏元素）；main 太短会回退 `main=body(fallback)` 并给 hint。
  `--mode md` 是有损渲染：标题/列表/表格/链接保留，样式与合并单元格会丢。
- `act` 默认最多 settle 3s；返回 `settle=pending` 不是失败，接 `ema2 await 10s`。
  点了链接但没导航会给 `nav=none`（`<a>` 只自动重试一次，输出 `retried=1`）。
  `.ema2` 脚本里写 `await 10s`；pending 且下一步不是 await 时脚本会停止。
- `.ema2` 只允许 act 动作 + `await 10s` + `waitFor url=... status=... timeout=5s`；
  不能写 `open/read/wait`（这些在 shell 里单独调用）。完整语法/示例：`ema2 act --help`。
- 列表尾部有 `total/shown/next`；`next=none` 表示读完，否则把 `next` 传给 `--cursor`/`--since`。
- `click` 会检查隐藏/遮罩（`not_clickable`/`obscured`）；`press` 优先用 CDP 可信按键，
  支持 Tab/Backspace/Enter/Ctrl+A 等浏览器默认行为（`trusted=1`）；debugger 被占用时回退合成事件（`trusted=0`）。
- **窗口必须在前台**：`shot --tab` 会自动激活目标标签；`open` 输出 `focused=0` 时先 `tabs activate <id>`。
- `files upload` 单次 ≤20 个文件、≤5 MiB，走页面 `DataTransfer`，不依赖 debugger。
- 默认端口 **7744**；`--port N` 时 server 与所有命令必须一致。
- `--timeout` 单位是秒；`wait/await/waitFor` 的时长可写 `5s` / `500ms`。

## 错误码怎么办

| code | 处理 |
|---|---|
| `relay_not_running` | 运行 `ema2 server` |
| `extension_not_connected` | 打开 Edge 扩展 popup，端口填 7744，保存并重连；确认扩展已加载 |
| `port_in_use` | `ema2 server stop`，或换 `--port`（扩展端口也要改） |
| `invalid_argument` | 看 `ema2 <cmd> --help`；脚本看 `ema2 act --help` |
| `stale_ref` / `ref_not_found` | 重新 `ema2 snapshot` |
| `target_not_found` | 用 snapshot 的 ref，或 `--target "button:提交"` |
| `not_clickable` / `obscured` | 重新 snapshot；隐藏元素或 overlay 下的元素不能点 |
| `invalid_selector` | CSS 语法错；换 selector 或用 snapshot ref |
| `element_disabled` / `element_readonly` / `unsupported_control` | 换控件或先启用 |
| `input_prevented` | 页面接管了输入（cancel 了 `beforeinput`）；改用 `js` 或逐键 `press` |
| `selector_not_found` | `read --selector` 没匹配到；换 selector 或用 snapshot ref |
| `js_exception` | 页面 JS 抛错；看 hint，先 `read`/`snapshot` 再改代码 |
| `script_timeout` | `js` 超过 `--timeout`（默认 10s），已终止；简化表达式 |
| `wait_timeout` / `response_timeout` / `tool_timeout` | 检查条件或提高 timeout；`tool_timeout` 先关对话框/重载 tab |
| `tab_not_active` | 先 `ema2 tabs activate <id>`；`shot --tab` 会自动激活 |
| `debugger_attach_failed` | 先重载该 tab；`ema2 js` 会自动降级到 scripting（看 `via=scripting`） |
| `internal_page` | `edge://` 等内置页不可脚本化 |
| `download_dir_unsupported` | Edge 不允许扩展改下载目录，手动在设置里改 |

## 端口错配诊断

```text
ema2 status
# relay=running ... extension=disconnected
```

1. 点 Edge 工具栏的 EMA2 图标；
2. 端口必须与 `ema2 status` 里的 `port=` 相同（默认 7744）；
3. 保存后点“重新连接”；
4. 仍不连：`edge://extensions` 里重新加载扩展。

## 收尾

```bash
ema2 tabs close --session   # 关掉本会话由 open/tabs new 打开的标签
ema2 server stop            # 显式关闭 daemon
```
<!-- ema2:end -->
