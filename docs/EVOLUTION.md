# EMA2 命令面演进方案

> 状态：**v2.4.3.1 已实施**（2.4.3.1，纯增量）；v2.4.3.11 / v2.4.3.21 仍是提案。
> 依据 v2.4.2.7 源码与文档（2026-10-22）。
> 实施完成后本文迁入 `docs/archive/`；现行行为以 `docs/TOOLS.md` 与 `ema2 <cmd> --help` 为准。

## 0. 结论

评测对「方向 9/10、命令面 7/10」的判断成立。但命令面的扣分**不在命令数量**，而在同一能力有多套语法。

现状：15 个顶层命令、22 个可调用形式、11 个工具、约 64 个 flag。对浏览器原语层而言，这个规模是合理的。真正的负担是四件事：

1. `batch <tool> [inner flags]` 是**第二套参数解析器**（`rust/src/cli/batch.rs` 的 `build_batch` + `parse_tool_flags` 约 270 行），与 `spec.rs` 的 clap 定义重复维护同一批 flag。
2. 一个「等」有六种写法：`await` / `wait` / `act --wait-for` / 脚本 `waitFor` / `open --wait` / `observe --wait`。
3. 同名不同义：`--filter`、`--wait`、`--timeout`、`--reuse`、`--tab` vs `--tab-ids`。
4. 嵌套子命令重复顶层原语：`tabs new` 与 `open` 各有一份建标签 + 登记 + 聚焦；`tabs current` 与 `status`/`tabs list` 信息重叠；`files download-dir` 的唯一功能是报错。

裁决摘要：

| 类别 | 动作 |
|---|---|
| 合并 | `batch` → `--tabs/--urls`；`tabs new` → `open --background`；`tabs current` / `files download-dir` → 隐藏 |
| 改名 | `--tab-ids` → `--tabs`；observe `--filter` → `--url-filter`；`open --wait` → `--until`；`observe --wait` → `--collect`；`--reuse session` 拆为 `last` / `url` |
| 修复 | `internal` 归一到已有错误码；漂移测试覆盖 `error=` 字面量 |
| 新增 | `read/snapshot/js/wait` 的 `--url/--urls`；`open --background` |
| 不做 | 不合并 `read/snapshot/act/js`；不删 `await`；不拆 `observe`；不放开 `.ema2` 的 read/snapshot/js |

公开命令面从 **15 顶层 / 22 形式** 收敛到 **14 顶层 / 18 形式**；工具仍是 **11 个**。

flag 总数不会显著下降，下降的是**语法套数**和**同名歧义**。这正是「繁杂」的来源：不是命令太多，而是同一个概念有多个名字、同一批参数有两套解析器。

分三段落地：**v2.4.3.1 增量统一 → v2.4.3.11 隐藏旧面 → v2.4.3.21 删代码**。中间小版本用于 bug 修复。

---

## 1. 现状测量

| 维度 | 数值 | 出处 |
|---|---|---|
| 顶层命令 | 15 | `rust/src/cli/spec.rs:38-309` |
| 可调用形式 | 22（含 `server stop`、`tabs` 5 个子命令、`files` 2 个） | 同上 |
| 工具白名单 | 11 | `rust/src/tools.rs:11-14` |
| CLI flag | 约 64（不含 `.ema2` 关键字） | `spec.rs` 逐项统计 |
| batch 第二解析器 | `build_batch` + `parse_tool_flags` 约 270 行 / 14 个内层 flag 名 | `rust/src/cli/batch.rs:11-278,58-75` |
| 等待语义 | 6 种写法 | 见 §2.2 |
| 错误码 | 表中约 60 条，`internal` 未收录 | `docs/TOOLS.md:62-137`；`extension/modules/core.js:408` |
| 全量命令文档 | 3 处 | `--help`、`docs/TOOLS.md`、`rust/assets/AGENTS.md` |

---

## 2. 复杂度归因

### 2.1 两套参数系统（最大的一处重复）

`ema2 batch read --urls ... --mode md --budget 4000` 的内层 flag 不是 clap 解析的：`build_batch` 先吞掉 `--tab-ids/--urls/--reuse/--concurrency/--port`，其余 token 交给 `parse_tool_flags`，后者为 open/read/snapshot/js/wait 各写了一份 `match`。

后果：

