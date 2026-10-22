# jss — 「最小核心，自由扩展」的参考实现

> EMA2 的核心只定义浏览器能力，不定义能力的使用方式。
> 这个目录检验的不是核心缺了什么，而是**外围扩展一个能力的成本**。

只在 `examples/jss/` 内运行，不修改 `extension/`、`rust/`、`docs/` 的行为契约。
目录原先放在 `bin/jss/`，而 `bin/` 在 `.gitignore` 里——证据不进库就会随工作区消失，
所以 v2.4.2.6 起迁到 `examples/jss/`。

---

## 一、这不是「补功能」，是「验证缝隙」

盲测第一轮我用错了标尺：把「没有搜索」判成核心的结构性缺陷。

按 EMA2 的定义重新看——**搜索是内容策略，不是浏览器能力**。把它塞进核心才是膨胀。真正该问的问题是：

> 补上这个缺口，要付多少代价？

实测答案：

| 检索环节 | 核心提供 | 本目录补的 | 代价 |
|---|---|---|---|
| 页内检索 | — | `__E.grep()` | 无核心改动 |
| 搜索入口 | — | `serp.js` + `search.sh` | 无核心改动 |
| 结构概览 | — | `__E.outline()` | 无核心改动 |
| 多源并发 | `batch open` / `batch js` | `batch.sh` + `__E.digest()` | 无核心改动 |

总量（`wc -l` 实测）：`lib.js` 202 行 + `serp.js` 100 行 + 三个包装器 97 行 = **399 行**，
另加 17 行 `_env.sh`（只负责找 exe）。**核心一行没动。**

结论：核心不需要为检索长大。它只需要把 `js` 这条缝隙留好。

---

## 二、盲测验证到的四件事

| 结论 | 证据 |
|---|---|
| **对 Agent 而言 CLI 优于 MCP** | 全程零 schema 协商、零工具注册开销；`--help` 即权威文档，未读一行源码即上手 |
| **反馈是可验证的事实，不是猜测** | `settle=pending`、`nav=none`、`retried=1`、`trusted=0`、`via=scripting`、`stale_ref` 都可直接判读 |
| **错误码可以机械分支** | `relay_not_running` / `extension_not_connected` / `port_in_use` 各配动作，不需试错 |
| **生命周期概念干净** | `--session` 登记 + `tabs close --session` 一次收尾，两轮共关 27 个标签，无残留 |

这四条不是脚本的功劳，是核心契约的功劳。

---

## 三、文件

| 文件 | 作用 |
|---|---|
| `lib.js` | 注入 `window.__E`：`grep / outline / links / main / digest / meta / tables`（v3；回 `replaced=<旧版本>`） |
| `serp.js` | 独立脚本，解析 Bing / DuckDuckGo / Google 结果页 → `rank\|title\|url\|snippet`，按 5000 字符封顶并回报 `results/total/chars` |
| `_env.sh` | 定位 exe：`EMA2_BIN` → `PATH` → 仓库内 `bin/ema2(.exe)` |
| `run.sh` | 单页包装器：把 `lib.js` 与表达式拼成**一次** `js` 调用 |
| `search.sh` | 一条命令完成「编码查询 → 打开 → 解析」 |
| `batch.sh` | 并发打开多个 URL → `batch js --file` 统一跑 `__E.digest()` |

---

## 四、用法

```bash
cd examples/jss

# 0) 找得到 ema2 即可，三种方式任选
export EMA2_BIN=/path/to/ema2        # 或把 ema2 放进 PATH，或让仓库里有 bin/ema2.exe

# 1) 搜索
./search.sh "php2shell CVE-2026-109618"
./search.sh --close "Linux 7.3 LTS"           # 读完即关标签

# 2) 页内检索
./run.sh "__E.grep('CVE',2,40)"  --tab <ID>   # 参数: 查询, 上下文行数, 最大行数
./run.sh "__E.grep('setuid')"    --tab <ID> --budget 1500

# 3) 结构概览（先看骨架再决定读哪段）
./run.sh "__E.outline(30)"       --tab <ID>

# 4) 正文与元信息（read 回退 body 时的替代）
./run.sh "__E.main(4000)"        --tab <ID>
./run.sh "__E.meta()"            --tab <ID>
./run.sh "__E.links('kernel')"   --tab <ID>
./run.sh "__E.tables()"          --tab <ID>

# 5) 多源并发摘要
./batch.sh "https://www.kernel.org/" "https://ubuntu.com/security/notices"

# 6) 多语句（自带 return）
./run.sh --raw "var n=__E.grep('CVE',0,5); return n.slice(0,200);" --tab <ID>
```

`run.sh` 会自动把表达式包成 `return ( ... );`；`--raw` 用于多语句。其余参数原样透传给 `ema2 js`。

---

## 五、契约层摩擦点：处理结果（v2.4.2.6）

原则不变：核心不加功能，但**缝隙的摩擦系数会乘到所有扩展上**。
下表是 v2.4.2.6 对每条诉求的处置，以及没做的理由。

