/**
 * EMA2 扩展核心：常量、共享状态、WS 连接、relay 协议、文本分页。
 * 由 background.js 在所有模块之前加载。
 */

const CONTRACT = globalThis.EMA2_CONTRACT;
if (!CONTRACT) throw new Error('EMA2 contract missing: contract.js must load first');

const DEFAULT_PORT = 7744;
const DEFAULT_WS_URL = `ws://127.0.0.1:${DEFAULT_PORT}`;
const RECONNECT_ALARM = 'ema2-reconnect';
const WATCHDOG_ALARM = 'ema2-watchdog';
const MAX_INCOMING_CHARS = 8 * 1024 * 1024;
const MAX_MESSAGE_CHARS = 48 * 1024 * 1024;
const MAX_OUTPUT_CHARS = 200000;
const DEFAULT_BUDGET = 4000;
const WAIT_TIMEOUT_MS = CONTRACT.timeouts.waitDefaultSecs * 1000;
const WAIT_MAX_SECONDS = CONTRACT.timeouts.waitMaxSecs;
const DEBUGGER_TASK_TIMEOUT_MS = CONTRACT.timeouts.debuggerTaskTimeoutMs;
const DEBUGGER_QUEUE_TIMEOUT_MS = CONTRACT.timeouts.debuggerQueueTimeoutMs;
// js 默认只跑 10s：无限循环必须尽快 terminateExecution，否则同一 tab 的
// executeScript / 下一次 js 都会一起挂住。--timeout 可以显式放宽。
const JS_DEFAULT_TIMEOUT_MS = CONTRACT.timeouts.jsDefaultSecs * 1000;
const JS_MAX_TIMEOUT_MS = CONTRACT.timeouts.jsMaxSecs * 1000;
const TOOL_WATCHDOG_MS = CONTRACT.timeouts.toolWatchdogMs;
const NET_PREVIEW_BYTES = 8192;
const NET_PREVIEW_CHARS = 800;
const NET_PREVIEW_TIMEOUT_MS = 1500;
const NET_MAX_ENTRIES = 300;
const MAX_BATCH_TABS = 8;
// 与 rust/src/tools.rs 的 READ_MODES 保持一致；cargo test 会对比两处列表。
const READ_MODES = ['md', 'text', 'html', 'visible'];
const LAST_ACT_KEY = 'ema2_last_act';
const LAST_ACT_TTL_MS = 5 * 60 * 1000;
const SETTLE_NAV_MS = 3000;
const SETTLE_NAV_IDLE_MS = 1500;
const SETTLE_PAGE_MS = 800;
const SETTLE_NONE_MS = 100;
const COMMAND_GUARD_ID = 'ema2-command-guard';
const COMMAND_GUARD_BRIDGE_ID = 'ema2-command-guard-bridge';
const COMMAND_GUARD_FILES = ['page_guard.js', 'page_guard_armed.js'];
const COMMAND_GUARD_BRIDGE_FILES = ['page_guard_bridge.js'];
// 动态 content script 不能按 tabId 限定；至少把注入面收窄到 EMA2 支持的 http(s)，
// 不碰 file/ftp/data/chrome-extension 等其它 scheme。租约结束即注销。
const COMMAND_GUARD_MATCHES = ['http://*/*', 'https://*/*'];
const COMMAND_GUARD_DISARM_MS = 800;
const COMMAND_GUARD_SWEEP_MS = 1500;
const LEGACY_GUARD_SWEEP_MS = 2500;
const COMMAND_GUARD_LIFECYCLE_MS = 3000;
// watchdog 报错后给工具一次收尾机会；仍不 settle 就强制释放租约，避免 guard 永久残留。
const COMMAND_GUARD_ABANDON_GRACE_MS = 2000;
const COMMAND_GUARD_KEEPALIVE_GRACE_MS = 5000;
const COMMAND_GUARD_TABS_KEY = 'ema2_command_guard_tabs';
const GUARD_PROTOCOL_KEY = 'ema2_guard_protocol';
const GUARD_PROTOCOL_VERSION = CONTRACT.guardProtocolVersion;
const SENSITIVE_HAR_HEADERS = new Set([
  'authorization', 'proxy-authorization', 'cookie', 'set-cookie',
]);

