# EMA2 项目哲学

> EMA2 是 AI 的**浏览器原语层**，不是网站自动化产品。  
> 它只保证“打开、观察、定位、操作、等待、报错”可靠，不替每个网站写适配器。

## 1. AI-first，而不是人-first

EMA2 的服务对象是通过 shell 工作的 AI：

- 输出是行式紧凑文本，不是 JSON
- 列表带 `total / shown / next`
- 文件结果给 `path=`，不把大块 base64 塞回上下文
- 错误首行 `error=<code> msg=... hint=...`
- 退出码粗粒度：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错
- ref 是主要定位方式：`[e12@d1] button "提交"`

好的输出应该让 AI 下一步**少猜一点**。

## 2. 动作结果必须可信

“点了”不等于“页面变了”，更不等于“操作成功”。

- `act` 的隐式 settle 只回答“现在大概是什么状态”
- `settle=pending` 不是错误，退出码仍为 0
- 需要继续等时，AI 显式 `ema2 await 10s`
- settle 超时不能伪装成动作失败
- 没有产生效果时要明确给 `nav=none` / `changed=none` / hint
- 不允许“命令返回 ok，但实际什么都没发生”的假成功

动作是否真正生效，比多做几个工具重要。

## 3. 先观察，再操作

- `snapshot` 给可引用 ref
- ref 内嵌文档句柄 `d1/d2…`，导航后旧 ref 报 `stale_ref`
- `read` 先给正文，截断时给 `next` 和 cursor hint
- `observe console/network` 看真实运行时
- `js` 是最后手段，不是默认路径

不要靠猜 CSS、猜页面结构、猜网络状态。

## 4. 小核心，拒绝网站适配层

EMA2 只保留 11 个通用工具：

```text
tabs open read snapshot act wait js observe files shot batch
```

`await` 是 CLI 命令，映射到 `wait` 工具，不新增第 12 个工具。
`batch` 是 `--tabs/--urls` 的 `/cmd` 执行器，不是用户命令；工具仍是 11 个。

不做：

- 携程/淘宝/Bing 专用下拉适配
- CKEditor / ProseMirror / Office 专用编辑器
- OCR、验证码、登录态管理
- 自动下单、自动发帖、业务工作流
- 每个网站一个 parser

这些应该由 `js`、`.ema2`、外部 Python/脚本在 EMA2 之上完成。  
**EMA2 保持薄，网站逻辑保持外置。**

## 5. 单 exe、单端口、无隐藏状态

- 一个 exe 同时是 daemon 和短命 CLI
- 默认端口 `7744`
- WS（扩展）与 HTTP（CLI）共享一个 listener，靠 `Upgrade` 头分流
- 无 MCP、无 token、无授权页、无 `ema2.toml`
- 无持久状态文件；只有临时 PID/日志和显式输出文件
- `ema2 server` 启动；`ema2 server stop` 显式关闭；不做 idle 自动退出

状态越少，AI 越容易理解，排障越直接。

## 6. 安全边界是“本机回环”，不是 token

没有 token/HMAC 是**有意为之**：

- 只 bind `127.0.0.1`
- 校验 `Host` 和 `Origin`
- 任何能访问本机 `7744` 的进程都能驱动浏览器

这是明确的取舍。不要偷偷加回 token/配对/授权页来“看起来更安全”；如果未来要改，必须作为架构变更讨论。

## 7. 真实浏览器，禁止模拟测试

- 功能/回归/验收必须在真实 Edge + 真实 CLI 上执行
- **绝对禁止 Node.js 测试**（`node:test`、jest、jsdom、Playwright、Puppeteer、VM 模拟）
- 禁止提交 `package.json` / `package-lock.json` / `node_modules`
- Rust 单测只做纯解析/校验/路由/落盘的快速反馈，不能替代真机验收

原因：模拟测试慢、维护成本高，而且找不到真实网页差异。  
v2 的 `type`/`select`、短 ref、`observe console --filter`、`files download-dir`、
`contenteditable` 重复输入、`read` 正文污染，都是真机才发现的。

## 8. 文本清洁要“有提示”，不要魔法

- `read --mode text|md` 跳过脚本、样式、表单控件、隐藏元素和页面壳
- `main` 太短回退 `body(fallback)`，必须给 hint
- `html` 保持原始输出，不偷偷清洗
- 不猜正文之外的东西；被截断就明确给 `next`

清洁是为了减少噪声，不是为了隐藏事实。

## 9. 契约稳定，破坏性变更要升级版本

- 端口、输出协议、退出码、ref 格式、工具名是公共契约
- 新增行为优先用字段/hint 扩展，不改变旧字段语义
- 改动页面 helper 时同步提升 `__ema2_helpers_version`
- 改动扩展行为时提升 `manifest.json` 版本，避免 Edge 缓存旧脚本

## 10. 判断一个改动值不值得做

按顺序问：

1. 它是否让动作结果更可信？
2. 它是否减少 AI 的猜测和上下文浪费？
3. 它是否是通用浏览器原语，而不是某个网站的特例？
4. 它能否用 `js` / `.ema2` / 外部脚本完成？
5. 它是否保持 11 个工具的薄表面？

如果 4 的答案是“能”，通常不该进 EMA2 核心。

## 一句话

**让 AI 用最少的猜测操作真实浏览器；EMA2 只保证原语可靠，不替网站写适配器。**
