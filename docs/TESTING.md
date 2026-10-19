# EMA2 测试说明（v1.8.0）

本文记录当前自动化测试分层、覆盖矩阵、运行命令和真实 Edge/MV3 验收结果。
测试目标是保护 2026-10-19 代码审查的 F01–F13 修复；主流程已在隔离 profile 的真实 Edge 上跑通，
未覆盖的边界在文末单独列出。

## 1. 测试分层

| 层 | 工具 | 数量 | 覆盖范围 |
|---|---|---:|---|
| Rust 单元/路由测试 | `cargo test` | 49 | 工具 schema/参数校验、超时预算、batch 递归校验、pending 代际隔离、token、SSE 清理、截图落盘、HTTP 路由 |
| 扩展回归 | Node VM + jsdom | 16 | F01–F03、F05–F07、F09–F13 |
| 脚本回归 | Node + PowerShell | 3 | F04 精确端口/进程身份、陌生监听者保护、脚本静态检查 |
| 真实 Edge/MV3 验收 | Node + 隔离 Edge profile | 16 场景 | tabs/open/snapshot、F01–F13 主流程、files、shot、bridge 重启重连 |
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

# 真实 Edge/MV3 验收（需要本机 Edge；约 2 分钟）
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

当前 `bin/ema2.exe` 已由同一源码构建为 v1.8.0；重新构建后建议用
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

## 4. 真实 Edge/MV3 验收

`tests/integration/edge-acceptance.cjs` 使用**独立临时 user-data-dir** 启动真实 Edge，
加载 `extension/` 解包扩展，并连接一个临时端口的 `bin/ema2.exe`。它不会附着到用户日常
Edge profile，也不会使用用户登录态。

已执行环境：

```text
Microsoft Edge 155.0.4283.62
EMA2 relay 1.8.0
Windows, 可见隔离窗口（用于 captureVisibleTab）
```

16 个场景全部通过：

1. `open` / `snapshot` / `doc=` ref
2. F02 密码脱敏
3. F09 Markdown 混合文本
4. F10 disabled/readonly/unsupported/动态移除
5. F13 press 定位目标与活动焦点
6. F01 真实流式 fetch 不被 preview 阻塞（headers ≈ 3ms）
7. F07 network 100 条分页 + HAR 落盘
8. F07 console 120 条分页
9. F03 导航 / 同 URL reload / 跨 tab stale ref
10. F11 跨导航 `waitFor` 命中新文档 `/api/save`
11. F12 batch 内层 `time=3600` 拒绝
12. F08 8 tab × 4s、concurrency=1 分波执行（32s，旧预算会提前失败）
13. F05 永不 resolve 的 JS 超时释放 debugger，下一次 `js` 成功
14. F06 后台 tab 截图拒绝，活动 tab 截图 + annotate 成功
15. files 真实 `DOM.setFileInputFiles` 上传
16. F04 真实 `bridge.ps1` status/stop/start + 扩展重连

仍有必要继续做实机验证的边界：

- 同源 iframe / open shadow root 的定位与 iframe marks 坐标偏移
- DevTools 抢占 debugger 的交互路径
- 多个 MCP 客户端同时调用
- 大文件下载、超慢 body、跨域 iframe
- 下载目录设置与不同 Edge 版本差异
- 候选包哈希、安装目录与长期运行的版本一致性

## 5. 当前已执行结果（2026-10-19）

```text
cargo test --locked --offline                         49 passed
cargo clippy --locked --all-targets -- -D warnings    passed
cargo fmt --check                                     passed
node --test tests/*.test.cjs                          19 passed
powershell scripts/bridge.ps1 -SelfTest               passed
foreign listener stop/start protection test           passed
cargo build --release --locked --offline              passed
真实 Edge/MV3 验收                                      16/16 scenarios passed
```

发布候选：

```text
rust/target/release/ema2.exe
sha256=af145c1d29e346e33565eff8cabcac4d13f33284697922c36b7b070c3c32fee5
version=1.8.0
```
