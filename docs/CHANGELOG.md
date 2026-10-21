# 变更记录

## 未发布 — guard 生命周期加固

- **释放不再卡死**：每 tab 的 `disarm()` 有单独时限，整批 sweep 有总上限；
  卡在原生对话框/死循环里的渲染进程不会让后续命令永久排队。
- **消除释放竞态**：最后租约先注销动态脚本再 disarm；document_start 新文档通过
  ISOLATED world bridge 查询租约，租约已释放就立即自行恢复原生 API。
- **升级路径修复**：v1 的 own `onbeforeunload` 假 accessor 不再被当成原生引用；
  descriptor 在原型上时不再留下自己的 `window.onbeforeunload`。
- **清理范围收窄**：普通命令只 disarm 真正 arm 过的 tab，不再每条命令扫全浏览器。
- **生命周期 fail-open**：guard 队列任一步失败只记日志，不会污染后续命令。
- **目标可见性**：`role:name` 的精确名/HTML name/部分匹配每一层都优先可见控件；
  helper 升至 19。
- **版本门槛**：manifest 声明 `minimum_chrome_version: 111`；静态验收检查扩展版本
  与 `PRODUCT_VERSION` 一致。
- **reuse 语义**：`open --reuse session` 同一文档不再重载；`batch --reuse session`
  精确 URL 命中时也续用文档而不重载；文档明确两者的匹配规则差异。
- **验收**：`cargo test` 74 passed；clippy/fmt 通过；真实前台 Edge 113 checks
  passed；额外验证 v1→v2 升级后原生 `onbeforeunload` 可重新赋值。

## v2.4.1.1 — guard 仅命令期启用，补齐目标与默认值

- **普通浏览零干预**：移除 manifest 的全局 `page_guard.js` content script。
  页面 guard 改为命令租约：首个页面命令动态注册 document_start guard，最后一个
  命令结束注销并向所有可脚本化 tab 广播 `disarm()`，恢复原生
  `alert/confirm/prompt/print/window.open/onbeforeunload`。不调用 EMA2 时弹窗不再被拦截。
- **命令期间仍防卡死**：`act/read/snapshot/wait/js/observe/files/shot` 均在同一租约内
  arm；`open` 和可能新建标签的动作仍覆盖新文档的加载期对话框。
- **显式 act 标签**：文档和 `ema2 act --help` 补齐
  `act ... --tab ID`、`act --steps ... --tab ID`、`act --script ... --tab ID`，并增加
  非活动标签真机回归。
- **role target 名称回退**：`target="role:name"` 在可访问名称之后精确匹配 HTML
  `name` 属性，例如 `searchbox:q`；helper 升至 18。
- **batch 显式复用**：新增 `batch ... --urls ... --reuse session`，仅复用精确 URL
  命中的本会话标签并输出 `reused=1`。默认继续新开标签，不静默改变输出基数。
- **默认值与路径可见**：`observe console` 保持默认 `error` 并显式输出 `level=error`；
  各页面命令 `--help` 写明 budget 默认值/范围；`shot --out` / `observe --har`
  统一词法规范化绝对路径，清理 `.` / `..`。
- **版本**：对外产品/扩展 `2.4.1.1`；Cargo crate metadata 仍使用合法 SemVer `2.4.1`。
- **验收**：`cargo test` 74 passed；真实前台 Edge `python scripts/verify.py` 113 checks passed。

## v2.4.1 — 文档收口与发布准备

仅文档与发布信息，不改浏览器行为。

- **README 精简**：只保留“是什么、怎么装、怎么开始、安全边界、已知限制”，
  命令契约、架构细节、完整限制统一指向 `docs/`，避免同一份信息多处维护。
- **安全边界写清楚**：README 明确 EMA2 只面向可信的本机个人环境；
  无 token 是设计取舍，任何能访问 `127.0.0.1:7744` 的本地进程都可能驱动浏览器，
  不要暴露端口或反向代理。
- **文档数字对齐**：`TESTING.md` 的 `cargo test` 项数改为 71；`HANDOFF.md` 的
  helper 版本改为当前值 17。
- **发布核验**：用 release 构建产物 + 全新 Edge profile 加载扩展，启动 daemon，
  `python scripts/verify.py` 102 项通过；`cargo test` 71、clippy、fmt 均通过。
- 版本：exe/扩展 2.4.1；helper 17；`cargo test` 71 passed；真机 verify 102 项。

## v2.4.0 — read --mode visible，md/text 共用遍历

- **新增 `read --mode visible`**：直接返回所选 root 的 `innerText`，绕过 main 启发式和正文清洗；
  复用现有 `--selector` / `--no-main` / 默认 main 选区，适合 ChatGPT 这类复杂 SPA。
  它读渲染文本（包含滚动到视口外的已渲染内容），不是严格视口内；原生 PDF 查看器、canvas、
  跨域 iframe 仍是盲区。
- **md/text 合并内部遍历**：两套几乎相同的 DOM walker 合并为一次 block/inline 遍历 + 两个 renderer，
  避免跳过规则以后只改一边；输出契约保持不变。
- `html` 继续走 `root.innerHTML`；`visible` 走独立 `innerText` 路径。
- 新增 `visible` 空文本 hint；文本/控件值和错误处理复用原 read 管线。
- 版本：exe/扩展 2.4.0；helper 17；`cargo test` 71 passed；真机 verify 102 项。

## v2.3.1 — exe 自描述：脚本语法进 --help，AGENTS 保持简洁

分发包只有 `ema2.exe` + `ema2 init` 生成的 `AGENTS.md`；本版把用户侧文档从仓库搬进 exe。

