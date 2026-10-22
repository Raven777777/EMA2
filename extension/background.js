/**
 * EMA2 Edge 扩展 · v2.4.2.7（AI 专用浏览器接口）
 * -------------------------------------------
 * 设计前提：这是给 AI 用的浏览器，不是给人用的自动化工具。
 *   - 输出一律行式紧凑文本（key=value / TSV / [ref]），不 pretty-print JSON
 *   - 列表都带 total/shown/next，长内容用 budget/cursor 分页
 *   - 错误统一 `error=<code> msg=... hint=...`，让模型能自我纠正
 *   - snapshot 给可引用 ref，act 按 ref 操作，避免 AI 猜 CSS 选择器
 *
 * 工具：tabs / open / read / snapshot / act / wait / js / observe / files / shot / batch
 *
 * WS：默认 ws://127.0.0.1:7744，端口由扩展 popup 配置（chrome.storage.local.port）。
 * 无 token、无配对、无验签：扩展裸连本机 daemon。
 *
 * 模块通过同步 importScripts 按依赖顺序加载；共享全局作用域是刻意的，
 * 页面注入函数仍保持自包含，不能引用模块外层变量。
 */

importScripts(
  'contract.js',
  'modules/core.js',
  'modules/tabs.js',
  'modules/network.js',
  'modules/open_wait.js',
  'modules/page.js',
  'modules/guard.js',
  'modules/injected_helpers.js',
  'modules/actions.js',
  'modules/hooks.js',
  'modules/act.js',
  'modules/files_shot.js',
  'modules/debugger.js',
  'modules/har.js',
  'modules/batch.js',
  'modules/observe.js',
  'modules/read_snapshot.js',
  'modules/dispatch.js'
);