let socket = null;
let connecting = false;
let retryAttempt = 0;
let reconnectTimer = null;
let relayReady = false;
let currentPort = DEFAULT_PORT;
let lastError = '';            // 连接错误，popup 展示
let debuggerQueue = Promise.resolve();
let heartbeatTimer = null;
// console 与 network 的 document epoch 是两套独立序列（c-* / n-*），
// 必须分开记录；共用一张表会在交替 observe 时误判导航并重置 since。
const observedConsoleDocs = new Map(); // tabId -> last console document epoch
const observedNetworkDocs = new Map(); // tabId -> last network document epoch
const SESSION_TABS_KEY = 'ema2_session_tabs';
// 本会话由 open/batch 打开的标签（有序）。service worker 会被回收，所以落 storage.session：
// 浏览器重启即清空，正好是本会话的自然边界。
let sessionTabs = [];
let sessionTabsLoaded = false;
let sessionTabsLoad = null;
let sessionWriteChain = Promise.resolve();
let lastActContext = null;
let lastActWrite = Promise.resolve();
let commandGuardLeases = 0;
let commandGuardLifecycle = Promise.resolve();
const commandGuardLeaseTokens = new Set();
let commandGuardLeaseSeq = 0;
// 只清理真正被 arm 过的 tab；动态 document_start 脚本会在新文档里报告自己。
// SW 可能随时被回收，已 arm 的 tab 同时写入 storage.session，重启后继续 disarm。
const commandGuardTabs = new Set();
let commandGuardTabsLoaded = false;
let commandGuardTabsLoad = null;
let commandGuardTabsWrite = Promise.resolve();

class ToolError extends Error {
  constructor(code, message, hint, lines) {
    super(message);
    this.code = code;
    this.hint = hint || '';
    this.lines = lines || [];
  }
}

function fail(code, message, hint, lines) {
  throw new ToolError(code, message, hint, lines);
}

// ---------------------------------------------------------------- connection

function isLocalHost(hostname) {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host === 'localhost.localdomain' || host === '::1') return true;
  if (/^127(?:\.\d{1,3}){3}$/.test(host) || /^10(?:\.\d{1,3}){3}$/.test(host) ||
      /^192\.168(?:\.\d{1,3}){2}$/.test(host) ||
      /^172\.(?:1[6-9]|2\d|3[0-1])(?:\.\d{1,3}){2}$/.test(host) ||
      /^169\.254(?:\.\d{1,3}){2}$/.test(host) || host === '0.0.0.0') return true;
  return /^(?:fc|fd)[0-9a-f]{2}:/i.test(host) || /^fe80:/i.test(host);
}

function validateWsUrl(value) {
  let url;
  try { url = new URL(value); } catch (_) { throw new Error('Invalid WebSocket URL'); }
  if (url.protocol !== 'ws:' && url.protocol !== 'wss:') throw new Error('WebSocket URL must use ws or wss');
  if (!isLocalHost(url.hostname)) throw new Error('WebSocket URL must point to localhost or a private local address');
  if (url.username || url.password) throw new Error('WebSocket URL must not contain credentials');
  return url.href;
}

async function readSettings() {
  try { return await chrome.storage.local.get(['port', 'wsUrl']); } catch (_) { return {}; }
}

/** popup 存的 port 优先；旧版本的 wsUrl 只作为一次性迁移来源。 */
function resolvePort(stored) {
  const direct = Number(stored && stored.port);
  if (Number.isInteger(direct) && direct > 0 && direct < 65536) return direct;
  const raw = stored && stored.wsUrl;
  if (typeof raw === 'string') {
    try {
      const parsed = Number(new URL(raw).port);
      // 旧默认 8765 直接迁到新默认 7744；自定义端口保留。
      if (Number.isInteger(parsed) && parsed > 0 && parsed < 65536) {
        return parsed === 8765 ? DEFAULT_PORT : parsed;
      }
    } catch (_) { /* fall through */ }
  }
  return DEFAULT_PORT;
}

