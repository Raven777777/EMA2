# 贡献指南

先读 [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) 和 [docs/HANDOFF.md](docs/HANDOFF.md)，再改代码。

## 硬性测试政策

> **绝对禁止使用 Node.js 跑测试。**
>
> - 不允许提交 `package.json`、`package-lock.json`、`node_modules/`。
> - 不允许使用 `node:test`、jest、jsdom、Playwright、Puppeteer、VM 模拟等
>   任何 Node 测试工具来测试扩展。
> - 所有功能、回归、验收测试必须在**真实 Edge 浏览器 + 真实 `ema2` CLI**上执行。
> - 不允许用模拟测试代替真机验证。原因：Node 模拟启动慢、维护成本高，
>   而且找不到真实浏览器差异——v2.0 的 `type`/`select`、短 ref、
>   `observe console --filter`、`files download-dir` 等问题都是真机验收才发现的。
> - Rust 单测只用于纯解析/校验的快速反馈，**不能替代真实浏览器验收**。

真机验收清单见 [docs/TESTING.md](docs/TESTING.md)。

## 构建

```bash
cargo build --release --manifest-path rust/Cargo.toml
```

或：

```bat
scripts\setup.cmd
```

产物是 `bin\ema2.exe`。`bin/` 已在 `.gitignore` 中，不要提交构建产物。

## 修改扩展

- 扩展是普通 MV3 service worker，没有构建步骤；
- 改完 `extension/` 后在 `edge://extensions/` 重新加载；
- Edge 可能按 `manifest.json` 的 `version` 缓存扩展脚本。行为没更新时，
  提升版本号或使用全新的 `--user-data-dir`；
- 修改页面 helper（`__ema2_helpers_version`）时必须同步提升版本号，
  否则已打开页面会继续使用旧 helper。

## Rust

```bash
cd rust
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

Rust 单测只覆盖纯函数、参数校验、路由和落盘逻辑；浏览器行为请按
[docs/TESTING.md](docs/TESTING.md) 用真实 Edge 验证。