- 同一 flag 的合法值、错误文案、默认值有两份（`spec.rs` 与 `batch.rs`）；
- 新增一个 flag 要改两处，漏改只在运行时暴露；
- `batch --reuse session` 与 `open --reuse session` 同名，语义却不同：前者按**精确 URL** 复用会话标签，后者复用**最近一个**会话标签并导航。

这不是「命令多」，是「同一能力有两套入口语法」。

### 2.2 一个「等」六种写法

| 写法 | 等什么 | 触发条件 |
|---|---|---|
| `await [10s]` | 上一个 act 的导航/页面稳定 | 必须有 lastAct；`open` 后无上下文会 `no_pending_action` |
| `wait text/selector/idle/time` | 页面条件 | 独立命令 |
| `act --wait-for "text=..."` | 本次动作的后置条件 | 动作之后 |
| 脚本 `waitFor` | 同上，脚本语法 | `.ema2` 内 |
| `open --wait load\|dom\|none` | 文档接位级别 | 打开时 |
| `observe --wait 10s` | 采集窗口 | 观察时 |

后两者只是借用了「等」字，语义分别是「返回到什么程度」和「采集多久」。同名不同义会让 agent 在错误的地方用错误的等待。

尤其要注意：**`await` 不是 sleep**。`open` 会清 lastAct（`HANDOFF.md` 开发陷阱 4），所以 `open` 之后 `await` 会报 `no_pending_action`；需要额外等待时应用 `wait idle` 或 `wait time`。

### 2.3 同名不同义

| flag | 在一处 | 在另一处 |
|---|---|---|
| `--filter` | `snapshot`：元素类型过滤 | `observe`：URL 子串 |
| `--wait` | `open`：加载级别 | `observe`：采集时长 |
| `--timeout` | `open`/`js`：数字秒 | `wait`：时长串（`30s`） |
| `--reuse session` | `open`：最近会话标签 | `batch`：精确 URL 匹配 |
| `--tab` / `--tab-ids` | 单目标 | 多目标（batch 专属） |

### 2.4 嵌套子命令重复顶层原语

- **`tabs new`**：`extension/modules/network.js:200-210` 建标签、focus 窗口、登记 session；`open` 在 `extension/modules/open_wait.js:69-72` 做同样三件事，还多等了文档、装了 hook、回了 `status=`。`tabs new` 是 `open` 的弱化版，两处代码会分别演化。
- **`tabs current`**：返回 `id/window/os/title/url`。`status` 已给 `active_tab`，`tabs list` 用 `*` 标记活动页；`os` 全仓只有 `network.js:199` 这一处消费。
- **`files download-dir`**：固定返回 `download_dir_unsupported`，没有成功路径。

### 2.5 文档三处全量维护

`ema2 <cmd> --help`（clap）、`docs/TOOLS.md`（764 行）、`rust/assets/AGENTS.md`（189 行）都维护一份命令表。错误码有漂移测试（`rust/src/tools.rs:1791`），命令表没有。仓库根 `AGENTS.md` 还是 `ema2 init` 生成的快照，改模板后需要重跑一次。

---

## 3. 裁决表

| 对象 | 现状 | 裁决 | 目标 | 理由 |
|---|---|---|---|---|
| `batch` | 独立子命令 + 第二解析器 | **合并** | `read/snapshot/js/wait --tabs/--urls`；`open --urls` | 同一能力两套语法；并发执行器保留为内部工具 |
| `tabs new` | 建标签的弱化版 | **合并** | `open --background` | 与 open 重复建标签/登记/focus |
| `tabs current` | 与 status/list 重叠 | **隐藏** | 保留 tombstone | 活动页可由 status/list 得到；`window/os` 无消费者，tombstone 仍输出 |
| `files download-dir` | 只报错 | **隐藏** | 保留 tombstone | 没有成功路径 |
| `await` | act 结算入口 | 保留 | 不改 | 短、高频、语义唯一；合并进 wait 会增加一个 kind |
| `wait` | 页面条件 | 保留 | 不改 | 与 await 触发条件不同 |
| `observe` | console/network 二合一 | 保留 | 只改 flag 名 | 拆开只会多一个命令，flag 仍在 |
| `read/snapshot/act/js` | 四个原语 | 保留 | 不改 | 各自不可替代 |
| `read --url` | 缺失 | **新增** | `--url` / `--urls` | 消除 open→read 的不对称 |
| `--tab-ids` | batch 专属 | **改名** | `--tabs` | 与 `--tab` 成对 |
| observe `--filter` | 与 snapshot 撞名 | **改名** | `--url-filter` | 一名一义 |
| `open --wait` | 借「等」字 | **改名** | `--until` | 表示「返回到什么程度」 |
| observe `--wait` | 借「等」字 | **改名** | `--collect` | 表示「采集多久」 |
| `--reuse session` | 两处不同义 | **拆分** | `last` / `url` | 消除歧义 |
| `--bodies=false` | 布尔取反写法 | **改名** | `--no-bodies` | 与 `--no-main` 一致 |
| `.ema2` 放开 read/snapshot/js | 评测建议 | **不做** | 保持边界 | 无变量/循环，输出聚合与预算无定义 |
| 正文容器评分 | 回退 body 噪声大 | **做（P2）** | 不改 CLI | 通用启发式，不是站点适配 |
| `internal` | 未收录的兜底码 | **修（P0）** | 映射 + 文档 + 测试 | 违反「错误码可自我纠正」契约 |