async function getWsUrl(settings) {
  const stored = settings || await readSettings();
  const port = resolvePort(stored);
  try {
    return validateWsUrl(`ws://127.0.0.1:${port}`);
  } catch (error) {
    console.error(`[EMA2] ${error.message}; falling back to ${DEFAULT_WS_URL}`);
    return DEFAULT_WS_URL;
  }
}

async function connect() {
  if (connecting) return;
  if (socket && (socket.readyState === WebSocket.CONNECTING || socket.readyState === WebSocket.OPEN)) return;
  connecting = true;
  const settings = await readSettings();
  let url;
  try { url = await getWsUrl(settings); } catch (error) {
    connecting = false;
    console.error('[EMA2] WebSocket URL rejected:', error.message);
    scheduleReconnect();
    return;
  }
  try { currentPort = Number(new URL(url).port) || DEFAULT_PORT; } catch (_) { /* keep previous */ }
  let ws;
  try { ws = new WebSocket(url); } catch (error) {
    connecting = false;
    console.error('[EMA2] WebSocket construct failed:', error.message);
    scheduleReconnect();
    return;
  }
  connecting = false;
  socket = ws;

  ws.onopen = () => {
    retryAttempt = 0;
    relayReady = false;
    clearReconnectTimer();
    chrome.alarms.clear(RECONNECT_ALARM);
    startHeartbeat();
    ws.send(JSON.stringify({
      jsonrpc: '2.0',
      method: 'bridge/hello',
      role: 'extension',
      extensionId: chrome.runtime.id,
      version: chrome.runtime.getManifest().version,
    }));
  };
  ws.onmessage = (event) => {
    handleRelayMessage(event.data, ws).catch((error) => console.error('[EMA2] handler failed:', error));
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    relayReady = false;
    lastError = 'relay disconnected';
    stopHeartbeat();
    scheduleReconnect();
  };
  ws.onerror = () => { lastError = 'websocket error'; };
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  // 本地回环：中继重启后最多等 10s，不要退避到 30s（扩展更新/重启时尤其明显）。
  const delayMs = Math.min(10000, 1000 * Math.pow(2, retryAttempt++));
  reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, delayMs);
  chrome.alarms.create(RECONNECT_ALARM, { delayInMinutes: 0.5 });
}

function clearReconnectTimer() {
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
}

function startHeartbeat() {
  stopHeartbeat();
  heartbeatTimer = setInterval(() => {
    try { chrome.runtime.getPlatformInfo(() => void chrome.runtime.lastError); } catch (_) {}
  }, 20000);
}

function stopHeartbeat() {
  if (heartbeatTimer) { clearInterval(heartbeatTimer); heartbeatTimer = null; }
}

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === RECONNECT_ALARM && !socket) connect();
  if (alarm.name === WATCHDOG_ALARM && (!socket || socket.readyState !== WebSocket.OPEN)) connect();
});

function ensureWatchdog() {
  chrome.alarms.create(WATCHDOG_ALARM, { periodInMinutes: 0.5 });
}

// ---------------------------------------------------------------- popup / relay probe

/** popup 用 /health 判断 daemon 是否在跑；连接状态以本扩展的 WS 为准。 */
async function probeRelay(port) {
  const target = Number(port) || currentPort || DEFAULT_PORT;
  try {
    const response = await fetch(`http://127.0.0.1:${target}/health`, { cache: 'no-store' });
    if (!response.ok) return { reachable: true, status: response.status, health: null };
    return { reachable: true, status: response.status, health: await response.json() };
  } catch (error) {
    return { reachable: false, error: String((error && error.message) || error), health: null };
  }
}

