# 历史文档归档

> **这些文件不代表当前行为。** 保留仅为追溯设计过程；
> 具体实现以 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)、[`../TOOLS.md`](../TOOLS.md)、
> [`../PHILOSOPHY.md`](../PHILOSOPHY.md) 为准。

| 文件 | 内容 | 状态 | 现行替代 |
|---|---|---|---|
| [`v2.1.6.0-architecture-plan.md`](v2.1.6.0-architecture-plan.md) | v2.0 改造前的形态共识（单 exe、单端口、无 MCP） | 已落地（v2.0） | [`../ARCHITECTURE.md`](../ARCHITECTURE.md) |
| [`v2.1.6.0-tools-plan.md`](v2.1.6.0-tools-plan.md) | v2.0 的命令参考草案（11 工具表、输出协议初稿） | 已过时（v2.1 起行为变更；v2.4.3.21 已删旧 batch 语法） | [`../TOOLS.md`](../TOOLS.md) |
| [`v2.3.0-evaluation-report.md`](v2.3.0-evaluation-report.md) | v2.3.0 外部评测报告（8.5/10） | 反馈项已在 v2.3.1 处理 | [`../TOOLS.md`](../TOOLS.md) / [`../HANDOFF.md`](../HANDOFF.md) |
| [`v2.4.1-release-plan.md`](v2.4.1-release-plan.md) | v2.4.1 发布前收尾建议（冻结范围、复测、安全边界、发布） | 已执行（v2.4.1 已发布） | [`../CHANGELOG.md`](../CHANGELOG.md) / [`../HANDOFF.md`](../HANDOFF.md) |
| [`v2.4.2.5-code-review.md`](v2.4.2.5-code-review.md) | 2026-10-22 代码质量审查（P0-1/P1 系列/P2） | 已修复（v2.4.2.5） | [`../TOOLS.md`](../TOOLS.md) / [`../HANDOFF.md`](../HANDOFF.md) |
| [`v2.4.3.21-evaluation.md`](v2.4.3.21-evaluation.md) | 命令面演进方案（v2.4.3.1 → v2.4.3.21：fan-out、等待/时长统一、internal 归一、隐藏后删旧解析器） | 已全部实施（三阶段） | [`../TOOLS.md`](../TOOLS.md) / [`../../rust/assets/AGENTS.md`](../../rust/assets/AGENTS.md) / [`../CHANGELOG.md`](../CHANGELOG.md) |

## 命名规则

文件名统一为 `v<版本>-<类型>.md`：版本是归档对应的产品版本，类型区分
`architecture-plan` / `tools-plan` / `evaluation` / `code-review` / `release-plan`。
新增归档按同一格式命名，本表只追加一行，方便按版本区分和后续扩展。

## 归档规则

1. 被新文档取代的整篇内容搬进本目录，不要留在 `docs/` 根下；
2. 文件头必须保留“归档状态 + 现行替代”说明；
3. 归档文件不再随版本更新，也不参与 `cargo test` / `verify.py`；
4. 不要再往归档文件里补新结论——新结论写进现行文档。
