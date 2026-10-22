# EMA2 文档索引

读文档的顺序就是“先想清楚边界，再看契约，最后看怎么验”：

| # | 文档 | 什么时候读 |
|---|---|---|
| 1 | [PHILOSOPHY.md](PHILOSOPHY.md) | 决定「这个功能该不该进 EMA2」之前 |
| 2 | [ARCHITECTURE.md](ARCHITECTURE.md) | 改进程模型 / 通道 / settle / ref 之前 |
| 3 | [TOOLS.md](TOOLS.md) | 查命令参数、输出协议、错误码 |
| 4 | [TESTING.md](TESTING.md) | 写完代码要验收时（真机、前台、禁止 Node） |
| 5 | [HANDOFF.md](HANDOFF.md) | 接手项目、看当前版本与已知弱点 |
| 6 | [CHANGELOG.md](CHANGELOG.md) | 查某个行为是哪一版改的 |

## 规划

| 文档 | 内容 |
|---|---|
| [EVOLUTION.md](EVOLUTION.md) | 命令面演进（v2.4.3.1 已实施；v2.4.3.11 → v2.4.3.21 仍为提案） |

## 目录

```text
docs/
├── README.md          ← 本文件
├── PHILOSOPHY.md      ← 边界与判断标准（最重要）
├── ARCHITECTURE.md    ← 单端口、daemon、扩展、settle/ref/read
├── TOOLS.md           ← 命令参考、输出格式、错误码表
├── TESTING.md         ← 真机验收规定与清单
├── HANDOFF.md         ← 交接：版本、契约、已知弱点、下一步
├── CHANGELOG.md       ← 版本历史（唯一版本号来源）
├── EVOLUTION.md       ← 命令面演进（v2.4.3.1 已实施，后续为提案）
├── archive/           ← 已归档的历史规划/评测，仅追溯
└── img/               ← README 用图
```

## 仓库里其它文档

| 文件 | 内容 |
|---|---|
| [`../README.md`](../README.md) | 项目简介、快速开始、命令总览、限制 |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | 构建、测试政策、改扩展的注意事项 |
| [`../examples/README.md`](../examples/README.md) | `.ema2` 脚本、`js` 抽取、批量读取示例 |
| [`../AGENTS.md`](../AGENTS.md) | 给 AI 的最短操作手册（由 `ema2 init` 维护） |
| [`../rust/assets/AGENTS.md`](../rust/assets/AGENTS.md) | `ema2 init` 写进用户项目的模板 |

## 归档

[`archive/`](archive/) 里的文件**不代表当前行为**，只用于追溯设计过程。
每个文件头部都标注了归档状态与对应的现行文档；包括 v2.3.0 外部评测报告、v2.4.1 发布收尾建议，
以及 `code-review-2026-10-22.md`（2026-10-22 代码审查，问题已在 v2.4.2.5 修复）。

## 维护脚本

- `scripts/reloadextension.py`：pywinauto/UIA 脚本，找到 Edge 窗口并最大化，
  打开 `edge://extensions`，按扩展卡片里的 `Type=Button` + `Name=重新加载` 精确点击。
  用法见仓库根目录 [`../AGENTS.md`](../AGENTS.md)。

## 验证文档是否过期

```bash
cargo test                                   # 含“错误码必须写进 TOOLS.md”的漂移测试
python scripts/verify.py --skip-real-edge     # 同一件事的快速版
python scripts/verify.py                      # 真实前台 Edge 冒烟（项数以输出为准）
```