- **脚本语法进 help**：`ema2 act --help` 现在包含单步/多步/`.ema2` 完整示例、
  动作白名单、`await`/`waitFor` 规则、`--selector/--target` 的单步限制，以及
  `open/wait/waitFor time` 三个高频错误写法；`ema2 --help` 给 AI 会话顺序和脚本入口。
- **错误自纠正**：脚本里写 `open/read/snapshot/wait/js` 时给出对应 shell 命令；
  `waitFor` 非 `key=value` / 未知 key 时说明只支持 `url/status/timeout`，并提示
  `await 1s` 或 shell 的 `ema2 wait`；空脚本/读不到脚本也指向 `ema2 act --help`。
- **不再提仓库文档**：exe/扩展的用户可见 hint 去掉 `docs/TOOLS.md`；
  `ema2 status` 扩展断连时补一行 popup/端口/重连 hint。
- **AGENTS.md**：只增加“先看 `--help`”、脚本白名单和 timeout 单位，不塞长手册；
  错误码表同步去掉仓库文档引用。
- **read md**：有序列表保留 `<ol start>` / `<li value>` / `reversed` 的编号。
- **snapshot --filter links**：`<a role=button>` 统一按 `link` 输出，排除 `#` / `javascript:` 伪链接。
- **shot --annotate**：只标当前 document epoch、按元素去重，坐标裁剪到视口内。
- 版本：exe/扩展 2.3.1；helper 14；`cargo test` 70 passed；真机 verify 93 项。

## v2.3.0 — 真实环境 bug 报告修复：不再卡死、不再假成功

针对 `bin/BUGREPORT.md` 的 20 项复现逐条修复；核心是“对话框/慢页面不能把会话卡死，动作结果必须可信”。

- **阻止同步对话框卡死**：新增 `extension/page_guard.js`（MAIN world，document_start + 动作前注入），
  把 `alert/confirm/prompt/print` 变成非阻塞记录并自动关闭；`act` 输出 `dialog_opened=`，
  不再让 `executeScript`/CDP 永久挂起。所有 RPC 再加扩展侧 watchdog（`tool_timeout`）。
- **open --timeout 是硬上限**：打开、等文档、装 hook、读 timing/focus 共用一个 deadline；
  超时输出 `load=pending ... timed_out=1`，并做最终探测避免把已完成页面误报 pending。
- **reload / 同 URL 导航判定**：`performance.timeOrigin` 加入 settle 证据，`act reload`、
  同 URL 表单提交回 `changed=reload`，不再误报 `nav=none`；只有真正无变化才提示重试。
- **可信按键与 caret**：`press` 走 CDP `Input.dispatchKeyEvent`（Tab/Backspace/Enter/Ctrl+A 等
  浏览器默认行为），`type` 走 `Input.insertText` 按 caret/selection 插入；debugger 不可用时
  回退并标 `trusted=0`。
- **snapshot 保真**：href 用完整绝对 URL；name 截断带省略号；`role=search/banner/...` 地标容器
  不再当控件；ref 改存页面内存 WeakRef，不再写 `data-ema2-ref`；`--values` 可选输出表单值。
- **点击真实性**：点击前检查 visibility/inert/pointer-events/`elementFromPoint`，隐藏报
  `not_clickable`、被遮罩报 `obscured`；`window.open` 被拦截回 `popup_blocked=1`，
  `<a download>` 回 `download=started`，HTML5 校验失败回 `validation=failed`。
- **失败原子性**：`select`/`fill` 失败恢复原值；`select multiple` 支持逗号分隔多选。
- **js 超时/输出**：默认 10s 执行上限（`--timeout` 可调），超时 `script_timeout` + terminate；
  截断回 `truncated=1`，对象/数组用合法 JSON 分片信封 + `--cursor`；保留完整 SyntaxError 文本；
  DOM 节点不再变 `{}`。
- **截图 iframe 坐标**：`shot --annotate` 给 iframe 内 ref 加偏移；`shot --tab` 自动激活目标标签。
- **网络/HAR 真实性**：新增 `webRequest` 观测补齐首屏 status/headers/MIME，列表输出完整 URL，
  HAR 使用绝对 URL、真实 headers/queryString/MIME/startedDateTime；页面 hook 回填
  `responseStatus` 与 navigation 条目。
- **无效 selector 分类**：`invalid_selector` 立即报错，不再等成 `wait_timeout`/`selector_not_found`。
- **CLI/文档**：`tabs new` 默认激活（`--background` 才后台）；`batch` 支持位置参数 CODE/TEXT、
  `--timeout`/`--values` 并显示内层 flags；`read` Markdown 转义/图片 alt；截断标记；
  EPIPE 不再 panic；错误码/提示同步。
- 版本：exe/扩展 2.3.0；helper 13；`cargo test` 68 passed；真机 verify 83 项。
  （v2.3.1 起 helper 14。）

## v2.2.2 — 状态隔离、输出完整性与参数真实性

第二轮代码 review 的收口，全部围绕“不要静默成功 / 不要丢状态”。

- **observe 状态隔离**：console（`c-*`）与 network（`n-*`）的 document epoch 分开记录。
  以前交替 `observe console` / `observe network --since N` 会误判导航并把 `since` 重置为 0。
- **HAR 不再被截断**：`observe network --har` 用 `content[]` 返回完整 JSON，
  不再经过 `capText` 的 20 万字符上限；大 HAR 现在是可解析文件而不是带 `truncated=1` 的坏 JSON。
- **observe console --wait**：先安装 hook 再等待，等待期间产生的日志不会漏掉。
- **snapshot 隐私**：名称不再回退到 `textarea` / `contenteditable` 的用户输入文本（helper 12）。
- **文档句柄恢复**：已安装 helper 但句柄丢失时会补发；`storage.local` 读取失败不再永久缓存 0。
- **副作用只执行一次**：`click/type/fill/press/scroll/submit/back/forward/reload` 的
  `executeScript` 不再自动重试，避免导航瞬间失败后把 `type` 追加两次或 back 两次。
