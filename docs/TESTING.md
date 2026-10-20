# EMA2 测试说明（v1.12.1）

本文记录当前自动化测试分层、覆盖矩阵、运行命令和真实 Edge/MV3 验收结果。
测试目标是保护 2026-10-19 代码审查的 F01–F13 修复、2026-10-20 的 P0/P1/P2/P3 加固，以及 v1.10 的配置合并/多终端/安装卸载、v1.11 的标签复用/真实状态码/截断续读、v1.12 的快照瘦身与句柄分配；
主流程已在隔离 profile 的真实 Edge 上跑通，
未覆盖的边界在文末单独列出。

## 1. 测试分层

| 层 | 工具 | 数量 | 覆盖范围 |
|---|---|---:|---|
| Rust 单元/路由测试 | `cargo test` | 98 | 工具 schema/参数校验、超时预算、batch 递归校验、pending 代际隔离、token 必需性、SSE 投递/替换/清理、锁中毒、JSONC/TOML 合并、备份、取消勾选、MCP 元数据、截图落盘、HTTP 路由 |
| 扩展回归 | Node VM + jsdom | 42 | F01–F03、F05–F07、F09–F13、P2-1、P2-4、配对/HMAC、终端选择、导航落点、快照瘦身、句柄并发 |
| 脚本回归 | Node + PowerShell | 3 | F04 精确端口/进程身份、陌生监听者保护、脚本静态检查 |
| 真实 Edge/MV3 验收 | Node + 隔离 Edge profile + 隔离 HOME | 22 场景 | tabs/open/snapshot、F01–F13 主流程、files、shot、bridge 重启重连、SSE 通知、token 网关、streaming 观测、五终端真实合并、install 识别已加载扩展 |
| 静态检查 | `cargo fmt` / `cargo clippy` | — | Rust 格式与 `-D warnings` |

Node 回归使用 `node:test`，唯一 devDependency 是 `jsdom`。测试通过 VM 加载真实
`extension/background.js`（截掉自动连接入口），并在 jsdom 页面上下文中执行真实的
`installHelpers` / `performActionInPage` / `installNetworkHook` 等函数。

## 2. 运行命令

```bash
# 安装测试依赖
npm install

# 扩展 + 脚本回归
npm run test:js

# 真实 Edge/MV3 验收（需要本机 Edge；约 3 分钟）
npm run test:edge
# 可选：EMA2_EDGE_SKIP_SLOW=1 跳过 60s 挂起 JS 场景
#       EMA2_EDGE_KEEP=1      失败时保留临时 profile / 日志
#       EMA2_EDGE_EXE=...      指定 Edge 路径

# 全部自动化测试（不含真实 Edge）
npm run test:all

# Rust 单独运行
cd rust
cargo test --locked --offline
cargo clippy --locked --offline --all-targets -- -D warnings
cargo fmt --check

# Windows 进程管理自检
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/bridge.ps1 -SelfTest
```

发布候选构建：

```bash
cd rust
cargo build --release --locked --offline
sha256sum target/release/ema2.exe
```

当前 `bin/ema2.exe` 已由同一源码构建为 v1.12.1；重新构建后建议用
`scripts\setup.cmd` 覆盖并记录 SHA-256。

## 3. 审查项覆盖矩阵

