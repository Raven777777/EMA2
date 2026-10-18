# 变更记录

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
