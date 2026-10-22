> **归档文件 · 不代表当前行为。**
> 2026-10-22 的 EMA2 代码质量审查报告（P0-1、P1-1..P1-10 与 P2 若干）。
> 报告中的问题已在 v2.4.2.5 修复；现行行为见 [`../TOOLS.md`](../TOOLS.md)、
> [`../ARCHITECTURE.md`](../ARCHITECTURE.md) 与 [`../HANDOFF.md`](../HANDOFF.md)。
> 归档时间：v2.4.2.5；不要在这里补新结论。

# EMA2 代码质量审查报告

审查时间：2026-10-22  
审查范围：Rust 核心与 CLI（约 8.1k 行）、扩展 JS（约 6.9k 行）、脚本/契约/文档  
审查方式：静态通读 + 4 个分域子代理并行审查 + 关键结论在源码与真实二进制上二次复核  
代码版本：`0fffb54 feat: v2.4.2.4 read fallback and output hints`（工作区另有 `docs/README.md` 修改与未跟踪的 `docs/PROJECT_CONVENTIONS.md`）

---

## 一、结论摘要

**总体评级：B+（良好偏上）。工程质量明显高于同类"浏览器自动化 CLI"项目**：分层清晰、契约测试真正生效、错误码与退出码有体系、生产路径 panic 面极小。

**但存在 1 个 P0 级正确性缺陷**：`act --wait-for url=/status=` 在新标签页（网络 hook 尚未安装）时，会把动作发生**之前**的页面资源回填当作本次动作的响应，输出 `response=200 ...` 并退出 0。这是"成功但结果是假的"，比报错更危险。

**次要系统性风险**：

1. 文档是 AI 的主要输入，但已有 4 处"照抄即失败"的示例（`--cursor next`、`waitFor` 脚本引号、`invalid_path`、budget 默认值）。
2. `verify.py` 会复用已在运行的旧 daemon 且不校验扩展/daemon 版本，验收存在"对旧代码开绿灯"的路径。
3. 扩展侧没有 per-tab 串行化，并发命令会出现 ref 突然失效、`await` 等错标签页。
4. daemon 的 keepalive 无法识别半开连接，`status` 可能长期显示 `connected` 而每条命令都跑满超时。

---

## 二、基线结果（本次实测）