### 3.1 目标命令面（v2.4.3.11 可见）

| 命令 | 可见子命令 | 变化 |
|---|---|---|
| `server` | `stop` | 不变 |
| `status` | — | 不变 |
| `init` | — | 不变 |
| `tabs` | `list` / `close` / `activate` | `new` / `current` 隐藏 |
| `open` | — | 新增 `--background`、`--urls` |
| `read` | — | 新增 `--url` / `--urls` / `--tabs` |
| `snapshot` | — | 新增 `--url` / `--urls` / `--tabs` |
| `act` | — | 不变（单步 / `--steps` / `--script`） |
| `await` | — | 不变 |
| `wait` | — | 新增 `--url` / `--urls` / `--tabs` |
| `js` | — | 新增 `--url` / `--urls` / `--tabs` |
| `observe` | `console` / `network` | `--url-filter` / `--collect` / `--no-bodies` |
| `files` | `upload` | `download-dir` 隐藏 |
| `shot` | — | 不变 |

`batch` 不在可见面，但仍可调用（v2.4.3.11），作为 `--tabs/--urls` 的执行器（v2.4.3.21）。

---

## 4. 三个合并

### 4.1 `batch` → `--tabs` / `--urls`

**目标语法**

```bash
# before
ema2 batch read --urls "https://a/,https://b/" --mode md --budget 4000 --concurrency 2
ema2 batch snapshot --tab-ids "1,2" --filter links
ema2 batch js --file lib.js --tab-ids "1,2"

# after
ema2 read --urls "https://a/,https://b/" --mode md --budget 4000 --concurrency 2
ema2 snapshot --tabs "1,2" --filter links
ema2 js --file lib.js --tabs "1,2"
```

**规则**

- `--tabs` 与 `--urls` 互斥，且都与 `--tab` / `--url` 互斥。
- `open` 的 `<url>` 变为可选：`open <url>` 或 `open --urls "u1,u2"`，二者必须给一个；`--urls` 时内层 args 不含 `url`，由执行器按目标注入。
- `--concurrency` 1..4，默认 2；`js` 因全局 debugger 串行，有效并发仍为 1。
- `--url/--urls` 打开的标签登记进会话，块头带 `tab=`，收尾统一用 `tabs close --session`。
- 输出保持现有 batch 契约：块之间 `---`，块头 `url=` / `tab=`，单块失败不影响其它块。
- `--url` 是 `--urls` 的单数形式：`read --url <u>` ≡ `read --urls "<u>"`。
- 单 URL 也走块格式：块头 `url=` 是请求目标，正文头 `url=` 是最终 URL（重定向后可能不同）；两者不同义，不是重复字段。若嫌外观重复，v2.4.3.1 可把块头改为 `target=`，但不在 v2.4.3.1 动。

**实现要点**