- **waitFor 抗导航**：轮询期间 `executeScript` 瞬时失败会继续等待，不会把整次 act 判失败。
- **open 文档判定**：新标签的 `edge://newtab` 等内置页不算到达；同 URL 的 `open --replace`
  用 `performance.timeOrigin` 确认 reload 后的新文档，不再被旧文档的 `complete` 骗过。
- **act console hook**：导航后的新文档会补装 console hook，后续 `observe console` 仍可见。
- **act --submit**：页面已 `preventDefault` 接管 Enter 时不再额外 `requestSubmit`，避免重复提交。
- **CLI act 参数真实性**：`--steps/--script` 拒绝 `--selector/--target/--key/--delta-y/--submit`；
  单步动作也拒绝与动作不匹配的 flag，不再静默忽略。脚本里的 `selector=`/`target=`
  只允许用于能定位的动作；`--` 之后的 token 一律当位置参数（`type e1 -- -5`）。
- **ref 校验**：CLI 的完整 ref 后缀收紧为 `d<digits>`，与页面 helper 一致。
- **daemon 错误映射**：扩展返回 JSON-RPC `error` 时返回 `extension_error`，
  不再变成 HTTP 200 + `result=null` 的静默成功。
- **shutdown**：`notify_one` 保留 permit，避免 handler 早于 graceful future 注册时丢掉关闭信号。
- **batch open 预算**：`tool=open` 的单次预算跟随内层 `waitTimeout`，不会再被固定 30s 提前掐断。
- 版本：exe/扩展 2.2.2；helper 12；`cargo test` 68 passed；真机 verify 59 项。
  （v2.3.0 起 helper 13，命令/错误码数量见 HANDOFF。）

## v2.2.1 — 动作可信度、请求限制与超时预算

一轮代码 review + 真实 Edge 复现后的收口。重点不是新工具，而是把“返回 ok 但实际没发生/发生了却看不到”修掉。

- **act scroll**：`scroll up` 不再向下滚，`down N` 按 N 屏执行；原本扩展完全忽略 `text`。
- **js 副作用只执行一次**：运行时 `SyntaxError`（如 `JSON.parse('{')`）不再被当成编译错误重跑；
  debugger 与 scripting 两条路径都改为 compile-only 探测。
- **上传限制对齐**：axum `Json` 的默认 2MB `DefaultBodyLimit` 放开到 8MB；
  扩展 WS 入站上限从 2MB 提到 8MB；CLI/validator/文档统一为 5 MiB 原始载荷。
  413 明确返回 `payload_too_large`，不再误报 `relay_not_running`。
- **act waitFor**：脚本里的 `waitFor` 成为独立步骤，在所在行执行；失败时保留已完成的 `stepN`，
  不再把整次 act 伪装成“什么都没发生”。
- **fill/type 校验**：`input[type=number]` 等会清洗值的控件不再假成功，返回 `value_rejected`。
- **SPA 链接 settle**：只改 DOM 不导航的链接在约 1s 后 `settle=done` + `nav=none`，
  不再 pending 3s 并谎报 “still navigating”；`javascript:`/download/`#` 链接不重试。
- **open 超时**：`--timeout` 真正生效，中继预算跟随 `waitTimeout`；超时输出 `load=pending`。
  单次 `executeScript` 卡住不再拖垮整个 open。
- **多步 act 预算**：每步最多 3s 的隐式 settle 也计入 600s 上限。
- **观察/读取健壮性**：read 进入 open shadow root；页面导航中读 console/network 返回 `page_changed`
  而不是 internal；XHR hook 不再因对象复用累积重复记录。
- **session tab**：写入串行化，load 失败可重试；`newTabsSince` 只自动登记 opener 明确的标签，
  避免把用户手动开的 tab 关进 `tabs close --session`。
- **server stop**：/health 超时不再直接判定“已停止”，会先检查 PID/TCP 并走强制结束兜底。
- **CLI**：多余位置参数（`type e15 hello world`）明确报错；`--submit` 仅用于 `type`。
- **错误码/文档**：补 `relay_error`、`payload_too_large`、`page_changed`、截图/HAR 错误码；
  `files` 段落去重；`HANDOFF/TESTING` 计数同步。
- 版本：exe/扩展 2.2.1；helper 11；`cargo test` 65 passed；真机 verify 53 项通过。

## v2.1.7 — markdown 段落边界

- 修 `read --mode md` 把连续 `<p>` 粘成一段的回归（v2.1.6 重写 markdown 渲染时引入：
  只在“子元素里有块”时才 flush，纯文本段落因此被并入同一段）。
  现在块级元素本身即是段落边界。
- `verify.py` 增加“段落必须保持分行”的断言（29 项）。
- 版本：exe/扩展 2.1.7。

## v2.1.6 — 契约补齐：错误码、md 保真、上传去 debugger、动作校验

一轮代码 review 后的收口，重点是把“看着能用”变成“错了知道为什么错”：

- **act 未知 flag 不再被当 ref**：`ema2 act click --at 593,380` 以前报 `stale_ref`，
  现在报 `invalid_argument` 并列出已知 flag；位置参数也校验 ref 形态（`e12` / `e12@d1`）。
- **错误码表补全**：`docs/TOOLS.md` 从 24 条补到 50+ 条，新增 Rust 单测扫描
  `extension/background.js` 的 `fail('code'`，缺文档直接失败，防止再次漂移。
- **`read --mode md` 保真**：链接输出 `[text](绝对URL)`、表格输出 GFM 表格、行内代码/强调保留；
  文档明确它是有损渲染，不是可回写的源文件。