| # | 现象 | 处置 |
|---|---|---|
| 1 | `js` 没有 `--arg` | **不采纳**。`--arg` 不消除转义成本，只是把它从代码体搬进命令行；`run.sh` 已把这段成本摊成 3 行。它还会新增参数通道——与本节目录的原则相反。 |
| 2 | `batch js` 只有 `--code` | **已实现** `batch js --file`：客户端读一次再分发；`--code`/位置参数与 `--file` 互斥，空文件在发请求前报 `invalid_argument`。 |
| 3 | 多行即 async 函数体，必须显式 `return` | **已写进 `ema2 js --help`**：求值规则 + 多行必须 `return` + `note=ran as an async function body`。 |
| 4 | 页面级注入会「粘住」 | **已写进 `ema2 js --help`**（注入生命周期）；`lib.js` v3 额外回 `replaced=<旧版本>`，改完库没生效立刻可见。 |
| 5 | `--cursor` 是字符偏移 | **不改成不透明游标**。`docs/TOOLS.md` 与 `read --help` 已把字符偏移写成契约，`tabs list` 的 cursor 单位还是行号；改成不透明令牌会废掉「把 `next` 原样传回」这条最稳定的用法。改为在 `js --help` 点明「换 code / budget 续读会错位」。 |
| 6 | `open` 报 `status=pending timed_out=1` 但正文可读 | **延后**。`--wait dom` 已能直接表达「DOM 就绪即可」；是否再加 `content=ready` 字段，先用外围脚本验证 `--wait dom` 是否够用。 |
| 7 | `focused=0` 在每个 `batch open` 都出现 | **延后**。核心保留这个事实字段；读任务在外围过滤（`batch.sh` 现在只打印 `id/title/status/error`）。 |

第 2、3、4 条这一轮已经落地；第 1、5 条明确不做，理由是它们与「减少表面积」冲突。

---

## 六、实测记录

### 2026-10-22 · ema2 2.4.2.6（当前）

- `lib.js` v3：`js --file lib.js` → `lib=installed v=3 methods=meta,grep,outline,links,main,digest,tables`；
  紧接着 `js "return __E.outline(5)"` 直接可用（注入粘住），无需重新投递库。
- `batch js --file`：同一份「库 + `return __E.digest(300)`」脚本一次投递跑完所有标签；
  `--file` 与 `--code` 同时给 → `invalid_argument`（exit 1）；空文件同样在发请求前拒绝。
- `search.sh "Rust 2024 edition"`：`serp=ok engine=bing results=10 total=10 chars=1867`，
  10 条全部写进正文（无 `capped=1`）。
- `search.sh "C++ operator= && std::vector"`：`results=9 total=9 chars=1951` ——
  空格、`+`、`=`、`&&` 全部百分号编码，查询不再走样。
- `batch.sh` 两源：一次 `batch open` + 一次 `batch js --file`，逐源摘要；输出按 `url → id → 明细` 重排，无重复 `url=`。

### 2026-10-22 · ema2 2.4.2.5（旧记录，计数口径已变）

- `serp.js` 对 Bing：三次查询 10 / 9 / 9 条。**注意**：当时的 `results=` 是「去重后匹配数」，
  不是「实际写进正文的行数」；`--budget 2500` 下尾部可能已被 CLI 截断。
  这个数字不能当作「全部结构化命中」的证据；v2.4.2.6 起改为 `results`（已输出）+ `total`（匹配）+ `chars`。
- `__E.outline()` 对 LWN 首页：21 个标题 ~700 字符；`__E.grep('CVE')`：`hits=2 shown=3` ~400 字符。

---

## 七、脚本层已知限制（**不是核心的限制**）

- **付费墙 / 登录墙**：脚本在页面里跑，读到什么就是什么。LWN 订阅页 403，正文根本不存在，
  `grep` 也变不出来。
- **DuckDuckGo / Google**：前者对自动化返回挑战页，后者常有同意页；Bing 最稳，故设为默认。
- **卡片式列表页**：`__E.digest()` 的密度启发式可能只选中单张卡片；此时以 `heads=` 为准，
  或用 `__E.links()` 枚举条目。
- **`__E.main()` 是启发式**，不是可读性算法。输出里 `main=<节点>` 会标明命中谁，
  便于判断是否需要 `--selector` 兜底。
- **`search.sh` 用 `od` 做百分号编码**：Git Bash / Linux 自带，纯 cmd 不能直接跑这些脚本。

这些都属于「外围实现得够不够好」，与核心无关——正是这个目录该承担的部分。

---

## 八、边界声明

- 这些脚本**不应该**被核心吸收。如果哪天核心开始内置 `grep` 或搜索引擎适配，
  说明缝隙没磨平，而不是这些功能值得内置。
- 核心要做的是让**写扩展这件事足够便宜**。本轮落地的三条都是契约层改动：
  `batch js --file` 对齐投递方式、`js --help` 写清求值与注入生命周期、`--file`/内联的互斥规则。
  它们不新增命令，反而降低使用成本。
- 反过来，`--arg` 与不透明游标被明确拒绝：前者新增参数通道却不消除转义，后者会把一条
  已文档化的稳定契约改成破坏性变更。
- 外围证明：不碰核心，也能把检索做到可用的程度。

> Just do what you dream. — 但先把缝隙磨平，梦才不用反复交税。
