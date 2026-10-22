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

async function executeTool(name, args, context) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) {
    fail('invalid_arguments', 'tool arguments must be an object', 'run `ema2 <command> --help`; for scripts run `ema2 act --help`');
  }
  switch (name) {
    case 'tabs': return tabsTool(args);
    case 'open': {
      const url = requireStringArg(args, 'url', 8192, false);
      if (!url) fail('missing_url', 'url is required', 'pass an http(s) url');
      if (!/^https?:\/\//i.test(url)) fail('invalid_url', 'url must use http or https', 'fix the url');
      if (args.reuse !== undefined && args.reuse !== 'session') {
        fail('invalid_argument', 'reuse must be session', 'use reuse:"session" or replace:true');
      }
      if (args.replace !== undefined && typeof args.replace !== 'boolean') {
        fail('invalid_argument', 'replace must be a boolean', 'use replace:true to navigate the current tab');
      }
      const modes = [args.tabId !== undefined, args.replace === true, args.reuse !== undefined]
        .filter(Boolean).length;
      if (modes > 1) {
        fail('invalid_argument', 'use only one of tabId, replace:true, reuse:"session"', 'pick one navigation target');
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