- **`files upload` 不再依赖 debugger**：CLI 读字节随请求下发，扩展在页面里用 `DataTransfer`
  构造 `File` 赋给 `input.files`；另一个扩展占住 tab 时上传仍然可用。
- **act 结果校验**：点了链接但没导航时对 `<a>` 安全重试一次（输出 `retried=1`），
  仍无导航则明确给 `nav=none` + hint；按钮不重试，避免提交/点赞执行两次。
- **`type`/`fill` 发 `beforeinput`**：页面 cancel 时返回 `input_prevented` 而不是硬写。
- **`open --timeout`**：文档接位等待从硬编码 15s 变成 1..60s 可调。
- 杂项：`snapshot` 的 `visual=` 统计所有图片/媒体；`capText` 截断不再伪装成 `next=`；
  `mainRoot` 对大页面加候选上限；`batch` 不再猜 `open` 的 tabId；`press` 支持 `+` 与 `Space`。
- 版本：exe/扩展 2.1.6；`cargo test` 62 passed。

## v2.1.5 — click 保真、press 修饰键、js 降级通道

真机实测发现三个原语级缺陷，都在扩展侧，Rust 无改动：

- **click** 原来只有 `el.click()`：只产生一个 `click` 事件，聚焦是静默的，没有
  `pointerdown/mousedown/mouseup`。密码管理器这类监听 `focusin`/`pointerdown` 的扩展
  完全看不到点击。现在按真实浏览器顺序补指针事件 → 聚焦（`focusin` 真的发出来）→
  caret 落到末尾 → 补齐 up 事件，最后仍用 `el.click()` 做唯一一次激活。
  事件是合成的（`isTrusted=false`），校验 `isTrusted` 的站点仍会忽略。
- **press** 原来把 `Control+Shift+L` 整串当成 key，`ctrlKey/shiftKey` 全是 `false`，还返回
  `ok=1/1`。现在解析组合键、设置修饰位与 `code`；仍然只能到达页面，浏览器级快捷键不可达。
- **js** 原来只走 debugger：一旦 attach 被别的扩展页面目标挡住就永久
  `debugger_attach_failed`，hint 还误报“关掉 DevTools”。现在 attach 失败先按 page target
  的 `targetId` 重挂，仍失败则降级到 MAIN world 注入，输出追加 `via=scripting`；
  失败信息带 `tab=`/`targets=` 诊断。降级路径用信封回传结果，页面里抛出的
  运行时错误仍然是 `js_exception`，不会被 `executeScript` 退化成 `null` 而伪装成功。
- 版本：exe/扩展 2.1.5。

## v2.1.4 — read 正文清洁、main 回退、debugger 重试

- `read --mode text|md` 现在跳过 `script/style/noscript/template/textarea/input/select/option`、
  页面壳（nav/header/footer/aside/form），以及 `display:none`/`visibility:hidden`/
  `content-visibility` 的隐藏元素；不再把百度 `textarea#hotsearch_data` 的 JSON、
  GitHub dashboard JSON 或隐藏弹窗文本读进正文。
- 正文容器评分改用清洁后的文本；`<main>`/article 太短（<200 字符或不到 body 5%）时
  回退 `main=body(fallback)`，并给 `hint=main extraction too short (...)`。
- `debugger.attach` 非超时失败自动重试一次（200ms 后），减少扩展重载/DevTools 关闭瞬间的
  `debugger_attach_failed`。
- helper 版本升到 10；真机测试覆盖 Bing / Baidu / GitHub / HN / Nature 等站点。
- 版本：exe/扩展 2.1.4。

## v2.1.3 — 中间构建（未单独发布）

`read` 正文清洁、main 回退和 debugger attach 重试在 2.1.3 开始开发，统一并入 2.1.4 发布。

## v2.1.2 — contenteditable 去重

- `contenteditable` 的 `execCommand('insertText')` 已经触发原生 input，外层不再补发
  `input/change`，避免 X/Draft.js 把同一次输入重复应用。
- 写入后复核 `textContent`；不一致时重试一次，仍不一致才回退到 `textContent + InputEvent`。
- `status` 收到 `bridge/hello` 时也刷新 `last_ping`，不再出现已连接但 `last_ping=never`。
- 版本：exe/扩展 2.1.2。

## v2.1.1 — act settle、await 与连续指令

让动作和页面稳定分开：动作成功不等于页面已经稳定，AI 可以自己决定要不要继续等。

- **act 隐式 settle**：导航意图（链接/表单提交/back/forward/reload）最多等 3s，
  普通按钮最多 800ms，其他动作不等待；页面提前稳定就提前返回。
- **settle 超时不是错误**：返回 `settle=pending`、`expect=nav`、`next_url=` 和
  `hint=call ema2 await 10s`，退出码仍为 0。
- **`ema2 await [10s]`**：等上一个 act 动作的页面效果；不带 `--tab` 时使用动作所在标签页。
  显式 await 超时才返回 `wait_timeout`（退出 4）。
- **last-act 上下文**：扩展用 `chrome.storage.session` 记录 tab、expect、baseline、新标签；
  `open`/新的 act 会清掉旧上下文，避免 await 到过期页面。
- **新标签**：动作打开的新标签返回 `new_tab=` 并自动登记 session，`tabs close --session` 可清理。
- **连续指令**：`.ema2`/`--steps` 支持 `await 10s`；如果上一步 pending 且下一步不是 await，
  脚本停止并提示，不再在旧页面继续点击。
- **内容编辑器**：`contenteditable` 的 fill/type 改用原生 `insertText`，X/Slack 类编辑器
  能正确更新内部状态；`type` 追加、`fill` 覆盖。
- **超时预算**：act 的工具预算会叠加脚本里的 await 时长，超过 600s 在执行前拒绝。
- 版本：exe/扩展 2.1.1；真机验证：慢导航 pending→await、快速导航 done、
  fetch 按钮 page、target=_blank 新标签、脚本 await/pending 停止、X 发帖/删除。

