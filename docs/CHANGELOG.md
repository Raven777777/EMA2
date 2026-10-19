# 变更记录

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