| 检查项 | 结果 |
|---|---|
| `cargo test` | **92 passed / 0 failed** |
| `cargo clippy --all-targets -- -D warnings` | 通过，零告警 |
| `cargo fmt --check` | 通过 |
| `node --check`（全部扩展 JS） | 全部通过 |
| `python -m py_compile`（scripts/*.py） | 通过 |
| `verify.py --skip-real-edge` | 8 项静态断言全 PASS（56 个错误码、17 个模块、62 个链接） |
| 未跟踪/误提交构建产物 | 无；`bin/`、`target/`、`*.log` 均已忽略 |

**结论：自动门禁是可信的，本次发现的问题都不在现有门禁的覆盖范围内。**

---

## 三、P0：会导致"静默假成功"

### P0-1 `act --wait-for` 会把动作前的历史请求当作本次响应

**位置**：`extension/modules/act.js:415`、`:662-675`、`:708-712`；`extension/modules/hooks.js:108-115`

**证据链**：

1. 动作前取基线（`act.js:415`）：
   ```js
   lastActionNetBaseline = await networkMark(tab.id);
   ```
2. 若该标签页从未执行过 `observe network` / `waitFor`，网络 hook 不存在，`networkMark` 返回空 epoch（`hooks.js:253-261`），且**不抛错**。
3. 动作后 `waitForResponse` 先 `ensureNetworkHook` 安装 hook；安装时用 `performance.getEntriesByType('resource')` 回填最近 100 条历史记录，`preview:'history'`、`backfilled:true`，但 **status 取 `responseStatus`，通常为正数**（`hooks.js:108-115`）。
4. 回到 `act.js:666`，基线 epoch `''` 与新 epoch `n-xxx` 不匹配 → `since = 0` → 从文档起点开始匹配。
5. 扫描循环只跳过 `entry.status < 0`（`act.js:708-712`），**不跳过 `backfilled`**。于是动作前就已完成的 `GET /api/save 200` 立刻命中，输出：
   ```
   response=200 GET https://.../api/save 12ms
   ```

**影响**：AI 认为提交成功并结束任务，实际点击可能没有发出任何请求。退出码 0、输出形态与真成功完全一致，属于最难发现的一类错误。

**修复建议**（三条同时做）：

1. 基线 epoch 为空时按"无基线"处理：`since = current.seq`，宁可漏报也不匹配历史；
2. 扫描时跳过 `entry.backfilled === true || entry.preview === 'history'`，除非 `entry.startedAt >= actionStartedAt`；
3. 把 `actionStartedAt` 随 baseline 一起下传，无法建立基线时在返回行加 `baseline_lost=1`，让调用方可判别。

**回归测试**：新建标签页 → 先加载一个含 `/api/ping` 资源的页面 → 执行 `act click <无请求按钮> --wait-for 'url=/api/ping'`，断言必须在 timeout 后返回 `wait_timeout`，而不是 `response=200`。

---

## 四、P1：真实缺陷，有明确触发条件

### P1-1 `page_guard` 把原生 API 暴露给页面，且 disarm 只信页面里的布尔值

**位置**：`extension/page_guard.js:16-17`、`:42-49`、`:248`；`extension/modules/guard.js:128-138`、`:214`

**问题**：

- `state.original` 保存了原生 `alert/confirm/prompt/print/open`，整个 `state` 又挂到 `window.__ema2_guard`（`page_guard.js:248`）。页面脚本可直接 `window.alert = window.__ema2_guard.original.alert` 解除包装。
- 页面可在注入前预置 `window.__ema2_guard = {version: 99, armed: false}`，命中 `page_guard.js:17` 的提前返回，guard 永远不安装。
- `runGuardDisarm` 只读 `guard.armed !== true` 就报告 `disarmed: true`（`guard.js:138`），随后把 tab 从 `commandGuardTabs` 移除。页面伪造 `armed:false` 即可让扩展**永久放弃**该 tab 的清理。

**影响**：页面一旦调用 `alert()`，渲染进程同步阻塞，该 tab 上所有 `executeScript` 挂起 → 每条命令白等约 70s 变 `tool_timeout`，且扩展误判"已 disarm"，形成不可恢复的坏 tab。

**修复建议**：

1. `original` 留在 IIFE 闭包，不放进暴露对象；
2. `arm(lease)` 写入一个每次命令随机的 nonce，disarm 必须回显该 nonce 才算成功，页面伪造的布尔值无法通过；
3. 校验失败返回 `guard_unverified`，不要把 tab 移出跟踪集合。

### P1-2 脚本里的 `waitFor` 不能表达含空格的值（已实测复现）

**位置**：`rust/src/cli/act.rs:470-472`

```rust
if action == "waitFor" {
    let mut parsed = parse_wait_for(&tokens[1..].join(" "))
```

`tokenize_line` 已把 `selector="#main a"` 解析成单个 token `selector=#main a`，这里再 `join(" ")` 后二次分词，值被空格撕开。

**实测**：

```
$ ema2 act --steps 'waitFor selector="#main a"'
error=invalid_argument msg=script line 1: act waitFor: expected key=value, got "a"
```

`ema2 act --help` 与 `docs/TOOLS.md:485` 都承诺"含空格的值加双引号"，但该承诺对 `waitFor` 不成立。后代选择器、多词中文文本（如 `text="保存 成功"`）在脚本里无法表达。

**修复**：`parse_wait_for` 改为接收 `&[String]`，直接 `split_once('=')`；`--wait-for` 字符串入口先 `tokenize_line` 一次，两路复用同一函数。

### P1-3 `ref` 与 `selector=`/`target=` 可同时存在，且静默以 ref 为准（已实测复现）

**位置**：`rust/src/cli/act.rs:508-540`（脚本）、`:91-133`（单步）；扩展侧 `extension/modules/actions.js:213`

**实测**：

```
$ ema2 act --steps 'click selector=#x e1'      # 通过，payload 同时含 selector 与 ref
$ ema2 act click e12 --selector '#x' --target 'button:y'   # 通过
```

实现没有互斥校验，扩展 `helpers.find(ref, selector, target)` 是 ref 优先，selector 被静默忽略。`ema2 act --help:148` 明确写"不能和裸 ref 同时用"。

**影响**：多写一个 ref 就会点到 ref 指向的元素，可能是"删除/提交"按钮；ref 跨导航失效时还会报 `stale_ref`，而不是回退到 selector。

**修复**：单步与脚本收敛到同一个 `ensure_single_locator(&step)`，三者计数 >1 即报错；同时修正 `rest.len() > 1` 的报错文案（当前一律提示"加引号"，会误导）。

### P1-4 daemon 无法识别半开连接，`status` 可能长期"假在线"

**位置**：`rust/src/state.rs:105-107`、`:195-217`；`rust/src/relay.rs:186-194`；`rust/src/server.rs:328`

**问题**：

- `extension_connected()` 只判断 `ext` 是否为 `Some`；
- `send_keepalive()` 只在 `mpsc` 通道已关闭时返回 `false`，而通道是无界内存队列，relay 任务活着就永远 `send` 成功；
- 返回值在 `relay.rs:192` 被丢弃；
- `last_ping_secs()` 只用于展示，没有任何健康判定使用它。

**影响**：浏览器崩溃但 TCP 未发 FIN、笔记本休眠、防火墙丢包时，`ext` 永远 `Some`。每条命令都要等满工具超时（`batch` 最长 605s），`status` 却显示 `connected last_ping=900s`，只能重启 daemon。

**修复**：keepalive 任务增加 janitor：`last_pong.elapsed() > 3 * interval` 时 `ext.take()` + `pending.clear()`；`sink.send` 加 5s 超时，超时即 `break` 并注销 generation。

### P1-5 `server stop` 以"端口关闭"判定"进程退出"，PID 兜底会提前失效

**位置**：`rust/src/main.rs:829-843`；`rust/src/server.rs:64-75`

`axum` 的 graceful shutdown 先 `drop(listener)`，再等待在途连接结束；hyper 的 `graceful_shutdown()` 只禁用 keep-alive，**当前请求会跑完**。因此若此刻有 `batch`/`js` 在等扩展：

1. 端口立刻不再监听，`wait_until_down` 返回 `true`；
2. CLI 打印 `relay=stopped` 并删除 PID 文件；
3. 旧 daemon 仍持有扩展 WS，最长再活 600s；
4. 紧接着 `ema2 server` 会启动**第二个 daemon**，扩展连不上新 daemon，所有命令 503；
5. PID 文件已删，强杀兜底也失效。

**修复**：`/shutdown` 用 `select!` 打断在途 `call_extension`（返回 503 `shutting_down`）；graceful 后再加 3s 硬期限 `process::exit(0)`；`wait_until_down` 同时校验 PID 是否消失，只有进程真退出才删 PID 文件。

**备注**：该结论来自 hyper/axum 源码语义与代码路径分析，建议按"`batch` 执行中 `server stop` + `server`"实测复现一次。

### P1-6 扩展没有 per-tab 串行化，`lastAct` 是全局单槽

**位置**：`extension/modules/act.js:33-51`、`extension/modules/page.js:81-92`、`extension/modules/core.js:33,80`

全仓只有 `debuggerQueue`、`commandGuardLifecycle`、`sessionWriteChain`、`lastActWrite`、`docSeqReady` 五个串行点，**没有 per-tab 锁**。两条 RPC 操作同一 tab 时会交错：

- `act` 的 settle 轮询期间另一条命令导航，ref 全部 `stale_ref`；
- 两个并发 `ensureHelpers` 各取一个文档句柄（`d5`、`d6`），后写覆盖先写；
- `saveLastAct` 用 `updatedAt` 主动丢弃较旧 tab 的上下文，`await` 可能落到另一条命令的 tab，输出 `await=done tab=<别的 tab>`。

**修复**：`executeTool` 外层对带 `tabId` 的工具做 `queuePerTab(tabId, fn)`；`LAST_ACT_KEY` 改为 `ema2_last_act:<tabId>`；把"取号 + 写句柄"合成一次注入并回读校验。

### P1-7 `withDebugger` 在 attach 超时后存在 detach 竞态

**位置**：`extension/modules/debugger.js:118-143`、`:155-158`

`withDeadline(chrome.debugger.attach(...))` 超时只是**放弃等待**，Promise 仍在飞；`break` 后 `finally` 立刻 `detach()`。若 detach 先落地、attach 后落地，tab 保持附着，且没有代码再释放它。之后该 tab 的所有 `js/press/type` 都报 `debugger_attach_failed`，直到 SW 回收或扩展重载。

**修复**：保存 attach Promise，`finally` 里 `await Promise.race([attachP, sleep(3000)])` 后再 detach；或 attach 超时后轮询 `getTargets()` 直到 `attached===true` 再有界重试 detach。

### P1-8 `verify.py` 会复用旧 daemon 且不校验版本，存在"假绿"

**位置**：`scripts/verify.py:536`、`:562`、`:591`

- 只要 `status` 含 `relay=running` 就复用现有 daemon；
- 连接检查只匹配 `extension=connected`，丢弃 `version=`；
- `binary exposes product version` 断言硬编码 `"2.4.2.4"`。

**影响**：改完 `extension/` 忘记在 `edge://extensions` 重新加载（HANDOFF 自己列的"开发陷阱 #2"），或旧 daemon 仍驻留时，验收会对着旧代码跑出绿灯；版本升级后又会误报红灯。

**修复**：从 `contract/runtime.json` 读 `productVersion`，用 `/health` 断言 daemon 与扩展版本；`already_running` 且版本不符时先 `server stop` 重启或直接 exit 2；删除硬编码。

### P1-9 `verify.py` 的清理不在 `finally`，失败会留下标签页

**位置**：`scripts/verify.py:1475-1481`

`tabs close --session` 位于 `try` 末尾、所有断言之后；任一步 `subprocess.run(timeout=90)` 超时或 Ctrl+C，清理被跳过。一次失败会在用户浏览器里留下十几到二十个标签，污染下一次运行。

**修复**：把 `tabs close --session` 放到 `finally` 第一行（best-effort、独立 timeout=20），再停 server；同时记录本次 `open`/`tabs new` 返回的 id 作为二次兜底。

### P1-10 `reloadextension.py` 多窗口时丢失命中窗口，可能在错误窗口点击

**位置**：`scripts/reloadextension.py:226-227`

`find_reload_button` 返回 `(edge, card, button, label)`，`main` 却写：

```python
card, button, label = found[1], found[2], found[3]
```

丢弃了 `found[0]`，改用 `ensure_edge_window` 挑出的"面积最大窗口"去操作**另一个窗口**里的按钮。有两个 Edge 窗口或两个 profile 时，最大化的是 A，按钮属于 B；`click_input` 退化路径会点空或误点，但脚本仍打印 `reloaded=1`。

**修复**：改为 `edge, card, button, label = found` 并只用该 `edge`；找不到时提示"确认扩展加载在哪个窗口/profile"。

---

## 五、P2：健壮性与可维护性

### 5.1 Rust 核心

| 问题 | 位置 | 建议 |
|---|---|---|
| `call_extension` 的所有失败统一映射 504 `extension_error`，包括"未连接/连接丢失"，CLI 退出码因此为 4 而非 3 | `server.rs:342-351` | 区分 `NotConnected/Lost/Timeout`，前两者 503 |
| 到扩展的 `mpsc` 无界，`sink.send` 串行阻塞时消息只在内存排队 | `relay.rs:45`、`:91-98` | 改 `channel(64)` + `try_send`，`Full` 视为连接拥塞 |
| 任何带 `id` 的消息都当响应，会吞掉扩展主动发来的 RPC 请求 | `relay.rs:125-133` | 先判 `method`，仅 `method==null && (result|error)` 走响应路径 |
| `save_*` 用 `tokio::fs::write` 直接覆盖，非原子；并发同路径会交错，崩溃会毁掉原文件 | `save.rs:216` | 复用 `main.rs::write_atomic` 的临时文件 + `sync_all` + rename |
| `--out ./shots/` 或 `--out .` 被 `with_extension` 当成文件名，静默写到相邻路径 | `save.rs:113-116` | 目标 `is_dir()` 时拼接默认文件名；非 png 后缀明确告知 |
| 错误码漂移：daemon 对未知工具/非对象 args 返回 `invalid_argument`，文档与扩展是 `unknown_tool`/`invalid_arguments`；`invalid_path` 只存在于文档 | `server.rs:303-317`；`docs/TOOLS.md:78/84/87` | 统一 code；把反向扫描扩展到 `server.rs` |
| PID 强杀只校验镜像名，Windows 用 `==`、unix 用 `contains`，PID 复用可能误杀另一端口实例 | `main.rs:907-950` | PID 文件记录 port/started_at，强杀前校验命令行 `--port` |
| CLI 客户端 `read_to_end` 无上限，`SO_RCVTIMEO` 是单次 read 超时而非总预算 | `client.rs:52-109` | 总 deadline + 累计字节上限；校验对端 `name=ema2` |
| `start_server` 12s 超时后不终止自己 spawn 的 daemon | `main.rs:237-265` | 超时分支先 kill，再打印日志尾部 |

### 5.2 CLI / 工具层

| 问题 | 位置 | 建议 |
|---|---|---|
| `act` 不支持 `--flag=value`；`--help` 写在位置参数后会被当未知 flag | `act.rs:91-133` | 复用 batch 的 `split_flag`；入口显式识别 `--help` |
| 重复 flag 静默 last-wins（`--tab 1 --tab 2`） | `act.rs:92-120` | `Option` 已存在时报 `given twice` |
| `waitFor` 的 CLI 校验与 daemon 不同步（timeout 60s 放行、空 `url=` 放行） | `act.rs:397-415` vs `tools.rs:589-603` | 复用同一校验函数，拒绝空值 |
| `--delta-y nan` 通过（变成 `null`）；`scroll down 1e30` 饱和成 `u64::MAX` | `act.rs:96-101`、`:354-361`、`:616-624` | 加 `is_finite()`；count 用 `u64` 解析并设上限 |
| 脚本行尾 `#` 注释报错；UTF-8 BOM 变成动作名一部分 | `act.rs:456-469` | 剥离 BOM；按 quote 状态截断行尾注释 |
| `--steps` 20 步 + `--wait-for` 被误报超过 20 步 | `act.rs:183-199` | 先判脚本 1..20，再判注入后 ≤21 |
| `--urls` 用逗号分隔且无转义，URL 自带逗号被静默拆分 | `batch.rs:107-116` | 支持重复 `--urls`，或在报错中明确指出逗号 |

### 5.3 扩展

| 问题 | 位置 | 建议 |
|---|---|---|
| `observe` 未知 `kind` 无 `default` 分支，返回空内容 + 退出码 0 | `observe.js:74-247` | 末尾 `fail('invalid_argument', ...)` |
| helpers 缺失时 `snapshot` 报 `snapshot_failed`，应为 `helpers_missing` | `read_snapshot.js:239-246` | 闭包内先判 `window.__ema2_helpers` |
| helper 版本号有两份字面量，契约测试 `rfind` 只校验一处 | `injected_helpers.js:7`、`:1213` | 单一 `const HELPERS_VERSION`，测试断言只出现一次 |
| `snapshot` 的 `href` 无长度上限，分页首行无条件收下，可突破 budget | `injected_helpers.js:91-93`、`:344-349` | href 截断到 2048；首行也受 budget 约束 |
| `refs` Map 无上限，长驻 SPA 反复 snapshot 会积累上千条目 | `injected_helpers.js:190-228` | 加软上限（如 5000）并批量失效旧 ref |
| `console_errors` 统计当前文档全部错误，不是本次 act 之后 | `act.js:596`、`:802-809` | 动作前取 `__ema2_console_seq` 水位 |
| `press/type` 在 `debugger_timeout` 时降级到合成事件，可能二次派发 | `actions.js:608-618` | 超时是"结果未知"，不要重放；回读 `activeElement` 判断 |
| `Runtime.terminateExecution` 会打断页面自己的脚本，文档未提示 | `debugger.js:66-69` | 错误 hint 明确说明；优先 `releaseObjectGroup` |
| `relayReady` 未就绪或消息超限时静默丢弃，不回 JSON-RPC 错误 | `core.js:300`、`:313` | 回 `-32000` / `payload_too_large` |
| 截图标注在 SW 内同时持有 4-5 份大对象，峰值可达数百 MB | `files_shot.js:113-141` | 显式 `bitmap.close()`；annotate 输出设字节上限 |
| command guard 按全站 http(s) 注册，长命令期间会吞掉无关标签的对话框 | `core.js:43-45` | 按目标 tab 的 origin 动态生成 matches |

### 5.4 脚本、契约与文档一致性

| 问题 | 位置 | 建议 |
|---|---|---|
| `--cursor next` / `--since next` 是文档示例，但 CLI 只接受数字，实测 exit 1 | `AGENTS.md:141`；`docs/TOOLS.md:686-687`；`examples/README.md:75` | 改为 `--cursor <上一条 next 的数值>`，或让 CLI 接受 `next` |
| 文档统一写 budget 默认 4000，实际 `js` 8000、`observe` 3000 | `docs/TOOLS.md:38`；`AGENTS.md:69`；`spec.rs:203/243` | 文档注明分工具默认值 |
| 测试计数过期：文档写 87 / 135，实际 92 / 141 | `docs/HANDOFF.md:103/278`；`docs/TESTING.md:31/37`；`docs/README.md:57` | 改为引用命令输出，不复制数字 |
| "分发包只带 exe + AGENTS.md"与实际不符（扩展是必需交付物）；`bin/AGENTS.md` 是旧模板 | `AGENTS.md:39`；`docs/TOOLS.md:6`；`bin/AGENTS.md` | 文案补 `extension/`；`setup.cmd` 同步复制最新 AGENTS.md |
| `examples/README.md` 的 `waitFor` 仍写"只接受 url=/status=/timeout=" | `examples/README.md:30` | 补 `text=/textGone=/selector=` |
| 契约只锁定 19 个超时键中的 8 个；`jsDefaultSecs` 等 9 个键无断言 | `rust/src/tools.rs:1856-1925` | 全部纳入断言，或用 build.rs 从 JSON 生成常量 |
| `registry_is_compact_and_complete` 对 `contains()` 的断言恒真 | `tools.rs:974-983` | 改为断言 `clap` 子命令集合 == `TOOL_NAMES` |
| `relay.rs` 零测试；CLI 大量 `.is_err()` 只证明"失败"不证明"原因" | `relay.rs` 全文；`act.rs:773-778` | 补握手/超时/断连用例；关键负例断言错误文案 |
| CHANGELOG 未记录 `reloadextension.py` 的提交 | `docs/CHANGELOG.md` | 在 v2.4.2.3/2.4.2.4 补一条 |
| `docs/README.md` 已索引 `PROJECT_CONVENTIONS.md`，但后者未跟踪 | `docs/README.md:13/26` | 两个文件同一提交 |

---

## 六、做得好的地方

1. **pending 生命周期闭环**：`PendingGuard` + generation 校验覆盖正常完成、超时、CLI 断开、扩展重连四条清理路径，并有"drop 掉 Call 不泄漏"的单测。
2. **崩溃面控制**：生产代码仅 4 处 `expect`，均位于不可能失败处；`panic=unwind` + `state::lock` 吞中毒配套完整。
3. **安全分层清晰**：Host 必须回环、`/cmd` 强制无 Origin + `X-EMA2-Client`、WS hello 的 `extensionId` 与 Origin 逐字相等，并有"本机网页 Origin 也拒绝"的测试。
4. **契约机械化**：`contract.js` 由脚本生成，版本、读模式、错误码、watchdog 与预算关系都有测试钉住；本次实测 56 个错误码 0 缺失。
5. **`ema2 init` 的写入安全**：只把 `NotFound` 当空文件、同目录临时文件 + `sync_all` + 原子 rename + 重试，4 个单测覆盖。
6. **前端无 XSS**：`popup.js` 全程 `textContent`，无 `innerHTML`/`eval`；`manifest` 权限基本对得上用途。
7. **输出契约细节**：`emit_stdout` 吞 EPIPE；截图/HAR 只回 `path=`，不把 base64 推给模型。

---

## 七、建议的修复顺序

**第一批（正确性，建议立即处理）**

1. P0-1：`waitFor` 历史回填误匹配（附回归测试）。
2. P1-2 / P1-3：脚本 `waitFor` 引号、定位参数互斥（两处都已被实测复现，修复成本低）。
3. P1-1：page_guard 不再暴露 `original`，disarm 加 nonce。
4. P1-4 / P1-5：keepalive 健康判定、`server stop` 真正等待进程退出。

**第二批（并发与验收可信度）**

5. P1-6：per-tab 串行化 + `lastAct` 按 tab 分槽。
6. P1-7：debugger attach 超时后的 detach 竞态。
7. P1-8 / P1-9 / P1-10：`verify.py` 版本校验与 finally 清理；`reloadextension.py` 使用命中窗口。

**第三批（契约与文档）**

8. 统一错误码（`unknown_tool`/`invalid_arguments`/`invalid_path`）。
9. 修 `--cursor next`、budget 默认值、测试计数、分发说明、examples。
10. 扩大契约断言（9 个未锁超时键、payload 字段名）与 relay 测试。

---

## 八、复核记录

**已在真实二进制上复现**：

- `ema2 act --steps 'waitFor selector="#main a"'` → `script line 1: ... got "a"`，exit 1。
- `ema2 act --steps 'click selector=#x e1'` → 通过解析，进入 daemon 阶段（exit 3，扩展未连），证明参数被接受。
- `ema2 act click e1 --tab 1 --tab 2` → 同样通过解析。
- `ema2 read --cursor next` → `invalid value 'next' for '--cursor <N>'`，exit 1。

**已由源码路径确认**：

- P0-1 的 epoch 不匹配 → `since=0` → 回填条目可命中（`act.js`、`hooks.js`）。
- `page_guard` 的 `original` 暴露与 `armed` 判定（`page_guard.js`、`guard.js`）。
- keepalive 的 `send()` 恒成功、`last_ping` 无判定（`state.rs`、`relay.rs`）。
- `server stop` 的端口判定与 graceful 语义（`main.rs`、`server.rs`）。
- `verify.py` 的硬编码版本与 `try` 内清理（`verify.py`）。

**建议实测确认（未下结论）**：

- `attachByTargetId` 用 `targetId` 附着后，后续 `sendCommand({tabId})` 是否被 Chrome 接受。若报 "Debugger is not attached to the tab with id"，则该救援路径实际不可用，应升级为 P1。
- P1-5 的 `server stop` 双实例场景，建议按"`batch` 执行中 stop + start"复现一次并观察 `tasklist`。