| 审查项 | 自动化验证 | 说明 |
|---|---|---|
| F01 网络 hook 非阻塞 | `F01 fetch hook...`、`F01 response preview...`、`F01 bodies=false...` | 永不结束的 clone reader 不阻塞页面；8KB/1.5s 上限；关闭 body 后不 clone |
| F02 敏感信息 | `F02 password values...` | password 名称、snapshot、candidates、marks、type 反馈均无虚构密码 |
| F03 ref 文档作用域 | `F03 refs are document-scoped...` | 同文档稳定；reload/跨 tab 旧 ref 失败；files 也拒绝 stale ref |
| F04 进程管理 | `F04 bridge scripts...`、self-test、foreign listener | 精确端口、进程路径校验、陌生监听者退出 2 且不被杀 |
| F05 debugger 恢复 | `F05 debugger queue...`、`F05 queued task...` | 挂起 Promise 超时后 terminate + detach，后续任务成功 |
| F06 截图目标 | `F06 shot rejects...` | 非活动 tab 拒绝；截图期间切 tab 失败 |
| F07 观测分页 | `F07 console pagination...`、`F07 observe resets...`、`F07 network pagination...` | next 按已交付记录；dropped/reset；100 条网络记录无 slice(-80) 丢失 |
| F08 超时预算 | Rust `timeout_budget` | wait 60/120/300；batch 8 tab 并发 1/2/4；超过 600s 拒绝 |
| F09 Markdown | `F09 markdown...` | 根直接文本、p/h1/pre、混合文本/块、列表 |
| F10 动作结果 | `F10 actions fail...` | disabled/readOnly/不支持控件失败；单次页面注入 |
| F11 跨导航 waitFor | `F11 waitForResponse...` | 新文档从 seq=0；`status=-1` 回填不作为成功证据 |
| F12 batch 参数 | Rust `batch_validation`、Node `F12 batch...` | 内层 validator 复用；time=3600 拒绝；扩展侧防御 |
| F13 press 目标 | `F13 press targets...` | 有 ref 时聚焦目标；无定位才用 activeElement |
| P0-1 默认鉴权 | Rust `token_is_required_by_default`、`explicit_anonymous_switch_is_required_for_no_token`、`anonymous_switch_parsing_is_strict`；Edge `P0-1 relay refuses...` | 无 token 且未显式允许时拒绝启动（真机进程退出码 2）；开关值严格解析；token 模式 401/200 |
| P0-2 锁/panic | Rust `lock_survives_a_poisoned_mutex` | 持锁线程 panic 后状态仍可用；release 使用 unwind |
| P1-1 SSE 投递 | Rust `event_stream_delivers_server_notifications`、`broadcast_reaches_every_session_stream`；Edge `P1-1 GET /mcp...` | 真机 WS 断开/重连时 GET /mcp 收到 disconnected + connected |
| P2-3 流替换 | Rust `re_registering_closes_the_superseded_stream`、`event_stream_cleanup_is_scoped_to_its_own_stream` | 同 session 重复注册只保留一条流，旧流立即关闭 |
| P2-1 bodyFilter | Node `P2-1 bodyFilter rejects...` | `(a+)+` / `(a|a)+` / 超长模式被拒；安全模式仍可匹配 |
| P2-4 pending 分页 | Node `P2-4 network observe keeps...`；Edge `P2-4 slow body...` | pending 之后的已就绪记录不被丢弃；真机慢响应标 `streaming=1` 且 `next` 前进 |
| 扩展图标 | Node `extension icons cover every manifest size...` | 16/48/128 图标存在、尺寸与 manifest 一致、RGBA 透明 |
| v1.11 标签复用 | Node `open reports the real document status...`、`open reuse=session...`、`open rejects conflicting...`；Rust `open_reuse_modes_are_exclusive` | 新开/`replace`/`reuse` 三选一；用户标签不进会话；状态码与 4xx hint |
| v1.11 会话收尾 | Node `tabs close scope=session...`、`tabs close tabIds[]...`；Rust `tabs_close_scopes` | 只关本会话标签；已消失的 id 跳过；非 close 动作传 tabIds/scope 报错 |
| v1.11 batch 复合 | Node `batch urls opens each url...`、`batch open accepts urls...`；Rust `batch_urls_open_and_composite` | `urls` 打开后执行目标工具；与 `tabIds` 互斥；open 成本计入预算 |
| v1.12 句柄唯一 | Node `two tabs installing at the same moment...`、`document handles resume from the persisted counter...` | 并发取号不重号；重载后续号而不是回到 d1 |
| v1.12 快照瘦身 | Node `refs carry a short document handle...`、`snapshot drops nameless containers...`、`each new document gets its own handle...`、`two tabs installing at the same moment...` | ref 短句柄、空节点丢弃、URL 截断、句柄唯一 |
| v1.11 导航落点 | Node `open waits for the new document...` | `tabs.update` 后旧文档仍 complete 时不提前返回，`url/title` 必须属于新文档 |
| v1.11 截断提示 | Node `read says explicitly that the text was truncated...` | 截断时给 `hint=truncated` + `cursor`；全文读完不给 |
| v1.11 js 语句体 | Node `js accepts a function body with top-level return...`、`js flags a body that produced no value...`、`js keeps a real runtime error...`、`js reports a syntax error after the body fallback...` | 表达式优先、语法错误回退 async 函数体；hint 区分语法/运行时；无返回值提示 |

## 4. 真实站点冒烟（用户自己的 Edge）

自动化测试跑在临时 profile 里，覆盖不到「用户真实登录态 + 真实站点」的水合、懒加载、
反爬和长 URL。`scripts/live-check.py` 补这一层：它直连正在运行的中继，对维基 / GitHub /
HN / 京东 / Bing 各跑一轮，断言快照结构、ref 句柄、act 解析、batch 复合与截断续读。

