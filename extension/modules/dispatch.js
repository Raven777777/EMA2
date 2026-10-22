/**
 * 工具分发、启动、连接、SW 生命周期监听。
 */

// ---------------------------------------------------------------- dispatch

function requireStringArg(args, key, max, allowEmpty) {
  if (args[key] === undefined) return undefined;
  if (typeof args[key] !== 'string' || args[key].length > max || (!allowEmpty && !args[key].trim())) {
    fail('invalid_argument', `${key} must be a string (max ${max})`, 'run `ema2 <command> --help`; for scripts run `ema2 act --help`');
  }
  return args[key];
}

/**
 * per-tab 调用链：同一 tab 上的 act/settle、read、snapshot、js 按提交顺序执行，
 * 不同 tab 之间仍然并发。链尾结算后只有「Map 里仍是自己的 tail」才删除，
 * 避免把后来者刚挂上的链一起删掉（那样会留下不会再被回收的空条）。
 */
const tabCallQueues = new Map();

function queuePerTab(key, run) {
  const previous = tabCallQueues.get(key) || Promise.resolve();
  // 两个 handler 都接上一条：前一条失败不能阻断后续调用。
  const tail = previous.then(run, run);
  const settled = tail.then(() => {}, () => {});
  tabCallQueues.set(key, settled);
  settled.then(() => {
    if (tabCallQueues.get(key) === settled) tabCallQueues.delete(key);
  });
  return tail;
}

/**
 * 计算这次调用该落在哪条 per-tab 链上；返回 null 表示不排队。
 * - `tabs`：只读标签列表，不碰页面状态。
 * - `batch`：内部每条子命令都会各自调用 executeTool 排队，外层再持锁就会等自己（自锁）。
 * - `open`：只有显式 tabId（导航已有标签）才与该 tab 的其他命令互斥；
 *   不带 tabId 是新建标签，没有可争用的页面状态。
 * - 其余工具没给 tabId 时按当前活动标签解析；解析失败返回 null，交给 dispatchTool 报错。
 */
async function tabQueueKey(name, args) {
  if (name === 'tabs' || name === 'batch') return null;
  const explicit = args && typeof args === 'object' ? args.tabId : undefined;
  if (Number.isInteger(explicit)) return `tab:${explicit}`;
  if (name === 'open') return null;
  try {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    if (tab && Number.isInteger(tab.id)) return `tab:${tab.id}`;
  } catch (_) { /* 没有可用窗口时不排队 */ }
  return null;
}

/** 工具入口：先解析 tab，再排队；这里不再加任何全局锁，batch 内部调用因此不会自锁。 */
async function executeTool(name, args, context) {
  const key = await tabQueueKey(name, args);
  if (!key) return dispatchTool(name, args, context);
  return queuePerTab(key, () => dispatchTool(name, args, context));
}

