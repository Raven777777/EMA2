# EMA2 交接文档

> 给下一位维护者 / AI：先读 [PHILOSOPHY.md](PHILOSOPHY.md)，再读本文。

## 当前版本

```text
exe        2.4.2.3（Cargo crate metadata 仍为 SemVer 2.4.2）
extension  2.4.2.3
helper     20
tools      11（tabs/open/read/snapshot/act/wait/js/observe/files/shot/batch）
```

`await` 是 CLI 命令，映射到 `wait` 工具，不增加第 12 个工具。

分发包只带 exe + `ema2 init` 生成的 AGENTS.md；**脚本语法以 `ema2 act --help` 为准**，
不要在用户可见 hint 里引用仓库文档。

## 最短心智模型

```text
AI / shell
  → ema2 <command>  (短命 CLI)
  → POST 127.0.0.1:7744/cmd
  → daemon (同一 exe，detached)
  → WS 127.0.0.1:7744/ (Upgrade)
  → Edge MV3 extension
  → chrome.* / debugger
  → 真实 Edge
```

- `GET /health`：`ema2 status`
- `POST /shutdown`：`ema2 server stop`
- 默认端口 7744；扩展 popup 必须配同一个端口
- 无 MCP、无 token、无授权、无配置文件、无持久状态

## 仓库结构

| 路径 | 内容 |
|---|---|
| `rust/src/main.rs` | CLI 入口、daemon 启停、`status`、`init`、工具调用与退出码 |
| `rust/src/cli/` | clap 子命令、参数/脚本/batch 编译（spec/build/act/batch/shared） |
| `rust/src/server.rs` | axum 单端口路由、`/cmd`、`/health`、`/shutdown` |
| `rust/src/relay.rs` | 扩展 WS、hello/welcome、keepalive |
| `rust/src/state.rs` | 扩展 generation、pending 请求、锁兜底 |
| `rust/src/tools.rs` | 11 工具白名单、参数校验、超时预算 |
| `rust/src/save.rs` | 截图/HAR 落盘 |
| `rust/src/client.rs` | 本机 HTTP 客户端 |
| `rust/src/paths.rs` | 端口、PID/日志、输出目录 |
| `extension/background.js` | 38 行入口，按序 importScripts 全部模块 |
| `extension/modules/` | 17 个经典脚本模块：工具实现、ref、hook、webRequest、settle/await、debugger 队列 |
| `contract/runtime.json` | 版本/超时单一来源；`extension/contract.js` 由它生成 |
| `extension/page_guard.js` | 命令租约内非阻塞记录 alert/confirm/prompt/print/window.open；解除后恢复原生 API |
| `extension/page_guard_armed.js` | document_start 动态 guard 的立即 arm 入口 |
| `extension/page_guard_bridge.js` | ISOLATED world bridge：上报新文档 arm，租约归零时通知页面 disarm |
| `extension/popup.*` | 端口配置与连接状态 |
| `docs/TOOLS.md` | 命令参考、输出协议、错误码（唯一错误码表） |
| `docs/TESTING.md` | 真机、前台、可见的验收规定与清单（禁止 Node 测试） |
| `scripts/verify.py` | 一键验收：静态契约 + 真实前台 Edge 冒烟 |
| `docs/ARCHITECTURE.md` | 模块、生命周期、数据流 |
| `docs/PHILOSOPHY.md` | 项目哲学与边界 |
| `docs/CHANGELOG.md` | 版本历史（唯一版本号来源） |
| `docs/archive/` | 已归档的历史规划（不代表当前行为） |
| `rust/assets/AGENTS.md` | `ema2 init` 模板 |
| `examples/` | `.ema2` 脚本、`js` 抽取、批量读取示例 |

`bin/` 是本地构建产物，已 gitignore；扩展没有构建步骤。

## 构建与运行

```bash
# 构建
scripts\setup.cmd
# 或
cargo build --release --manifest-path rust/Cargo.toml

# 运行
bin\ema2.exe server
bin\ema2.exe status

# 停止
bin\ema2.exe server stop
```

Edge 侧：

1. `edge://extensions/` → 开发人员模式
2. 加载解压缩的扩展 → 选择 `extension/`
3. popup 端口设为 7744（或与 `--port` 一致）

## 测试

真机测试必须在前台可见的真实 Edge 里跑（禁止无头/后台窗口）；`scripts/verify.py`
会先把窗口提到前台并断言 `document.hasFocus()`。