- 在 clap 里用 `#[command(flatten)]` 共享一组 `FanoutArgs`（`tabs/urls/url/concurrency/reuse`），5 个子命令不复制整组定义。
- 每个子命令仍用自己的 builder 生成 `ToolCall`；带 `--tabs/--urls` 时，`run_built` 把已生成的 `{tool, args}` 包成 batch 请求，**不再有第二套 flag 解析器**。
- 扩展侧 `batchTool`（`extension/modules/batch.js`）保留，继续负责并发、开页、分块；它不再是面向用户的命令语法。
- 每个被截断的块追加一行 `resume=<tool> --tab <id> --cursor <next>`（`wait` 无 cursor，跳过），解决「cursor 不知道属于哪个 tab」。

**为什么不直接删掉 batch**

并发、按 URL 开页、块级失败隔离都是真实能力，删掉会退化。要删的是**第二套参数语法**，不是执行器。

**迁移**

- v2.4.3.1：新增 `--tabs/--urls`，旧 `batch` 原样可用。
- v2.4.3.11：`batch` 在 help 中隐藏；除 v2.4.3.1 新增的 `resume=` 行外，运行时输出保持字节兼容（不加 `deprecated=` 行，避免破坏按 `---` 分块的解析器）。
- v2.4.3.21：删除 `build_batch` / `parse_tool_flags`（约 270 行）；`batch` 作为 `/cmd` 工具保留，供 `--tabs/--urls` 调用。

### 4.2 `tabs new` → `open --background`

```bash
# before
ema2 tabs new https://example.com/ --background

# after
ema2 open https://example.com/ --background
```

- `open` 新增 `--background`：只对「新建标签」生效；与 `--tab` / `--replace` / `--reuse` 同时出现报 `invalid_argument`。
- 输出仍带 `id / url / title / status / focused`；后台打开时 `focused=0`。
- 默认行为不变：不写 `--background` 仍然新建、激活、聚焦窗口。
- v2.4.3.11 起 `tabs new` 隐藏；**保留原实现和原输出**，在**末尾追加**一行 `deprecated=1 use ema2 open --background`（放在开头会破坏按首行 `id=` 解析的脚本；转发到 `open` 则会丢掉 `window=`，都不算兼容）。v2.4.3.21 删除 CLI 子命令；扩展的 `tabs action=new` 保留给 `/cmd` 兼容。若 v2.4.3.21 想彻底删掉 `action=new`，先给 `open` 补 `window=` 再转发。

### 4.3 `tabs current` / `files download-dir` → 隐藏 + tombstone

| 旧写法 | v2.4.3.11 行为 | 替代 |
|---|---|---|
| `ema2 tabs current` | 原输出，末尾追加 `deprecated=1 use ema2 status or ema2 tabs list` | 活动页用 `status` 的 `active_tab=` 或 `tabs list` 的 `*`；`window/os` 仅 tombstone 仍有 |
| `ema2 files download-dir` | 原错误，末尾追加 `deprecated=1 set the directory in edge://settings/downloads` | 无（Edge 不允许扩展改下载目录） |

两者都从 help 和 AGENTS 模板中移除，但运行时不硬失败；v2.4.3.21 再决定是否彻底删除。

---

## 5. 四项统一

### 5.1 时长：一种写法

全部接受 `30`、`30s`、`500ms`、`1.5s`；裸数字等于秒。`parse_duration_secs`（`rust/src/cli/shared.rs:20`）已经支持这套语法，问题只在 `open --timeout` 与 `js --timeout` 仍用 `f64` 接收：

```bash
ema2 open https://a/ --timeout 20s     # 与 20 等价
ema2 js 'return 1' --timeout 500ms
ema2 wait text "Done" --timeout 30s
ema2 observe network --collect 5s
```

改法：这两个 flag 改为 `String` + `parse_duration_secs`，保留原范围（open 1..60s，js 0.1..300s）。向后兼容，不改变输出。

### 5.2 等待：一名一义

| 场景 | 写法 | 说明 |
|---|---|---|
| 动作结算 | `await [10s]` | 只跟随上一个 act 所在标签；不是 sleep |
| 页面条件 | `wait text\|textGone\|selector\|time\|idle` | 独立命令 |
| 动作 + 后置条件 | `act ... --wait-for` | 本次动作的响应/文本/选择器 |
| 脚本内 | `await` / `waitFor` | 不能写 `wait` |
| 打开返回时机 | `open --until load\|dom\|none` | 旧 `--wait` 隐藏别名 |
| 采集窗口 | `observe --collect 5s` | 旧 `--wait` 隐藏别名 |