```bash
scripts\start-bridge.cmd          # 先确保中继在跑、扩展已连接
python scripts/live-check.py      # 59 项断言，约 2~3 分钟
```

- 只做只读动作（`hover` / `read` / `snapshot`），结束时 `tabs close scope=session` 关掉自己开的标签
- 令牌自动从中继状态文件读，无需参数；`EDGE_MCP_URL` 可指向别的端口

## 5. 真实 Edge/MV3 验收

`tests/integration/edge-acceptance.cjs` 使用**独立临时 user-data-dir** 启动真实 Edge，
加载 `extension/` 解包扩展，并连接一个临时端口、临时 `EDGE_MCP_STATE_DIR` 的 `bin/ema2.exe`。
中继以「待授权态」启动，扩展通过真实的 `pairWithRelay()` 完成配对后再跑后续场景。它不会附着到用户日常
Edge profile，也不会使用用户登录态。

已执行环境：

```text
Microsoft Edge 155.0.4283.62
EMA2 relay 1.11.0
Windows, 可见隔离窗口（用于 captureVisibleTab）
```

22 个场景全部通过：

1. `open` / `snapshot` / `doc=` ref
2. F02 密码脱敏
3. F09 Markdown 混合文本
4. F10 disabled/readonly/unsupported/动态移除
5. F13 press 定位目标与活动焦点
6. F01 真实流式 fetch 不被 preview 阻塞（headers ≈ 3ms）
7. P2-4 慢响应（3s 不发 body）被标为 `streaming=1` + `pending=<n> seqs=...`，水位继续前进
8. F07 network 100 条分页 + HAR 落盘
9. F07 console 120 条分页
10. F03 导航 / 同 URL reload / 跨 tab stale ref
11. F11 跨导航 `waitFor` 命中新文档 `/api/save`
12. F12 batch 内层 `time=3600` 拒绝
13. F08 8 tab × 4s、concurrency=1 分波执行（32s，旧预算会提前失败）
14. F05 永不 resolve 的 JS 超时释放 debugger，下一次 `js` 成功
15. F06 后台 tab 截图拒绝，活动 tab 截图 + annotate 成功
16. files 真实 `DOM.setFileInputFiles` 上传
17. P1-1 真实 WS 断开/重连时，`GET /mcp` 的 SSE 收到 disconnected + connected 通知，
   重连后工具仍可用
18. P0-1 token 模式对 `/mcp` 返回 401/200
19. F04 真实 `bridge.ps1` status/stop/start + 扩展重连
20. 配对：全新中继（无 token）→ 扩展真实 `pairWithRelay()` → token 落盘 →
    `?token=` 重连并通过 HMAC 验签 → `tools/call` 成功 → 授权页列出 5 个终端；
    隔离 HOME 里的 opencode `.jsonc`（含注释）、Claude Code、Codex TOML、Pi、Claude Desktop
    全部按真实格式写入且保留原有键
21. 取消勾选：只保留 opencode → Pi 的 `edge-browser` 条目被移除、`.bak` 刷新为移除前内容，
    opencode 条目保留，`/pair/status` 里 Pi 的 `selected=false`
22. install：从真实 Edge profile 的 `Preferences` 识别已加载的解包扩展，原地覆盖更新；
    扩展离线时说明“下次启动生效”，随后在 75s 内自动重连

> 隔离 profile 首次启动时，Edge 偶发不保留解包扩展的 service worker。runner 会在
> 45s 内没有可配置的 worker 时换一个全新 profile 重试一次（日志打印 `relaunching Edge`），
> 避免把浏览器环境抖动误判成产品缺陷。

仍有必要继续做实机验证的边界：

- 同源 iframe / open shadow root 的定位与 iframe marks 坐标偏移
- DevTools 抢占 debugger 的交互路径
- 多个 MCP 客户端同时调用
- 大文件下载、超慢 body、跨域 iframe
- 下载目录设置与不同 Edge 版本差异
- 候选包哈希、安装目录与长期运行的版本一致性

## 6. 当前已执行结果（2026-10-20）

```text
cargo test --locked --offline                         98 passed
cargo clippy --locked --all-targets -- -D warnings    passed
cargo fmt --check                                     passed
node --test tests/*.test.cjs                          45 passed
bridge.ps1 -SelfTest                                  passed
foreign listener stop/start protection test           passed
cargo build --release --locked --offline              passed
真实 Edge/MV3 验收                                      22/22 scenarios passed
```

发布候选：

```text
rust/target/release/ema2.exe
version=1.12.1
sha256=c28acee0f91351852d4e10db85d830a0f5690437a5524a00bec1c1789b3cc311（Windows release）
```