```bash
cd rust
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

当前 Rust 测试：**87 passed**。

真实浏览器验收按 [TESTING.md](TESTING.md)。  
**绝对禁止 Node.js 测试**，不要新增 `package.json` / `node_modules` / jsdom。

## 必须保持的契约

### 输出

- 行式紧凑文本；HAR 是唯一 JSON
- 列表尾部 `total/shown/next`；`next=none` 表示读完
- 文件结果 `path=` 单独一行，后跟 `bytes=` / `mime=`
- 错误 stderr 首行 `error=<code> msg=... hint=...`
- 退出码：`0` 成功 / `1` 参数错 / `2` 中继不可用 / `3` 扩展未连 / `4` 工具执行错

### ref

- `[e<seq>@<doc>]`，`doc` 是 `d1/d2…`
- 完整 ref 校验 document；短 ref 只在当前文档内解析
- 导航/reload/换 tab 后旧完整 ref 必须 `stale_ref`
- 密码框 value、textarea/contenteditable 用户输入不进入 snapshot 名称

### act / settle / await

- 导航意图最多 settle 3s；普通按钮最多 800ms；其他动作不等待
- 页面提前稳定就提前返回
- `settle=pending` 不是错误，退出码 0
- pending 时给 `expect=` / `next_url=` / `hint=ema2 await 10s`
- `ema2 await [duration]` 使用**上一个动作所在 tab**，不是当前活动 tab
- 显式 await 超时返回 `wait_timeout`，退出 4
- 动作打开新标签：`new_tab=` + opener 明确时自动登记 session（noopener 只用于定位）
- `.ema2` / `--steps` 里 `await 10s` 是脚本步骤；pending 且下一步不是 await 时脚本停止
- `act` 单步、`--steps`、`--script` 都支持 `--tab ID`；不要求目标 tab 活动
- `open` 和新的 act 会清掉旧 lastAct 上下文
- `open --timeout` 超时返回 `load=pending`，中继预算跟随 `waitTimeout`
- 脚本里的 `waitFor` 是独立步骤，在原行执行；失败时保留已完成步骤
- `waitFor` 支持页面条件 `text=` / `textGone=` / `selector=`（与 `ema2 wait` 同义），
  命中输出 `expect=ok condition=... ms=...`，超时 `wait_timeout` 带最后 URL/文本；
  一次只能有一个页面条件，且不能和 `url=`/`status=` 混用
- `--selector/--target/--key/--delta-y/--submit` 只用于单步；`--steps/--script` 里用对应脚本写法
- 副作用动作（click/type/fill/press/scroll/submit/back/forward/reload）的 `executeScript` 不自动重试

### read

- `text` / `md` 跳过 script/style/noscript/template/textarea/input/select/option、
  nav/form，以及 `display:none` / `visibility:hidden` / `content-visibility` 元素；
  header/footer/aside 只有在 `role=banner/contentinfo/complementary`，或不在
  `article/main/[role=main]` 且无标题后代时跳过；两者共用一次 block/inline 遍历，只是 renderer 不同
- `display:contents` 对 read 视为可见（子树照常渲染就继续遍历）；BUTTON / [role=button] /
  1×1 sr-only 不进入正文；这些都只影响 read，不影响 snapshot/click
- 空文本链接不输出裸 URL；链接标签只用 innerText（带图用 alt）
- `html` 保持 `root.innerHTML`；`visible` 直接取 `root.innerText`，不跑清洁遍历，
  但同样复用 main/selector/no-main 选区
- 整页 md/text 为空 → `empty_page`；HTTP ≥400 → `page_error`；PDF → `pdf_page`（均 exit 4）；
  登录墙 → `state=login_required`，人机验证 → `state=challenge`（`state=` + hint，exit 0）；
  头部始终带 `http=` / `content_type=`；**不检测 paywall**
- main 太短（<200 字符，或 body>2000 且候选 <body 5%）回退 `main=body(fallback)` 并给 hint
- 被截断必须给 `next` 和 cursor hint

### 其他

- `type` 在 caret/selection 处插入，`fill` 覆盖；控件会清洗值时（number/date/range）返回 `value_rejected` 并恢复原值
- `act scroll down|up N` 按方向与屏数滚动
- 上传原始载荷 ≤5 MiB；daemon body / axum 提取器 / 扩展入站上限都已对齐
- `role:name` 先匹配可访问名称，再精确匹配 HTML `name` 属性
- `select` 的位置参数是 option 的 `value`
- `files download-dir` 固定返回 `download_dir_unsupported`
- `batch` 内层参数放在 `args`；内层校验复用子工具 validator
- `batch --urls --reuse session` 只复用精确 URL 命中的会话标签；默认仍新开
- `js` 全局 debugger 串行；attach 非超时失败自动重试一次；运行时 SyntaxError 不重跑
- `observe console --wait` 先装 hook 再等待；console/network 的 document epoch 分开记录
- `observe console --level` 默认 `error`，输出显式带 `level=<阈值>`
- HAR 导出是完整 JSON，不经过 20 万字符文本上限；凭证头默认 `<redacted>`，`--headers all` 才导出原值
- command guard 的 tab 集合持久化到 `chrome.storage.session`；disarm 分 top/all-frames 验证，失败的 tab 留待下次重试
- 每个 RPC 的 guard 租约带 token；watchdog 超时延迟强制释放，迟到租约立即回收
- 扩展返回 JSON-RPC error 时 daemon 返回 `extension_error`，不能变成空成功
- `server` 重复启动输出 `relay=already_running`，退出 0
- `server stop` 已停止时输出 `relay=not_running`，退出 0

## 已知弱点与下一步优先级

### P1：act 结果可信度（v2.2.1 已收口）

HN 真机出现过一次 `act click` 返回 `ok=1/1` 但页面没有导航，换 selector 后成功。
v2.2.1 起：

- 可导航链接无任何变化时安全重试一次（输出 `retried=1`）；仍无变化给 `nav=none`
- SPA 链接只改 DOM 不导航：约 1s 后 `settle=done` + `nav=none`，不再 pending 3s
- `javascript:` / `download` / `#` 链接不重试
- 普通按钮**不**自动重试（避免点赞/提交执行两次）