async function handlePageMessage(message, sender) {
  const port = Number(message.port) || currentPort || DEFAULT_PORT;
  switch (message.type) {
    case 'ema2/guard-armed': {
      const tabId = sender && sender.tab && sender.tab.id;
      // 租约期间的导航文档也纳入清理集合；租约已释放则让页面自己 disarm。
      if (commandGuardLeases > 0 && Number.isInteger(tabId)) {
        commandGuardTabs.add(tabId);
        await persistCommandGuardTabs();
      }
      return { ok: true, lease: commandGuardLeases };
    }
    case 'ema2/state': {
      const relay = await probeRelay(port);
      return {
        ok: true,
        port: currentPort,
        requestedPort: port,
        connected: !!socket && socket.readyState === WebSocket.OPEN && relayReady,
        socketOpen: !!socket && socket.readyState === WebSocket.OPEN,
        relayReady,
        relay,
        lastError,
        extensionVersion: chrome.runtime.getManifest().version,
      };
    }
    case 'ema2/reconnect': {
      lastError = '';
      try { if (socket) socket.close(); } catch (_) { /* ignore */ }
      socket = null;
      relayReady = false;
      clearReconnectTimer();
      retryAttempt = 0;
      connect();
      return { ok: true };
    }
    default:
      return { ok: false, error: `unknown message: ${message.type}` };
  }
}

// ---------------------------------------------------------------- relay plumbing

async function handleRelayMessage(data, ws) {
  if (typeof data !== 'string' || data.length > MAX_INCOMING_CHARS) return;
  if (ws !== socket) return; // 旧连接的迟到消息不得影响当前状态
  let req;
  try { req = JSON.parse(data); } catch (_) { return; }
  if (!req || typeof req !== 'object') return;

  const hasId = Object.prototype.hasOwnProperty.call(req, 'id');
  if (req.method === 'bridge/welcome') {
    // v2 无验签：welcome 只确认对端是 ema2 daemon，然后开始执行 tools/call。
    relayReady = true;
    lastError = '';
    return;
  }
  if (!relayReady) return;
  try {
    if (req.method === 'ping') { if (hasId) sendResponse(ws, req.id, { result: {} }); return; }
    if (req.method !== 'tools/call') {
      if (hasId) sendResponse(ws, req.id, { error: { code: -32601, message: `Method not found: ${req.method}` } });
      return;
    }
    const params = req.params && typeof req.params === 'object' ? req.params : {};
    const toolArgs = normalizeArgs(params.arguments);
    // 一次 RPC 内所有 guard 租约都挂在同一个 context 上：watchdog 超时后即使
    // executeTool 永不 settle，也能在宽限期后统一释放，而不是等 finally。
    const guardContext = { leases: new Set(), closed: false, abandoned: false };
    let result;
    try {
      // 所有 RPC 都有硬上限：即使某个 chrome.* 调用永远不返回，也必须把错误
      // 交回 CLI，而不是让 CLI/daemon 一起悬在那里。真正的执行仍可能继续，
      // 但页面侧的 alert/print 已被 page_guard.js 变成非阻塞记录。
      const watchdog = toolWatchdogMs(params.name, toolArgs);
      result = toResult(await withDeadline(
        withKeepAlive(
          () => executeTool(params.name, toolArgs, guardContext),
          watchdog + COMMAND_GUARD_KEEPALIVE_GRACE_MS,
        ),
        watchdog,
        'tool_timeout',
        `${params.name} did not return within ${Math.round(watchdog / 1000)}s`,
        'the tab may still be busy; inspect with `ema2 status`, close an open dialog, or reload the tab',
      ));
    } catch (error) {
      // 超时后工具可能永远不 settle；它的 finally 不会运行，必须由 watchdog 兜底。
      if (error instanceof ToolError && error.code === 'tool_timeout') {
        abandonCommandGuard(guardContext);
      }
      result = { content: [{ type: 'text', text: formatError(error) }], isError: true };
    }
    if (hasId && ws.readyState === 1) sendResponse(ws, req.id, { result });
  } catch (error) {
    if (hasId && ws.readyState === 1) {
      sendResponse(ws, req.id, { error: { code: -32603, message: String(error && error.message || error) } });
    }
  }
}

async function withKeepAlive(run, maxMs) {
  const timer = setInterval(() => {
    try { chrome.runtime.getPlatformInfo(() => void chrome.runtime.lastError); } catch (_) {}
  }, 20000);
  // 永不 settle 的工具会让 finally 永远不执行；keepalive 必须有独立上限，
  // 否则 service worker 会被自己 20s 一次的 ping 永久保活。
  let maxTimer = null;
  if (Number.isFinite(maxMs) && maxMs > 0) {
    maxTimer = setTimeout(() => {
      clearInterval(timer);
      maxTimer = null;
    }, maxMs);
  }
  try { return await run(); } finally {
    clearInterval(timer);
    if (maxTimer) clearTimeout(maxTimer);
  }
}

