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

下面是给 AI 的最短操作手册（`<!-- ema2:begin -->` 区块由 `ema2 init` 维护，勿手改）：

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

## 命令表

| 命令 | 用途 |
|---|---|
| `ema2 server` / `ema2 server stop` | 启动 / 关闭本机中继 daemon |
| `ema2 status` | 中继、扩展、标签页状态 |
| `ema2 init` | 在当前目录写/更新本文件 |
| `ema2 tabs [list\|current\|new\|close\|activate]` | 标签页 |
| `ema2 open <url> [--replace\|--reuse session\|--tab ID] [--timeout N]` | 打开 URL，回 `status=` / `net_hook=` / `focused=` |
| `ema2 read [--mode md\|text\|html] [--selector X] [--budget N] [--cursor N]` | 读页面正文 |
| `ema2 snapshot [--filter interactive\|full\|links\|buttons\|form]` | ref 快照 `[e12@d1] button "X"` |
| `ema2 act <action> ...` / `--steps` / `--script x.ema2` | click/type/fill/select/press/hover/scroll/submit/back/forward/reload |
| `ema2 await 10s` | 等上一个 act 动作的导航/页面稳定 |
| `ema2 wait <text\|textGone\|selector\|time\|idle> ...` | 等页面条件 |
| `ema2 js '<code>' [--file f.js]` | 页面主世界执行 JS |
| `ema2 observe console\|network ... [--har x.har]` | console / network 增量 |
| `ema2 files upload <path...> --ref R` | 上传本地文件 |
| `ema2 shot [--annotate] [--format png\|jpeg] [--out path]` | 截图落盘 |
| `ema2 batch <open\|read\|snapshot\|js\|wait> --urls "u1,u2"` | 多标签并发 |

常用细节：

- 输出是行式紧凑文本；HAR 是唯一 JSON。`path=` 单独一行。
- `ref` 形如 `e12@d1`；导航/刷新后旧完整 ref 报 `stale_ref`，重新 `snapshot`。
  手敲可用短 ref `e12`（当前文档）；要严格防止误点就用 snapshot 原样返回的完整 ref。
- `type` 追加到现有值，`fill` 覆盖整个值；`select` 的位置参数是 option 的 `value`。
- `read` 默认清洁正文（跳过脚本/隐藏元素）；main 太短会回退 `main=body(fallback)` 并给 hint。
  `--mode md` 是有损渲染：标题/列表/表格/链接保留，样式与合并单元格会丢。
- `act` 默认最多 settle 3s；返回 `settle=pending` 不是失败，接 `ema2 await 10s`。
  点了链接但没导航会给 `nav=none`（`<a>` 只自动重试一次，输出 `retried=1`）。
  `.ema2` 脚本里写 `await 10s`；pending 且下一步不是 await 时脚本会停止。
- 列表尾部有 `total/shown/next`；`next=none` 表示读完，否则把 `next` 传给 `--cursor`/`--since`。
- `click`/`press` 是合成事件：能触发页面的 `focusin`/`pointerdown`，但 `event.isTrusted=false`；
  浏览器级快捷键（`chrome.commands`）收不到。
- **窗口必须在前台**：`shot` 还要求目标 tab 是活动 tab；`open` 输出 `focused=0` 时先 `tabs activate <id>`。
- `files upload` 单次 ≤20 个文件、≤5 MiB，走页面 `DataTransfer`，不依赖 debugger。
- 默认端口 **7744**；`--port N` 时 server 与所有命令必须一致。
- 批量脚本后缀 `.ema2`，每行一条动作，`waitFor` 独立成行。

## 错误码怎么办

| code | 处理 |
|---|---|
| `relay_not_running` | 运行 `ema2 server` |
| `extension_not_connected` | 打开 Edge 扩展 popup，端口填 7744，保存并重连；确认扩展已加载 |
| `port_in_use` | `ema2 server stop`，或换 `--port`（扩展端口也要改） |
| `invalid_argument` | 看 `ema2 <cmd> --help` / `docs/TOOLS.md` |
| `stale_ref` / `ref_not_found` | 重新 `ema2 snapshot` |
| `target_not_found` | 用 snapshot 的 ref，或 `--target "button:提交"` |
| `element_disabled` / `element_readonly` / `unsupported_control` | 换控件或先启用 |
| `input_prevented` | 页面接管了输入（cancel 了 `beforeinput`）；改用 `js` 或逐键 `press` |
| `selector_not_found` | `read --selector` 没匹配到；换 selector 或用 snapshot ref |
| `js_exception` | 页面 JS 抛错；看 hint，先 `read`/`snapshot` 再改代码 |
| `wait_timeout` / `response_timeout` | 检查条件或提高 timeout |
| `tab_not_active` | 先 `ema2 tabs activate <id>`；截图只截活动页 |
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