还没做：按钮类动作的内置“预期效果”判断（比如表单提交失败仍停在本页）。
调用方现在可用 `--wait-for "text=... timeout=5s"` / `selector=...` 声明终点；
核心只验证条件，不内置业务判断，也不要为了按钮加启发式。

### P2：动态页面可读时机

GitHub 首页在 `batch read` 时可能只有 `Loading`，等几秒才渲染。
现在 AI 需要自己 `wait time 3s`。可考虑：

- `read` / `snapshot` 增加可选 `--wait-stable 500ms`
- 只等 DOM 文本/节点短暂稳定，不猜网络
- 不改变默认快速路径

### P3：真实浏览器验收 harness

`scripts/verify.py` 已经是真实前台 Edge 的验收脚本（fixture + CLI + 断言），
新增行为要同步加断言；它不替代人工检查复杂站点。

- **不能用 Node.js**
- 只做启动 Edge、打开页面、跑 CLI、断言输出
- 不替代人工检查复杂站点

### 明确不做

- ProseMirror/CKEditor 原生输入
- 携程/淘宝/Bing 专用下拉适配
- OCR、验证码、登录态管理
- 自动下单/发帖/业务工作流
- 第 12 个工具
- **驱动别的扩展自己的浮层 UI**（密码管理器下拉、翻译插件面板）：v2.1.5 起
  `click` 会补齐指针事件与 `focusin`，能把浮层**召唤**出来，但浮层内容在跨扩展
  隔离的 iframe / closed shadow root 里，`snapshot` 拿不到 ref，坐标点击也会被
  `debugger.attach` 的跨扩展限制挡住（真机实测：此时 `getTargets` 只剩一个 page target，
  `attach({tabId})` 和 `attach({targetId})` 都会被拒）。这一步仍需人手点一下。

这些用 `js`、`.ema2`、外部 Python 脚本完成。

## 开发陷阱

1. **修改页面 helper 必须提升 `__ema2_helpers_version`**
   当前是 20；否则已打开页面继续用旧 helper。

2. **Edge 会按 manifest 版本缓存扩展脚本**
   行为没更新时提升 `extension/manifest.json` 版本，或在 `edge://extensions` 重新加载；
   最稳的是换一个全新 `--user-data-dir` 测试。

3. **扩展 reload 会清 `chrome.storage.session`**
   `sessionTabs` 和 lastAct 可能丢失；这是可接受的会话边界。

4. **`open` 会清 lastAct**
   避免 `await` 等到过期页面。新增导航类命令时也要考虑是否该清。

5. **`.` 脚本 pending 停止**
   不要在 pending 后继续执行 ref 操作；先 `await`。