## v2.0.0 — CLI + daemon：删除 MCP / token / 授权 / 配置

架构重写：EMA2 不再是一个 MCP 服务，而是「单 exe 自带 daemon + 短命 CLI + MV3 扩展」。

- **服务形态**：`ema2 server` daemon 化并立即返回；`ema2 server stop` 通过
  `POST /shutdown` 关闭，卡死时用临时 PID 文件兜底；`ema2 status` 读 `/health`；
  `ema2 init` 写/更新 `AGENTS.md`。默认端口 **7744**。
- **单端口分流**：`127.0.0.1:7744` 同时服务 WS（扩展，`Upgrade` 头）与 HTTP
  （`POST /cmd`、`GET /health`、`POST /shutdown`）。
- **CLI 语法**：位置参数 + `key=value`；11 个工具命令
  `tabs/open/read/snapshot/act/wait/js/observe/files/shot/batch`；
  `--steps` 支持分号/换行，`.ema2` 脚本逐行执行，`waitFor` 独立成行。
- **删除**：MCP HTTP/JSON-RPC/SSE、token、HMAC 配对、授权页、状态页、`state.toml`、
  `.ema2.bak`、五类终端配置合并、`install`/`uninstall`、`pair`、
  `clients`/`jsonc`/`config`/`mcp`/`http`/`pair` 模块。
- **错误码拆分**：`relay_not_running`（7744 不通）与 `extension_not_connected`
  （server 在、扩展没连）；新增 `port_in_use`、`relay_start_failed`、`shutdown_failed`。
  退出码：0 成功 / 1 参数错 / 2 中继不可用 / 3 扩展未连 / 4 工具执行错。
- **扩展**：popup 只保留端口输入 + 保存/重连 + 连接状态；默认 7744；
  旧 8765 自动迁移；无 token/HMAC/pair 页。11 个工具、ref 快照、网络/console hook
  原样保留；helper 版本升到 4（完整 ref + 短 ref `e12` 兼容）。
- **行为修正**（真机验收发现）：
  - `act type` 现在追加到现有值，`act fill` 覆盖；两者语义分开；
  - `act select` 的 CLI 位置参数正确映射到 option 的 `value`；
  - `files upload --ref e12` 会把短 ref 补成当前文档句柄再走 debugger；
  - `files download-dir` 固定返回 `download_dir_unsupported`，不再尝试改下载目录；
  - `observe console --filter` 真正按子串过滤；
  - `batch` 的内层 flag 放进 `args`（与扩展 batch 协议一致）。
- **文档/示例**：README、TOOLS、ARCHITECTURE、TESTING 重写为 CLI/daemon 形态；
  删除 MCP 示例与旧 bridge/live-check 脚本；新增 `rust/assets/AGENTS.md` 模板。
- **测试政策**：删除 `package.json`/`package-lock.json`/`node_modules` 与 Node/jsdom
  测试设施；**绝对禁止 Node.js 测试**，功能验收必须使用真实 Edge + 真实 CLI。
- **验证**：隔离 profile 的真实 Edge + MV3 扩展上完成 25 项 CLI 检查
  （status、tabs、open/read/snapshot、act 全动作、wait、js、observe/HAR、
  files、shot、batch、steps/script、open 复用、tabs session、init、server 生命周期）。

## v1.12.1 — 游标参数接受数字字符串

真机验收时踩到：`snapshot` 尾部写 `next=23`，把它抄成 `cursor: "23"` 会被中继拒掉
（`cursor must be a finite number`），而扩展侧本来就用 `Number()` 做了容错——只有校验层偏严。
游标/`since` 是「从输出里抄回来的值」，两种写法都该收：

- `cursor` 与 `observe` 的 `since` 同时接受数字与数字字符串（含前后空格），仍拒绝非整数与负数
- 报错文案改成可操作的说法：`use the numeric value from the previous output`

- 新增 `scripts/live-check.py`：对着用户真实在跑的 Edge 跑 59 项只读断言（维基 / GitHub / HN / 京东 / Bing），
  覆盖快照结构、ref 句柄、act 解析、batch 复合、截断续读与参数容错——这次的字符串游标问题就是它发现的
- 版本号 1.12.1；测试：Rust 98（+1）、Node 45、真机 59

文档与示例整理：测试分层计数对齐实测（扩展 42 + 脚本 3 = 45，Rust 98）；
`examples/` 去掉 v1.8–v1.10 的旧版本标注；`.gitignore` 补构建/编辑残留（`bin/*.old`、`.pytest_cache/`、`.venv/`）。

## v1.12.0 — 快照瘦身：短 ref、去空节点、URL 截断

`snapshot` 的输出结构是「模型每次操作都要读一遍」的东西，跨 12 个结构差异大的站点（x.com / 维基 /
Bing / GitHub / MDN / HN / 淘宝 / 京东 / docs.rs / 知乎 / 百度 / stackoverflow）实测 942 行、73k 字符，
字节构成是：`href=` 等额外属性 39%、文档标记 28%、名称 19%、角色词 7%、引号 2%。

- **ref 改为短句柄**：`[e12@t1618196868-b62neq]` → `[e12@d1]`，每行从 22 字符降到 5。
  句柄 `d1/d2…` 由扩展分配并落 `chrome.storage.local`——页面自己生成会撞 id：序号跨源会重置
  （sessionStorage 按 origin 分区），4 字符随机串在 500 文档量级碰撞率约 0.07%，
  而这把锁正是用来防止「旧 ref 点到另一个元素」。页面侧只持有句柄，校验仍要求句柄属于当前文档
