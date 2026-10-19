# EMA2 测试说明（v1.8.0）

本文记录当前自动化测试分层、覆盖矩阵、运行命令和尚未完成的真实浏览器验收边界。
测试目标是保护 2026-10-19 代码审查的 F01–F13 修复，而不是宣称所有浏览器行为都已实机验证。

## 1. 测试分层

| 层 | 工具 | 数量 | 覆盖范围 |
|---|---|---:|---|
| Rust 单元/路由测试 | `cargo test` | 49 | 工具 schema/参数校验、超时预算、batch 递归校验、pending 代际隔离、token、SSE 清理、截图落盘、HTTP 路由 |
| 扩展回归 | Node VM + jsdom | 16 | F01–F03、F05–F07、F09–F13 |
| 脚本回归 | Node + PowerShell | 3 | F04 精确端口/进程身份、陌生监听者保护、脚本静态检查 |
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

# 全部自动化测试
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

## 4. 尚未完成的真实浏览器验收

以下项目需要独立 Edge profile、无账号测试页和人工/实机验证，当前自动化测试不能替代：

- MV3 service worker 生命周期、重连与 watchdog 真实时序
- 真实流式响应、慢 body、大文件下载对页面 fetch 的影响
- DevTools 抢占 debugger、真实 `Runtime.terminateExecution` 行为
- 同源 iframe / open shadow root 的定位、iframe marks 坐标偏移
- 8 tab 分波 `wait`、多 MCP 客户端并发
- 真实 Edge 的 `captureVisibleTab` 活动页/窗口焦点竞态
- 候选包哈希、manifest 版本与安装目录一致性

## 5. 当前已执行结果（2026-10-19）

```text
cargo test --locked --offline                         49 passed
cargo clippy --locked --all-targets -- -D warnings    passed
cargo fmt --check                                     passed
node --test tests/*.test.cjs                          19 passed
powershell scripts/bridge.ps1 -SelfTest               passed
foreign listener stop/start protection test           passed
cargo build --release --locked --offline              passed
```

发布候选：

```text
rust/target/release/ema2.exe
sha256=af145c1d29e346e33565eff8cabcac4d13f33284697922c36b7b070c3c32fee5
version=1.8.0
```