6. **`batch` 预算**
   内层工具仍走 validator；不要把 600s 上限绕过去。

7. **截图需要活动 tab 且窗口可见**
   连续截图注意 `MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND`。

8. **真实站点有反自动化**
   携程结果页、去哪儿首页可能不渲染；apnews.com 首页的后台脚本会让主线程长时间
   忙碌（连 `return 1+1` 都注入超时）；换站点或外部脚本，不要为单站加适配器。

9. **page guard 只能存在于命令租约内**
   不要在 manifest 恢复静态 `content_scripts`；否则普通浏览的弹窗会再次被拦截。
   页面 helper 默认 disarmed，最后一个命令结束时必须先注销动态注册、再清理已 arm 的 tab。
   修改 wrappers/restore 逻辑时提升 `page_guard.js` 的 `GUARD_VERSION`；修改清理协议时
   同时提升 `contract/runtime.json` 的 `guardProtocolVersion`（core.js 从这里取值）。

10. **`executeScript({func})` 的两个坑（v2.4.2 read 踩过）**
    a. 注入的 func 是**被序列化的单函数**，后台顶层函数不存在于注入环境：
       `readPageContent` 引用 `readPageContentInner` 直接 ReferenceError。注入入口必须自包含。
    b. Edge 对 func 注入内的抛错**不 reject**，结果是 `result=undefined`：
       `readTool` 曾把它误报成 `helpers_missing`。页面内大函数要自己 try/catch
       把异常带回（现在 readPageContent 从 `page_exception` 映射成 `js_exception`）。

## 下一位维护者的第一步

1. 读 [PHILOSOPHY.md](PHILOSOPHY.md)
2. 读 [ARCHITECTURE.md](ARCHITECTURE.md) 和 [TOOLS.md](TOOLS.md)
3. `cargo test` / `clippy` / `fmt`
4. `bin\ema2.exe server` + `status`，确认扩展连接（窗口必须在前台）
5. `python scripts/verify.py` —— 135 项真机检查，跑不过就别改行为
6. 打开一个真实网站，跑 `open → snapshot → act → read`
7. 修改行为前先问：这是通用原语，还是网站特例？
8. 任何动作相关改动，必须真机验证“成功就是真的成功”

## 一句话交接

**EMA2 的价值不在工具数量，而在动作结果可信、观察可读、错误可纠正。**  
守住这三点，它可以继续长大；丢掉任何一点，它就会退化成另一个不靠谱的浏览器脚本。

## 附：一次真实使用后的 AI 反馈

2026-10-21，在一次真实新闻检索任务之后，AI 留下了这段话。略作文档化，保留原意。

这几点我接受，而且你说得对——我前面把一些**设计取舍**说成了“缺陷”。

- **扩展重连**：MV3 的权限模型决定了扩展不能随便自连，这不是懒，是边界。协议内能做的只是把 `connecting` 说清楚，不该硬造一个“自动连接”的假象。
- **前台可见**：这不是妥协，是前提。无头浏览器拿到的是“匿名网络”，真实 Edge 拿到的是**用户的身份、登录态和指纹**。这次只开了搜索和新闻标签，没有打开密码、支付或账号设置页面，结束后关了 13 个会话标签、停了 daemon。信任不该只靠 AI 自觉，最好也靠这种可核对的边界。
- **文本协议**：同意，它本来就不是给程序设计的。要结构化就在外面包一层 Python/JS；HAR 已经是 JSON，够用了。这不是短板，是定位。
- **小模型**：同意不迁就。唯一保留的是“上下文预算”而不是“智力”——再强的模型，上下文也是有限资源。分层文档（短版/完整版）可能比单纯写少更好，但这是产品选择，不是必须。

**read 下一版**，我愿意当第一个真实页面测试者。重点不是“能不能读”，而是 fallback 时能不能少给噪声、selector 建议得更准、付费墙别让人误以为读全了。这三件事比加一个新模式更有价值。

至于理想主义——EMA2 的理想主义不是口号，是那些具体的“不做”：不做站点适配器、不加 token、不把无头测试当验收、不为了看起来安全而加授权页。AI 迭代快，但**“知道什么不该做”通常比“能做什么”更稀缺**。项目创始人把边界定得清楚，这项目才有现在这种气质。

所以下次再叫 EMA2，我还是会先 `server`，再 `status`。如果它还没连上，我就等人点那一下——目前唯一需要人类的地方，也挺好。