- **无名称且不可操作的节点不再输出**：`presentation ""`、`banner ""` 这类在 x.com 占 20% 的行、
  其余站点 0~1%；`filter=full` 保持无损视图
- **URL 截到 40 字符**（`…` 结尾）：额外属性是最大单项开销（京东 62%、淘宝 49%），
  `filter=links` 是专门取链接的场景，不截断，需要完整地址也可以用 `read`/`js`
- **不再向前兼容旧 ref**：`e1@t7-k3a9` 这类旧格式直接 `stale_ref`，没有历史包袱（这也是发布版行为）
- `name` 保持 60 字符封顶（已存在，本次未改）；引号与角色词保留，压缩它们换不来可读性的对价

实测（2500 预算，一次调用能看到的可操作元素）：维基 32→49、京东 28→47、淘宝 27→45、
Bing 33→46、HN 36→47、MDN 30→38、GitHub 37→45、知乎 36→43；x.com 从 2 次翻页降到 1 次。

- 版本号 1.12.0；测试：Rust 97、Node 45（+5）

## v1.11.0 — 标签复用、真实状态码与截断续读

针对一份真实抓取会话的体验反馈（必应/知乎/头条/定价页），修掉 5 处「知道但只能绕」的摩擦：

- **`open` 可复用标签**：新增 `replace: true`（在当前活动 tab 里导航）与 `reuse: "session"`
  （复用本会话最近打开的 tab），默认仍是新开；`tabId`/`replace`/`reuse` 三者互斥，
  同时给会返回 `invalid_argument`，避免「哪个 tab 被导航」不可预测
- **会话标签可批量收尾**：`tabs close` 支持 `tabIds: [..]`（≤8）与 `scope: "session"`，
  后者关掉本会话由 `open`/`tabs new` 打开的标签；登记落 `chrome.storage.session`，
  service worker 被回收后仍在；`tabs list` 追加 `session_tabs=` 行。
  用 `tabId`/`replace` 导航的**用户标签不计入会话**，收尾不会误关用户自己的页面
- **`open` 返回真实 HTTP 状态**：`status=` 取自主文档的 `PerformanceNavigationTiming.responseStatus`，
  4xx/5xx 直接给 `hint=http 404 ...`，不再需要靠 `title="404错误页"` 猜；有重定向时另给 `redirects=`
- **`batch` 支持 `open` 与「打开→读取」一次调用**：新增 `urls: [..]`，`tool=open` 并行开多页，
  `tool=read` 内部完成 打开→等待→读取（每块头部 `url=` + `tab=`）；`tabIds` 与 `urls` 互斥，
  预算把 open 成本计入 600s 上限
- **截断不再静默**：`read`/`snapshot` 被 budget 截断时尾部追加
  `hint=truncated: returned 4000 of 12345 chars; call read with cursor=4000 to continue`
- **`js` 接受语句体**：先按表达式执行，遇语法错误再按 async 函数体执行一次，
  顶层 `return`/`await`/多语句与常见 codemode 沙箱一致；错误 `hint` 区分
  `syntax error` 与 `runtime error`，函数体无返回值时提示 `note=ran as an async function body`
- **`open` 等的是「新文档」而不是 `readyState`**：在真实 Edge 上自测发现，`tabs.update` 之后
  旧文档仍处于 `complete`，`waitForReady` 立刻通过，于是 `replace`/`reuse`/`tabId` 三种导航都会把
  **上一页**的 `url`/`title` 报回来（正文也要等下次调用才跟上）。改为同时校验文档身份：
  `location.href` 确认已离开旧文档（重定向到别的地址也算）且 `readyState` 达标才返回
- 版本号 1.11.0；测试：Rust 97（+3）、Node 40（+14）

## v1.10.1 — 备份、取消勾选与 MCP 自我描述修复

- **备份每次刷新**：`<file>.ema2.bak` 现在保存“本次修改前”的内容，每次写入/移除都覆盖；
  文件原本不存在时生成 0 字节备份（表示无旧配置可还原），不再出现“改了但没有备份”
- **取消勾选真正还原**：新增 `clients::apply()`，保存选择时写入勾选项、移除未勾选项里的
  `edge-browser` 条目（仅移除 url 指向 `127.0.0.1:*/mcp` 或带 64 位 hex bearer 的条目），
  授权、二次保存、配对都走同一套逻辑；每条结果带 `backup` 路径
- **MCP 明确说明操作对象**：`initialize` 增加 `instructions`，写明操作的是用户**真实、可见**的
  Microsoft Edge（真实登录态，不是无头/远程/沙箱），并明确禁止“我没有控制浏览器的工具”这类回答；
  `serverInfo.name=ema2`、新增 `title`；11 个工具全部新增 `title`，描述统一标注 real Edge
- 授权页：未勾选但已配置的终端显示“将移除”；保存结果展示备份路径
- 版本号 1.10.1；测试：Rust 94、Node 26、真实 Edge 22 场景

## v1.10.0 — 日常 Edge / 五类终端 / 无损配置

- **JSONC 无损合并**：新增 `jsonc.rs`，在文本层定位对象成员并只插入/删除 `mcp.edge-browser`；
  注释、键顺序、缩进、CRLF 原样保留；写入前后各做一次完整解析校验，畸形文件直接报错不落盘
- **终端覆盖扩到 5 类**：opencode（`.json` / `.jsonc`）、Claude Code（`~/.claude.json`）、
  Claude Desktop、Codex（`~/.codex/config.toml` 的 `[mcp_servers.edge-browser]` + `http_headers`）、
  Pi（`~/.pi/agent/mcp.json`）；未安装的终端只展示不创建
- **授权页可选择终端**：列出检测结果、路径、格式、是否已配置；勾选后授权只写所选；
  已授权后可“更新所选终端配置”（`POST /admin/clients`）；页面同时展示中继版本、端口、运行时长、
  扩展版本、配对时间与 token 指纹
