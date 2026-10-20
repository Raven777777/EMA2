# 历史规划归档

> **这些文件不代表当前行为。** 保留仅为追溯设计过程；
> 具体实现以 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)、[`../TOOLS.md`](../TOOLS.md)、
> [`../PHILOSOPHY.md`](../PHILOSOPHY.md) 为准。

| 文件 | 内容 | 状态 | 现行替代 |
|---|---|---|---|
| [`v2-architecture-plan.md`](v2-architecture-plan.md) | v2.0 改造前的形态共识（单 exe、单端口、无 MCP） | 已落地（v2.0） | [`../ARCHITECTURE.md`](../ARCHITECTURE.md) |
| [`v2-tools-plan.md`](v2-tools-plan.md) | v2.0 的命令参考草案（11 工具表、输出协议初稿） | 已过时（v2.1 起有多处行为变更） | [`../TOOLS.md`](../TOOLS.md) |

## 归档规则

1. 被新文档取代的整篇内容搬进本目录，不要留在 `docs/` 根下；
2. 文件头必须保留“归档状态 + 现行替代”说明；
3. 归档文件不再随版本更新，也不参与 `cargo test` / `verify.py`；
4. 不要再往归档文件里补新结论——新结论写进现行文档。
