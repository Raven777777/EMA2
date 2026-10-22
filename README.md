<p align="center">
  <img src="docs/img/ema2log_t.svg" alt="EMA2 — Edge Agent" width="520">
</p>

# EMA2

通过 shell 操作**常用Edge**：指纹、登录态，无依赖 CDP，无MCP。

```text
AI / shell ──ema2 CLI──> 127.0.0.1:7744 ──daemon──> WebSocket ──> Edge 扩展 (MV3) ──> 真实 Edge
```

## 安装

### 1. 构建

```bat
scripts\setup.cmd
```
```bash
cargo build --release --manifest-path rust/Cargo.toml
```

产物： `bin\ema2.exe`（单文件）。

分发包 = `bin\ema2.exe` + `extension/` + 在目标项目里执行 `ema2 init` 生成的 `AGENTS.md`。

### 2. 加载扩展

1. 打开 `edge://extensions/`，开启“开发人员模式”；
2. “加载解压缩的扩展”，选择仓库的 `extension/` 目录（需要 Edge/Chromium **111+**）；
3. 打开 EMA2 拓展菜单，确认端口是 **7744**，点“保存并连接”。

升级扩展后，在 `edge://extensions/` 点“重新加载”。

### 3. 启动 daemon

```bash
ema2 server
ema2 status
```

`relay=running` + `extension=connected` 即就绪。扩展未连时先检查 EMA2 拓展菜单；
端口错误与重连见 [TOOLS.md](docs/TOOLS.md)。

## 快速开始

```bash
ema2 open https://example.com/
ema2 read --mode md
ema2 snapshot
ema2 act click e1
ema2 shot --annotate
ema2 tabs close --session
ema2 server stop
```

`settle=pending` 不是失败，接 `ema2 await 10s`；导航后旧 ref 会 `stale_ref`，重新 `snapshot`。
脚本语法用 `ema2 act --help`，完整命令参数用 `ema2 <cmd> --help`。

## 安全边界

EMA2 只绑定 `127.0.0.1` 并校验 `Host`/`Origin`，**没有 token、HMAC 或应用层授权**——
这是有意的设计取舍。

- 任何能访问本机 `7744` 的进程，都可能获得驱动浏览器的能力。
- 仅适用于**可信的本机个人环境**；不要暴露端口、反向代理或共享给不可信用户。
- EDGE 拓展菜单 里的端口必须与 daemon 一致；任何占用该端口的本机服务都能冒充 daemon。
- 需要更强隔离时，请在外围（容器、独立用户、操作系统网络策略）解决，
  不要把认证临时塞进核心。

完整模型见 [ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [PHILOSOPHY.md](docs/PHILOSOPHY.md) | 项目边界与判断标准（先读） |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | 进程模型、单端口、settle/ref/read |
| [TOOLS.md](docs/TOOLS.md) | 命令参数、输出协议、错误码表 |
| [TESTING.md](docs/TESTING.md) | 真机验收规定与清单 |
| [HANDOFF.md](docs/HANDOFF.md) | 版本、契约、已知弱点与下一步 |
| [CHANGELOG.md](docs/CHANGELOG.md) | 版本历史 |
| [docs/README.md](docs/README.md) | 文档索引 |

## 开发与验收

```bash
cd rust
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

真实浏览器验收（必须**前台可见的真实 Edge**，禁止无头/后台窗口）：

```bash
python scripts/verify.py
```

**禁止 Node.js 测试**：不要提交 `package.json` / `node_modules`，不要用 jsdom、Playwright、
Puppeteer 或 VM 模拟。Rust 单测只做纯解析/校验的快速反馈，不能替代真机验收。
规则见 [CONTRIBUTING.md](CONTRIBUTING.md)，清单见 [TESTING.md](docs/TESTING.md)。

## 已知限制

- 跨域 iframe 不可遍历；`edge://` 等内置页不可脚本化。
- 截图与输入要求窗口可见；`shot --tab` 会自动激活目标标签。
- `click` 是合成事件（`isTrusted=false`）；`press` 在 debugger 可用时才是可信按键。
- 网络 body 只是预览，HAR 是采样，不是完整抓包代理。
- 别的扩展的浮层 UI（密码管理器、翻译面板）不可点击。
- `read --mode md` 是有损渲染；`visible` 会带出导航/侧栏，但不做正文清洗。

更完整的限制与降级行为见 [TOOLS.md](docs/TOOLS.md) 和 [ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## License

Just do what you dream / 去做你梦想的事 — 见 [LICENSE](LICENSE)。