- **install 默认面向日常 Edge**：读取 Edge `User Data` 的 `Preferences`，找到已加载的 EMA2 扩展就
  原地覆盖更新，否则部署到 `%LOCALAPPDATA%\EMA2\extension` 并打开 `edge://extensions` 给出一次性
  加载指引；`--managed-profile` 保留旧的独立窗口模式
- **登录自启改为 Startup 目录**：不再写 `HKCU\...\Run`（受管机器上 `reg.exe` 可能被禁用），
  卸载时同时清理旧版遗留的 Run 值（通过 `winreg`，不调用外部命令）
- **新增 `install --uninstall`**：删除自启、五类终端里的 `edge-browser` 条目、托管 profile、
  部署的扩展与 `state.json`；`--purge-output` 一并删除截图/HAR
- 版本号 1.10.0；测试：Rust 87、Node 26、真实 Edge 21 场景（含五类终端真实合并 + install 识别已加载扩展）

## v1.9.0 — 一次安装、一次授权

- 新增 `ema2.exe install`：展开扩展到 `%LOCALAPPDATA%\EMA2\extension`、注册登录自启（隐藏启动）、
  打开独立 EMA2 Edge 窗口（`--load-extension` + 独立 profile，不碰日常 Edge）、拉起中继等待授权
- 新增首次授权流程：无 token 时中继进入待授权态，只开放 `/pair/status` 与 `?pair=1` 配对通道，
  `/mcp` 一律 503；扩展自动打开 `pair.html`，点一次「授权此浏览器」完成配对
- 新增中继身份校验：扩展每次连接带随机 challenge，校验
  `HMAC-SHA256(token, "ema2-welcome|"+challenge)` 通过后才执行工具调用，
  修复“任何本地进程占用 8765 并回 welcome 就能控制浏览器”的既有缺口
- 配对完成后自动写入 opencode / Claude Desktop 的 MCP 配置（改前 `.ema2.bak` 备份；
  检测到 `opencode.jsonc` 时跳过，避免生成同名 `.json` 覆盖）；
  `EMA2_SKIP_CLIENT_CONFIG=1` 可关闭
- `ema2.exe status` 未授权时读 `/pair/status`；新增 `ema2.exe pair [--reset]`
- `bridge.ps1` 把 `/pair/status` 也视为健康，支持待授权态启动
- 版本号 1.9.0；测试：Rust 72、Node 25、真实 Edge 20 场景（新增真实配对 + 授权页渲染）

## v1.8.1 — 安全与可用性加固

对应 2026-10-20 代码审查的 P0-1/P0-2/P1/P2/P3：

- **P0-1 默认鉴权**：未设 `EDGE_MCP_TOKEN` 且未显式声明时中继拒绝启动（退出码 2），
  只接受 `--insecure-allow-no-token` 或 `EDGE_MCP_ALLOW_ANONYMOUS=1` 作为显式例外；
  匿名启动会打 `ERROR` 日志；`bridge.ps1` 在启动前拦截并给出配置指引；
  扩展在无 token 且反复重连时打印设置提示
- **P0-2 panic/锁**：`panic = "abort"` 改为 `"unwind"`；所有 `std::sync::Mutex`
  取锁走 `state::lock` 忽略毒化；扩展 WS 任务用 Drop guard 注销 generation，
  任务 panic 也不会让中继一直认为扩展在线
- **P1-1 死端 SSE**：`GET /mcp` 现在真的会收到消息——扩展连接/断开时 relay 广播
  `notifications/message`；流建立时先发注释帧确认；新增端到端测试
- **P1-2 download-dir**：`files` 的 description 明确标注 Edge 固定返回
  `download_dir_unsupported`，减少无效调用；文档同步
- **P2-1 bodyFilter ReDoS**：编译期拒绝量词嵌套 `(a+)+` 与量词组内分支 `(a|a)+`，
  限制 512 字符，匹配前截断输入到 2000 字符（预览本身 ≤800）
- **P2-2 manifest 版本漂移**：`reload` 失败（含 POST 失败）时把 `extension/manifest.json`
  原样写回；仅在 `/status` 确认扩展已连接后才 bump
- **P2-3 事件流叠加**：明确“一个 session 一条流”语义，重复 `GET /mcp` 替换并立即关闭旧流，
  返回被替换的 stream id 写入日志，不再依赖未定义行为
- **P2-4 observe pending**：不再截断 pending 之后的已就绪条目；记录行照常输出并标
  `streaming=1`，尾部给 `pending=<n> seqs=...`，分页水位始终前进
- **P3-1** id 生成统一到 `next_id()`，去掉误导性的 `+ 1`
- **P3-2** batch 预算收敛为单一 `batch_budget()`，`timeout_for` 与参数校验共用
- **P3-3** 新增测试：事件流投递、同 session 重复注册替换旧流、锁中毒后仍可用、
  config 鉴权裁决、observe pending 与 bodyFilter 形态、扩展图标尺寸（Rust 49→56，Node 19→22）
- **P3-4** `Cargo.toml` 声明 `rust-version = "1.85"`，README 增加构建要求
- **P3-5** 文档标注 `js` 超时的 `Runtime.terminateExecution` 会终止该 tab 全部 JS
- **品牌**：扩展图标（16/48/128）改为由 `docs/img/ema2log.svg` 光栅化的 RGBA PNG
  （内容裁切 viewBox `64 32 384 384`，保证 16px 可辨）；README 顶部加入
  `docs/img/ema2log_t.svg` 横幅
- **真机验收**：新增 3 个真实 Edge 场景——P1-1（真实 WS 断开/重连触发 SSE 通知）、
  P0-1（无 token 启动被拒 + token 模式 401/200）、P2-4（3s 无 body 的慢响应标
  `streaming=1`/`pending`）；隔离 profile 偶发丢扩展 worker 时自动换 profile 重试一次；
  19/19 通过（Edge 155.0.4283.62）