`await` 保留。把它并进 `wait` 需要一个 `settle` kind，还要处理 lastAct 上下文，收益只是少一个顶层命令，不值得。

### 5.3 observe：把撞名的 flag 改掉

```bash
ema2 observe console --level warn --collect 5s
ema2 observe network --url-filter api/ --no-bodies --har out.har
```

- `--filter` → `--url-filter`（旧名隐藏别名），`--filter` 从此只表示 snapshot 的元素过滤。
- `--wait` → `--collect`（旧名隐藏别名），`--wait` 只属于页面条件。
- `--bodies=false` → `--no-bodies`（旧名保留），与 `--no-main` 一致。

### 5.4 `--reuse`：拆成两个明确的词

| 写法 | 语义 | 适用 |
|---|---|---|
| `open --reuse last` | 复用本会话**最近**的标签并导航；同文档不重载 | 单 URL 连续翻页 |
| `--urls ... --reuse url` | 按**精确 URL** 匹配会话标签，命中则不重载 | 批量读取去重 |

旧的 `--reuse session` 在两处都保留为隐藏别名，但新文档只写 `last` / `url`。同名不同义是 agent 误用的高发点，必须在文档层消灭。

---

## 6. P0：`internal` 不是一个可自我纠正的错误码

**现状**

- `extension/modules/core.js:408` 的 `formatError` 把所有非 `ToolError` 异常包成
  `error=internal msg=... hint=retry; if it persists reload the extension`。
- `extension/modules/batch.js:174` 还有一处 `error=internal` 文本。
- `docs/TOOLS.md` 的错误码表没有 `internal`。
- 漂移测试 `rust/src/tools.rs:1791` 只扫 `fail('` 和 `code: '`，扫不到 `error=` 字面量。所以测试是绿的，契约是漏的。

这正是评测抓到的 `error=internal msg=Frame with ID 0 is showing error page`：信息量低，且没有可执行动作。

**映射（在 `formatError` 单点做）**

| 浏览器异常文本 | 归一为 | hint |
|---|---|---|
| `Frame with ID 0 is showing error page` | `page_error` | 页面在浏览器错误页；换可用 URL 或检查网络 |
| `No tab with id` | `tab_not_found` | `tabs list` 拿 id |
| `Cannot access a chrome:// URL` / `edge://` | `internal_page` | 换 http(s) 页面 |
| `Debugger is not attached` / `Detached while handling command` | `debugger_busy` | 关 DevTools 或重载 tab 后重试 |
| `Cannot find context with specified id` / `Target closed` | `page_changed` | 页面已导航；重新 snapshot |

保留 `internal` 作为最后兜底，但写进错误码表，并让测试能看见它。

**测试**

- `documented_error_codes_cover_extension_codes` 增加 `error=` 标记，覆盖 `extension/modules/*.js`。
- `scripts/verify.py` 增加真机用例：打开一个不存在的域名 → `read` 必须 `error=page_error`、退出 4。
- 这样新的 `error=` 字面量要么被映射，要么被文档收录，不会再出现「测试绿、契约漏」。

---

## 7. 评测五条建议的逐条裁决

| # | 建议 | 裁决 | 落点 |
|---|---|---|---|
| 1 | 补 `internal` 或归一成 `page_error` | **接受** | §6，P0 |
| 2 | `read` 支持 `--url` | **接受，并扩展为对称的 `--url` / `--urls`** | §4.1，P1 |
| 3 | batch 续读带 tab 标识 | **接受，结构解决 + `resume=` 行** | §4.1，P1 |
| 4 | 脚本 DSL 放开 `snapshot`/`read`/`js` | **不接受** | 见下 |
| 5 | 正文抽取加容器评分 | **部分接受，P2 独立做** | 见下 |

### 7.1 为什么不让 `.ema2` 写 `read/snapshot/js`

不是保守，是这三个动作放进静态 DSL 后**没有可定义的契约**：