async function dispatchTool(name, args, context) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) {
    fail('invalid_arguments', 'tool arguments must be an object', 'run `ema2 <command> --help`; for scripts run `ema2 act --help`');
  }
  switch (name) {
    case 'tabs': return tabsTool(args);
    case 'open': {
      const url = requireStringArg(args, 'url', 8192, false);
      if (!url) fail('missing_url', 'url is required', 'pass an http(s) url');
      if (!/^https?:\/\//i.test(url)) fail('invalid_url', 'url must use http or https', 'fix the url');
      if (args.reuse !== undefined && args.reuse !== 'session' && args.reuse !== 'last') {
        fail('invalid_argument', 'reuse must be last|session', 'use reuse:"last" or replace:true');
      }
      if (args.replace !== undefined && typeof args.replace !== 'boolean') {
        fail('invalid_argument', 'replace must be a boolean', 'use replace:true to navigate the current tab');
      }
      if (args.background !== undefined && typeof args.background !== 'boolean') {
        fail('invalid_argument', 'background must be a boolean', 'use background:true to open without switching tabs');
      }
      const modes = [args.tabId !== undefined, args.replace === true, args.reuse !== undefined]
        .filter(Boolean).length;
      if (modes > 1) {
        fail('invalid_argument', 'use only one of tabId, replace:true, reuse:"last"', 'pick one navigation target');
      }
      if (args.background === true && modes > 0) {
        fail('invalid_argument', 'background only applies when opening a new tab', 'drop background or the navigation target');
      }
      const openArgs = { ...args, url };
      return withCommandGuard('open', openArgs, () => openTool(openArgs), context);
    }
    case 'read': return withCommandGuard('read', args, () => readTool(args), context);
    case 'snapshot': return withCommandGuard('snapshot', args, () => snapshotTool(args), context);
    case 'act': return withCommandGuard('act', args, () => actTool(args), context);
    case 'wait': return withCommandGuard('wait', args, () => waitTool(args), context);
    case 'js': {
      const code = requireStringArg(args, 'code', 100000, false);
      if (!code) fail('missing_code', 'code is required', 'pass a JS expression');
      const jsArgs = { ...args, code };
      return withCommandGuard('js', jsArgs, () => jsTool(jsArgs), context);
    }
    case 'observe': return withCommandGuard('observe', args, () => observeTool(args), context);
    case 'files': return withCommandGuard('files', args, () => filesTool(args), context);
    case 'batch': return withCommandGuard('batch', args, () => batchTool(args, context), context);
    case 'shot': return withCommandGuard('shot', args, () => shotTool(args), context);
    default: fail('unknown_tool', `unknown tool: ${name}`, 'use tabs|open|read|snapshot|act|wait|js|observe|files|shot|batch');
  }
}

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

/** 给一个可能卡在 executeScript 里的 Promise 加外层期限；超时返回 fallback，原 Promise 继续但不再阻塞调用方。 */
function within(promise, ms, fallback) {
  return Promise.race([
    Promise.resolve(promise).catch(() => fallback),
    sleep(Math.max(1, ms)).then(() => fallback),
  ]);
}

/** 旧版本留下的 token / 配对字段在 v2 全部作废；8765 迁移到 7744。 */
async function migrateSettings() {
  try {
    const stored = await chrome.storage.local.get(['wsUrl', 'port']);
    const port = resolvePort(stored);
    if (Number(stored.port) !== port) await chrome.storage.local.set({ port });
    await chrome.storage.local.remove(['token', 'pairedClients']);
  } catch (_) { /* storage 不可用时仍按默认端口连接 */ }
}

/** tab 被关闭时无条件注销，忽略仍在进行的 observe 引用计数。 */
function dropWebRequestCapture(tabId) {
  const capture = webRequestCaptures.get(tabId);
  if (capture) capture.refs = 1;
  stopWebRequestCapture(tabId);
  webRequestLog.delete(tabId);
}

chrome.tabs.onRemoved.addListener((tabId) => {
  observedConsoleDocs.delete(tabId);
  observedNetworkDocs.delete(tabId);
  dropWebRequestCapture(tabId);
  if (commandGuardTabs.delete(tabId)) void persistCommandGuardTabs();
});
ensureWatchdog();
// 动态 command guard 不应跨 service worker 重生长时间残留。
// 统一走 guard 队列，避免启动清理和第一条命令的 register 交叉。
cleanupLegacyGuardOnce();
connect();
chrome.runtime.onStartup.addListener(() => { ensureWatchdog(); connect(); });
chrome.runtime.onInstalled.addListener(() => {
  ensureWatchdog();
  // 扩展更新/重载会清空 storage.session；already-armed 页面可能还在旧文档里。
  // 重装时全量扫一遍，避免只依赖已经丢失的跟踪集合。
  cleanupLegacyGuardOnce(true);
  migrateSettings().finally(() => connect());
});
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message || typeof message.type !== 'string' || !message.type.startsWith('ema2/')) return undefined;
  handlePageMessage(message, sender).then(
    (result) => sendResponse(result),
    (error) => sendResponse({ ok: false, error: String((error && error.message) || error) }),
  );
  return true;
});