## v1.8.0 — 浏览器正确性与可观测性修复

对应 2026-10-19 代码审查的 F01–F13：

- F01 网络 hook 先返回原始 Response；响应预览改为独立、限 8KB/1.5s、可取消的 clone reader；
  事件流/二进制跳过，`bodies=false` 真正关闭新 body 采集
- F02 snapshot/候选/marks/debugger 名称不再使用输入值；type/fill 反馈只回写入状态与字符数
- F03 ref 改为 `e<seq>@<documentEpoch>`；同一 document 稳定，硬导航/reload/跨 tab 旧 ref 明确 `stale_ref`
- F04 `start/stop-bridge` 改用 PowerShell 精确端口 + 进程路径校验，陌生监听者报错且不强杀；
  `setup.cmd` 不再硬编码单机 cargo 路径并检查 copy 结果
- F05 debugger 串行队列加入任务 deadline、排队期限、`Runtime.terminateExecution` 与 finally detach
- F06 `shot` 只接受活动 tab；截图前后复核 tab，annotate 额外复核 document，避免返回别的页图片
- F07 console/network 按记录分页，`next` 只推进到已交付水位；输出 `dropped/reset/pending`，
  不再用 `slice(-80)` 丢掉未读网络条目
- F08 中继超时按已校验参数计算：wait 使用 time/timeout，batch 按波次与有效并发；
  js 受全局 debugger 串行限制；超过 600s 的 batch 在执行前拒绝
- F09 Markdown 按 childNodes 遍历，根节点直接文本和“文本 + 块元素”混排不再漏字/重复
- F10 动作查找、校验、执行、结果检查合并为一次页面注入；disabled/readOnly/控件类型错误不再假成功
- F11 `waitFor` 网络水位改为 `{documentEpoch, seq}`；跨导航从新文档起点匹配，
  历史 `status=-1` 条目不作为成功证据，并明确 `observation_gap`
- F12 batch 内层参数递归复用子工具校验，扩展侧也做资源范围防御；schema 补充范围与结构
- F13 `press` 有定位时先聚焦目标再发键；无定位时才使用当前焦点
- SSE 事件流/legacy session 增加 Drop guard，客户端直接丢弃流时仍会清理
- `ema2.exe reload` 先校验 /status 与扩展连接，再 bump manifest，服务不通时不污染版本
- 示例固定传 `tabId`；文档同步 ref 生命周期、脱敏、截图活动页、超时/分页和 HAR 采样边界
- 测试：49 个 Rust 单元测试、19 项 Node VM/jsdom 回归、PowerShell 进程管理自检；
  `cargo clippy -- -D warnings` 通过；覆盖矩阵见 `docs/TESTING.md`
- 真实 Edge/MV3 验收：Edge 155.0.4283.62 隔离 profile，16/16 场景通过；
  新增 `npm run test:edge` 与 `tests/integration/edge-acceptance.cjs`

## v1.7.5 — 最终版

- `act` 支持 `waitFor={url,status,timeout}`：动作前记录网络基线，动作后等匹配响应
- `observe network` 支持 `bodyFilter`（响应体正则）与 `har=true`（中继落盘 .har）
- 新增 `batch`：多标签并发执行 `read/snapshot/js/wait`（默认并发 2，上限 4）
- `open`/`act` 自动安装 console + network hook，首次 `observe` 不再漏日志
- `snapshot` 头部带 `visual=`，纯视觉页面给 `shot annotate` 提示
- 输出加固：`path=` 单独一行（避免含空格路径歧义）
- 修复：`waitFor` 基线顺序、`batch` 递归防护、`batch` 中 `wait(time)` 超时预算、`wait.timeout` 回归
- 工具 11 个，常驻 schema ≈ 4.2k 字符；47 个 Rust 测试；真机能力测试 33/33

## v1.6.0

- `read` 默认正文提取（`main=true`，去导航/页脚）
- `observe network` 改为 hook `fetch`/`XMLHttpRequest`，拿真实状态码与响应体
- `shot annotate=true`：截图叠加 ref 编号，返回坐标列表 + 标注图
- `snapshot` 新增 `filter=links|buttons|form` 与 `near=<ref>`
- 截图落盘支持 text+image 混合 content

## v1.5.0

- `snapshot`/`read`/`act`/`wait` 遍历 shadow DOM 与同源 iframe
- 新增 `files`：`DOM.setFileInputFiles` 上传 / 下载目录
- `act target="role:name"`，目标未渲染自动重试；`ref_not_found` 与 `stale_ref` 区分
- 中继新增 `POST /reload` 与 `ema2.exe reload`（bump manifest + `chrome.runtime.reload()`）
- Windows 上传路径校验，避免 CDP 崩溃渲染进程

## v1.4.0

- 面向 AI 的彻底重写：工具收敛为粗粒度接口，行式紧凑输出
- `ref` 快照模型（`[e12]`），`act` 按 ref 操作
- `budget/cursor` 分页、`total/shown/next`、`error=<code> hint=...` 自纠错
- MCP Streamable HTTP + legacy SSE；协议版本协商与回落

## v1.3.0

- 截图链路修复（MCP image、落盘、上限统一）
- `ema2.exe status` 支持 token；`EDGE_MCP_PORT` 生效
- 超时预算按工具区分；Host/Origin/Content-Type 校验；常量时间 token
- 会话 GC、pending 取消释放、MV3 心跳

## v1.0.0

- 单进程 Rust 中继（MCP over HTTP）+ Edge MV3 扩展
- 真实浏览器控制、登录态复用、WebSocket 桥接