function normalizeArgs(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return value;
  const out = {};
  for (const [key, item] of Object.entries(value)) {
    if (item !== null && item !== undefined) out[key] = item;
  }
  return out;
}

function formatError(error) {
  if (error instanceof ToolError) {
    const head = `error=${error.code} msg=${oneLine(error.message) || error.code}${error.hint ? ` hint=${oneLine(error.hint)}` : ''}`;
    const extra = (error.lines || []).map(line => `  ${line}`).join('\n');
    return extra ? `${head}\n${extra}` : head;
  }
  return `error=internal msg=${oneLine(error && error.message || String(error))} hint=retry; if it persists reload the extension`;
}

function sendResponse(ws, id, payload) {
  const response = { jsonrpc: '2.0', id };
  if (payload.error) response.error = payload.error;
  else response.result = payload.result;
  if (ws.readyState !== 1) return;
  let encoded = JSON.stringify(response);
  if (encoded.length > MAX_MESSAGE_CHARS) {
    response.result = { content: [{ type: 'text', text: 'error=result_too_large msg=response exceeds relay limit hint=use budget/cursor or shot without inline' }], isError: true };
    delete response.error;
    encoded = JSON.stringify(response);
  }
  ws.send(encoded);
}

function capText(text) {
  const value = String(text == null ? '' : text);
  if (value.length <= MAX_OUTPUT_CHARS) return value;
  return value.slice(0, MAX_OUTPUT_CHARS)
    + '\ntruncated=1 total=' + value.length
    + ' hint=use budget/cursor or narrow the query';
}

function toResult(value) {
  if (value && typeof value === 'object' && Array.isArray(value.content)) return value;
  return { content: [{ type: 'text', text: capText(value) }] };
}

// ---------------------------------------------------------------- text helpers

function oneLine(value) {
  return String(value == null ? '' : value).replace(/\s+/g, ' ').trim();
}

function shortText(value, max) {
  const text = oneLine(value);
  return text.length > max ? text.slice(0, max - 1) + '…' : text;
}

function normalizeBudget(value) {
  const budget = Number(value);
  if (!Number.isFinite(budget)) return DEFAULT_BUDGET;
  return Math.max(200, Math.min(200000, Math.round(budget)));
}

function normalizeCursor(value) {
  const cursor = Number(value);
  if (!Number.isFinite(cursor) || cursor < 0) return 0;
  return Math.round(cursor);
}

/** 按字符预算切片，返回 {text,total,next}。 */
function sliceText(text, budget, cursor) {
  const total = text.length;
  let start = Math.min(Math.max(0, cursor), total);
  // 游标落在低位代理上时向前挪一格，避免返回半个 emoji/汉字扩展字符。
  if (start > 0 && start < total) {
    const code = text.charCodeAt(start);
    if (code >= 0xDC00 && code <= 0xDFFF) start -= 1;
  }
  if (start >= total) return { text: '', total, next: null };
  let end = Math.min(total, start + budget);
  if (end < total) {
    const code = text.charCodeAt(end - 1);
    if (code >= 0xD800 && code <= 0xDBFF) end -= 1;
  }
  if (end <= start) end = Math.min(total, start + 1);
  return { text: text.slice(start, end), total, next: end < total ? end : null };
}

function footer(total, shown, next) {
  return `total=${total} shown=${shown} next=${next == null ? 'none' : next}`;
}

/**
 * 截断提示：只有 total/shown/next 时，模型容易把「读到这里」当成「全部读到了」。
 * 明确说出被截断、以及下一次该传什么。
 */
function truncationHint(toolName, total, shown, next, cursor, unit) {
  if (next == null) return '';
  const where = cursor ? ` (from cursor=${cursor})` : '';
  return `hint=truncated: returned ${shown} of ${total} ${unit || 'chars'}${where}; call ${toolName} with cursor=${next} to continue`;
}