1. **没有变量和循环。** 真实的多步需求是「读列表 → 点第 N 条 → 读详情」，需要变量和下标的脚本语言；给 `.ema2` 加 `read` 仍然表达不了，只会让人误以为它能。
2. **输出聚合无定义。** 20 步脚本里三次 `read`，每次都可能被 budget 截断；返回体是拼在一起、分块、还是只回最后一次？每步的 `next` 怎么给？现有 `act` 只聚合动作结果，不聚合正文。
3. **超时预算会被击穿。** 单次 `read` 的中继预算是 80s；一个 20 步脚本只要有几次 read/wait，就会逼近或超过 600s 调用上限，现有 `act` 的预算模型没有为正文读取留额度。
4. **已有替代。** `js --file` 能在页面主世界做抽取 + DOM 操作（除可信输入），外部 Python 能做编排和循环；`act --script` 负责可信输入。三者组合已经覆盖复杂流程。

保留边界，但把组合方式写进文档：

```bash
# 可信输入 + 抽取，两次调用
ema2 act --script login.ema2
ema2 js --file extract.js
```

如果未来真要做，应该引入一个独立的 `flow` 执行器（支持变量/循环），而不是把 `.ema2` 逐步改成半门编程语言。那是新产品，不是命令精简。

### 7.2 容器评分：做，但不进命令面

现状：`read` 的 `main` 选取按清洁后文本量/链接密度打分，候选太短就回退 `body(fallback)`；百度百科这类门户会因此带出 27k 导航噪声（评测实测）。

建议的通用启发式（只改 `extension/modules/read_snapshot.js`，不加 flag）：

- 候选：`article`、`main`、`[role=main]`、`section`、`div` 中文本长度 > 200 的子树。
- 评分：文本量 × (1 − 链接密度)² + 段落数奖励 + 标题数奖励 + class/id 正负词（`article/content/post/story/detail` 加分，`nav/menu/sidebar/comment/footer/related/recommend/breadcrumb` 减分）。
- 只有最高分低于阈值（例如不足 body 文本的 15% 且 < 200 字符）才回退 body；回退时保留现有 `main=body(fallback)` + hint。
- 不引入域名表、不写站点适配器；评分逻辑是通用原语。

验收：在 fixture 集（文章页、带侧栏的门户、搜索结果、SPA 列表、论坛帖、文档页）上对比回退率和噪声字符数；不得让现有文章页的 `main=` 变差。此项与 v2.4.3.1 的 CLI 改动解耦，避免把行为风险和语法风险混在一个版本。

---

## 8. 分期计划

### v2.4.3.1 — 语义统一（纯增量，不改旧行为）✅ 已实施

见 `docs/CHANGELOG.md` 的 v2.4.3.1 条目。实际落地与计划一致，只记录两处实现取舍：

- `--reuse last` / `--reuse url` 发到 /cmd 时分别写成 `last` / `url`；扩展仍接受旧的 `session`，
  保证旧脚本和旧 /cmd 调用不断。
- fan-out 的 `--tab` 互斥在 CLI 侧前置报错（`invalid_argument`，退出 1），不等到扩展到 validator。
  目标数上限 8、`--reuse`/`--concurrency` 未配多目标也前置拒绝。

| 项 | 内容 |
|---|---|
| CLI | `FanoutArgs` + `--tabs/--urls/--url/--concurrency`；`open --background`；`open/js --timeout` 接受时长串；observe 三个新名 + 旧别名；`--reuse last/url` + 旧别名 |
| 扩展 | `formatError` 浏览器异常映射；batch 块 `resume=` 行；提升 `manifest.json` 版本（未改页面 helper，不需要 `helpersVersion++`） |
| 文档 | TOOLS.md 新增「目标与并发」「等待语义」「时长」三节；AGENTS 模板同步；重跑 `ema2 init` |
| 测试 | clap 互斥/时长解析单测；错误码扫描扩展到 `error=`；命令表漂移测试；verify.py 新增 `read --url`、`--tabs`、`--background`、错误页映射 |
| 契约 | 只增不减；退出码不变；工具仍 11 个 |
| 回滚 | 回退版本即可；旧语法未动 |

### v2.4.3.11 — 隐藏旧面（运行时兼容）

| 项 | 内容 |
|---|---|
| CLI | `batch`、`tabs new`、`tabs current`、`files download-dir` 从 help 隐藏；`tabs new/current`、`files download-dir` 回 `deprecated=1`；`batch` 输出保持字节兼容 |
| 文档 | 旧写法进「legacy」小节，不再出现在主命令表和 AGENTS 模板 |
| 测试 | 旧写法仍可执行；help 不再列出；新写法全覆盖 |
| 契约 | 可见面缩小，运行时不破坏 |

### v2.4.3.21 — 删代码（可选，视 v2.4.3.11 使用情况）

| 项 | 内容 |
|---|---|
| CLI | 删除 `build_batch` / `parse_tool_flags`（约 270 行）、`tabs new/current` CLI 分支、`files download-dir` 分支 |
| 工具 | `batch` 保留为 `/cmd` 执行器；`registry_is_compact_and_complete` 增加显式例外，工具数仍 11 |
| 文档 | PHILOSOPHY 补一句「batch 是 `--tabs/--urls` 的执行器」；HANDOFF 版本/工具说明同步 |
| 测试 | registry 测试更新；verify.py 全量真机 |

### 8.1 主要风险与缓解

| 风险 | 缓解 |
|---|---|
| `--tab` 与 `--tabs` 误用 | CLI 前置互斥校验；help 并列；verify.py 负例 |
| `read --url` 留下标签 | 块头带 `tab=`；文档固定写 `tabs close --session`；不自动关，避免误关用户页 |
| `open --background` 后直接 `shot` | `shot --tab` 会自动激活；文档提示先 `tabs activate` |
| `internal` 映射掩盖原始信息 | `msg=` 保留原始异常文本，只改 `code` / `hint` |
| 旧脚本按首行解析 tombstone | `deprecated=` 一律追加到末尾，不动首行 |
| `--tabs` 把单块输出变成多块 | 单目标仍用 `--tab`；复数 flag 明确改变输出形状，文档并列示例 |

**如果只做三件事**：§6 的 `internal` 修复 → §5.1 的时长统一 → §4.1 的 fan-out 语法。第一件是 P0，后两件是 P1；第三件是结构性的那件。

---

## 9. 明确不做

- **不合并 `read/snapshot/act/js` 成一个 `browser <verb>`。** 会毁掉「一条命令一个动词」的可预测性，help 膨胀，错误码路由变复杂。
- **不删 `await`。** 它是 act 结算的唯一入口，短且高频；合并进 `wait` 只会增加一个 kind 和一套 lastAct 判定。
- **不拆 `observe console|network`。** 拆开只多一个命令，flag 集合仍然要按 kind 校验。
- **不让 `.ema2` 写 `read/snapshot/js`。** 理由见 §7.1。
- **不做站点适配器、正文模板、OCR、登录态管理。** 与 PHILOSOPHY §4 一致。
- **不为了「看起来精简」把 flag 改成位置参数。** `act` 已经是位置参数风格，其他命令保持 flag，避免两套风格。
- **不在 v2.4.3.1 动正文评分。** 行为风险和语法风险分开版本。

---

## 10. 验收标准

1. `cargo test` 全绿，新增：
   - `--tabs`/`--urls` 与 `--tab`/`--url` 互斥；
   - `--timeout 20` / `20s` / `500ms` 解析一致；
   - 旧 `batch` / `tabs new` / `tabs current` / `files download-dir` 仍可执行；
   - 所有 `fail('code'`、`code: '`、`error=<code>` 字面量都在 `docs/TOOLS.md` 表中；
   - `docs/TOOLS.md` 与 `rust/assets/AGENTS.md` 的命令表与 clap 可见子命令一致。
2. `python scripts/verify.py` 真机全绿，新增：
   - `read --url` 单 URL；`read --urls` 两块且各带 `tab=`；
   - `open --background` 不改变活动标签，`focused=0`；
   - 错误页 → `error=page_error`，退出 4；
   - 截断块带 `resume=read --tab <id> --cursor <n>`。
3. v2.4.3.11 起 `ema2 --help` 只列 14 个顶层命令；`ema2 batch --help` 仍可用（隐藏）。
4. 旧脚本（`examples/*.ema2`、`examples/*.sh`）不改一行仍能跑。
5. 文档：TOOLS.md 的 batch 内层 flag 小节删除，替换为「目标与并发」；AGENTS 模板不再出现 `batch` / `tabs new` / `tabs current` / `files download-dir`。

---

## 11. 一句话

**EMA2 不需要更少的命令，需要更少的语法。** 把 `batch` 的第二套参数系统收进 `--tabs/--urls`，把六种「等」按语义拆开（结算、条件、返回时机、采集），把 `internal` 变成可纠正的错误码；核心的 11 个原语一个不少，这才是「小而可靠」的演进方向。
