/**
 * EMA2 Edge 扩展 · v2.2（AI 专用浏览器接口）
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
 */

const DEFAULT_PORT = 7744;
const DEFAULT_WS_URL = `ws://127.0.0.1:${DEFAULT_PORT}`;
const RECONNECT_ALARM = 'ema2-reconnect';
const WATCHDOG_ALARM = 'ema2-watchdog';
const MAX_INCOMING_CHARS = 8 * 1024 * 1024;
const MAX_MESSAGE_CHARS = 48 * 1024 * 1024;
const MAX_OUTPUT_CHARS = 200000;
const DEFAULT_BUDGET = 4000;
const WAIT_TIMEOUT_MS = 30000;
const WAIT_MAX_SECONDS = 300;
const DEBUGGER_TASK_TIMEOUT_MS = 60000;
const DEBUGGER_QUEUE_TIMEOUT_MS = 600000;
// js 默认只跑 10s：无限循环必须尽快 terminateExecution，否则同一 tab 的
// executeScript / 下一次 js 都会一起挂住。--timeout 可以显式放宽。
const JS_DEFAULT_TIMEOUT_MS = 10000;
const JS_MAX_TIMEOUT_MS = 300000;
const TOOL_WATCHDOG_MS = 70000;
const NET_PREVIEW_BYTES = 8192;
const NET_PREVIEW_CHARS = 800;
const NET_PREVIEW_TIMEOUT_MS = 1500;
const NET_MAX_ENTRIES = 300;
const MAX_BATCH_TABS = 8;
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
const COMMAND_GUARD_DISARM_MS = 800;
const COMMAND_GUARD_SWEEP_MS = 1500;
const LEGACY_GUARD_SWEEP_MS = 2500;
const GUARD_PROTOCOL_KEY = 'ema2_guard_protocol';
const GUARD_PROTOCOL_VERSION = 3;

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
let commandGuardLeases = 0;
let commandGuardLifecycle = Promise.resolve();
// 只清理真正被 arm 过的 tab；动态 document_start 脚本会在新文档里报告自己。
const commandGuardTabs = new Set();

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
      if (commandGuardLeases > 0 && Number.isInteger(tabId)) commandGuardTabs.add(tabId);
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
    let result;
    try {
      // 所有 RPC 都有硬上限：即使某个 chrome.* 调用永远不返回，也必须把错误
      // 交回 CLI，而不是让 CLI/daemon 一起悬在那里。真正的执行仍可能继续，
      // 但页面侧的 alert/print 已被 page_guard.js 变成非阻塞记录。
      const watchdog = toolWatchdogMs(params.name, toolArgs);
      result = toResult(await withDeadline(
        withKeepAlive(() => executeTool(params.name, toolArgs)),
        watchdog,
        'tool_timeout',
        `${params.name} did not return within ${Math.round(watchdog / 1000)}s`,
        'the tab may still be busy; inspect with `ema2 status`, close an open dialog, or reload the tab',
      ));
    } catch (error) {
      result = { content: [{ type: 'text', text: formatError(error) }], isError: true };
    }
    if (hasId && ws.readyState === 1) sendResponse(ws, req.id, { result });
  } catch (error) {
    if (hasId && ws.readyState === 1) {
      sendResponse(ws, req.id, { error: { code: -32603, message: String(error && error.message || error) } });
    }
  }
}

async function withKeepAlive(run) {
  const timer = setInterval(() => {
    try { chrome.runtime.getPlatformInfo(() => void chrome.runtime.lastError); } catch (_) {}
  }, 20000);
  try { return await run(); } finally { clearInterval(timer); }
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

// ---------------------------------------------------------------- tabs

/** 会话标签登记：open/batch 打开的标签按顺序记下来，供复用与批量关闭使用。 */
async function loadSessionTabs() {
  if (sessionTabsLoaded) return sessionTabs;
  if (!sessionTabsLoad) {
    sessionTabsLoad = (async () => {
      try {
        const store = chrome.storage && chrome.storage.session;
        if (!store) {
          sessionTabsLoaded = true;
          return;
        }
        const stored = (await store.get(SESSION_TABS_KEY)) || {};
        const ids = stored[SESSION_TABS_KEY];
        if (Array.isArray(ids)) sessionTabs = ids.filter(id => Number.isInteger(id) && id >= 0);
        sessionTabsLoaded = true;
      } catch (_) { /* storage.session 暂时不可用：下次调用再试，不把失败缓存成空数组 */ }
    })();
  }
  await sessionTabsLoad;
  sessionTabsLoad = null;
  return sessionTabs;
}

/** 写入串行化：open/act/batch 并发登记时，后写不能覆盖先写。 */
async function persistSessionTabs() {
  const snapshot = [...sessionTabs];
  const write = sessionWriteChain.then(async () => {
    try {
      const store = chrome.storage && chrome.storage.session;
      if (store) await store.set({ [SESSION_TABS_KEY]: snapshot });
    } catch (_) { /* 忽略：内存登记仍然可用 */ }
  });
  sessionWriteChain = write;
  return write;
}

async function rememberSessionTab(tabId) {
  await loadSessionTabs();
  sessionTabs = sessionTabs.filter(id => id !== tabId).concat(tabId);
  await persistSessionTabs();
}

async function forgetSessionTab(tabId) {
  await loadSessionTabs();
  if (!sessionTabs.includes(tabId)) return;
  sessionTabs = sessionTabs.filter(id => id !== tabId);
  await persistSessionTabs();
}

/** 最近一个仍然存在的会话标签；不存在时返回 null（调用方新建标签）。 */
async function lastLiveSessionTab() {
  await loadSessionTabs();
  for (let index = sessionTabs.length - 1; index >= 0; index--) {
    try { return await chrome.tabs.get(sessionTabs[index]); } catch (_) { /* 已被关掉 */ }
  }
  return null;
}

async function closeTabIds(ids) {
  const closed = [];
  for (const id of ids) {
    try {
      await chrome.tabs.remove(id);
      observedConsoleDocs.delete(id);
      observedNetworkDocs.delete(id);
      webRequestLog.delete(id);
      closed.push(id);
    } catch (_) { /* 已经不存在：跳过，不算失败 */ }
  }
  await loadSessionTabs();
  sessionTabs = sessionTabs.filter(id => !closed.includes(id));
  await persistSessionTabs();
  return closed;
}

// ---------------------------------------------------------------- webRequest
//
// 页面 hook 只能看到安装之后的 fetch/XHR；首屏文档/资源的状态码、响应头只能靠
// webRequest。这里按 tab 保存最近 500 条，observe network 用它补齐 status/headers/MIME，
// 并把 HAR 的 request.url/时间戳/headers 变成真实值。响应体仍由页面 hook 采样。
const WEBREQ_PER_TAB = 500;
const webRequestLog = new Map(); // tabId -> Map<requestId, record>

function webRequestBucket(tabId, create) {
  let bucket = webRequestLog.get(tabId);
  if (!bucket && create) {
    bucket = new Map();
    webRequestLog.set(tabId, bucket);
  }
  return bucket || null;
}

function webRequestAdd(details) {
  if (!details || !Number.isInteger(details.tabId) || details.tabId < 0) return;
  const bucket = webRequestBucket(details.tabId, true);
  bucket.set(details.requestId, {
    requestId: details.requestId,
    url: String(details.url || ''),
    method: String(details.method || 'GET').toUpperCase(),
    type: String(details.type || ''),
    startedAt: Math.round(Number(details.timeStamp) || Date.now()),
    status: -1,
    timeMs: 0,
    requestHeaders: [],
    responseHeaders: [],
    mimeType: '',
    fromCache: false,
    error: '',
  });
  if (bucket.size > WEBREQ_PER_TAB) {
    const overflow = bucket.size - WEBREQ_PER_TAB;
    let index = 0;
    for (const key of bucket.keys()) {
      if (index++ >= overflow) break;
      bucket.delete(key);
    }
  }
}

function webRequestPatch(details, patch) {
  if (!details || !Number.isInteger(details.tabId) || details.tabId < 0) return;
  const bucket = webRequestBucket(details.tabId, true);
  const record = bucket.get(details.requestId);
  if (!record) return;
  Object.assign(record, patch || {});
}

function headerMime(headers) {
  if (!Array.isArray(headers)) return '';
  const header = headers.find(item => String(item.name || '').toLowerCase() === 'content-type');
  return header ? String(header.value || '').split(';')[0].trim().toLowerCase() : '';
}

function getWebRequests(tabId) {
  const bucket = webRequestBucket(tabId, false);
  if (!bucket) return [];
  return [...bucket.values()].sort((left, right) => left.startedAt - right.startedAt);
}

function registerWebRequestListeners() {
  try {
    const api = chrome.webRequest;
    if (!api || !api.onBeforeRequest) return;
    api.onBeforeRequest.addListener(
      details => webRequestAdd(details),
      { urls: ['<all_urls>'] },
    );
    api.onBeforeSendHeaders.addListener(
      details => webRequestPatch(details, {
        requestHeaders: Array.isArray(details.requestHeaders) ? details.requestHeaders : [],
      }),
      { urls: ['<all_urls>'] },
      ['requestHeaders', 'extraHeaders'],
    );
    api.onHeadersReceived.addListener(
      details => webRequestPatch(details, {
        status: Number(details.statusCode) || 0,
        responseHeaders: Array.isArray(details.responseHeaders) ? details.responseHeaders : [],
        mimeType: headerMime(details.responseHeaders),
        fromCache: !!details.fromCache,
      }),
      { urls: ['<all_urls>'] },
      ['responseHeaders', 'extraHeaders'],
    );
    api.onCompleted.addListener(
      details => webRequestPatch(details, {
        status: Number(details.statusCode) || 0,
        fromCache: !!details.fromCache,
        ip: String(details.ip || ''),
        timeMs: Math.max(0, Math.round((Number(details.timeStamp) || Date.now())
          - (webRequestBucket(details.tabId, false)?.get(details.requestId)?.startedAt || Date.now()))),
      }),
      { urls: ['<all_urls>'] },
    );
    api.onErrorOccurred.addListener(
      details => webRequestPatch(details, {
        status: 0,
        error: String(details.error || ''),
        timeMs: Math.max(0, Math.round((Number(details.timeStamp) || Date.now())
          - (webRequestBucket(details.tabId, false)?.get(details.requestId)?.startedAt || Date.now()))),
      }),
      { urls: ['<all_urls>'] },
    );
  } catch (error) {
    console.warn('[EMA2] webRequest listeners unavailable:', error && error.message || error);
  }
}

function isInternalUrl(url) {
  return /^(edge|chrome|chrome-search|about|devtools|view-source|chrome-extension|extension):/.test(url || '');
}

async function activeTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab || tab.id === undefined) fail('no_active_tab', 'no active tab', 'open a tab first');
  return tab;
}

async function resolveTab(args) {
  const tabId = args.tabId;
  if (tabId === undefined) return activeTab();
  if (!Number.isInteger(tabId) || tabId < 0) fail('invalid_tab', 'tabId must be a non-negative integer', 'call tabs to list ids');
  try { return await chrome.tabs.get(tabId); }
  catch (_) { fail('tab_not_found', `tab ${tabId} does not exist`, 'call tabs to list ids'); }
}

function tabLine(tab) {
  return `${tab.id}\t${tab.active ? '*' : '-'}\t${shortText(tab.title || '', 50)}\t${shortText(tab.url || '', 100)}`;
}

async function tabsTool(args) {
  const action = args.action || 'list';
  if (action === 'list') {
    const tabs = await chrome.tabs.query({});
    const lines = tabs.map(tabLine);
    const budget = normalizeBudget(args.budget);
    const cursor = normalizeCursor(args.cursor);
    const total = lines.length;
    const from = Math.min(cursor, total);
    const out = [];
    let used = 0;
    let index = from;
    while (index < total) {
      const line = lines[index];
      if (out.length && used + line.length + 1 > budget) break;
      out.push(line);
      used += line.length + 1;
      index++;
    }
    await loadSessionTabs();
    const live = sessionTabs.filter(id => tabs.some(tab => tab.id === id));
    const parts = [out.join('\n')];
    // 会话标签单独一行，不混进 TSV 行数，避免 total 计到非标签行。
    if (live.length) parts.push(`session_tabs=${live.join(',')}`);
    parts.push(footer(total, out.length, index < total ? index : null));
    return parts.filter(Boolean).join('\n');
  }
  if (action === 'current') {
    const tab = await activeTab();
    return `id=${tab.id}\nwindow=${tab.windowId}\nos=${await currentOs()}\ntitle=${oneLine(tab.title || '')}\nurl=${tab.url || ''}`;
  }
  if (action === 'new') {
    const url = args.url;
    if (!url) fail('missing_url', 'tabs new requires url', 'call open instead');
    const tab = await chrome.tabs.create({ url, active: args.active !== false });
    if (args.active !== false) {
      try { await chrome.windows.update(tab.windowId, { focused: true }); } catch (_) { /* ignore */ }
    }
    await rememberSessionTab(tab.id);
    return `id=${tab.id}\nurl=${tab.pendingUrl || tab.url || url}\nwindow=${tab.windowId}`;
  }
  if (action === 'close') {
    if (args.scope !== undefined && args.scope !== 'session') {
      fail('invalid_argument', 'scope must be session', 'use scope="session" to close tabs opened this session');
    }
    if (args.scope === 'session') {
      await loadSessionTabs();
      const closed = await closeTabIds([...sessionTabs]);
      return `closed=${closed.length ? closed.join(',') : 'none'}\ncount=${closed.length}`;
    }
    if (Array.isArray(args.tabIds) && args.tabIds.length) {
      if (args.tabIds.length > MAX_BATCH_TABS) {
        fail('invalid_argument', `tabIds must contain 1..${MAX_BATCH_TABS} entries`, 'split the close call');
      }
      for (const [index, tabId] of args.tabIds.entries()) {
        if (!Number.isInteger(tabId) || tabId < 0) {
          fail('invalid_argument', `tabIds[${index}] must be a non-negative integer`, 'call tabs to list ids');
        }
      }
      const closed = await closeTabIds(args.tabIds);
      if (!closed.length) fail('tab_not_found', 'none of tabIds exist', 'call tabs to list ids');
      return `closed=${closed.join(',')}\ncount=${closed.length}`;
    }
    const tab = await resolveTab(args);
    await closeTabIds([tab.id]);
    return `closed=${tab.id}`;
  }
  if (action === 'activate') {
    const tab = await resolveTab(args);
    await chrome.tabs.update(tab.id, { active: true });
    // 只切 tab 不会把窗口提到前台；前台语义必须显式 focus 窗口，
    // 否则截图/密码管理器/输入焦点都会落在后台窗口上。
    let focused = false;
    try {
      await chrome.windows.update(tab.windowId, { focused: true });
      focused = true;
    } catch (_) { /* 某些窗口类型不允许 focus */ }
    return `active=${tab.id}${focused ? ' focused=1' : ' focused=0'}`;
  }
  fail('invalid_action', `unknown tabs action: ${action}`, 'use list|current|new|close|activate');
}

// ---------------------------------------------------------------- open / wait

/** 主文档的导航计时：真实 HTTP 状态与重定向次数（Chromium 109+ 的 responseStatus）。 */
async function navigationTiming(tabId) {
  try {
    return await runInTab(tabId, () => {
      const entries = performance.getEntriesByType ? performance.getEntriesByType('navigation') : [];
      const nav = entries && entries[0];
      if (!nav) return null;
      return {
        status: typeof nav.responseStatus === 'number' ? nav.responseStatus : 0,
        redirects: nav.redirectCount || 0,
        type: nav.type || '',
      };
    }, []);
  } catch (_) {
    return null;
  }
}

async function openTool(args) {
  await clearLastAct();
  const wait = args.wait || 'load';
  let tab;
  let reuse = 'new';
  let previousUrl = '';
  let previousMarker = 0;
  if (args.tabId !== undefined) {
    // 显式导航用户已有的标签：不改会话登记，scope=session 不会关掉用户的标签。
    tab = await resolveTab(args);
    const currentUrl = String(tab.url || tab.pendingUrl || '');
    // batch --reuse session 命中精确 URL 时用内部标记续用文档，不重载。
    if (args.reuseSamePage === true
        && samePageTarget(currentUrl, args.url) && !samePageHashOnly(currentUrl, args.url)) {
      previousUrl = currentUrl;
      reuse = 'session';
    } else {
      const nav = await navigateTab(tab, args.url, wait !== 'none');
      previousUrl = nav.before;
      previousMarker = nav.marker;
      reuse = 'tabId';
    }
  } else if (args.replace === true) {
    tab = await activeTab();
    const nav = await navigateTab(tab, args.url, wait !== 'none');
    previousUrl = nav.before;
    previousMarker = nav.marker;
    reuse = 'replace';
  } else if (args.reuse === 'session') {
    const live = await lastLiveSessionTab();
    if (live) {
      tab = live;
      const currentUrl = String(tab.url || tab.pendingUrl || '');
      if (samePageTarget(currentUrl, args.url) && !samePageHashOnly(currentUrl, args.url)) {
        // “复用”同一个文档时不要重载：保留滚动位置/已填表单。
        previousUrl = currentUrl;
      } else {
        const nav = await navigateTab(tab, args.url, wait !== 'none');
        previousUrl = nav.before;
        previousMarker = nav.marker;
      }
      reuse = 'session';
    }
  }
  if (!tab) {
    tab = await chrome.tabs.create({ url: args.url, active: true });
    await rememberSessionTab(tab.id);
    // 新标签必须让用户看得见：窗口提到前台（测试也依赖前台语义）。
    try { await chrome.windows.update(tab.windowId, { focused: true }); } catch (_) { /* ignore */ }
  }
  let loaded = true;
  let docTimeout = 0;
  // --timeout 覆盖“打开标签 + 等文档 + 装 hook + 读 timing/focus”全流程。
  // 只给 waitForDocument 设 deadline 会让后面的 hook/timing/focus 再各花几秒，
  // 慢页面/挂起页面因此把 3s 拉成 15s。
  const openBudget = wait === 'none' ? 5000 : Math.min(60000, Math.max(1000, Number(args.waitTimeout || 15) * 1000));
  if (wait !== 'none') docTimeout = openBudget;
  const deadline = Date.now() + openBudget;
  const remaining = () => Math.max(0, deadline - Date.now());
  if (wait !== 'none') {
    // 单个 executeScript 也可能卡住；外层硬截止保证 open 一定按 budget 返回。
    loaded = await within(
      waitForDocument(
        tab.id, args.url, previousUrl,
        wait === 'load' ? 'complete' : 'interactive', docTimeout, previousMarker,
      ),
      docTimeout + 250,
      false,
    );
    // 超时后再做一次单步探测：页面可能刚好在 deadline 后完成，
    // 不要把已经加载好的页面误报成 load=pending。
    if (!loaded) {
      loaded = await within(
        waitForDocument(
          tab.id, args.url, previousUrl,
          wait === 'load' ? 'complete' : 'interactive', 1, previousMarker,
        ),
        Math.min(1000, remaining() + 400),
        false,
      );
    }
  }
  let current;
  try { current = await chrome.tabs.get(tab.id); }
  catch (_) { fail('tab_not_found', `tab ${tab.id} was closed during open`, 'call tabs to list ids'); }
  // 等页面稳定后自动装网络 hook，这样后续 fetch/XHR 都能被 observe network 看到。
  if (remaining() > 0) await sleep(Math.min(180, remaining()));
  let hook = false;
  try {
    hook = remaining() > 0
      ? await within(ensureNetworkHook(tab.id), Math.min(2500, remaining()), false)
      : false;
  } catch (_) { hook = false; }
  try {
    if (remaining() > 0) await within(ensureConsoleHook(tab.id), Math.min(1500, remaining()), false);
  } catch (_) { /* ignore */ }
  try {
    if (remaining() > 0) await within(ensurePageGuard(tab.id), Math.min(1000, remaining()), false);
  } catch (_) { /* ignore */ }
  const timing = wait === 'none' || remaining() <= 0
    ? null
    : await within(navigationTiming(tab.id), Math.min(1500, remaining()), null);
  const status = !timing ? (wait === 'none' ? 'pending' : loaded ? 'unknown' : 'pending')
    : timing.status > 0 ? String(timing.status) : 'unknown';
  // hook/timing 期间可能又发生重定向；用最新 tab 元数据，而不是早先的快照。
  let latest = current;
  try { latest = await chrome.tabs.get(tab.id); } catch (_) { /* keep current */ }
  // 导航尚未提交时 tab.url/title 仍是旧文档；不要把旧标题当成新页面。
  const pendingNav = !loaded && !samePageTarget(latest.url || '', args.url);
  const shownUrl = pendingNav ? (latest.pendingUrl || args.url) : (latest.url || args.url);
  const shownTitle = pendingNav ? '' : (latest.title || '');
  const shownStatus = pendingNav ? 'pending' : status;
  const lines = [
    `id=${latest.id}`,
    `url=${shownUrl}`,
    `title=${oneLine(shownTitle)}`,
    `status=${shownStatus}`,
    `reuse=${reuse}`,
    `net_hook=${hook ? 'on' : 'off'}`,
  ];
  if (!loaded) {
    lines.push(`load=pending timeout=${Math.round(docTimeout / 1000)}s timed_out=1`);
    lines.push('hint=the document did not reach the requested readyState before --timeout; the tab is open, read/snapshot may still work (try `ema2 wait idle`)');
  }
  if (timing && timing.redirects > 0) lines.push(`redirects=${timing.redirects}`);
  const guardEvents = (!loaded || pendingNav)
    ? []
    : await within(
      guardSince(tab.id, 0),
      Math.min(800, Math.max(1, remaining())),
      [],
    );
  const guardDialogs = (Array.isArray(guardEvents) ? guardEvents : [])
    .filter(event => event.kind === 'dialog');
  for (const event of guardDialogs.slice(0, 3)) {
    lines.push(`dialog_opened=${event.dialog || 'dialog'} handled=${event.handled || 'dismissed'}`
      + (event.message ? ` msg=${oneLine(event.message).slice(0, 120)}` : ''));
  }
  const blockedPopups = (Array.isArray(guardEvents) ? guardEvents : [])
    .filter(event => event.kind === 'popup' && event.blocked);
  if (blockedPopups.length) lines.push(`popup_blocked=1 url=${shortText(blockedPopups[0].url || '', 120)}`);
  const focused = await within(
    runInTab(tab.id, () => document.hasFocus(), []),
    Math.min(1200, Math.max(1, remaining())),
    false,
  );
  lines.push(`focused=${focused ? 1 : 0}`);
  if (!focused) {
    lines.push('hint=the browser window is not focused; call `ema2 tabs activate ' + tab.id
      + '` (or bring Edge to front) before shot/press/password-manager flows');
  }
  if (timing && timing.status >= 400) {
    lines.push(`hint=http ${timing.status}: the page is probably the site's own error page, not the content you asked for`);
  } else if (timing && timing.status > 0 && timing.status >= 300 && timing.status < 400) {
    lines.push(`hint=http ${timing.status}: redirect left in place (page may need a click to follow)`);
  }
  return lines.join('\n');
}

/** 两个 URL 是否指向同一页面（忽略 hash 与结尾斜杠）。 */
function samePageTarget(a, b) {
  const normalize = (value) => {
    try {
      const url = new URL(value);
      url.hash = '';
      return url.href.replace(/\/$/, '');
    } catch (_) {
      return String(value || '').replace(/#.*$/, '').replace(/\/$/, '');
    }
  };
  return normalize(a) === normalize(b);
}

/** 只改了 hash 的同一页面不会触发 reload。 */
function samePageHashOnly(a, b) {
  try {
    const left = new URL(a);
    const right = new URL(b);
    const stripHash = (url) => url.href.slice(0, url.href.length - url.hash.length);
    return left.hash !== right.hash && stripHash(left) === stripHash(right);
  } catch (_) {
    return false;
  }
}

/**
 * 等目标文档真正接位再判定就绪。
 * 只判 readyState 会被旧文档骗过：tabs.update 之后新文档尚未提交时，
 * 旧文档仍是 complete，于是 open 返回的 url/title/正文都停留在上一页。
 * 这里同时看 document 的 location.href：只要确认已经离开旧文档（或落到目标/重定向后的地址）
 * 且 readyState 达标，才算加载完成。
 */
async function waitForDocument(tabId, targetUrl, previousUrl, state, timeoutMs, previousMarker = 0) {
  const deadline = Date.now() + timeoutMs;
  const reloadingSamePage = samePageTarget(previousUrl, targetUrl);
  // 同 URL 的 open 必须等到新文档，否则旧文档的 complete 会立刻骗过 readyState 检查。
  const requireReload = reloadingSamePage && !samePageHashOnly(previousUrl, targetUrl)
    && previousMarker > 0;
  while (Date.now() < deadline) {
    let info = null;
    try {
      info = await runInTab(
        tabId,
        () => ({
          href: location.href,
          ready: document.readyState,
          origin: Number(performance.timeOrigin) || 0,
        }),
        [],
      );
    } catch (_) { /* 导航中：旧文档已销毁、新文档未就绪 */ }
    if (info) {
      let arrived;
      if (!previousUrl) {
        // 新标签的初始 about:blank / edge://newtab 不能算“已到达”，否则 open 会提前返回。
        arrived = !!info.href && info.href !== 'about:blank' && !info.href.startsWith('about:');
      } else {
        arrived = reloadingSamePage
          ? samePageTarget(info.href, previousUrl || targetUrl)
          : !samePageTarget(info.href, previousUrl);
      }
      // 浏览器自己的错误页/新标签页不是目标文档；宁可等超时并报 load=pending。
      if (arrived && isInternalUrl(info.href)) arrived = false;
      if (arrived && requireReload && Number(info.origin) === previousMarker) arrived = false;
      const readyOk = state === 'interactive'
        ? info.ready === 'interactive' || info.ready === 'complete'
        : info.ready === 'complete';
      if (arrived && readyOk) return true;
    }
    await sleep(120);
  }
  return false;
}

/**
 * 导航到指定 URL，返回导航前的地址与文档时间原点。
 * timeOrigin 用来区分“旧文档仍是 complete”和 reload 后的新文档。
 */
async function navigateTab(tab, url, wantMarker) {
  const before = tab.url || '';
  let marker = 0;
  if (wantMarker) {
    // 只用于等待新文档；--wait none 不该为了读 marker 而多等。
    marker = await within(
      runInTab(tab.id, () => Number(performance.timeOrigin) || 0, []),
      1000,
      0,
    );
  }
  await chrome.tabs.update(tab.id, { url });
  return { before, marker: Number(marker) || 0 };
}

async function waitForReady(tabId, state, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const ready = await runInTab(tabId, () => document.readyState, []);
      if (state === 'interactive' && (ready === 'interactive' || ready === 'complete')) return true;
      if (state === 'complete' && ready === 'complete') return true;
    } catch (_) { /* navigating */ }
    await sleep(150);
  }
  return false;
}

async function waitTool(args) {
  if (args.await) return awaitPendingAction(args);
  const tab = await resolveTab(args);
  const timeoutSeconds = args.timeout === undefined ? WAIT_TIMEOUT_MS / 1000 : Number(args.timeout);
  if (!Number.isFinite(timeoutSeconds) || timeoutSeconds < 0 || timeoutSeconds > WAIT_MAX_SECONDS) {
    fail('invalid_argument', `timeout must be between 0 and ${WAIT_MAX_SECONDS} seconds`, 'fix timeout');
  }
  const timeoutMs = Math.round(timeoutSeconds * 1000);
  const start = Date.now();
  if (args.time !== undefined) {
    const seconds = Number(args.time);
    if (!Number.isFinite(seconds) || seconds < 0 || seconds > WAIT_MAX_SECONDS) {
      fail('invalid_argument', `time must be between 0 and ${WAIT_MAX_SECONDS} seconds`, 'fix time');
    }
    await sleep(Math.round(seconds * 1000));
    return `ok=time elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.idle) {
    const ready = await waitForReady(tab.id, 'complete', timeoutMs);
    if (!ready) fail('wait_timeout', `page did not become idle in ${timeoutMs / 1000}s`, 'retry, or wait for a specific selector/text');
    return `ok=idle elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.selector !== undefined) {
    await ensureHelpers(tab.id);
    // 非法 CSS 应立即返回 invalid_selector，而不是等到 timeout 后报 wait_timeout。
    await requireValidSelector(tab.id, args.selector);
  }
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    let matched = false;
    try {
      if (args.selector !== undefined) {
        matched = await runInTab(tab.id, (selector) => !!(window.__ema2_helpers && window.__ema2_helpers.find(null, selector, null)), [args.selector]);
      } else if (args.textGone !== undefined) {
        matched = await runInTab(tab.id, (text) => !(document.body ? document.body.innerText : '').includes(text), [args.textGone]);
      } else if (args.text !== undefined) {
        matched = await runInTab(tab.id, (text) => (document.body ? document.body.innerText : '').includes(text), [args.text]);
      }
    } catch (_) { matched = false; }
    if (matched) {
      const what = args.selector !== undefined ? `selector=${args.selector}`
        : args.textGone !== undefined ? `textGone=${args.textGone}` : `text=${args.text}`;
      return `ok=${what} elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
    }
    await sleep(250);
  }
  const what = args.selector !== undefined ? `selector=${args.selector}`
    : args.textGone !== undefined ? `textGone=${args.textGone}` : `text=${args.text}`;
  fail('wait_timeout', `timeout after ${timeoutMs / 1000}s: ${what}`, 'call snapshot to see current state, or raise timeout');
}

// ---------------------------------------------------------------- page runs

async function runInTab(tabId, func, args, world, attempts = 2) {
  const options = { target: { tabId }, func, args };
  if (world) options.world = world;
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const results = await chrome.scripting.executeScript(options);
      return results && results[0] ? results[0].result : undefined;
    } catch (error) {
      lastError = error;
      // debugger attach/detach 之后偶尔会瞬时拿不到页面访问权，重试一次通常就好。
      if (attempt + 1 < attempts) await sleep(180);
    }
  }
  throw lastError;
}

/**
 * 命令期间才启用 guard。动态 content script 负责覆盖导航后新文档在
 * document_start 阶段出现的对话框；已经打开的 tab 由这里立即注入并 arm。
 */
async function ensurePageGuard(tabId) {
  try {
    await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      files: COMMAND_GUARD_FILES,
      world: 'MAIN',
    });
    if (Number.isInteger(tabId)) commandGuardTabs.add(tabId);
    return true;
  } catch (_) {
    return false;
  }
}

async function registerCommandGuard() {
  // 逐条注销：旧版本只注册过 main id，一次传不存在的 id 可能让整批失败。
  for (const id of [COMMAND_GUARD_ID, COMMAND_GUARD_BRIDGE_ID]) {
    try { await chrome.scripting.unregisterContentScripts({ ids: [id] }); } catch (_) { /* not registered */ }
  }
  // bridge 先注册：这样任何被 MAIN guard arm 的新文档都已经有监听者。
  try {
    await chrome.scripting.registerContentScripts([{
      id: COMMAND_GUARD_BRIDGE_ID,
      matches: ['<all_urls>'],
      js: COMMAND_GUARD_BRIDGE_FILES,
      runAt: 'document_start',
      allFrames: true,
      world: 'ISOLATED',
      persistAcrossSessions: false,
    }]);
  } catch (error) {
    // bridge 只是释放竞态的加固；失败时主 guard 仍然工作。
    console.warn('[EMA2] command guard bridge registration failed:', error && error.message || error);
  }
  let mainOk = false;
  try {
    await chrome.scripting.registerContentScripts([{
      id: COMMAND_GUARD_ID,
      matches: ['<all_urls>'],
      js: COMMAND_GUARD_FILES,
      runAt: 'document_start',
      allFrames: true,
      world: 'MAIN',
      persistAcrossSessions: false,
    }]);
    mainOk = true;
  } catch (error) {
    console.warn('[EMA2] command guard registration failed:', error && error.message || error);
  }
  return mainOk;
}

async function unregisterCommandGuard() {
  for (const id of [COMMAND_GUARD_ID, COMMAND_GUARD_BRIDGE_ID]) {
    try { await chrome.scripting.unregisterContentScripts({ ids: [id] }); } catch (_) { /* not registered */ }
  }
}

async function disarmPageGuard(tabId) {
  let needsUpgrade = false;
  try {
    const results = await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      world: 'MAIN',
      func: () => {
        const guard = window.__ema2_guard;
        if (!guard) return { guard: false, disarmed: false };
        if (typeof guard.disarm !== 'function') return { guard: true, disarmed: false };
        guard.disarm();
        return { guard: true, disarmed: true };
      },
    });
    needsUpgrade = (results || []).some(result =>
      result && result.result && result.result.guard && !result.result.disarmed);
  } catch (_) { /* internal/restricted tab */ }
  if (!needsUpgrade) return;
  try {
    // 旧 v1 没有 disarm()：注入 passive v2 会恢复它保存的原生引用。
    await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      files: ['page_guard.js'],
      world: 'MAIN',
    });
  } catch (_) { /* page became restricted */ }
}

async function allScriptableTabIds() {
  try {
    const tabs = await chrome.tabs.query({});
    return tabs
      .filter(tab => Number.isInteger(tab.id) && !isInternalUrl(tab.url))
      .map(tab => tab.id);
  } catch (_) {
    return [];
  }
}

/**
 * 清理指定 tab。每个 tab 单独限时，整体再设硬上限：一个卡在原生对话框或
 * 死循环里的渲染进程不能把 release/启动清理拖到命令预算之外。
 */
async function disarmPageGuards(tabIds, budget = COMMAND_GUARD_SWEEP_MS) {
  const ids = [...new Set(tabIds)].filter(id => Number.isInteger(id));
  for (const id of ids) commandGuardTabs.delete(id);
  if (!ids.length) return true;
  const results = await within(
    Promise.all(ids.map(id => within(
      disarmPageGuard(id).then(() => true),
      COMMAND_GUARD_DISARM_MS,
      false,
    ))),
    budget,
    null,
  );
  return Array.isArray(results) && results.every(Boolean);
}

async function disarmTrackedPageGuards(budget = COMMAND_GUARD_SWEEP_MS) {
  return disarmPageGuards([...commandGuardTabs], budget);
}

/**
 * guard 链只允许成功/失败都继续；任何一步 reject 都不能污染后续命令。
 * 返回的 Promise 本身永远 resolve（失败只记日志）。
 */
function enqueueCommandGuard(step) {
  const run = () => Promise.resolve().then(step).catch(error => {
    console.warn('[EMA2] command guard step failed:', error && error.message || error);
  });
  commandGuardLifecycle = commandGuardLifecycle.then(run, run);
  return commandGuardLifecycle;
}

/**
 * 登记一个命令租约。第一个租约注册 document_start guard，最后一个释放时
 * 注销注册并恢复已 arm 文档的原生对话框 API。
 */
async function acquireCommandGuard() {
  // guard 是优化而不是前置条件：这里失败也必须放行工具调用。
  try {
    await enqueueCommandGuard(async () => {
      commandGuardLeases += 1;
      if (commandGuardLeases === 1) await registerCommandGuard();
    });
  } catch (_) { /* fail open */ }
}

async function releaseCommandGuard() {
  try {
    await enqueueCommandGuard(async () => {
      if (commandGuardLeases <= 0) return;
      commandGuardLeases -= 1;
      if (commandGuardLeases !== 0) return;
      // 顺序很重要：先注销动态脚本，再 disarm。反序会在注销窗口里把正在
      // 加载的新文档 arm 掉，而 sweep 已经结束，guard 会永久残留。
      await unregisterCommandGuard();
      await disarmTrackedPageGuards();
    });
  } catch (_) { /* guard failure must not fail the command */ }
}

async function cleanupLegacyGuardOnce() {
  let legacy = true;
  try {
    const stored = await chrome.storage.local.get(GUARD_PROTOCOL_KEY);
    legacy = Number(stored && stored[GUARD_PROTOCOL_KEY]) < GUARD_PROTOCOL_VERSION;
  } catch (_) { /* storage may be unavailable during startup */ }

  let complete = false;
  await enqueueCommandGuard(async () => {
    if (commandGuardLeases !== 0) return;
    await unregisterCommandGuard();
    // 协议升级要扫所有 tab（清 v1/v2 遗留）；普通 SW 启动只清理已 arm 的 tab。
    const ids = legacy ? await allScriptableTabIds() : [...commandGuardTabs];
    complete = await disarmPageGuards(ids, legacy ? LEGACY_GUARD_SWEEP_MS : COMMAND_GUARD_SWEEP_MS);
  });
  if (legacy && complete) {
    try { await chrome.storage.local.set({ [GUARD_PROTOCOL_KEY]: GUARD_PROTOCOL_VERSION }); } catch (_) { /* ignore */ }
  }
}

async function guardTargetForTool(name, args) {
  // open 不在这里：它先导航，再由 openTool 对最终文档 ensurePageGuard。
  // act 只在这里 arm 一次；actTool 不要在动作前重复注入。
  const guarded = new Set([
    'act', 'wait', 'read', 'snapshot', 'js', 'observe', 'files', 'shot',
  ]);
  if (!guarded.has(name)) return;
  if (name === 'files' && args.action === 'download-dir') return;
  let tabId = args.tabId;
  if (name === 'wait' && args.await === true && tabId === undefined) {
    const context = await loadLastAct().catch(() => null);
    tabId = context && (context.targetTabId || context.tabId);
  }
  if (tabId === undefined) {
    try {
      const tab = await activeTab();
      tabId = tab.id;
    } catch (_) {
      return;
    }
  }
  if (Number.isInteger(tabId)) await ensurePageGuard(tabId);
}

async function withCommandGuard(name, args, run) {
  await acquireCommandGuard();
  try {
    await guardTargetForTool(name, args);
    return await run();
  } finally {
    await releaseCommandGuard();
  }
}

/** 当前 guard 事件水位；用于区分“本次动作打开的对话框/弹窗”。 */
async function guardMark(tabId) {
  try {
    return await runInTab(tabId, () => (window.__ema2_guard ? window.__ema2_guard.seq || 0 : 0), [], 'MAIN');
  } catch (_) {
    return 0;
  }
}

async function guardSince(tabId, seq) {
  try {
    const events = await runInTab(tabId, (since) => {
      const guard = window.__ema2_guard;
      if (!guard) return [];
      return (guard.events || []).filter(event => (event.seq || 0) > since);
    }, [Number(seq) || 0], 'MAIN');
    return Array.isArray(events) ? events : [];
  } catch (_) {
    return [];
  }
}

/** 返回空字符串表示 selector 语法合法；否则返回浏览器给出的 SyntaxError。 */
async function selectorError(tabId, selector) {
  if (!selector) return '';
  try {
    return await runInTab(tabId, (value) => {
      try {
        document.documentElement.matches(value);
        return '';
      } catch (error) {
        return String((error && error.message) || error);
      }
    }, [String(selector)], undefined, 1) || '';
  } catch (_) {
    return '';
  }
}

async function requireValidSelector(tabId, selector) {
  const error = await selectorError(tabId, selector);
  if (error) {
    fail('invalid_selector', `invalid CSS selector: ${selector} (${oneLine(error)})`,
      'fix the selector syntax, or use a snapshot ref/target instead');
  }
}

/** 页面内 helper（作为 executeScript 的 func 注入，必须自包含）。 */
function installHelpers() {
  if (window.__ema2_helpers && window.__ema2_helpers_version === 19) {
    return { installed: false, handle: String(window.__ema2_doc_handle || '') };
  }
  const clean = (value) => String(value == null ? '' : value).replace(/\s+/g, ' ').trim();

  const roleOf = (el) => {
    const role = el.getAttribute('role'); if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'summary') return 'summary';
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio') return t;
      if (['submit', 'button', 'reset', 'image'].includes(t)) return 'button';
      if (t === 'file') return 'file';
      if (t === 'search') return 'searchbox';
      return 'textbox';
    }
    if (el.isContentEditable) return 'textbox';
    return 'element';
  };
  const labelText = (el) => {
    const doc = el.ownerDocument || document;
    const labelled = el.getAttribute('aria-labelledby');
    if (labelled) {
      const text = String(labelled).split(/\s+/).map(id => {
        const node = doc.getElementById ? doc.getElementById(id) : null;
        return node ? (node.innerText || node.textContent || '') : '';
      }).filter(Boolean).join(' ');
      if (clean(text)) return text;
    }
    if (el.labels && el.labels.length) {
      const text = [...el.labels].map(label => label.innerText || label.textContent || '').join(' ');
      if (clean(text)) return text;
    }
    const wrapping = el.closest ? el.closest('label') : null;
    if (wrapping) {
      const clone = wrapping.cloneNode ? wrapping.cloneNode(true) : null;
      if (clone) {
        for (const control of clone.querySelectorAll ? clone.querySelectorAll('input,textarea,select,button') : []) control.remove();
        const text = clone.innerText || clone.textContent || '';
        if (clean(text)) return text;
      }
    }
    return '';
  };
  /** 名称只来自标签/ARIA/占位符等元数据；绝不使用用户输入的值（密码、textarea、富文本）。 */
  const nameOf = (el) => {
    const tag = el.tagName;
    const type = tag === 'INPUT' ? String(el.type || 'text').toLowerCase() : '';
    const safeValue = tag === 'INPUT' && ['button', 'submit', 'reset'].includes(type) ? el.value : '';
    // textarea / contenteditable 的 innerText 就是用户输入；input 的 value 同理。
    const userValueControl = tag === 'TEXTAREA' || el.isContentEditable === true
      || (tag === 'INPUT' && !['button', 'submit', 'reset', 'image'].includes(type));
    const text = el.getAttribute('aria-label')
      || labelText(el)
      || el.getAttribute('placeholder')
      || el.getAttribute('alt')
      || el.getAttribute('title')
      || safeValue
      || (userValueControl ? '' : el.innerText)
      || '';
    const clipped = clean(text);
    return clipped.length > 80 ? clipped.slice(0, 79) + '…' : clipped;
  };
  const clip = (value, max) => {
    const text = clean(value);
    return text.length > max ? text.slice(0, max - 1) + '…' : text;
  };
  const absoluteHref = (el) => {
    const raw = el.getAttribute('href') || '';
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  const extraOf = (el) => {
    const parts = [];
    // 表单当前值默认不输出（密码/textarea/富文本可能含敏感输入）；
    // 显式 snapshot --values 时才附加。read --selector 仍可直接读单个控件。
    const showValues = () => !!window.__ema2_show_values;
    // href 必须是完整绝对 URL：scheme/host/query/fragment 都是定位信息，
    // 截断成 path 会让 AI 跳到同源错误地址或分不清 mailto/tel。
    if (el.tagName === 'A' && el.getAttribute('href')) {
      parts.push('href=' + absoluteHref(el));
      if (el.hasAttribute('download')) parts.push('download');
    }
    if (el.tagName === 'INPUT') {
      const t = (el.type || 'text').toLowerCase();
      if (t !== 'text') parts.push('type=' + t);
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (el.checked) parts.push('checked');
      if (t === 'file') {
        parts.push('accept=' + clip(el.getAttribute('accept') || '*', 30));
        const names = el.files ? [...el.files].map(file => file.name).slice(0, 5) : [];
        if (names.length) parts.push('files=' + clip(names.join(','), 100));
      } else if (t === 'password') {
        // 密码值永远不进入快照，只确认“已填写”。
        if (showValues() && el.value) parts.push('value=<hidden>');
      } else if (showValues()
          && !['checkbox', 'radio', 'hidden', 'submit', 'reset', 'button', 'image'].includes(t)
          && el.value) {
        parts.push('value=' + clip(el.value, 80));
      }
    }
    if (el.tagName === 'TEXTAREA') {
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (showValues() && el.value) parts.push('value=' + clip(el.value, 80));
    }
    if (el.tagName === 'SELECT') {
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (el.multiple) parts.push('multiple');
      const selected = [...el.options].filter(option => option.selected).map(option => option.value);
      if (showValues() && selected.length) parts.push('value=' + clip(selected.join(','), 100));
    }
    // contenteditable 的当前文本只作为 extra 展示，不参与 nameOf，避免把用户输入当成标签。
    if (showValues() && el.isContentEditable && el.textContent) parts.push('value=' + clip(el.textContent, 80));
    if (el.disabled) parts.push('disabled');
    if (el.getAttribute('aria-expanded')) parts.push('expanded=' + el.getAttribute('aria-expanded'));
    return parts.join(' ');
  };
  const visible = (el) => {
    if (!el || el.hidden) return false;
    try {
      if (typeof el.checkVisibility === 'function') {
        if (!el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true })) return false;
      } else {
        if (!el.getClientRects().length) return false;
        const view = (el.ownerDocument && el.ownerDocument.defaultView) || window;
        const style = view.getComputedStyle(el);
        if (style.visibility === 'hidden' || style.display === 'none') return false;
      }
    } catch (_) {
      if (!el.getClientRects().length) return false;
    }
    return true;
  };
  const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role],[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';

  const collectRoots = () => {
    const roots = [];
    let frameSeq = 0;
    const seen = new Set();
    const visit = (root, frame, frameEl, win) => {
      if (!root || seen.has(root)) return;
      seen.add(root);
      const owner = win || (root.ownerDocument && root.ownerDocument.defaultView) || window;
      roots.push({ root, frame, frameEl: frameEl || null, win: owner });
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) visit(el.shadowRoot, frame, frameEl, owner);
      }
      for (const frameNode of root.querySelectorAll('iframe,frame')) {
        let doc = null;
        try { doc = frameNode.contentDocument; } catch (_) { /* cross-origin */ }
        if (doc) {
          visit(doc, frame ? frame + '.f' + (++frameSeq) : 'f' + (++frameSeq),
            frameNode, doc.defaultView || owner);
        }
      }
    };
    visit(document, '', null, window);
    return roots;
  };

  // 文档句柄（d1、d2…）由扩展侧统一分配：页面自己生成会撞 id，跨文档的 ref 就失去校验意义。
  // 页面只负责持有它，导航后的新文档由扩展重新下发。
  const docEpoch = () => window.__ema2_doc_handle || '';
  const setDocHandle = (handle) => { window.__ema2_doc_handle = String(handle || ''); };
  const parseRef = (value) => {
    const text = String(value == null ? '' : value).trim().replace(/^\[|\]$/g, '').replace(/"/g, '');
    // 全写 e12@d1 带文档校验；CLI 示例里的短写 e12 视作“当前文档”，方便手敲。
    const match = /^e(\d+)(?:@(d\d+))?$/i.exec(text);
    return match ? { seq: Number(match[1]), epoch: match[2] || '' } : null;
  };
  const refIsCurrent = (value) => {
    const parsed = parseRef(value);
    return !!parsed && !!docEpoch() && (!parsed.epoch || parsed.epoch === docEpoch());
  };

  // ref 只存在页面内存：不再写 data-ema2-ref，避免触发 MutationObserver /
  // 框架 hydration / CSS [data-*] 选择器。WeakRef 让长页面反复 snapshot 也不会
  // 把已移除的节点永久留在表里。
  const refs = window.__ema2_refs instanceof Map ? window.__ema2_refs : new Map();
  window.__ema2_refs = refs;
  const elRefs = window.__ema2_el_refs instanceof WeakMap ? window.__ema2_el_refs : new WeakMap();
  window.__ema2_el_refs = elRefs;
  const deref = (entry) => {
    if (typeof WeakRef === 'function' && entry instanceof WeakRef) return entry.deref();
    return entry;
  };
  const refElement = (ref) => {
    const entry = refs.get(ref);
    if (!entry) return null;
    const el = deref(entry);
    if (!el || !el.isConnected) {
      refs.delete(ref);
      return null;
    }
    return el;
  };
  const setRef = (el, ref) => {
    refs.set(ref, typeof WeakRef === 'function' ? new WeakRef(el) : el);
    elRefs.set(el, ref);
    if (el.getAttribute && el.getAttribute('data-ema2-ref')) el.removeAttribute('data-ema2-ref');
  };
  const refEntries = (since) => {
    const floor = Number.isFinite(Number(since)) ? Number(since) : -1;
    const out = [];
    for (const [ref, entry] of refs) {
      const parsed = parseRef(ref);
      if (!parsed || parsed.seq <= floor) continue;
      const el = deref(entry);
      if (!el || !el.isConnected) {
        refs.delete(ref);
        continue;
      }
      out.push({ ref, el });
    }
    out.sort((left, right) => (parseRef(left.ref) || { seq: 0 }).seq - (parseRef(right.ref) || { seq: 0 }).seq);
    return out;
  };
  const refOf = (el) => {
    const existing = elRefs.get(el);
    if (existing && refIsCurrent(existing)) return existing;
    // 旧版本写进 DOM 的 data-ema2-ref：迁移到内存表，然后清掉属性。
    const legacy = el.getAttribute('data-ema2-ref');
    if (refIsCurrent(legacy)) {
      const parsed = parseRef(legacy);
      const full = parsed && !parsed.epoch ? 'e' + parsed.seq + '@' + docEpoch() : legacy;
      setRef(el, full);
      return full;
    }
    const seq = (window.__ema2_ref_seq || 0) + 1;
    window.__ema2_ref_seq = seq;
    const ref = 'e' + seq + '@' + docEpoch();
    setRef(el, ref);
    return ref;
  };

  const find = (ref, selector, target) => {
    const roots = collectRoots();
    if (ref) {
      const wanted = String(ref).trim().replace(/^\[|\]$/g, '').replace(/"/g, '');
      if (!refIsCurrent(wanted)) return null;
      const parsed = parseRef(wanted);
      const full = parsed && !parsed.epoch ? `e${parsed.seq}@${docEpoch()}` : wanted;
      const fromMap = refElement(full);
      if (fromMap) return fromMap;
      // 兼容上一个 helper 版本遗留的 DOM 属性；找到后迁移进内存表。
      for (const { root } of roots) {
        const el = root.querySelector('[data-ema2-ref="' + full + '"]');
        if (el) { setRef(el, full); return el; }
      }
      return null;
    }
    if (selector) {
      for (const { root } of roots) {
        try {
          const el = root.querySelector(selector);
          if (el) return el;
        } catch (_) { /* invalid selector in this root */ }
      }
    }
    if (target) {
      const match = /^([a-zA-Z]+)\s*:\s*(.+)$/.exec(String(target).trim());
      if (match) {
        const role = match[1].toLowerCase();
        const name = match[2].trim().toLowerCase();
        const candidates = [];
        for (const { root } of roots) {
          for (const el of root.querySelectorAll(INTERACTIVE)) {
            const currentRole = roleOf(el).toLowerCase();
            if (currentRole !== role) continue;
            // 地标容器（role=search 等）不能作为 target 命中；真正的控件在后代里。
            if (!isActionableRole(el, currentRole)) continue;
            candidates.push(el);
          }
        }
        // 每一层都先取可见控件；只有整层都不可见时才回退隐藏控件。
        const pick = (test) => {
          const matching = candidates.filter(test);
          return matching.find(visible) || matching[0] || null;
        };
        const exact = pick(el => nameOf(el).toLowerCase() === name);
        if (exact) return exact;
        // role:name 同时接受 HTML name 属性；可访问名称精确匹配仍优先。
        const named = pick(el => String(el.getAttribute('name') || '').toLowerCase() === name);
        if (named) return named;
        const partial = pick(el => nameOf(el).toLowerCase().includes(name));
        if (partial) return partial;
      }
    }
    return null;
  };

  const describe = (el) => '[' + refOf(el) + '] ' + roleOf(el) + ' "' + nameOf(el) + '"';

  const candidates = (limit) => {
    const out = [];
    for (const { root } of collectRoots()) {
      for (const el of root.querySelectorAll(INTERACTIVE)) {
        if (!visible(el)) continue;
        out.push(describe(el));
        if (out.length >= (limit || 5)) return out;
      }
    }
    return out;
  };

  /** 能承载动作的角色；其余角色只在有名称时才值得占一行。 */
  const ACTIONABLE_ROLES = new Set([
    'link', 'button', 'textbox', 'searchbox', 'combobox', 'listbox', 'checkbox', 'radio',
    'switch', 'slider', 'spinbutton', 'tab', 'option', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'treeitem', 'file', 'summary', 'input', 'textarea', 'select',
  ]);
  // 真正可操作的 ARIA widget role。`role=search/banner/main/...` 是地标容器，
  // 不是控件：把 div[role=search] 当 searchbox 会让 AI 对着不可写的容器 fill。
  const WIDGET_ROLES = new Set([
    'button', 'link', 'textbox', 'searchbox', 'combobox', 'listbox', 'option', 'checkbox',
    'radio', 'switch', 'slider', 'spinbutton', 'tab', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'treeitem', 'gridcell', 'columnheader', 'rowheader', 'scrollbar',
  ]);
  const ACTIONABLE_MATCH = 'a[href],button,input,textarea,select,summary,[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';
  const isActionableRole = (el, role) => {
    if (WIDGET_ROLES.has(String(role || '').toLowerCase())) return true;
    try { if (el.matches(ACTIONABLE_MATCH)) return true; } catch (_) { /* ignore */ }
    // label 本身可点击（for= 或包着控件），但它是“激活代理”而不是输入控件。
    if (el.tagName === 'LABEL' && (el.htmlFor || el.querySelector('input,select,textarea,button'))) return true;
    return ['A', 'BUTTON', 'INPUT', 'TEXTAREA', 'SELECT', 'SUMMARY'].includes(el.tagName)
      && (el.tagName !== 'A' || el.hasAttribute('href'));
  };

  /**
   * 额外属性（主要是 href）是所有字段里最占预算的：实测占 snapshot 字符的 39%，p90 达 84 字符。
   * 截到 40 字符保留路径主干，需要完整 URL 时用 read/js；filter=links 是专门取链接的场景，不截。
   */
  const clipExtra = (extra, keepFull) => {
    if (!extra || keepFull) return extra;
    // href 必须完整；value/accept/files 等也在 extraOf 里单独限制了长度。
    // 这里不再做 40 字符的总截断，否则 value=<hidden> 会被切成误导性的半截文本。
    return extra;
  };

  /** snapshot 过滤器：links / buttons / form。 */
  const filterFor = (name) => {
    if (name === 'links') return (el) => {
      if (el.tagName !== 'A') return false;
      const href = String(el.getAttribute('href') || '').trim();
      // `#`/`javascript:` 是浏览器操作入口，不是可读链接；Bing 的 skip/feedback
      // 按钮就是这种 <a role=button href="#">，别让它们混进 links 结果。
      return !!href && href !== '#' && !/^javascript:/i.test(href);
    };
    if (name === 'buttons') return (el) => roleOf(el) === 'button';
    if (name === 'form') {
      return (el) => ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON', 'LABEL', 'FIELDSET'].includes(el.tagName)
        || roleOf(el) === 'combobox' || roleOf(el) === 'textbox' || roleOf(el) === 'checkbox' || roleOf(el) === 'radio';
    }
    return null;
  };

  const snapshot = (filter, budget, cursor, nearRef, values) => {
    window.__ema2_show_values = !!values;
    const predicate = filterFor(filter);
    let container = null;
    if (nearRef) {
      const anchor = find(nearRef, null, null);
      if (anchor) {
        container = anchor.closest('form,section,article,dialog,[role="dialog"],table,ul,ol,main,nav,header,footer') || anchor.parentElement || anchor;
      }
    }
    const seen = new Set();
    const lines = [];
    let refs = 0;
    for (const { root, frame } of collectRoots()) {
      for (const el of root.querySelectorAll('h1,h2,h3,' + INTERACTIVE)) {
        if (seen.has(el)) continue;
        seen.add(el);
        if (container && el !== container && !container.contains(el)) continue;
        if (!visible(el)) continue;
        const tag = el.tagName;
        if (tag === 'H1' || tag === 'H2' || tag === 'H3') {
          if (filter === 'full') {
            const text = clean(el.innerText);
            if (text) lines.push('#' + tag[1] + ' ' + text.slice(0, 100) + (frame ? ' frame=' + frame : ''));
          }
          continue;
        }
        if (predicate && !predicate(el)) continue;
        // filter=links 时按链接展示：Bing 等站点会给 <a href> 加 role=button，
        // 直接信 role 会让“只看链接”的结果里出现 button 行。
        const role = filter === 'links' ? 'link' : roleOf(el);
        const name = nameOf(el);
        // 地标容器（例如 Wikipedia 的 div[role=search]）不是控件；只有 filter=full
        // 才保留。实际输入框/链接会作为各自的后代单独出现。
        if (filter !== 'full' && !predicate && !isActionableRole(el, role)) continue;
        // label 只在 --filter form 里出现；默认 interactive 不把表单标签当控件，
        // 否则大表单的 snapshot 会被标签文本占满。
        if (filter !== 'full' && !predicate && tag === 'LABEL') continue;
        // 无名称又不可操作：点了也没意义，只占预算（x.com 这类站点能到 20%）。
        // filter=full 是「无损视图」，保持原样。
        if (filter !== 'full' && !name && !ACTIONABLE_ROLES.has(role)) continue;
        refs++;
        const extra = clipExtra(extraOf(el), filter === 'links');
        lines.push('[' + refOf(el) + '] ' + role + ' "' + name + '"'
          + (extra ? ' ' + extra : '') + (frame ? ' frame=' + frame : ''));
      }
    }
    const total = lines.length;
    const from = Math.min(cursor, total);
    const out = [];
    let used = 0;
    let index = from;
    while (index < total) {
      const line = lines[index];
      if (used > 0 && used + line.length + 1 > budget) break;
      out.push(line);
      used += line.length + 1;
      index++;
    }
    return {
      header: 'url=' + location.href + '\ntitle=' + clean(document.title)
        + '\nscroll=' + Math.round(window.scrollY) + '/'
        + Math.max(0, document.documentElement.scrollHeight - window.innerHeight)
        + ' refs=' + refs + ' roots=' + collectRoots().length
        + ' doc=' + docEpoch()
        + ' visual=' + document.querySelectorAll('canvas,video,img,picture,object,embed').length
        + (container ? ' near=' + (container.getAttribute('data-ema2-ref') || container.tagName.toLowerCase()) : ''),
      body: out.join('\n'),
      total,
      shown: out.length,
      next: index < total ? index : null,
    };
  };

  /** 页面可见性：checkVisibility 能识别 display:none / visibility:hidden / content-visibility。 */
  const isReadVisible = (el) => {
    if (!el) return false;
    if (el.hidden || el.getAttribute('aria-hidden') === 'true') return false;
    try {
      if (typeof el.checkVisibility === 'function') {
        return el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true });
      }
      const view = (el.ownerDocument && el.ownerDocument.defaultView) || window;
      const style = view.getComputedStyle(el);
      return style.display !== 'none' && style.visibility !== 'hidden';
    } catch (_) {
      return true;
    }
  };

  /** 正文容器启发式：取文本最多、链接密度最低的候选；太短就回退 body。 */
  const mainRoot = () => {
    const selectors = ['main', '[role="main"]', 'article', '#content', '#main', '.content', '.main',
      '.article', '.post', '.markdown-body', '.rich_media_content', '.article-content', '.post-content'];
    let best = null;
    let bestScore = 0;
    let bestLength = 0;
    let candidateLength = 0;
    let sawCandidate = false;
    let evaluated = 0;
    for (const selector of selectors) {
      for (const el of document.querySelectorAll(selector)) {
        sawCandidate = true;
        // 先用便宜的 textContent 长度做门槛：大页面上候选可能几十个，
        // 每个都跑一遍 plainText（整棵子树遍历）会把 read 拖成秒级。
        if ((el.textContent || '').length < 200) continue;
        if (evaluated >= 12) break;
        evaluated++;
        // 用清洁后的文本评分，避免 script/style 把候选撑大。
        const text = plainText(el);
        candidateLength = Math.max(candidateLength, text.length);
        if (text.length < 200) continue;
        const linkLength = [...el.querySelectorAll('a')].reduce((sum, link) => sum + (link.innerText || '').length, 0);
        const score = text.length - linkLength * 1.5;
        if (score > bestScore) { bestScore = score; best = el; bestLength = text.length; }
      }
    }
    const body = document.body || document.documentElement;
    const bodyLength = body ? plainText(body).length : 0;
    if (!best) {
      return {
        root: body,
        selector: sawCandidate ? 'body(fallback)' : 'body',
        fallback: sawCandidate,
        bodyLength,
        textLength: candidateLength,
      };
    }
    const tooShort = bestLength < 200 || (bodyLength > 2000 && bestLength < bodyLength * 0.05);
    if (tooShort) {
      return { root: body, selector: 'body(fallback)', fallback: true, bodyLength, textLength: bestLength };
    }
    return {
      root: best,
      selector: best.tagName.toLowerCase() + (best.id ? '#' + best.id : ''),
      fallback: false,
      bodyLength,
      textLength: bestLength,
    };
  };

  // ---------------------------------------------------------------- read 渲染
  // md / text 共用一次 DOM 遍历，产出轻量 block/inline 结构；html 走 innerHTML，
  // visible 直接使用浏览器渲染后的 innerText。三种来源共用 read 的选区、预算和游标。
  const READ_SKIP = /^(SCRIPT|STYLE|NOSCRIPT|IFRAME|SVG|CANVAS|VIDEO|AUDIO|TEMPLATE|TEXTAREA|INPUT|SELECT|OPTION|NAV|HEADER|FOOTER|ASIDE|FORM)$/;
  const READ_BLOCK = /^(ADDRESS|ARTICLE|ASIDE|BLOCKQUOTE|DIV|DL|FIELDSET|FIGCAPTION|FIGURE|FOOTER|FORM|H[1-6]|HEADER|HR|LI|MAIN|NAV|OL|P|PRE|SECTION|TABLE|TBODY|TD|TFOOT|TH|THEAD|TR|UL)$/;

  const hrefOf = (el) => {
    const raw = el.getAttribute('href') || '';
    if (!raw || /^javascript:/i.test(raw) || raw === '#') return '';
    // 同源 iframe 里的相对链接要按 iframe 的 baseURI 解析，不能用顶层 location。
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  // 链接文字里的 ] 会提前闭合 markdown；URL 里的括号/空格会截断目标。
  const escapeMdText = (value) => String(value).replace(/([\\\[\]])/g, '\\$1');
  const escapeMdUrl = (value) => String(value)
    .replace(/ /g, '%20')
    .replace(/\(/g, '%28')
    .replace(/\)/g, '%29')
    .replace(/</g, '%3C')
    .replace(/>/g, '%3E');
  const srcOf = (el) => {
    const raw = el.currentSrc || el.getAttribute('src') || el.getAttribute('data-src') || '';
    if (!raw) return '';
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  const controlValue = (root) => {
    if (!root) return null;
    if (root.tagName === 'INPUT') return String(root.value == null ? '' : root.value);
    if (root.tagName === 'TEXTAREA') return String(root.value == null ? '' : root.value);
    if (root.tagName === 'SELECT') {
      return [...root.selectedOptions].map(option => option.value).join(',');
    }
    return null;
  };
  const cellText = (cell) => clean(cell.innerText || cell.textContent || '');
  const tableRows = (table) => [...table.querySelectorAll('tr')]
    .map(row => [...row.children]
      .filter(cell => /^(TD|TH)$/.test(cell.tagName))
      .map(cellText))
    .filter(cells => cells.length);
  const mdCell = (value) => String(value).replace(/\|/g, '\\|');

  /** 单个不可再拆的行内节点；A/CODE/STRONG/EM 保持和旧 inline() 一样的原子语义。 */
  const inlineNode = (el) => {
    const tag = el.tagName;
    if (tag === 'A') {
      return {
        t: 'link',
        text: clean(el.innerText || el.textContent || ''),
        href: hrefOf(el),
      };
    }
    if (tag === 'IMG') {
      return {
        t: 'img',
        alt: clean(el.getAttribute('alt') || el.getAttribute('aria-label') || el.getAttribute('title') || ''),
        src: srcOf(el),
      };
    }
    if (tag === 'CODE') return { t: 'code', text: clean(el.innerText || '') };
    if (tag === 'STRONG' || tag === 'B') return { t: 'strong', text: clean(el.innerText || '') };
    if (tag === 'EM' || tag === 'I') return { t: 'em', text: clean(el.innerText || '') };
    return { t: 'text', v: '' };
  };

  /** 行内层：文本节点、BR、链接、图片、行内代码/强调；其他元素递归。 */
  const inlineIn = (parent) => {
    const out = [];
    for (const node of parent.childNodes || []) {
      if (node.nodeType === 3) {
        out.push({ t: 'text', v: node.nodeValue || '' });
        continue;
      }
      if (node.nodeType !== 1) continue;
      const el = node;
      const tag = el.tagName;
      if (!tag || READ_SKIP.test(tag)) continue;
      if (!isReadVisible(el)) continue;
      if (el.shadowRoot) { out.push(...inlineIn(el.shadowRoot)); continue; }
      if (tag === 'BR') { out.push({ t: 'br' }); continue; }
      if (['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(tag)) {
        out.push(inlineNode(el));
        continue;
      }
      out.push(...inlineIn(el));
    }
    return out;
  };

  const listBlock = (el) => {
    const ordered = el.tagName === 'OL';
    const reversed = ordered && el.hasAttribute('reversed');
    const parsedStart = Number.parseInt(el.getAttribute('start') || '', 10);
    const items = [...el.children]
      .filter(item => item.tagName === 'LI')
      .map(li => {
        const ownValue = Number.parseInt(li.getAttribute('value') || '', 10);
        return {
          inlines: inlineIn(li),
          value: Number.isFinite(ownValue) ? ownValue : null,
        };
      });
    return {
      b: 'list',
      ordered,
      reversed,
      start: Number.isFinite(parsedStart) ? parsedStart : null,
      items,
    };
  };

  /** 块级层：段落、标题、代码块、表格、列表；段落内部用 inlineIn。 */
  const readBlocks = (root) => {
    const blocks = [];
    let pending = [];
    const flush = () => {
      if (pending.length) {
        blocks.push({ b: 'p', inlines: pending });
        pending = [];
      }
    };
    const walk = (parent) => {
      for (const node of parent.childNodes || []) {
        if (node.nodeType === 3) {
          pending.push({ t: 'text', v: node.nodeValue || '' });
          continue;
        }
        if (node.nodeType !== 1) continue;
        const el = node;
        const tag = el.tagName;
        if (!tag || READ_SKIP.test(tag)) continue;
        if (!isReadVisible(el)) continue;
        if (el.shadowRoot) { flush(); walk(el.shadowRoot); flush(); continue; }
        if (tag === 'BR') { pending.push({ t: 'br' }); continue; }
        if (['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(tag)) {
          pending.push(inlineNode(el));
          continue;
        }
        if (!el.innerText && !el.querySelector('img,picture,video')) continue;
        if (/^H[1-4]$/.test(tag)) {
          flush();
          const text = clean(el.innerText);
          if (text) blocks.push({ b: 'h', level: Number(tag[1]), text });
          continue;
        }
        if (tag === 'PRE') {
          flush();
          const code = (el.innerText || '').replace(/^\n+/, '').replace(/\s+$/, '');
          if (code.trim()) blocks.push({ b: 'pre', text: code });
          continue;
        }
        if (tag === 'TABLE') {
          const rows = tableRows(el);
          if (rows.length) { flush(); blocks.push({ b: 'table', rows }); }
          continue;
        }
        if (tag === 'UL' || tag === 'OL') {
          flush();
          const list = listBlock(el);
          if (list.items.length) blocks.push(list);
          continue;
        }
        if (READ_BLOCK.test(tag)) {
          flush();
          walk(el);
          flush();
          continue;
        }
        pending.push(...inlineIn(el));
      }
    };
    if (root && ['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(root.tagName)) {
      return [{ b: 'p', inlines: [inlineNode(root)] }];
    }
    if (root && root.shadowRoot) walk(root.shadowRoot);
    else walk(root);
    flush();
    return blocks;
  };

  const markdownInline = (nodes, separator = '') => nodes.map(node => {
    switch (node.t) {
      case 'text': return node.v;
      case 'br': return '\n';
      case 'link':
        if (!node.text) return node.href ? escapeMdUrl(node.href) + ' ' : '';
        return node.href
          ? '[' + escapeMdText(node.text) + '](' + escapeMdUrl(node.href) + ') '
          : escapeMdText(node.text);
      case 'img':
        if (!node.src) return node.alt ? escapeMdText(node.alt) + ' ' : '';
        return '![' + escapeMdText(node.alt) + '](' + escapeMdUrl(node.src) + ') ';
      case 'code': return node.text ? '`' + node.text + '`' : '';
      case 'strong': return node.text ? '**' + node.text + '**' : '';
      case 'em': return node.text ? '*' + node.text + '*' : '';
      default: return '';
    }
  }).join(separator);

  const markdown = (root) => {
    // 直接 read 一个控件时，md 也应返回当前值；否则 --selector "#text" 会是空。
    const control = controlValue(root);
    if (control !== null) return control;
    const out = [];
    for (const block of readBlocks(root)) {
      if (block.b === 'p') {
        const text = clean(markdownInline(block.inlines));
        if (text) out.push(text);
      } else if (block.b === 'h') {
        out.push('#'.repeat(block.level) + ' ' + block.text);
      } else if (block.b === 'pre') {
        out.push('```\n' + block.text + '\n```');
      } else if (block.b === 'table') {
        const width = Math.max(...block.rows.map(row => row.length));
        const pad = (cells) => [...cells, ...Array(Math.max(0, width - cells.length)).fill('')];
        const head = pad(block.rows[0]).map(mdCell);
        const lines = [
          '| ' + head.join(' | ') + ' |',
          '| ' + head.map(() => '---').join(' | ') + ' |',
        ];
        for (const row of block.rows.slice(1)) {
          lines.push('| ' + pad(row).map(mdCell).join(' | ') + ' |');
        }
        out.push(lines.join('\n'));
      } else if (block.b === 'list') {
        let number = block.start != null
          ? block.start
          : (block.reversed ? block.items.length : 1);
        const lines = [];
        for (const item of block.items) {
          if (block.ordered && item.value != null) number = item.value;
          // 列表项沿用旧行为：每个直接行内节点之间补一个空格。
          const text = clean(markdownInline(item.inlines, ' '));
          if (text) lines.push((block.ordered ? number + '. ' : '- ') + text);
          if (block.ordered) number += block.reversed ? -1 : 1;
        }
        if (lines.length) out.push(lines.join('\n'));
      }
    }
    return out.join('\n\n');
  };

  const textInline = (nodes) => {
    const lines = [];
    let parts = [];
    const flush = () => {
      const text = clean(parts.join(' '));
      if (text) lines.push(text);
      parts = [];
    };
    for (const node of nodes) {
      if (node.t === 'br') { flush(); continue; }
      const value = node.t === 'text' ? node.v
        : node.t === 'link' ? node.text
        : node.t === 'img' ? node.alt
        : (node.text || '');
      if (value) parts.push(value);
    }
    flush();
    return lines;
  };

  const plainText = (root) => {
    // `read --selector "#input"` 应该给出控件的当前值，而不是空字符串。
    const control = controlValue(root);
    if (control !== null) return control;
    const lines = [];
    for (const block of readBlocks(root)) {
      if (block.b === 'p') {
        lines.push(...textInline(block.inlines));
      } else if (block.b === 'h') {
        const text = clean(block.text);
        if (text) lines.push(text);
      } else if (block.b === 'pre') {
        const text = clean(block.text);
        if (text) lines.push(text);
      } else if (block.b === 'table') {
        for (const row of block.rows) {
          for (const cell of row) {
            const text = clean(cell);
            if (text) lines.push(text);
          }
        }
      } else if (block.b === 'list') {
        for (const item of block.items) lines.push(...textInline(item.inlines));
      }
    }
    return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim();
  };

  /** visible：浏览器渲染后的文本；不做正文清洗，默认仍受 read 的 main/selector 选区约束。 */
  const visibleText = (root) => {
    const control = controlValue(root);
    if (control !== null) return control;
    if (!root) return '';
    try {
      return String(root.innerText == null ? '' : root.innerText).trim();
    } catch (_) {
      return '';
    }
  };

  /** iframe 内元素的 rect 是相对 iframe viewport 的；标注要换算到顶层截图坐标。 */
  const frameOffset = (frameEl) => {
    if (!frameEl) return { x: 0, y: 0 };
    const rect = frameEl.getBoundingClientRect();
    const parentWin = frameEl.ownerDocument && frameEl.ownerDocument.defaultView;
    const parentFrame = parentWin && parentWin.frameElement ? parentWin.frameElement : null;
    const base = frameOffset(parentFrame);
    return {
      x: base.x + rect.left + (Number(frameEl.clientLeft) || 0),
      y: base.y + rect.top + (Number(frameEl.clientTop) || 0),
    };
  };

  /** 当前视口内的 ref 标记（用于截图标注，set-of-marks）。 */
  const marks = () => {
    // 同一元素在旧 document epoch 下可能还留着一个 ref；只保留当前文档的标记，
    // 并按元素去重（后分配的 ref 胜出），否则截图会出现同一元素两个编号。
    const byElement = new Map();
    for (const { ref, el } of refEntries(0)) {
      if (!refIsCurrent(ref)) continue;
      const rect = el.getBoundingClientRect();
      if (rect.width < 2 || rect.height < 2) continue;
      const win = (el.ownerDocument && el.ownerDocument.defaultView) || window;
      const offset = frameOffset(win.frameElement || null);
      // 标注坐标必须落在截图范围内；负坐标/视口外元素会画到图外或误导 AI。
      const x = Math.max(0, Math.round(rect.left + offset.x));
      const y = Math.max(0, Math.round(rect.top + offset.y));
      const right = Math.min(window.innerWidth, Math.round(rect.right + offset.x));
      const bottom = Math.min(window.innerHeight, Math.round(rect.bottom + offset.y));
      const w = right - x;
      const h = bottom - y;
      if (w < 2 || h < 2) continue;
      byElement.set(el, {
        ref,
        role: roleOf(el),
        name: nameOf(el).slice(0, 30),
        x,
        y,
        w,
        h,
      });
    }
    return [...byElement.values()];
  };

  /**
   * 写值。options.mode=replace（fill/select）覆盖；insert（type）按当前 caret/selection 插入。
   *
   * 所有失败路径都会恢复原值：命令返回 error 时页面不能被改坏，
   * 否则 AI 继续提交表单就会提交空值。
   */
  const setValue = (el, value, options) => {
    if (!el) return { ok: false, code: 'target_not_found', message: 'target not found' };
    if (el.disabled) return { ok: false, code: 'element_disabled', message: 'target is disabled' };
    const tag = el.tagName;
    if (tag === 'INPUT') {
      const type = String(el.type || 'text').toLowerCase();
      if (type === 'file' || type === 'checkbox' || type === 'radio') {
        return { ok: false, code: 'unsupported_control', message: `cannot type into input[type=${type}]` };
      }
    }
    const doc = el.ownerDocument || document;
    const win = doc.defaultView || window;
    const text = String(value == null ? '' : value);
    const mode = options && options.mode === 'insert' ? 'insert' : 'replace';

    if (el.isContentEditable) {
      // type "" 是合法 no-op；不要让 execCommand('insertText','') 的 false 误报为失败。
      if (mode === 'insert' && text === '') return { ok: true, selfDispatched: true };
      // X/Slack/Discord 等富文本编辑器依赖原生 input 事件；直接写 textContent
      // 不会更新它们内部状态（例如 X 的“发帖”按钮仍然 disabled）。
      const beforeText = String(el.textContent || '');
      const beforeHtml = String(el.innerHTML || '');
      const restore = () => {
        try { el.innerHTML = beforeHtml; } catch (_) { el.textContent = beforeText; }
      };
      const insert = () => {
        el.focus();
        try {
          const selection = win.getSelection();
          if (selection) {
            if (mode === 'replace') {
              const range = doc.createRange();
              range.selectNodeContents(el);
              selection.removeAllRanges();
              selection.addRange(range);
            } else {
              const anchor = selection.anchorNode;
              const inside = !!anchor && (el === anchor || el.contains(anchor));
              if (!selection.rangeCount || !inside) {
                // 没有 caret 时插到末尾；有选区时必须尊重它，不能清空重写。
                const range = doc.createRange();
                range.selectNodeContents(el);
                range.collapse(false);
                selection.removeAllRanges();
                selection.addRange(range);
              }
            }
          }
          return doc.execCommand('insertText', false, text);
        } catch (_) {
          return false;
        }
      };
      let inserted = insert();
      if (mode === 'replace' && String(el.textContent || '') !== text) {
        // 极少数编辑器在首次聚焦/重载后会把同一次 insertText 应用两次；复核并修一次。
        inserted = insert();
      }
      if (mode === 'replace' && (!inserted || String(el.textContent || '') !== text)) {
        el.textContent = text;
        try {
          el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
        } catch (_) { /* ignore */ }
      } else if (!inserted) {
        restore();
        return { ok: false, code: 'input_prevented', message: 'the editor rejected insertText' };
      }
      // execCommand 已经触发原生 input；外层不要再补发，避免某些编辑器重复应用。
      return { ok: true, selfDispatched: true };
    }

    let proto = null;
    if (tag === 'INPUT' && win.HTMLInputElement) proto = win.HTMLInputElement.prototype;
    else if (tag === 'TEXTAREA' && win.HTMLTextAreaElement) proto = win.HTMLTextAreaElement.prototype;
    else if (tag === 'SELECT' && win.HTMLSelectElement) proto = win.HTMLSelectElement.prototype;
    if (!proto) return { ok: false, code: 'not_writable', message: `cannot write to ${String(tag).toLowerCase()}` };
    if (el.readOnly && tag !== 'SELECT') return { ok: false, code: 'element_readonly', message: 'target is read-only' };
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (!descriptor || typeof descriptor.set !== 'function') {
      return { ok: false, code: 'not_writable', message: 'value setter is unavailable' };
    }

    if (tag === 'SELECT') {
      const previous = [...el.options].map(option => option.selected);
      const restore = () => {
        [...el.options].forEach((option, index) => { option.selected = !!previous[index]; });
      };
      const requested = el.multiple
        ? text.split(',').map(part => part.trim()).filter(Boolean)
        : [text];
      const unique = [...new Set(requested)];
      const missing = unique.filter(want => ![...el.options].some(option => option.value === want));
      if (missing.length) {
        restore();
        return {
          ok: false, code: 'value_not_available',
          message: `select has no option with value: ${missing.join(',')}`,
        };
      }
      try {
        if (el.multiple) {
          // <select multiple> 支持逗号分隔的多个 value；真实用户是 Ctrl+click 多选。
          for (const option of el.options) option.selected = unique.includes(option.value);
          const done = unique.every(want => [...el.options].some(option => option.selected && option.value === want));
          if (!done) { restore(); return { ok: false, code: 'value_not_available', message: 'select did not accept all values' }; }
        } else {
          descriptor.set.call(el, text);
          if (String(el.value) !== text) {
            restore();
            return { ok: false, code: 'value_not_available', message: 'select has no matching option' };
          }
        }
      } catch (error) {
        restore();
        return { ok: false, code: 'value_rejected', message: clean(error && error.message || error) };
      }
      return { ok: true };
    }

    const previousValue = String(el.value == null ? '' : el.value);
    const previousStart = el.selectionStart;
    const previousEnd = el.selectionEnd;
    const restore = () => {
      try {
        descriptor.set.call(el, previousValue);
        if (typeof previousStart === 'number' && typeof previousEnd === 'number') {
          el.setSelectionRange(previousStart, previousEnd);
        }
      } catch (_) { /* 控件可能不支持 selection */ }
    };

    let nextValue;
    let caret = null;
    if (mode === 'insert') {
      const current = previousValue;
      const start = typeof previousStart === 'number'
        ? Math.min(previousStart, current.length) : current.length;
      const end = typeof previousEnd === 'number'
        ? Math.min(Math.max(previousEnd, start), current.length) : start;
      nextValue = current.slice(0, start) + text + current.slice(end);
      caret = start + text.length;
    } else {
      nextValue = text;
      caret = nextValue.length;
    }

    // 输入掩码（IMask / 金额 / 手机号格式化）依赖 beforeinput；
    // 页面主动 cancel 时说明它自己接管了输入，这时不强写，避免和页面状态打架。
    try {
      const before = new InputEvent('beforeinput', {
        bubbles: true, cancelable: true, inputType: 'insertText', data: text,
      });
      if (!el.dispatchEvent(before)) {
        return {
          ok: false,
          code: 'input_prevented',
          message: 'the page cancelled beforeinput for this control',
        };
      }
    } catch (_) { /* 老浏览器/特殊控件忽略 */ }

    try {
      descriptor.set.call(el, nextValue);
    } catch (error) {
      restore();
      return { ok: false, code: 'value_rejected', message: clean(error && error.message || error) };
    }
    // number/date/range 等控件会清洗值（例如 "abc" -> ""）；不能把 setter 没抛错当成写入成功。
    if (String(el.value) !== nextValue) {
      restore();
      return {
        ok: false,
        code: 'value_rejected',
        message: 'the control normalized the value (type/maxlength/range?); the previous value was restored',
      };
    }
    if (caret != null) {
      try { el.setSelectionRange(caret, caret); } catch (_) { /* number/date 等不支持 selection */ }
    }
    return { ok: true };
  };

  // 升级 helper 时把旧版本遗留的 DOM 属性清掉（新 ref 只在内存表里）。
  try {
    for (const { root } of collectRoots()) {
      for (const el of root.querySelectorAll('[data-ema2-ref]')) el.removeAttribute('data-ema2-ref');
    }
  } catch (_) { /* ignore */ }

  window.__ema2_helpers = {
    snapshot, markdown, plainText, visibleText, mainRoot, find, refOf, refEntries, clean, candidates,
    collectRoots, roleOf, nameOf, marks, docEpoch, setDocHandle, parseRef, refIsCurrent,
    setValue, isActionableRole, absoluteHref,
  };
  window.__ema2_helpers_version = 19;
  return { installed: true };
}

// 必须用 local 而不是 session：session 在扩展重载/更新时会被清空，
// 计数器归零后新文档会重新拿到 d1、d2…，和仍然开着的旧页面撞号，
// 于是旧 ref 就能在另一个文档上通过校验。
const DOC_SEQ_KEY = 'ema2_doc_seq';
let docHandleSeq = 0;
let docSeqReady = null;

/**
 * 只从 storage.session 读一次。
 * 必须缓存「读取过程」而不是「读取结果」：两个 tab 同时首次取号时，
 * 各自 await 一次 get() 会把已经自增过的值重新覆盖回 0，于是两边都拿到 d1。
 */
function ensureDocSeq() {
  if (!docSeqReady) {
    docSeqReady = (async () => {
      try {
        const stored = await chrome.storage.local.get(DOC_SEQ_KEY);
        // 内存里可能已经发过号（上一次写入失败）；不能回退到更小的持久值。
        docHandleSeq = Math.max(docHandleSeq, Number(stored && stored[DOC_SEQ_KEY]) || 0);
      } catch (_) {
        // 读取失败不能永久缓存成 0：下次调用重试；内存里已发过的号必须保留。
        docSeqReady = null;
      }
    })();
  }
  return docSeqReady;
}

/** 下一个文档句柄（d1、d2…）。初始化之后的自增是同步的，两个文档不会拿到同一个号。 */
async function nextDocHandle() {
  await ensureDocSeq();
  docHandleSeq += 1;
  try {
    // await：service worker 在写入前被回收会让序号回绕，旧 ref 就可能在新文档上通过校验。
    await chrome.storage.local.set({ [DOC_SEQ_KEY]: docHandleSeq });
  } catch (_) { /* 没有 storage 就只在内存里递增 */ }
  return 'd' + docHandleSeq;
}

/**
 * 注入页面 helper，并给新文档分配一个短句柄。
 * 句柄只在「本次真的装了 helper」时分配：已装好的页面不消耗号，号就不会被空调用推高。
 */
async function ensureHelpers(tabId) {
  const result = await runInTab(tabId, installHelpers, []);
  // 注入结果丢了（帧刚切走）时按「装了」处理：宁可变废旧 ref，也不能让页面带着空句柄跑。
  const installed = result == null || result === true || result.installed === true;
  // 已安装且句柄还在：不消耗新号。
  if (!installed && result && result.handle) return true;
  // 新安装，或旧页面丢了句柄（上次 setDocHandle 失败）：补一个。
  const handle = await nextDocHandle();
  try {
    await runInTab(tabId, (value) => window.__ema2_helpers.setDocHandle(value), [handle]);
  } catch (_) { /* 途中导航：新文档会重新安装并拿到新句柄 */ }
  return true;
}

/** 在 MAIN/isolated world 中执行一次完整的“查找 + 校验 + 动作”，失败结果由调用方映射为错误。 */
function performActionInPage(spec) {
  const helpers = window.__ema2_helpers;
  if (!helpers) return { found: false, code: 'helpers_missing', message: 'page helpers are not installed', hint: 'reload the page and retry' };
  const action = spec.action;
  const ref = spec.ref || null;
  const selector = spec.selector || null;
  const target = spec.target || null;
  const hasLocator = !!(ref || selector || target);
  const missing = () => ({
    found: false,
    stale: !!ref && !helpers.refIsCurrent(ref),
    hadSnapshot: !!helpers.docEpoch(),
    candidates: helpers.candidates(5),
  });

  /** 穿透 shadow root 与同源 iframe 的命中测试。坐标相对传入 document 的 viewport。 */
  const hitTest = (doc, x, y) => {
    if (!doc) return null;
    let hit = doc.elementFromPoint(x, y);
    for (let guard = 0; hit && hit.shadowRoot && guard < 10; guard++) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    if (hit && /^(IFRAME|FRAME)$/.test(hit.tagName)) {
      try {
        const innerDoc = hit.contentDocument;
        if (innerDoc) {
          const rect = hit.getBoundingClientRect();
          const inner = hitTest(
            innerDoc,
            x - rect.left - (Number(hit.clientLeft) || 0),
            y - rect.top - (Number(hit.clientTop) || 0),
          );
          if (inner) return inner;
        }
      } catch (_) { /* cross-origin */ }
    }
    return hit;
  };

  const relatedTo = (el, hit) => {
    if (!el || !hit) return false;
    if (el === hit || el.contains(hit) || hit.contains(el)) return true;
    // shadow DOM：沿 host 链向上找目标元素。
    let node = hit;
    for (let guard = 0; node && guard < 20; guard++) {
      if (node === el) return true;
      node = node.parentNode || node.host || null;
    }
    return false;
  };

  /**
   * 点击前检查真实可点性：隐藏/ inert / pointer-events:none / 被 overlay 覆盖。
   * 返回 null 表示可点；否则返回可直接映射为 ToolError 的结果。
   */
  const clickBlocker = (el) => {
    if (!helpers.isActionableRole(el, helpers.roleOf(el))) {
      return {
        code: 'not_clickable',
        message: 'target is a non-interactive container',
        hint: 'snapshot again and click the real button/link/input ref',
      };
    }
    const doc = el.ownerDocument || document;
    const view = doc.defaultView || window;
    let style;
    try { style = view.getComputedStyle(el); } catch (_) { style = null; }
    if (style && (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0')) {
      return {
        code: 'not_clickable',
        message: 'target is not visible',
        hint: 'the element may have been hidden after the snapshot; take a fresh snapshot',
      };
    }
    let inert = false;
    try { inert = !!el.closest('[inert]'); } catch (_) { inert = false; }
    if (inert) {
      return {
        code: 'not_clickable',
        message: 'target is inside an inert subtree',
        hint: 'close the modal/overlay that made the page inert, then retry',
      };
    }
    if (style && style.pointerEvents === 'none') {
      return {
        code: 'not_clickable',
        message: 'target has pointer-events:none',
        hint: 'click the element that actually receives pointer events',
      };
    }
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return { code: 'not_clickable', message: 'target has an empty box', hint: 'snapshot again' };
    }
    const x = Math.round(rect.left + rect.width / 2);
    const y = Math.round(rect.top + rect.height / 2);
    let hit = null;
    try { hit = hitTest(doc, x, y); } catch (_) { hit = null; }
    if (!hit) {
      return {
        code: 'not_clickable',
        message: 'target is outside the viewport',
        hint: 'scroll it into view (act scroll) and snapshot again',
      };
    }
    if (!relatedTo(el, hit)) {
      const tag = String(hit.tagName || 'element').toLowerCase();
      return {
        code: 'obscured',
        message: `target is covered by <${tag}>`,
        hint: 'close the overlay/modal/cookie banner first, or click the element that actually receives the pointer',
      };
    }
    return null;
  };

  const formValidationFailed = (form, submitter) => {
    if (!form || typeof form.checkValidity !== 'function') return false;
    if (form.noValidate) return false;
    if (submitter && submitter.formNoValidate) return false;
    try { return !form.checkValidity(); } catch (_) { return false; }
  };

  // press 的 key 支持 "Control+Shift+L"：最后一段是键，其余是修饰位。
  const parseKeyCombo = (raw) => {
    const source = String(raw || '').trim();
    // '+' 本身无法用分隔符解析，单独处理（真实事件是 Shift+Equal）。
    if (source === '+') {
      return { key: '+', code: 'Equal', ctrlKey: false, shiftKey: true, altKey: false, metaKey: false };
    }
    const parts = source.split('+').map(part => part.trim()).filter(Boolean);
    const token = parts.pop() || '';
    const combo = { key: '', code: '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
    for (const part of parts) {
      const lower = part.toLowerCase();
      if (lower === 'ctrl' || lower === 'control') combo.ctrlKey = true;
      else if (lower === 'shift') combo.shiftKey = true;
      else if (lower === 'alt' || lower === 'option') combo.altKey = true;
      else if (lower === 'meta' || lower === 'cmd' || lower === 'command' || lower === 'win') combo.metaKey = true;
    }
    if (!token) return combo;
    const alias = {
      esc: 'Escape', escape: 'Escape', enter: 'Enter', return: 'Enter', tab: 'Tab', space: ' ',
      backspace: 'Backspace', delete: 'Delete', del: 'Delete', insert: 'Insert', home: 'Home',
      end: 'End', pageup: 'PageUp', pagedown: 'PageDown', arrowup: 'ArrowUp', up: 'ArrowUp',
      arrowdown: 'ArrowDown', down: 'ArrowDown', arrowleft: 'ArrowLeft', left: 'ArrowLeft',
      arrowright: 'ArrowRight', right: 'ArrowRight',
    };
    const lower = token.toLowerCase();
    if (alias[lower] !== undefined) {
      combo.key = alias[lower];
      combo.code = alias[lower] === ' ' ? 'Space' : alias[lower];
      return combo;
    }
    if (token.length === 1) {
      combo.key = token;
      combo.code = /[a-z]/i.test(token) ? `Key${token.toUpperCase()}`
        : (/[0-9]/.test(token) ? `Digit${token}` : '');
      return combo;
    }
    combo.key = token;
    combo.code = /^f\d{1,2}$/i.test(token) ? token.toUpperCase() : token;
    return combo;
  };

  if (action === 'fill') {
    const fields = Array.isArray(spec.fields) ? spec.fields : [];
    const results = [];
    for (const item of fields) {
      const label = item.ref || item.selector || item.target || '(none)';
      const el = helpers.find(item.ref || null, item.selector || null, item.target || null);
      if (!el) {
        results.push({ ok: false, target: label, code: item.ref && !helpers.refIsCurrent(item.ref) ? 'stale_ref' : 'target_not_found' });
        continue;
      }
      const written = helpers.setValue(el, item.value);
      if (!written.ok) {
        results.push({ ok: false, target: label, code: written.code, message: written.message });
        continue;
      }
      if (!written.selfDispatched) {
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      results.push({ ok: true, target: label });
    }
    const failed = results.filter(item => !item.ok);
    if (failed.length) {
      return {
        found: true,
        ok: false,
        code: failed[0].code || 'target_not_found',
        message: 'some fill targets failed: ' + failed.map(item => item.target).join(','),
        fields: results,
      };
    }
    return { found: true, ok: true, info: `fields=${results.length}` };
  }

  let el;
  if (action === 'press' && !hasLocator) {
    el = document.activeElement || document.body;
  } else if (action === 'submit' && !hasLocator) {
    el = document.activeElement || document.body;
  } else {
    el = helpers.find(ref, selector, target);
    if (!el) return missing();
  }
  if (!el) return missing();
  try { el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); } catch (_) { /* ignore */ }
  const disabled = !!el.disabled || el.getAttribute('aria-disabled') === 'true';

  switch (action) {
    case 'click': {
      if (disabled) return { found: true, ok: false, code: 'element_disabled', message: 'target is disabled' };
      const blocker = clickBlocker(el);
      if (blocker) return { found: true, ok: false, ...blocker };
      const tag = el.tagName.toLowerCase();
      const form = el.closest && el.closest('form');
      const anchor = tag === 'a' && !!(el.href || el.getAttribute('href'));
      const submitter = (tag === 'button' && (!el.type || el.type === 'submit'))
        || (tag === 'input' && (el.type === 'submit' || el.type === 'image'));
      const rawHref = anchor ? String(el.getAttribute('href') || '') : '';
      const lowerHref = rawHref.trim().toLowerCase();
      // 只有 http(s)/相对导航链接才值得按“导航”等待/重试；
      // javascript:、mailto:、纯 # 锚点、download 都不是页面导航。
      const navigable = anchor
        && !el.hasAttribute('download')
        && !lowerHref.startsWith('javascript:')
        && !lowerHref.startsWith('mailto:')
        && !lowerHref.startsWith('tel:')
        && !lowerHref.startsWith('#');
      const download = anchor && el.hasAttribute('download');
      const validationFailed = submitter && formValidationFailed(form, el);
      const effect = validationFailed ? 'none'
        : download ? 'download'
          : navigable || (submitter && !!form) ? 'nav' : 'page';
      const nextUrl = anchor ? (el.href || el.getAttribute('href') || '') : (submitter && form ? form.action || '' : '');
      // 纯 el.click() 只产生一个 click 事件：没有 pointer/mouse 序列，聚焦还是静默的，
      // 密码管理器这类靠 focusin/pointerdown 出浮层的扩展完全看不到这次点击。
      // 按真实浏览器顺序补事件，真正的激活仍然交给 el.click()，只发生一次。
      try {
        const rect = el.getBoundingClientRect();
        const x = Math.round(rect.left + rect.width / 2);
        const y = Math.round(rect.top + rect.height / 2);
        const base = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window };
        const pointer = (type, buttons) => el.dispatchEvent(new PointerEvent(type, {
          ...base, button: 0, buttons, pointerId: 1, pointerType: 'mouse', isPrimary: true,
        }));
        const mouse = (type, buttons) => el.dispatchEvent(new MouseEvent(type, { ...base, button: 0, buttons, detail: 1 }));
        pointer('pointerover', 0);
        pointer('pointerenter', 0);
        pointer('pointermove', 0);
        pointer('pointerdown', 1);
        mouse('mousedown', 1);
        try { el.focus({ preventScroll: true }); } catch (_) { try { el.focus(); } catch (_) { /* ignore */ } }
        if (typeof el.setSelectionRange === 'function') {
          try {
            const length = String(el.value == null ? '' : el.value).length;
            el.setSelectionRange(length, length);
          } catch (_) { /* 非文本控件不支持选区，忽略 */ }
        }
        pointer('pointerup', 0);
        mouse('mouseup', 0);
      } catch (_) { /* 补事件是尽力而为，不影响下面的真实激活 */ }
      try { el.click(); } catch (error) {
        return { found: true, ok: false, code: 'click_failed', message: String(error && error.message || error) };
      }
      const info = ref ? `ref=${ref}` : target ? `target=${target}` : `selector=${selector}`;
      // canRetry 只影响 settle 后的重试策略：只有可导航链接允许安全重试，
      // javascript:/download/# 链接和按钮都不重试，避免副作用执行两次。
      const kind = navigable ? 'anchor' : submitter ? 'submit' : 'node';
      return {
        found: true, ok: true, info, effect, nextUrl, kind, canRetry: navigable,
        download, validation: validationFailed ? 'failed' : '',
      };
    }
    case 'type': {
      const text = spec.text === undefined ? '' : String(spec.text);
      // type 在 caret/selection 处插入；fill 才覆盖整个值。
      const written = helpers.setValue(el, text, { mode: 'insert' });
      if (!written.ok) return { found: true, ...written };
      try { el.focus(); } catch (_) { /* ignore */ }
      if (!written.selfDispatched) {
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      let effect = 'none';
      let nextUrl = '';
      let submitted = false;
      let validation = '';
      if (spec.submit) {
        // 页面已经接管 Enter（preventDefault，例如自己提交过）时不再重复 requestSubmit。
        const notCanceled = el.dispatchEvent(new KeyboardEvent('keydown', {
          key: 'Enter', code: 'Enter', bubbles: true, cancelable: true,
        }));
        const form = el.closest && el.closest('form');
        if (formValidationFailed(form, null)) {
          // HTML5 校验失败时浏览器不会提交；不要伪装成 nav。
          validation = 'failed';
        } else if (notCanceled && form && form.requestSubmit) {
          form.requestSubmit();
          submitted = true;
          effect = 'nav';
          nextUrl = form.action || '';
        }
      }
      // 只返回写入长度；输入内容（尤其是密码）绝不进入工具输出。
      return {
        found: true,
        ok: true,
        info: `wrote=1 chars=${text.length} trusted=0${spec.submit ? ` submit=${submitted ? 1 : 0}` : ''}`,
        effect,
        nextUrl,
        validation,
      };
    }
    case 'select': {
      if (el.tagName !== 'SELECT') {
        return { found: true, ok: false, code: 'not_select', message: 'select action requires a <select> element' };
      }
      const value = spec.value === undefined ? '' : String(spec.value);
      const written = helpers.setValue(el, value);
      if (!written.ok) return { found: true, ...written };
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return { found: true, ok: true, info: `value=${value.slice(0, 40)}` };
    }
    case 'press': {
      const combo = parseKeyCombo(String(spec.key || ''));
      if (!combo.key) return { found: true, ok: false, code: 'missing_key', message: 'press requires key' };
      if (hasLocator) { try { el.focus(); } catch (_) { /* ignore */ } }
      const pressTarget = el || document.activeElement || document.body;
      if (!pressTarget) return { found: true, ok: false, code: 'target_not_found', message: 'no element to receive the key' };
      const opts = {
        key: combo.key,
        code: combo.code,
        bubbles: true,
        cancelable: true,
        ctrlKey: combo.ctrlKey,
        shiftKey: combo.shiftKey,
        altKey: combo.altKey,
        metaKey: combo.metaKey,
      };
      const notCanceled = pressTarget.dispatchEvent(new KeyboardEvent('keydown', opts));
      pressTarget.dispatchEvent(new KeyboardEvent('keyup', opts));
      let effect = 'page';
      let nextUrl = '';
      let validation = '';
      if (combo.key === 'Enter' && notCanceled) {
        const form = pressTarget.closest && pressTarget.closest('form');
        if (formValidationFailed(form, null)) {
          validation = 'failed';
          effect = 'none';
        } else if (form && form.requestSubmit) {
          form.requestSubmit();
          effect = 'nav';
          nextUrl = form.action || '';
        }
      }
      const where = ref ? `ref=${ref}` : selector ? `selector=${selector}` : target ? `target=${target}` : 'target=active';
      return { found: true, ok: true, info: `key=${spec.key} ${where} trusted=0`, effect, nextUrl, validation };
    }
    case 'hover': {
      const rect = el.getBoundingClientRect();
      const x = rect.x + rect.width / 2;
      const y = rect.y + rect.height / 2;
      const opts = { bubbles: true, clientX: x, clientY: y };
      const pointerOpts = { ...opts, pointerId: 1, pointerType: 'mouse', isPrimary: true };
      el.dispatchEvent(new PointerEvent('pointerover', pointerOpts));
      el.dispatchEvent(new PointerEvent('pointerenter', pointerOpts));
      el.dispatchEvent(new PointerEvent('pointermove', pointerOpts));
      el.dispatchEvent(new MouseEvent('mouseover', opts));
      el.dispatchEvent(new MouseEvent('mouseenter', opts));
      el.dispatchEvent(new MouseEvent('mousemove', opts));
      return { found: true, ok: true, info: '' };
    }
    case 'submit': {
      const form = el.closest && el.closest('form');
      if (!form || !form.requestSubmit) {
        return { found: true, ok: false, code: 'no_form', message: 'target is not inside a form', hint: 'use click on the submit button or press Enter on the field' };
      }
      if (formValidationFailed(form, null)) {
        return { found: true, ok: true, info: 'submitted=0 validation=failed', effect: 'none', validation: 'failed' };
      }
      form.requestSubmit();
      return { found: true, ok: true, info: 'submitted=1', effect: 'nav', nextUrl: form.action || '' };
    }
    default:
      return { found: true, ok: false, code: 'invalid_action', message: `unsupported action: ${action}` };
  }
}

/**
 * 在页面内定位并聚焦目标，供 CDP Input.* 可信按键使用。
 * 只做校验/聚焦，不修改值；type 还需要知道控件是否支持 Input.insertText。
 */
function focusInPage(spec) {
  const helpers = window.__ema2_helpers;
  if (!helpers) {
    return { found: false, code: 'helpers_missing', message: 'page helpers are not installed', hint: 'reload the page and retry' };
  }
  const ref = spec.ref || null;
  const selector = spec.selector || null;
  const target = spec.target || null;
  const el = helpers.find(ref, selector, target);
  if (!el) {
    return {
      found: false,
      stale: !!ref && !helpers.refIsCurrent(ref),
      candidates: helpers.candidates(5),
    };
  }
  if (el.disabled) return { found: true, ok: false, code: 'element_disabled', message: 'target is disabled' };
  const activeInfo = (node) => {
    if (!node) return { tag: '', role: '', name: '' };
    return {
      tag: String(node.tagName || '').toLowerCase(),
      role: helpers.roleOf(node),
      name: helpers.nameOf(node).slice(0, 80),
    };
  };
  const before = activeInfo(document.activeElement);
  try { el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); } catch (_) { /* ignore */ }
  let focused = false;
  try {
    el.focus({ preventScroll: true });
    focused = document.activeElement === el;
  } catch (_) {
    try { el.focus(); focused = document.activeElement === el; } catch (_) { /* ignore */ }
  }
  if (!focused && el.tagName !== 'BODY') {
    return {
      found: true,
      ok: false,
      code: 'not_clickable',
      message: 'target cannot receive keyboard focus',
      hint: 'focus a nearby input/button, or use click instead',
    };
  }
  const tag = el.tagName;
  const inputType = tag === 'INPUT' ? String(el.type || 'text').toLowerCase() : '';
  const editable = el.isContentEditable
    || tag === 'TEXTAREA'
    || (tag === 'INPUT' && !['file', 'checkbox', 'radio', 'hidden', 'submit', 'reset', 'button', 'image'].includes(inputType));
  const insertable = el.isContentEditable
    || tag === 'TEXTAREA'
    || (tag === 'INPUT' && ['text', 'search', 'email', 'url', 'tel', 'password'].includes(inputType));
  const form = el.closest && el.closest('form');
  const formInvalid = !!form && !form.noValidate && !el.formNoValidate
    && typeof form.checkValidity === 'function' && !form.checkValidity();
  return {
    found: true,
    ok: true,
    editable,
    insertable,
    readOnly: !!el.readOnly,
    formInvalid,
    nextUrl: form ? (form.action || '') : '',
    before,
    after: activeInfo(document.activeElement),
  };
}

/**
 * 背景页侧的键名解析（页面内的 parseKeyCombo 只负责合成事件）。
 * 与页面侧保持同一套别名；CDP 需要 windowsVirtualKeyCode。
 */
function parseKeyComboBackground(raw) {
  const source = String(raw || '').trim();
  if (source === '+') {
    return { key: '+', code: 'Equal', ctrlKey: false, shiftKey: true, altKey: false, metaKey: false };
  }
  const parts = source.split('+').map(part => part.trim()).filter(Boolean);
  const token = parts.pop() || '';
  const combo = { key: '', code: '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
  for (const part of parts) {
    const lower = part.toLowerCase();
    if (lower === 'ctrl' || lower === 'control') combo.ctrlKey = true;
    else if (lower === 'shift') combo.shiftKey = true;
    else if (lower === 'alt' || lower === 'option') combo.altKey = true;
    else if (lower === 'meta' || lower === 'cmd' || lower === 'command' || lower === 'win') combo.metaKey = true;
  }
  if (!token) return combo;
  const alias = {
    esc: 'Escape', escape: 'Escape', enter: 'Enter', return: 'Enter', tab: 'Tab', space: ' ',
    backspace: 'Backspace', delete: 'Delete', del: 'Delete', insert: 'Insert', home: 'Home',
    end: 'End', pageup: 'PageUp', pagedown: 'PageDown', arrowup: 'ArrowUp', up: 'ArrowUp',
    arrowdown: 'ArrowDown', down: 'ArrowDown', arrowleft: 'ArrowLeft', left: 'ArrowLeft',
    arrowright: 'ArrowRight', right: 'ArrowRight',
  };
  const lower = token.toLowerCase();
  if (alias[lower] !== undefined) {
    combo.key = alias[lower];
    combo.code = alias[lower] === ' ' ? 'Space' : alias[lower];
    return combo;
  }
  if (token.length === 1) {
    combo.key = token;
    combo.code = /[a-z]/i.test(token) ? `Key${token.toUpperCase()}`
      : (/[0-9]/.test(token) ? `Digit${token}` : '');
    return combo;
  }
  combo.key = token;
  combo.code = /^f\d{1,2}$/i.test(token) ? token.toUpperCase() : token;
  return combo;
}

function modifierMask(combo) {
  return (combo.altKey ? 1 : 0) | (combo.ctrlKey ? 2 : 0)
    | (combo.metaKey ? 4 : 0) | (combo.shiftKey ? 8 : 0);
}

function virtualKeyCode(combo) {
  const map = {
    Enter: 13, Tab: 9, Backspace: 8, Delete: 46, Escape: 27, ' ': 32,
    ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
    Home: 36, End: 35, PageUp: 33, PageDown: 34, Insert: 45,
    Shift: 16, Control: 17, Alt: 18, Meta: 91,
  };
  if (map[combo.key] != null) return map[combo.key];
  if (combo.code === 'Equal') return 187;
  if (/^Key[A-Z]$/.test(combo.code)) return combo.code.charCodeAt(3);
  if (/^Digit\d$/.test(combo.code)) return 48 + Number(combo.code.slice(5));
  if (/^F\d{1,2}$/.test(combo.key)) return 111 + Number(combo.key.slice(1));
  const upper = String(combo.key).toUpperCase();
  return upper.length === 1 ? upper.charCodeAt(0) : 0;
}

function commandsFor(combo) {
  if ((!combo.ctrlKey && !combo.metaKey) || combo.altKey) return [];
  const key = String(combo.key).toLowerCase();
  if (key === 'a') return ['selectAll'];
  if (key === 'c') return ['copy'];
  if (key === 'x') return ['cut'];
  if (key === 'v') return ['paste'];
  if (key === 'z') return [combo.shiftKey ? 'redo' : 'undo'];
  return [];
}

async function dispatchTrustedKey(tabId, combo, focusedTag) {
  let key = combo.key;
  if (key.length === 1 && /[a-z]/i.test(key)) key = combo.shiftKey ? key.toUpperCase() : key.toLowerCase();
  const vk = virtualKeyCode(combo);
  const base = {
    key,
    code: combo.code || '',
    modifiers: modifierMask(combo),
    windowsVirtualKeyCode: vk,
    nativeVirtualKeyCode: vk,
  };
  const isEnter = combo.key === 'Enter';
  const isSpace = combo.key === ' ' && /^(BUTTON|A|SUMMARY)$/.test(String(focusedTag || '').toUpperCase());
  const plainText = key.length === 1 && !combo.ctrlKey && !combo.metaKey && !combo.altKey && !isSpace;
  const down = { ...base, type: plainText || isEnter ? 'keyDown' : 'rawKeyDown' };
  if (plainText) { down.text = key; down.unmodifiedText = key; }
  if (isEnter) { down.text = '\r'; down.unmodifiedText = '\r'; }
  const commands = commandsFor(combo);
  if (commands.length) down.commands = commands;
  await chrome.debugger.sendCommand({ tabId }, 'Input.dispatchKeyEvent', down);
  await chrome.debugger.sendCommand({ tabId }, 'Input.dispatchKeyEvent', { ...base, type: 'keyUp' });
  await sleep(35);
}

async function activeElementInfo(tabId) {
  return runInTab(tabId, () => {
    const helpers = window.__ema2_helpers;
    const active = document.activeElement;
    if (!active) return { tag: '', role: '', name: '', formAction: '', invalid: false };
    const form = active.closest && active.closest('form');
    return {
      tag: String(active.tagName || '').toLowerCase(),
      role: helpers ? helpers.roleOf(active) : '',
      name: helpers ? helpers.nameOf(active).slice(0, 80) : '',
      formAction: form ? (form.action || '') : '',
      invalid: !!form && !form.noValidate && typeof form.checkValidity === 'function' && !form.checkValidity(),
    };
  }, [], undefined, 1).catch(() => ({ tag: '', role: '', name: '', formAction: '', invalid: false }));
}

/** 可信按键：CDP Input.dispatchKeyEvent 会触发浏览器默认行为（Tab/Backspace/Enter/Ctrl+A）。 */
async function pressViaDebugger(tab, step) {
  const spec = {
    ref: step.ref || null,
    selector: step.selector || null,
    target: step.target || null,
  };
  const hasLocator = !!(spec.ref || spec.selector || spec.target);
  let prepared;
  if (hasLocator) {
    prepared = await runInTab(tab.id, focusInPage, [spec], undefined, 1);
    if (!prepared) return null;
    if (prepared.found === false) return prepared;
    if (!prepared.ok) return prepared;
  } else {
    const before = await activeElementInfo(tab.id);
    prepared = { ok: true, before, after: null, formInvalid: !!before.invalid, nextUrl: '' };
  }
  const combo = parseKeyComboBackground(String(step.key || ''));
  if (!combo.key) return { found: true, ok: false, code: 'missing_key', message: 'press requires key' };
  try {
    await withDebugger(tab.id, async () => {
      await dispatchTrustedKey(tab.id, combo, prepared.after ? prepared.after.tag : prepared.before.tag);
    }, { timeoutMs: 8000, queueWaitMs: 15000 });
  } catch (error) {
    // debugger 被占用时退回页面合成事件；结果里会标 trusted=0。
    if (error instanceof ToolError && ['debugger_attach_failed', 'debugger_attach_timeout', 'debugger_busy', 'debugger_timeout'].includes(error.code)) {
      return null;
    }
    throw error;
  }
  const after = await activeElementInfo(tab.id);
  const focusChanged = (prepared.after ? prepared.after.tag : prepared.before.tag) !== after.tag
    || (prepared.after ? prepared.after.name : prepared.before.name) !== after.name;
  const validation = (prepared.formInvalid || after.invalid) && combo.key === 'Enter' ? 'failed' : '';
  // 按键可能已经触发导航，after 里的 form 可能已经消失；用按键前的 form action 判定。
  const formAction = prepared.nextUrl || (prepared.before && prepared.before.formAction) || after.formAction || '';
  const effect = combo.key === 'Enter' && ((formAction && !validation) || after.tag === 'a')
    ? 'nav'
    : 'page';
  const where = spec.ref ? `ref=${spec.ref}` : spec.selector ? `selector=${spec.selector}` : spec.target ? `target=${spec.target}` : 'target=active';
  return {
    found: true,
    ok: true,
    info: `key=${step.key} ${where} trusted=1 focus_changed=${focusChanged ? 1 : 0} active=${after.tag || 'none'}`,
    effect,
    nextUrl: effect === 'nav' ? formAction : '',
    validation,
  };
}

/** 可信输入：CDP Input.insertText 在 caret/selection 处插入，不再总是追加到末尾。 */
async function typeViaDebugger(tab, step) {
  const spec = {
    ref: step.ref || null,
    selector: step.selector || null,
    target: step.target || null,
  };
  if (!spec.ref && !spec.selector && !spec.target) return null;
  const prepared = await runInTab(tab.id, focusInPage, [spec], undefined, 1);
  if (!prepared) return null;
  if (prepared.found === false) return prepared;
  if (!prepared.ok) return prepared;
  if (prepared.readOnly) return { found: true, ok: false, code: 'element_readonly', message: 'target is read-only' };
  if (!prepared.editable) return { found: true, ok: false, code: 'not_writable', message: 'target is not an editable control' };
  // number/date/range 等不支持 Input.insertText：交给页面 setValue（同样会校验并回滚）。
  if (!prepared.insertable) return null;
  const text = step.text === undefined ? '' : String(step.text);
  try {
    await withDebugger(tab.id, async () => {
      await chrome.debugger.sendCommand({ tabId: tab.id }, 'Input.insertText', { text });
      if (step.submit) {
        await dispatchTrustedKey(tab.id, parseKeyComboBackground('Enter'), 'INPUT');
      }
    }, {
      timeoutMs: Math.max(8000, Math.min(60000, text.length * 5 + 3000)),
      queueWaitMs: 20000,
    });
  } catch (error) {
    if (error instanceof ToolError && ['debugger_attach_failed', 'debugger_attach_timeout', 'debugger_busy', 'debugger_timeout'].includes(error.code)) {
      return null;
    }
    throw error;
  }
  const after = await activeElementInfo(tab.id);
  const validation = step.submit && after.invalid ? 'failed' : '';
  const formAction = prepared.nextUrl || after.formAction || '';
  const effect = step.submit && formAction && !validation ? 'nav' : 'none';
  return {
    found: true,
    ok: true,
    info: `wrote=1 chars=${text.length} trusted=1${step.submit ? ` submit=${validation ? 0 : 1}` : ''}`,
    effect,
    nextUrl: effect === 'nav' ? formAction : '',
    validation,
  };
}

/** 页面内：安装 console 采集钩子（MAIN world）。 */
function installConsoleHook() {
  if (window.__ema2_console) return true;
  let epoch = '';
  try {
    const bytes = new Uint32Array(2);
    crypto.getRandomValues(bytes);
    epoch = 'c-' + bytes[0].toString(36) + bytes[1].toString(36);
  } catch (_) {
    epoch = 'c-' + Math.random().toString(36).slice(2, 10);
  }
  window.__ema2_console = [];
  window.__ema2_console_seq = 0;
  window.__ema2_console_epoch = epoch;
  const stringify = (value) => {
    const seen = new WeakSet();
    try {
      return JSON.stringify(value, (key, item) => {
        if (typeof item === 'bigint') return `${item}n`;
        if (item instanceof Error) return { name: item.name, message: item.message };
        if (item && typeof item === 'object') {
          if (seen.has(item)) return '[Circular]';
          seen.add(item);
        }
        return item;
      });
    } catch (_) { return '[Unserializable]'; }
  };
  const order = { log: 1, info: 2, warn: 3, error: 4 };
  for (const lvl of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[lvl];
    console[lvl] = (...items) => {
      const seq = ++window.__ema2_console_seq;
      window.__ema2_console.push({
        seq,
        level: lvl,
        order: order[lvl] || 1,
        text: items.map(x => (x && typeof x === 'object' ? stringify(x) : String(x))).join(' ').slice(0, 500),
      });
      if (window.__ema2_console.length > 300) window.__ema2_console.splice(0, window.__ema2_console.length - 300);
      original.apply(console, items);
    };
  }
  return true;
}

async function ensureConsoleHook(tabId) {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      await runInTab(tabId, installConsoleHook, [], 'MAIN');
      const ready = await runInTab(tabId, () => !!window.__ema2_console, [], 'MAIN');
      if (ready) return true;
    } catch (_) { /* page still navigating */ }
    await sleep(150);
  }
  return false;
}

/** 页面内：记录 fetch/XHR 的真实状态码与受限响应预览（MAIN world）。 */
function installNetworkHook() {
  if (window.__ema2_net) return true;
  const makeEpoch = () => {
    let random = '';
    try {
      const bytes = new Uint32Array(2);
      crypto.getRandomValues(bytes);
      random = bytes[0].toString(36) + bytes[1].toString(36);
    } catch (_) {
      random = Math.random().toString(36).slice(2, 10);
    }
    return 'n-' + random;
  };
  const state = {
    seq: 0,
    entries: [],
    epoch: makeEpoch(),
    captureBodies: true,
    dropped: 0,
  };
  window.__ema2_net = state;
  const push = (entry) => {
    entry.seq = ++state.seq;
    state.entries.push(entry);
    if (state.entries.length > 300) {
      const overflow = state.entries.length - 300;
      state.entries.splice(0, overflow);
      state.dropped += overflow;
    }
    return entry;
  };
  // 安装前的请求无法拿到响应体，但 PerformanceResourceTiming.responseStatus
  // （Chromium 109+）能给真实状态码；它的 name 也已经是绝对 URL。
  try {
    const timeOrigin = Number(performance.timeOrigin) || Date.now();
    const nav = performance.getEntriesByType('navigation')[0];
    if (nav) {
      push({
        type: 'document', method: 'GET', url: location.href,
        status: typeof nav.responseStatus === 'number' ? nav.responseStatus : -1,
        ms: Math.round(nav.duration || 0), body: '', preview: 'history', backfilled: true,
        startedAt: Math.round(timeOrigin),
      });
    }
    for (const entry of performance.getEntriesByType('resource').slice(-100)) {
      push({
        type: entry.initiatorType || 'resource', method: 'GET', url: entry.name,
        status: typeof entry.responseStatus === 'number' ? entry.responseStatus : -1,
        ms: Math.round(entry.duration || 0), body: '', preview: 'history', backfilled: true,
        startedAt: Math.round(timeOrigin + (entry.startTime || 0)),
      });
    }
  } catch (_) { /* ignore */ }
  const preview = (text) => String(text == null ? '' : text).slice(0, 800);

  /** 读取响应 clone 的有限前缀；绝不阻塞页面，也绝不无限读完整 body。 */
  const readPreview = async (response) => {
    const contentType = (() => {
      try { return String(response.headers && response.headers.get('content-type') || '').toLowerCase(); }
      catch (_) { return ''; }
    })();
    if (/event-stream|octet-stream|^(?:image|audio|video|font)\//.test(contentType)) {
      return { body: '', preview: 'skipped_binary_or_stream' };
    }
    let clone;
    try { clone = response.clone(); } catch (_) { return { body: '', preview: 'unavailable' }; }
    if (!clone.body || typeof clone.body.getReader !== 'function') {
      try {
        return { body: preview(await clone.text()), preview: 'done' };
      } catch (_) {
        return { body: '', preview: 'error' };
      }
    }
    const reader = clone.body.getReader();
    const decoder = new TextDecoder();
    const deadline = performance.now() + 1500;
    let bytes = 0;
    let text = '';
    try {
      while (bytes < 8192 && performance.now() < deadline) {
        const { done, value } = await reader.read();
        if (done) break;
        if (value) {
          bytes += value.byteLength || value.length || 0;
          text += decoder.decode(value, { stream: true });
        }
        if (bytes >= 8192) break;
      }
    } catch (_) {
      return { body: '', preview: 'error' };
    } finally {
      try { await reader.cancel(); } catch (_) { /* ignore */ }
    }
    return { body: preview(text), preview: bytes >= 8192 ? 'truncated' : 'done' };
  };

  const originalFetch = window.fetch;
  window.fetch = async function (...args) {
    const start = performance.now();
    const startedAt = Date.now();
    const input = args[0];
    const init = args[1] || {};
    let url = '';
    try { url = typeof input === 'string' ? input : (input && input.url) || String(input); } catch (_) { url = ''; }
    const method = String(init.method || (input && input.method) || 'GET').toUpperCase();
    try {
      const response = await originalFetch.apply(this, args);
      const entry = push({
        type: 'fetch', method, url, status: response.status,
        ms: Math.round(performance.now() - start), body: '', preview: 'off', startedAt,
      });
      // 先返回原始 Response；预览在独立任务中读取 clone，失败不影响页面请求。
      if (state.captureBodies) {
        entry.preview = 'pending';
        Promise.resolve()
          .then(() => readPreview(response))
          .then(result => { entry.body = result.body; entry.preview = result.preview; })
          .catch(() => { entry.preview = 'error'; });
      }
      return response;
    } catch (error) {
      push({
        type: 'fetch', method, url, status: 0,
        ms: Math.round(performance.now() - start), body: 'ERR ' + (error && error.message), preview: 'error',
        startedAt,
      });
      throw error;
    }
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this.__ema2 = { method: String(method || 'GET').toUpperCase(), url: String(url || ''), start: 0 };
    return originalOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    const meta = this.__ema2 || {};
    meta.start = performance.now();
    meta.startedAt = Date.now();
    // once: XHR 对象被复用时不会累积 loadend 监听器，避免一个请求产生多条记录。
    this.addEventListener('loadend', () => {
      let body = '';
      let previewState = 'off';
      if (state.captureBodies) {
        try {
          if (!this.responseType || this.responseType === 'text') {
            body = preview(this.responseText);
            previewState = 'done';
          } else {
            previewState = 'skipped_binary_or_stream';
          }
        } catch (_) { previewState = 'error'; }
      }
      push({
        type: 'xhr',
        method: meta.method || 'GET',
        url: this.responseURL || meta.url || '',
        status: this.status || 0,
        ms: Math.round(performance.now() - (meta.start || performance.now())),
        body, preview: previewState, startedAt: meta.startedAt || Date.now(),
      });
    }, { once: true });
    return originalSend.apply(this, args);
  };
  return true;
}

async function ensureNetworkHook(tabId) {
  // 导航过程中注入可能落在旧文档上，装完回读确认，失败重试几次。
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await runInTab(tabId, installNetworkHook, [], 'MAIN');
      const ready = await runInTab(tabId, () => !!window.__ema2_net, [], 'MAIN');
      if (ready) return true;
    } catch (_) { /* page still navigating */ }
    await sleep(300);
  }
  return false;
}

/** 显式开关响应体预览；bodies=false 时不再 clone/读取新响应。 */
async function setNetworkBodyCapture(tabId, enabled) {
  return runInTab(tabId, (value) => {
    if (window.__ema2_net) window.__ema2_net.captureBodies = !!value;
    return !!(window.__ema2_net && window.__ema2_net.captureBodies);
  }, [!!enabled], 'MAIN');
}

/** 网络水位用 {epoch, seq} 表示；不同文档的 seq 不可比较。 */
async function networkMark(tabId) {
  try {
    return await runInTab(tabId, () => {
      const state = window.__ema2_net;
      return state ? { epoch: state.epoch || '', seq: state.seq || 0 } : { epoch: '', seq: 0 };
    }, [], 'MAIN');
  } catch (_) {
    return { epoch: '', seq: 0 };
  }
}

// ---------------------------------------------------------------- act / settle / await

async function loadLastAct() {
  try {
    const store = chrome.storage && chrome.storage.session;
    if (store) {
      const stored = (await store.get(LAST_ACT_KEY)) || {};
      const value = stored[LAST_ACT_KEY];
      if (value && typeof value === 'object'
          && (!value.updatedAt || Date.now() - value.updatedAt <= LAST_ACT_TTL_MS)) {
        lastActContext = value;
        return value;
      }
    }
  } catch (_) { /* storage.session 不可用：用内存兜底 */ }
  if (lastActContext
      && (!lastActContext.updatedAt || Date.now() - lastActContext.updatedAt <= LAST_ACT_TTL_MS)) {
    return lastActContext;
  }
  lastActContext = null;
  return null;
}

async function saveLastAct(context) {
  const value = { ...context, updatedAt: Date.now() };
  lastActContext = value;
  try {
    const store = chrome.storage && chrome.storage.session;
    if (store) await store.set({ [LAST_ACT_KEY]: value });
  } catch (_) { /* 忽略：内存上下文仍然可用 */ }
  return value;
}

async function clearLastAct() {
  lastActContext = null;
  try {
    const store = chrome.storage && chrome.storage.session;
    if (store) await store.remove(LAST_ACT_KEY);
  } catch (_) { /* ignore */ }
}

/**
 * 动作期间新建的标签。
 *
 * openerTabId 明确指向动作 tab 的最可靠；noopener/target=_blank 会丢失 opener，
 * 只能退化为“同窗口 + 新出现 + 当前 active”。后者只用于定位结果，
 * 不会自动登记进 session（避免用户手动开的 tab 被 `tabs close --session` 关掉）。
 */
function newTabsSince(context, tabs) {
  const known = new Set(context.tabIds || []);
  const original = tabs.find(tab => tab.id === context.tabId);
  return tabs
    .filter(tab => !known.has(tab.id) && tab.id !== context.tabId)
    .filter(tab => tab.openerTabId === context.tabId
      || (context.windowId != null && tab.windowId === context.windowId
        && tab.active && original && !original.active))
    .map(tab => ({ tab, safe: tab.openerTabId === context.tabId }));
}

function actionChangedKeys(baseline, state) {
  const changed = [];
  if (baseline.url && state.url && baseline.url !== state.url) changed.push('url');
  // performance.timeOrigin 在 reload / 同 URL 导航后会变，pushState 不会。
  if (baseline.origin && state.origin && baseline.origin !== state.origin) changed.push('reload');
  if (baseline.doc && state.doc && baseline.doc !== state.doc) changed.push('doc');
  if (baseline.title !== state.title) changed.push('title');
  if (baseline.refSeq !== state.refSeq
      || baseline.nodes !== state.nodes
      || baseline.textLen !== state.textLen) changed.push('dom');
  return changed;
}

/**
 * 等待上一个动作的效果。nav 等待导航/新标签；page 只等短暂 DOM 稳定；
 * 提前稳定就提前返回，最多 timeoutMs。
 */
async function waitForActionEffect(context, timeoutMs) {
  const startedAt = Date.now();
  const baseline = context.baseline || {};
  let targetTabId = context.targetTabId || context.tabId;
  let newTabIds = context.newTabId ? [context.newTabId] : [];
  let started = !!context.newTabId;
  let lastSig = '';
  let quietSince = Date.now();
  let lastState = {
    url: context.url || baseline.url || '',
    title: context.title || baseline.title || '',
    origin: baseline.origin || 0,
    doc: '',
  };
  let changed = [];

  while (Date.now() - startedAt < timeoutMs) {
    let tabs = [];
    try { tabs = await chrome.tabs.query({}); } catch (_) { /* ignore */ }
    const created = newTabsSince(context, tabs);
    for (const item of created) {
      const tab = item.tab;
      if (!newTabIds.includes(tab.id)) {
        newTabIds.push(tab.id);
        if (item.safe) await rememberSessionTab(tab.id);
      }
    }
    if (newTabIds.length) {
      if (!newTabIds.includes(targetTabId)) targetTabId = newTabIds[0];
      started = true;
    }

    let tabInfo = null;
    try { tabInfo = await chrome.tabs.get(targetTabId); } catch (_) {
      return {
        settled: false, closed: true, elapsedMs: Date.now() - startedAt,
        targetTabId, newTabIds, url: lastState.url, title: lastState.title, changed,
      };
    }
    let state;
    try { state = await pageState(targetTabId); } catch (_) {
      state = { url: '', title: '', origin: 0, refSeq: 0, doc: '', nodes: 0, textLen: 0 };
    }
    const url = state.url || tabInfo.url || '';
    const title = state.title || tabInfo.title || '';
    lastState = { ...state, url, title };
    const sig = [
      tabInfo.status || '',
      state.url || '',
      state.title || '',
      state.origin || 0,
      state.doc || '',
      state.refSeq || 0,
      state.nodes || 0,
      state.textLen || 0,
    ].join('|');
    if (sig !== lastSig) {
      lastSig = sig;
      quietSince = Date.now();
    }

    const keys = actionChangedKeys(baseline, { ...state, url, title });
    for (const key of keys) if (!changed.includes(key)) changed.push(key);
    if (newTabIds.length && !changed.includes('new_tab')) changed.push('new_tab');
    if (tabInfo.status === 'loading') started = true;
    if (baseline.url && url && url !== baseline.url) started = true;
    if (baseline.origin && state.origin && state.origin !== baseline.origin) started = true;
    if (baseline.doc && state.doc && state.doc !== baseline.doc) started = true;

    const elapsed = Date.now() - startedAt;
    const quietFor = Date.now() - quietSince;
    const complete = tabInfo.status === 'complete' || tabInfo.status === undefined;

    if (context.expect === 'nav') {
      const navEvidence = changed.includes('url') || changed.includes('reload') || changed.includes('doc')
        || newTabIds.length > 0 || tabInfo.status === 'loading';
      const pageActivity = changed.length > 0;
      if (complete && url) {
        if (navEvidence && started && quietFor >= 250) {
          return {
            settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
            url, title, changed,
          };
        }
        // SPA 链接可能只改 DOM/标题、不导航：1s 内没有新动静就按“无导航”结算，
        // 交给 act 输出 nav=none，而不是一直 pending 到 3s 并谎报“still navigating”。
        if (pageActivity && quietFor >= 600 && elapsed >= 1000) {
          return {
            settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
            url, title, changed,
          };
        }
        // 完全没有动静的链接/按钮：给一次重试机会（只有 canRetry 的链接会重试）。
        if (!pageActivity && elapsed >= SETTLE_NAV_IDLE_MS) {
          return {
            settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
            url, title, changed,
          };
        }
      }
    } else if (context.expect === 'page') {
      if (changed.length && quietFor >= 250) {
        return {
          settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
          url, title, changed,
        };
      }
      // 轮询是 120ms 一步；阈值留出一步，避免刚好在 timeout 边界退出成 pending。
      if (!changed.length && elapsed >= SETTLE_PAGE_MS - 120) {
        return {
          settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
          url, title, changed,
        };
      }
    } else {
      return {
        settled: true, elapsedMs: elapsed, targetTabId, newTabIds,
        url, title, changed,
      };
    }
    await sleep(120);
  }

  return {
    settled: false, elapsedMs: Date.now() - startedAt, targetTabId, newTabIds,
    url: lastState.url, title: lastState.title, changed,
  };
}

async function settleAfterAction(tab, step, outcome, beforeState, beforeTabs) {
  // download 不是页面内变化，但也不该套用“导航失败”的 nav=none 语义。
  const expect = outcome.effect === 'download' ? 'page'
    : outcome.effect === 'nav' || outcome.effect === 'page' ? outcome.effect : 'none';
  const context = {
    tabId: tab.id,
    action: step.action,
    expect,
    startedAt: Date.now(),
    settled: true,
    targetTabId: tab.id,
    newTabId: null,
    nextUrl: outcome.nextUrl || '',
    url: beforeState.url || '',
    title: beforeState.title || '',
    baseline: {
      url: beforeState.url || '',
      title: beforeState.title || '',
      origin: beforeState.origin || 0,
      doc: beforeState.doc || '',
      refSeq: beforeState.refSeq || 0,
      nodes: beforeState.nodes || 0,
      textLen: beforeState.textLen || 0,
    },
    tabIds: beforeTabs.map(item => item.id),
    windowId: (beforeTabs.find(item => item.id === tab.id) || {}).windowId,
  };

  if (expect === 'none') {
    await sleep(SETTLE_NONE_MS);
    await saveLastAct(context);
    return {
      context,
      settle: {
        settled: true, expect, elapsedMs: SETTLE_NONE_MS,
        targetTabId: tab.id, newTabIds: [], url: context.url, title: context.title, changed: [],
      },
    };
  }

  const timeoutMs = expect === 'nav' ? SETTLE_NAV_MS : SETTLE_PAGE_MS;
  const result = await waitForActionEffect(context, timeoutMs);
  context.settled = result.settled;
  context.targetTabId = result.targetTabId || tab.id;
  context.newTabId = (result.newTabIds || [])[0] || null;
  context.url = result.url || context.url;
  context.title = result.title || context.title;
  await saveLastAct(context);
  return { context, settle: { ...result, expect } };
}

async function awaitPendingAction(args) {
  const timeoutSeconds = args.timeout === undefined ? 3 : Number(args.timeout);
  if (!Number.isFinite(timeoutSeconds) || timeoutSeconds < 0 || timeoutSeconds > WAIT_MAX_SECONDS) {
    fail('invalid_argument', `await timeout must be between 0 and ${WAIT_MAX_SECONDS} seconds`, 'fix await duration');
  }
  const context = await loadLastAct();
  if (!context) {
    fail('no_pending_action', 'no recent act to await',
      'call `ema2 act ...` first, or use `ema2 wait text|selector|time|idle`');
  }
  if (args.tabId !== undefined
      && args.tabId !== context.tabId
      && args.tabId !== context.targetTabId) {
    fail('no_pending_action', `the last act belongs to tab ${context.tabId}`,
      `use --tab ${context.tabId}, or run act on tab ${args.tabId}`);
  }
  if (context.settled) {
    return `await=done elapsed=0.0s tab=${context.targetTabId || context.tabId}${context.url ? ` url=${context.url}` : ''}`;
  }

  const start = Date.now();
  const result = await waitForActionEffect(context, Math.round(timeoutSeconds * 1000));
  context.settled = result.settled;
  context.targetTabId = result.targetTabId || context.targetTabId || context.tabId;
  context.newTabId = (result.newTabIds || [])[0] || context.newTabId || null;
  context.url = result.url || context.url;
  context.title = result.title || context.title;
  await saveLastAct(context);

  if (result.closed) {
    fail('tab_not_found', `tab ${context.targetTabId} was closed while waiting`,
      'inspect tabs with `ema2 tabs list`');
  }
  if (!result.settled) {
    fail('wait_timeout',
      `await timeout after ${timeoutSeconds}s (expect=${context.expect})`,
      `call \`ema2 await ${Math.max(1, Math.round(timeoutSeconds))}s\` again, or inspect with \`ema2 read --tab ${context.targetTabId || context.tabId}\``);
  }
  const elapsed = ((Date.now() - start) / 1000).toFixed(1);
  const bits = [
    `await=done elapsed=${elapsed}s`,
    `tab=${context.targetTabId || context.tabId}`,
  ];
  if (context.url) bits.push(`url=${context.url}`);
  if (context.newTabId) bits.push(`new_tab=${context.newTabId}`);
  return bits.join(' ');
}

async function actTool(args) {
  const tab = await resolveTab(args);
  const batch = Array.isArray(args.steps) && args.steps.length > 0;
  const steps = batch ? args.steps : [args];
  if (steps.length > 20) fail('too_many_steps', 'at most 20 steps', 'split the batch');
  await clearLastAct();
  try { await ensureHelpers(tab.id); } catch (_) { /* action will report a clearer error */ }
  const before = await pageState(tab.id);
  // 动作前装 hook，并记录网络水位 {document epoch, seq}；不同文档的 seq 不可比较。
  try { await ensureNetworkHook(tab.id); } catch (_) { /* non-http page */ }
  const netBaseline = await networkMark(tab.id);
  let lastActionNetBaseline = netBaseline;
  try { await ensureConsoleHook(tab.id); } catch (_) { /* ignore */ }
  // page_guard 已由 withCommandGuard/guardTargetForTool 在动作前安装；
  // 这里不能重复注入，否则每个 act 都会多一次 all-frames executeScript。
  const guardSeq = await guardMark(tab.id);
  const results = [];
  const newTabIds = [];
  let lastSettle = null;
  let awaitedTargetTabId = null;
  let stoppedAt = null;
  let okCount = 0;
  let downloadStarted = false;
  let validationFailed = false;

  for (let index = 0; index < steps.length; index++) {
    const step = steps[index];
    if (!step || !step.action) fail('invalid_step', `steps[${index}] has no action`, 'each step needs action');

    if (step.action === 'await') {
      try {
        const text = await awaitPendingAction({ timeout: step.timeout, tabId: tab.id });
        results.push(`step${index + 1} ${text}`);
        okCount++;
        // await 可能发现了 settle 阶段还没出现的新标签；后续 waitFor 必须用它。
        const awaited = await loadLastAct();
        if (awaited && awaited.targetTabId) awaitedTargetTabId = awaited.targetTabId;
        if (awaited && awaited.newTabId && !newTabIds.includes(awaited.newTabId)) {
          newTabIds.push(awaited.newTabId);
        }
        if (lastSettle && awaited) {
          lastSettle = {
            ...lastSettle,
            settled: true,
            targetTabId: awaited.targetTabId || lastSettle.targetTabId,
            url: awaited.url || lastSettle.url,
            title: awaited.title || lastSettle.title,
          };
          if (awaited.newTabId) {
            const ids = new Set([...(lastSettle.newTabIds || []), awaited.newTabId]);
            lastSettle.newTabIds = [...ids];
          }
        }
        await sleep(80);
      } catch (error) {
        if (error instanceof ToolError) {
          if (batch) error.message = `steps[${index + 1}] await: ${error.message}`;
          error.lines = [...(error.lines || []), ...results];
        }
        throw error;
      }
      continue;
    }

    if (step.action === 'waitFor') {
      try {
        const waitTabId = (lastSettle && lastSettle.targetTabId) || awaitedTargetTabId || tab.id;
        const text = await waitForResponse(waitTabId, step, lastActionNetBaseline);
        results.push(`step${index + 1} ${text}`);
        okCount++;
        await sleep(60);
      } catch (error) {
        if (error instanceof ToolError) {
          if (batch) error.message = `steps[${index + 1}] waitFor: ${error.message}`;
          error.lines = [...(error.lines || []), ...results];
        }
        throw error;
      }
      continue;
    }

    const stepBefore = await pageState(tab.id);
    let tabsBefore = [];
    try { tabsBefore = await chrome.tabs.query({}); } catch (_) { /* ignore */ }
    // waitFor 要匹配“上一步动作”产生的响应/导航，baseline 必须在该动作之前取。
    lastActionNetBaseline = await networkMark(tab.id);
    lastActionNetBaseline.pageUrl = stepBefore.url || '';
    lastActionNetBaseline.pageOrigin = stepBefore.origin || 0;
    let outcome;
    try {
      await ensureHelpers(tab.id);
      if (step.selector) await requireValidSelector(tab.id, step.selector);
      outcome = await runStepWithRetry(tab, step);
    } catch (error) {
      await clearLastAct();
      if (error instanceof ToolError) {
        if (batch) error.message = `steps[${index + 1}] ${step.action}: ${error.message}`;
        error.lines = [...(error.lines || []), ...results];
      }
      throw error;
    }

    let settled = await settleAfterAction(tab, step, outcome, stepBefore, tabsBefore);
    // 链接点了但没导航：只对可导航的 <a> 安全重试一次；
    // DOM/title 已经有变化说明动作生效，绝不重试（避免 SPA/JS 处理器执行两次）。
    if (outcome.canRetry && outcome.effect === 'nav' && settled.settle.settled) {
      const firstChanged = settled.settle.changed || [];
      const navMissing = !firstChanged.includes('url')
        && !(settled.settle.newTabIds || []).length;
      if (navMissing && firstChanged.length === 0) {
        await sleep(250);
        const retryBefore = await pageState(tab.id);
        let retryTabs = [];
        try { retryTabs = await chrome.tabs.query({}); } catch (_) { /* ignore */ }
        const retryOutcome = await runStepWithRetry(tab, step);
        const retrySettled = await settleAfterAction(tab, step, retryOutcome, retryBefore, retryTabs);
        if (!retrySettled.settle.settled
            || retrySettled.settle.changed.length
            || (retrySettled.settle.newTabIds || []).length) {
          outcome = retryOutcome;
          settled = retrySettled;
        }
        results.push('retried=1 reason=anchor_did_not_navigate');
      }
    }
    const settle = settled.settle;
    settle.nextUrl = outcome.nextUrl || '';
    settle.action = step.action;
    if (outcome.download) downloadStarted = true;
    if (outcome.validation === 'failed') validationFailed = true;
    lastSettle = settle;
    for (const id of settle.newTabIds || []) {
      if (!newTabIds.includes(id)) newTabIds.push(id);
    }
    const pending = !settle.settled;
    results.push(`step${index + 1} ok=${step.action}${outcome.info ? ` ${outcome.info}` : ''}${pending ? ' settle=pending' : ''}`);
    okCount++;

    if (pending && batch) {
      const next = steps[index + 1];
      if (!next || next.action !== 'await') {
        stoppedAt = index + 1;
        break;
      }
    }
    if (!pending) await sleep(60);
  }

  const summaryTabId = (lastSettle && lastSettle.targetTabId) || awaitedTargetTabId || tab.id;
  let waitInfo = '';
  if (args.waitFor) {
    try {
      waitInfo = await waitForResponse(summaryTabId, args.waitFor, lastActionNetBaseline);
    } catch (error) {
      if (error instanceof ToolError) {
        // 动作已经执行了；把已完成的步骤带在错误下面，避免 AI 以为整次调用都没发生。
        error.lines = [...(error.lines || []), ...results];
      }
      throw error;
    }
  }
  try { await ensureNetworkHook(summaryTabId); } catch (_) { /* non-http page */ }
  // 导航后的新文档没有 console hook；补装后至少后续错误能被 observe 到。
  try { await ensureConsoleHook(summaryTabId); } catch (_) { /* ignore */ }

  const after = await pageState(summaryTabId);
  let afterTab = null;
  try { afterTab = await chrome.tabs.get(summaryTabId); } catch (_) { /* tab closed */ }
  const url = after.url || (afterTab && afterTab.url) || '';
  const title = after.title || (afterTab && afterTab.title) || '';
  const summaryUrl = (afterTab && afterTab.url) || after.url || '';
  const changed = [];
  if (before.url !== summaryUrl) changed.push('url');
  if (before.title !== title) changed.push('title');
  // reload / 同 URL 导航：URL/title 可能完全一样，但 timeOrigin 变了。
  if (before.origin && after.origin && before.origin !== after.origin) changed.push('reload');
  if (before.doc && after.doc && before.doc !== after.doc) changed.push('doc');
  if (newTabIds.length) changed.push('new_tab');
  // settle 阶段观察到的 DOM/title 变化也要保留：SPA 链接不导航但有实际效果。
  if (lastSettle && Array.isArray(lastSettle.changed)) {
    for (const key of lastSettle.changed) {
      if (!changed.includes(key)) changed.push(key);
    }
  }

  // 这次动作产生的对话框/弹窗记录（来自页面主世界的 page_guard）。
  const guardEvents = [];
  const guardSeen = new Set();
  const guardSources = [[tab.id, guardSeq]];
  if (summaryTabId !== tab.id) guardSources.push([summaryTabId, 0]);
  for (const [tabIdValue, since] of guardSources) {
    for (const event of await guardSince(tabIdValue, since)) {
      const key = `${event.kind}|${event.dialog || ''}|${event.url || ''}|${event.at || 0}`;
      if (guardSeen.has(key)) continue;
      guardSeen.add(key);
      guardEvents.push(event);
    }
  }
  const dialogs = guardEvents.filter(event => event.kind === 'dialog');
  const popups = guardEvents.filter(event => event.kind === 'popup');
  const blockedPopups = popups.filter(event => event.blocked);

  const lines = [...results, `ok=${okCount}/${steps.length}`];
  if (stoppedAt !== null) lines.push(`stopped=${stoppedAt} reason=pending`);
  if (lastSettle) {
    lines.push(`settle=${lastSettle.settled ? 'done' : 'pending'} ms=${lastSettle.elapsedMs || 0} expect=${lastSettle.expect}`);
    if (lastSettle.targetTabId && lastSettle.targetTabId !== tab.id) lines.push(`tab=${lastSettle.targetTabId}`);
    if (newTabIds.length) lines.push(`new_tab=${newTabIds[0]}${newTabIds.length > 1 ? ` new_tabs=${newTabIds.join(',')}` : ''}`);
    if (!lastSettle.settled) {
      if (lastSettle.nextUrl) lines.push(`next_url=${lastSettle.nextUrl}`);
      lines.push(stoppedAt !== null
        ? `hint=stopped at step${stoppedAt}; add \`await 10s\` after it, or call \`ema2 await 10s\``
        : 'hint=page still navigating; call `ema2 await 10s` to keep waiting');
    } else if (lastSettle.expect === 'nav'
        && !changed.includes('url') && !changed.includes('reload') && !changed.includes('doc')
        && !changed.includes('new_tab') && !newTabIds.length) {
      // 点了导航意图但 URL / document 身份 / 新标签都没变：动作执行了但没生效。
      lines.push('nav=none');
      const action = lastSettle.action || '';
      if (action === 'back' || action === 'forward') {
        lines.push(`hint=no history entry for ${action}; the page did not change`);
      } else if (validationFailed) {
        lines.push('hint=HTML5 validation blocked the submit; fill the required fields first');
      } else if (dialogs.length) {
        lines.push('hint=a dialog was opened and auto-dismissed; the page may require confirmation');
      } else if (blockedPopups.length) {
        lines.push('hint=the click tried to open a popup; see popup_blocked above');
      } else if (changed.includes('dom') || changed.includes('title')) {
        lines.push('hint=no navigation, but the page DOM/title changed; the action had an effect, do not retry it blindly');
      } else {
        lines.push('hint=the action ran but produced no navigation; snapshot/read before retrying, and do not blindly repeat a side-effecting action');
      }
    }
  }
  for (const event of dialogs.slice(0, 3)) {
    lines.push(`dialog_opened=${event.dialog || 'dialog'} handled=${event.handled || 'dismissed'}`
      + (event.message ? ` msg=${oneLine(event.message).slice(0, 120)}` : ''));
  }
  if (dialogs.length) {
    const onlyBeforeUnload = dialogs.every(event => event.dialog === 'beforeunload');
    lines.push(onlyBeforeUnload
      ? 'hint=beforeunload handler was suppressed; navigation will not show a browser confirmation'
      : 'hint=dialog was auto-dismissed so the command could return; if the page needed confirmation, re-run only after checking the effect');
  }
  if (blockedPopups.length) {
    lines.push(`popup_blocked=1 url=${shortText(blockedPopups[0].url || '', 120)}`);
    lines.push('hint=window.open was blocked because the click was synthetic; open the popup URL directly with `ema2 open` instead of retrying the click');
  } else if (popups.length) {
    lines.push(`popup=1${popups[0].url ? ` url=${shortText(popups[0].url, 120)}` : ''}`);
  }
  if (downloadStarted) {
    lines.push('download=started');
    lines.push('hint=a download was started; check the browser Downloads folder (EMA2 cannot change the download directory)');
  }
  if (validationFailed) {
    lines.push('validation=failed');
    lines.push('hint=HTML5 form validation failed; fill the required fields or use a different submit path');
  }
  if (url) lines.push(`url=${url}`);
  if (title) lines.push(`title=${oneLine(title)}`);
  if (waitInfo) lines.push(waitInfo);
  if (changed.length) lines.push(`changed=${changed.join(',')}`);
  if (summaryTabId === tab.id) {
    const newRefs = await newRefsSince(tab.id, before.refSeq, before.doc);
    if (newRefs.length) lines.push('new=' + newRefs.join(' | '));
  }
  const consoleErrors = await consoleErrorCount(summaryTabId);
  if (consoleErrors > 0) lines.push(`console_errors=${consoleErrors}`);
  return lines.join('\n');
}


/** 等某个响应或页面导航出现（url 子串 + 可选状态码），命中返回一行摘要。 */
async function waitForResponse(tabId, spec, baselineOverride) {
  await ensureNetworkHook(tabId);
  const wantUrl = spec.url ? String(spec.url).toLowerCase() : '';
  const wantStatus = typeof spec.status === 'number' ? spec.status : null;
  const timeoutMs = Math.max(0, Math.min(300000, Math.round((typeof spec.timeout === 'number' ? spec.timeout : 10) * 1000)));
  const beforeUrl = baselineOverride && typeof baselineOverride === 'object' ? String(baselineOverride.pageUrl || '') : '';
  const beforeOrigin = baselineOverride && typeof baselineOverride === 'object' ? Number(baselineOverride.pageOrigin) || 0 : 0;
  let pageUrl = beforeUrl;
  if (!pageUrl) {
    try { pageUrl = (await chrome.tabs.get(tabId)).url || ''; } catch (_) { pageUrl = ''; }
  }
  const absoluteUrl = (value) => {
    try { return new URL(String(value || ''), pageUrl).href; } catch (_) { return String(value || ''); }
  };
  const current = await networkMark(tabId);
  let since = current.seq;
  let reset = false;
  if (baselineOverride && typeof baselineOverride === 'object') {
    if (baselineOverride.epoch === current.epoch) {
      since = Math.min(baselineOverride.seq || 0, current.seq);
    } else {
      // 导航后 hook 重建、seq 从 0 开始：从新文档起点匹配，而不是跨序列取 min。
      since = 0;
      reset = true;
    }
  } else if (typeof baselineOverride === 'number') {
    since = Math.min(baselineOverride, current.seq);
  }
  const deadline = Date.now() + timeoutMs;
  let uncertain = false;
  while (Date.now() < deadline) {
    try {
      const currentTab = await chrome.tabs.get(tabId);
      if (currentTab && currentTab.url) pageUrl = currentTab.url;
    } catch (_) { /* navigating */ }
    // 页面导航本身也可以是 waitFor 的目标（慢导航不一定产生网络条目，
    // 同 URL 表单提交甚至不会改 URL；用 timeOrigin 识别 reload）。
    if (!wantStatus && wantUrl) {
      try {
        const tabInfo = await chrome.tabs.get(tabId);
        const state = await pageState(tabId);
        const href = state.url || tabInfo.url || '';
        const navigated = (beforeUrl && href && href !== beforeUrl)
          || (beforeOrigin && state.origin && state.origin !== beforeOrigin);
        if (navigated && href && !isInternalUrl(href) && href.toLowerCase().includes(wantUrl)) {
          return `nav=${href}`;
        }
      } catch (_) { /* tab may be navigating */ }
    }
    // 导航/重定向期间 executeScript 可能瞬时失败；waitFor 应该继续等，而不是把整次 act 判失败。
    let scan = null;
    try {
      scan = await runInTab(tabId, (sinceSeq, url, status) => {
        const state = window.__ema2_net;
        if (!state) return { hit: null, uncertain: false };
        let gap = false;
        for (const entry of state.entries) {
          if (entry.seq <= sinceSeq) continue;
          const urlMatch = !url || String(entry.url).toLowerCase().includes(url);
          // status=-1 是 hook 安装前的 performance 回填：只能说明发生过，不能证明本次动作的响应。
          if (entry.status < 0) {
            if (urlMatch) gap = true;
            continue;
          }
          if (!urlMatch) continue;
          if (status != null && entry.status !== status) continue;
          return {
            hit: { method: entry.method, status: entry.status, url: entry.url, ms: entry.ms, seq: entry.seq },
            uncertain: gap,
          };
        }
        return { hit: null, uncertain: gap };
      }, [since, wantUrl, wantStatus], 'MAIN');
    } catch (_) {
      // 导航中可重试；tab 被关掉则立即失败，不要空等到 timeout。
      try { await chrome.tabs.get(tabId); }
      catch (_) { fail('tab_not_found', `tab ${tabId} was closed while waiting for a response`, 'inspect tabs with `ema2 tabs list`'); }
    }
    if (scan && scan.uncertain) uncertain = true;
    if (scan && scan.hit) {
      return `response=${scan.hit.status} ${scan.hit.method} ${absoluteUrl(scan.hit.url)} ${scan.hit.ms}ms${reset ? ' doc=reset' : ''}`;
    }
    await sleep(120);
  }
  fail('response_timeout',
    `no response or navigation matching ${wantUrl || '*'} ${wantStatus == null ? '' : wantStatus} within ${timeoutMs / 1000}s${uncertain ? ' observation_gap=status_unavailable' : ''}`,
    uncertain
      ? 'the request happened before the hook was installed; use observe network or repeat the action after open/act installed the hook'
      : 'pass timeout=Ns inside --wait-for (max 30s), or call observe network to inspect requests');
}

async function runStepWithRetry(tab, step) {
  let attempt = 0;
  for (;;) {
    try {
      return await runStep(tab, step);
    } catch (error) {
      // SPA 还没渲染完：给 target/selector 最多 3 次机会（约 1s）；ref 过期重试没用。
      const recoverable = error instanceof ToolError
        && error.code === 'target_not_found'
        && (step.target || step.selector)
        && attempt < 2;
      if (!recoverable) throw error;
      attempt++;
      await sleep(350);
      await ensureHelpers(tab.id);
    }
  }
}

async function pageState(tabId) {
  try {
    return await runInTab(tabId, () => ({
      url: location.href,
      title: (document.title || '').slice(0, 120),
      // origin 是 navigation/reload 的硬证据：同 URL reload 时 doc 句柄可能暂时为空，
      // 但 performance.timeOrigin 一定换新。pushState 不会改它。
      origin: Number(performance.timeOrigin) || 0,
      refSeq: window.__ema2_ref_seq || 0,
      doc: window.__ema2_helpers ? window.__ema2_helpers.docEpoch() : '',
      nodes: document.getElementsByTagName('*').length,
      textLen: document.body ? (document.body.textContent || '').length : 0,
    }), []);
  } catch (_) {
    return { url: '', title: '', origin: 0, refSeq: 0, doc: '', nodes: 0, textLen: 0 };
  }
}

async function newRefsSince(tabId, seq, epoch) {
  try {
    return await runInTab(tabId, (since, beforeEpoch) => {
      const helpers = window.__ema2_helpers;
      if (helpers && beforeEpoch && helpers.docEpoch() !== beforeEpoch) return [];
      // ref 现在是页面内存表，不再写 DOM 属性；旧页面遗留属性作为兜底。
      const entries = helpers && helpers.refEntries
        ? helpers.refEntries(since)
        : [...document.querySelectorAll('[data-ema2-ref]')].map((el) => ({
          ref: el.getAttribute('data-ema2-ref'), el,
        }));
      const out = [];
      for (const { ref, el } of entries) {
        const parsed = helpers ? helpers.parseRef(ref) : null;
        const n = parsed ? parsed.seq : Number(String(ref).slice(1));
        if (!Number.isFinite(n) || n <= since) continue;
        const name = helpers ? helpers.nameOf(el) : (el.innerText || el.getAttribute('aria-label') || '');
        out.push('[' + ref + '] ' + el.tagName.toLowerCase() + ' "' + String(name).replace(/\s+/g, ' ').trim().slice(0, 50) + '"');
      }
      return out.slice(0, 12);
    }, [seq, epoch || null]);
  } catch (_) {
    return [];
  }
}

async function consoleErrorCount(tabId) {
  try {
    return await runInTab(tabId, () => {
      if (!window.__ema2_console) return 0;
      return window.__ema2_console.filter(e => (e.order || 0) >= 4).length;
    }, [], 'MAIN');
  } catch (_) { return 0; }
}

function mapActionResult(step, result) {
  if (!result) fail('action_failed', `${step.action} produced no result`, 'check the page state and retry');
  if (result.found === false) {
    if (result.code === 'helpers_missing') {
      fail('helpers_missing', result.message || 'page helpers are not installed',
        result.hint || 'reload the page and retry');
    }
    if (result.stale) fail('stale_ref', `ref ${step.ref} is stale (page/document changed)`, 'call snapshot again');
    if (step.ref) {
      fail('ref_not_found', `ref ${step.ref} is not in the current page`, 'refs are document-scoped; call snapshot again', result.candidates);
    }
    fail('target_not_found', `target not found: ${step.ref || step.selector || step.target || '(none)'}`, 'use snapshot refs, or target="role:name"', result.candidates);
  }
  if (!result.ok) {
    const details = (result.fields || [])
      .filter(item => !item.ok)
      .map(item => `${item.target}: ${item.code || 'failed'}${item.message ? ' - ' + item.message : ''}`);
    fail(result.code || 'action_failed', result.message || `${step.action} failed`,
      result.hint || 'inspect the page and retry with a fresh snapshot', details);
  }
  return {
    info: result.info || '',
    effect: result.effect || 'none',
    nextUrl: result.nextUrl || '',
    kind: result.kind || '',
    canRetry: !!result.canRetry,
    validation: result.validation || '',
    download: !!result.download,
  };
}

async function runStep(tab, step) {
  const action = step.action;
  if (action === 'back' || action === 'forward' || action === 'reload') {
    // 有副作用的调用只执行一次：executeScript 在导航瞬间失败时重试会 back/reload 两次。
    await runInTab(tab.id, (kind) => {
      if (kind === 'back') history.back();
      else if (kind === 'forward') history.forward();
      else location.reload();
      return true;
    }, [action], undefined, 1);
    return { info: '', effect: 'nav', nextUrl: '' };
  }
  if (action === 'scroll') {
    const pixels = typeof step.deltaY === 'number' ? step.deltaY : null;
    let pages = null;
    if (pixels === null) {
      const match = /^(up|down)(?:\s+(\d+))?$/i.exec(String(step.text || '').trim());
      const direction = match && match[1].toLowerCase() === 'up' ? -1 : 1;
      const count = match ? Math.max(1, Number(match[2] || 1)) : 1;
      pages = direction * count;
    }
    const info = await runInTab(tab.id, (deltaPixels, pageCount) => {
      const before = window.scrollY;
      const dy = typeof deltaPixels === 'number'
        ? deltaPixels
        : Math.round(pageCount * window.innerHeight * 0.8);
      if (dy !== 0) window.scrollBy(0, dy);
      return 'scroll=' + before + '->' + window.scrollY;
    }, [pixels, pages], undefined, 1);
    return { info: info || '', effect: 'none', nextUrl: '' };
  }

  await ensureHelpers(tab.id);
  const spec = {
    action,
    ref: step.ref || null,
    selector: step.selector || null,
    target: step.target || null,
    text: step.text === undefined ? '' : String(step.text),
    value: step.value === undefined ? '' : String(step.value),
    key: String(step.key || ''),
    submit: !!step.submit,
  };
  if (action === 'fill') {
    const fields = step.fields || [];
    if (!Array.isArray(fields) || !fields.length) fail('missing_fields', 'fill requires fields[]', 'pass [{ref|selector,value}]');
    // 写值不是幂等操作：重试会让 input/change 触发两次。
    const result = await runInTab(tab.id, performActionInPage, [{ action: 'fill', fields }], undefined, 1);
    return mapActionResult(step, result);
  }
  // press/type 优先走 CDP 可信输入：Tab/Backspace/Enter/Ctrl+A 和 caret 插入
  // 只有真实 Input.* 事件才具备浏览器默认行为。debugger 被占用时回退到合成事件。
  if (action === 'press') {
    const trusted = await pressViaDebugger(tab, step);
    if (trusted) return mapActionResult(step, trusted);
  }
  if (action === 'type') {
    const trusted = await typeViaDebugger(tab, step);
    if (trusted) return mapActionResult(step, trusted);
  }
  // 查找与执行在同一次页面注入内完成：目标被动态移除时不会假成功。
  // attempts=1：type 会改值，executeScript 重试会把同一段文字输入两次。
  const result = await runInTab(tab.id, performActionInPage, [spec], undefined, 1);
  return mapActionResult(step, result);
}

/** 在 MAIN world 里按 ref / selector / "role:name" 深度查找（含 shadow root / 同源 iframe）。 */
function deepFindForDebugger(ref, selector, target) {
  // 新版 helper 的 ref 存在页面内存表里；优先走它能同时处理 shadow DOM/iframe。
  const helpers = window.__ema2_helpers;
  if (helpers && typeof helpers.find === 'function') {
    const found = helpers.find(ref || null, selector || null, target || null);
    if (found) return found;
  }
  const clean = (value) => String(value == null ? '' : value).replace(/\s+/g, ' ').trim();
  const roleOf = (el) => {
    const role = el.getAttribute('role'); if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'summary') return 'summary';
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio') return t;
      if (['submit', 'button', 'reset', 'image'].includes(t)) return 'button';
      if (t === 'file') return 'file';
      if (t === 'search') return 'searchbox';
      return 'textbox';
    }
    if (el.isContentEditable) return 'textbox';
    return 'element';
  };
  const nameOf = (el) => {
    const tag = el.tagName;
    const type = tag === 'INPUT' ? String(el.type || 'text').toLowerCase() : '';
    const safeValue = tag === 'INPUT' && ['button', 'submit', 'reset'].includes(type) ? el.value : '';
    const userValueControl = tag === 'TEXTAREA' || el.isContentEditable === true
      || (tag === 'INPUT' && !['button', 'submit', 'reset', 'image'].includes(type));
    return clean(el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.getAttribute('alt') || el.getAttribute('title') || safeValue || (userValueControl ? '' : el.innerText) || '').slice(0, 60);
  };
  const visible = (el) => {
    if (!el || el.hidden) return false;
    try {
      if (typeof el.checkVisibility === 'function') {
        return el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true });
      }
      if (!el.getClientRects().length) return false;
      const view = (el.ownerDocument && el.ownerDocument.defaultView) || window;
      const style = view.getComputedStyle(el);
      return style.visibility !== 'hidden' && style.display !== 'none';
    } catch (_) {
      return !!el.getClientRects().length;
    }
  };
  const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role],[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';
  const roots = [];
  const seen = new Set();
  (function visit(root) {
    if (!root || seen.has(root)) return;
    seen.add(root);
    roots.push(root);
    for (const el of root.querySelectorAll('*')) {
      if (el.shadowRoot) visit(el.shadowRoot);
    }
    for (const frame of root.querySelectorAll('iframe,frame')) {
      try { if (frame.contentDocument) visit(frame.contentDocument); } catch (_) { /* cross-origin */ }
    }
  })(document);
  if (ref) {
    const wanted = String(ref).trim().replace(/^\[|\]$/g, '').replace(/"/g, '');
    for (const root of roots) {
      const el = root.querySelector('[data-ema2-ref="' + wanted + '"]');
      if (el) return el;
    }
    return null;
  }
  if (selector) {
    for (const root of roots) {
      try { const el = root.querySelector(selector); if (el) return el; } catch (_) { /* invalid here */ }
    }
  }
  if (target) {
    const match = /^([a-zA-Z]+)\s*:\s*(.+)$/.exec(String(target).trim());
    if (match) {
      const role = match[1].toLowerCase();
      const name = match[2].trim().toLowerCase();
      const candidates = [];
      for (const root of roots) {
        for (const el of root.querySelectorAll(INTERACTIVE)) {
          if (roleOf(el).toLowerCase() === role) candidates.push(el);
        }
      }
      // 与页面 helper 一致：每一层都优先可见控件，整层不可见才回退隐藏控件。
      const pick = (test) => {
        const matching = candidates.filter(test);
        return matching.find(visible) || matching[0] || null;
      };
      const exact = pick(el => nameOf(el).toLowerCase() === name);
      if (exact) return exact;
      const named = pick(el => String(el.getAttribute('name') || '').toLowerCase() === name);
      if (named) return named;
      const partial = pick(el => nameOf(el).toLowerCase().includes(name));
      if (partial) return partial;
    }
  }
  return null;
}

// ---------------------------------------------------------------- files / js / observe / shot

let platformOs = null;
async function currentOs() {
  if (platformOs) return platformOs;
  try {
    const info = await chrome.runtime.getPlatformInfo();
    platformOs = info.os;
  } catch (_) {
    platformOs = 'unknown';
  }
  return platformOs;
}

function looksAbsolute(path, os) {
  if (os === 'win') {
    return /^[A-Za-z]:[\\/]/.test(path) || path.startsWith(String.fromCharCode(92, 92));
  }
  return path.startsWith('/');
}

async function filesTool(args) {
  const tab = await resolveTab(args);
  const action = args.action;
  if (action === 'upload') {
    const payload = Array.isArray(args.files) ? args.files : [];
    const paths = Array.isArray(args.paths) ? args.paths.filter(p => typeof p === 'string' && p.trim()) : [];
    if (!payload.length && !paths.length) fail('missing_files', 'upload requires paths[]', 'pass absolute file paths');
    if (!args.ref && !args.selector && !args.target) {
      fail('missing_target', 'upload requires ref, selector or target', 'call snapshot and pass the file input ref');
    }
    await ensureHelpers(tab.id);
    if (args.selector) await requireValidSelector(tab.id, args.selector);
    let targetRef = args.ref || null;
    if (targetRef) {
      // 短 ref（e12）先补成 e12@dX；完整 ref 必须通过当前文档校验。
      const current = await runInTab(tab.id, (ref) => {
        const helpers = window.__ema2_helpers;
        if (!helpers || !helpers.refIsCurrent(ref)) return null;
        const parsed = helpers.parseRef(ref);
        return {
          ref: parsed && !parsed.epoch ? `e${parsed.seq}@${helpers.docEpoch()}` : ref,
        };
      }, [targetRef]);
      if (!current || !current.ref) {
        fail('stale_ref', `ref ${targetRef} is stale or belongs to another document`, 'call snapshot again');
      }
      targetRef = current.ref;
    }
    if (payload.length) {
      if (payload.length > 20) fail('too_many_files', 'at most 20 files per upload', 'split into multiple calls');
      // 字节由 CLI 读好后随请求下发；页面侧用 DataTransfer 赋给 input.files，
      // 不依赖 debugger（别的扩展占住 tab 时 upload 仍然可用）。
      const result = await runInTab(tab.id, (ref, selector, target, files) => {
        const helpers = window.__ema2_helpers;
        if (!helpers) return { ok: false, code: 'upload_failed', message: 'page helpers are not installed' };
        const el = helpers.find(ref, selector, target);
        if (!el) {
          return {
            ok: false,
            code: ref && !helpers.refIsCurrent(ref) ? 'stale_ref' : 'target_not_found',
            message: 'file input not found',
          };
        }
        if (el.tagName !== 'INPUT' || String(el.type || '').toLowerCase() !== 'file') {
          return { ok: false, code: 'unsupported_control', message: 'target is not input[type=file]' };
        }
        try {
          const transfer = new DataTransfer();
          for (const item of files) {
            const binary = atob(String(item.data || ''));
            const bytes = new Uint8Array(binary.length);
            for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
            transfer.items.add(new File([bytes], String(item.name || 'file'), {
              type: String(item.type || 'application/octet-stream'),
            }));
          }
          el.files = transfer.files;
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true, names: [...el.files].map(file => file.name) };
        } catch (error) {
          return { ok: false, code: 'upload_failed', message: String((error && error.message) || error) };
        }
      }, [targetRef, args.selector || null, args.target || null, payload.map(item => ({
        name: String(item.name || 'file'),
        type: String(item.type || 'application/octet-stream'),
        data: String(item.data || ''),
      }))], undefined, 1);
      if (!result || !result.ok) {
        fail((result && result.code) || 'upload_failed',
          (result && result.message) || 'cannot assign files',
          'pass the file input ref from snapshot, or use --selector "input[type=file]"');
      }
      return 'uploaded=' + result.names.length + '\nfiles=' + result.names.join(',');
    }
    // 旧路径：只给了路径没有字节时，仍走 debugger（扩展读不了本地文件）。
    const os = await currentOs();
    const bad = paths.find(p => !looksAbsolute(p, os));
    if (bad) {
      fail('invalid_path', `not an absolute path for ${os}: ${bad}`,
        os === 'win' ? 'use a Windows path like C:/dir/file.pdf' : 'use a path starting with /');
    }
    return withDebugger(tab.id, async () => {
      await chrome.debugger.sendCommand({ tabId: tab.id }, 'DOM.enable');
      const expression = '(' + deepFindForDebugger.toString() + ')('
        + [targetRef, args.selector || null, args.target || null].map(v => JSON.stringify(v)).join(',')
        + ')';
      const evaluated = await chrome.debugger.sendCommand({ tabId: tab.id }, 'Runtime.evaluate', {
        expression,
        returnByValue: false,
      });
      if (evaluated.exceptionDetails) {
        fail('upload_failed', 'cannot evaluate file input lookup', 'call snapshot again');
      }
      const objectId = evaluated.result && evaluated.result.objectId;
      if (!objectId) {
        fail('target_not_found', 'file input not found', 'call snapshot and pass the file input ref, or target="file:..."');
      }
      await chrome.debugger.sendCommand({ tabId: tab.id }, 'DOM.setFileInputFiles', {
        files: paths,
        objectId,
      });
      return 'uploaded=' + paths.length + '\nfiles=' + paths.map(p => p.split(/[\\/]/).pop()).join(',');
    });
  }
  if (action === 'download-dir') {
    // Edge 不允许扩展修改下载目录；在触碰浏览器之前就给出稳定错误。
    fail('download_dir_unsupported',
      'Edge does not allow an extension to change the download directory',
      'set it manually in edge://settings/downloads; files land in the default Downloads folder');
  }
  fail('invalid_action', `unknown files action: ${action}`, 'use upload|download-dir');
}

function withDeadline(promise, ms, code, message, hint) {
  if (!Number.isFinite(ms) || ms <= 0) {
    return Promise.reject(new ToolError(code, message, hint || 'retry when the queue is free'));
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new ToolError(code, message, hint || 'terminate the long-running JS or retry later')),
      ms,
    );
    Promise.resolve(promise).then(
      value => { clearTimeout(timer); resolve(value); },
      error => { clearTimeout(timer); reject(error); },
    );
  });
}

/**
 * 扩展侧 RPC watchdog。必须大于扩展自己正常的等待上限，否则会在
 * `wait time 300s` / 20 步 act / batch 还没结束时先被这里掐断。
 * 数值与 rust/src/tools.rs 的 timeout_for 保持同一数量级即可；
 * daemon 侧仍然是最终兜底。
 */
function toolWatchdogMs(name, args) {
  const value = args && typeof args === 'object' ? args : {};
  const numberFrom = (source, key, fallback) => {
    const number = Number(source && source[key]);
    return Number.isFinite(number) ? Math.max(0, number) : fallback;
  };
  const seconds = (key, fallback) => numberFrom(value, key, fallback);
  switch (name) {
    case 'wait':
      return Math.round((value.await ? seconds('timeout', 3) : seconds('time', seconds('timeout', 30))) * 1000) + 8000;
    case 'open':
      return Math.round(seconds('waitTimeout', 15) * 1000) + 14000;
    case 'js':
      return Math.round(seconds('timeout', JS_DEFAULT_TIMEOUT_MS / 1000) * 1000) + 12000;
    case 'act': {
      const steps = Array.isArray(value.steps) ? value.steps : [value];
      let extra = 0;
      for (const step of steps) {
        if (!step || typeof step !== 'object') continue;
        if (step.action === 'await') extra += numberFrom(step, 'timeout', 3) * 1000;
        if (step.action === 'waitFor') extra += numberFrom(step, 'timeout', 10) * 1000;
      }
      if (value.waitFor && typeof value.waitFor === 'object') {
        const waitTimeout = Number(value.waitFor.timeout);
        extra += (Number.isFinite(waitTimeout) ? Math.max(0, waitTimeout) : 10) * 1000;
      }
      return 70000 + steps.length * 3000 + extra;
    }
    case 'batch':
      return 620000;
    default:
      return TOOL_WATCHDOG_MS;
  }
}

async function abortDebuggerEvaluation(tabId) {
  try { await withDeadline(chrome.debugger.sendCommand({ tabId }, 'Runtime.terminateExecution'), 2000, 'debugger_timeout', 'terminateExecution did not return'); }
  catch (_) { /* older builds may not support it */ }
}

/// attach({tabId}) 在 tab 内存在其他扩展的页面目标时会被拒；直接按 targetId 挂 page target。
async function attachByTargetId(tabId) {
  try {
    const targets = await chrome.debugger.getTargets();
    const page = targets.find(target => target.tabId === tabId && target.type === 'page' && !target.attached);
    if (!page) return null;
    const target = { targetId: page.id };
    await withDeadline(chrome.debugger.attach(target, '1.3'), 10000, 'debugger_attach_timeout', `cannot attach debugger to target ${page.id}`);
    return target;
  } catch (_) {
    return null;
  }
}

/// attach 失败时列出该 tab 的全部 debugger target，避免把"另一个扩展的页面目标"误报成 DevTools。
async function describeAttachFailure(tabId) {
  const parts = [];
  try {
    const tab = await chrome.tabs.get(tabId);
    parts.push(`tab=${shortText(tab.url || '', 80)}`);
  } catch (_) {
    parts.push('tab=missing');
  }
  try {
    const targets = await chrome.debugger.getTargets();
    const mine = targets.filter(target => target.tabId === tabId);
    if (!mine.length) parts.push('targets=0');
    else parts.push('targets=' + mine.map(target => `${target.type}:${shortText(target.url || '', 70)}`).join(' '));
  } catch (error) {
    parts.push(`targets=? ${oneLine(error && error.message || error).slice(0, 120)}`);
  }
  return parts.join(' ');
}

function withDebugger(tabId, run, options = {}) {
  const taskTimeout = options.timeoutMs || DEBUGGER_TASK_TIMEOUT_MS;
  const queueWait = options.queueWaitMs || DEBUGGER_QUEUE_TIMEOUT_MS;
  const queuedAt = Date.now();
  const task = debuggerQueue.then(async () => {
    const waited = Date.now() - queuedAt;
    if (waited >= queueWait) {
      throw new ToolError('debugger_busy', `debugger queue wait exceeded ${Math.round(queueWait / 1000)}s`, 'close other long-running js/files calls and retry');
    }
    const remaining = Math.min(taskTimeout, queueWait - waited);
    let attachError = null;
    let target = { tabId };
    // 扩展重载/DevTools 关闭的瞬间会偶发 attach 失败；非超时错误重试一次。
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        await withDeadline(chrome.debugger.attach(target, '1.3'), Math.min(10000, remaining), 'debugger_attach_timeout', `cannot attach debugger to tab ${tabId}`);
        attachError = null;
        break;
      } catch (error) {
        attachError = error;
        if (error instanceof ToolError && error.code === 'debugger_attach_timeout') break;
        if (attempt === 0) {
          await sleep(200);
          const byTarget = await attachByTargetId(tabId);
          if (byTarget) {
            target = byTarget;
            attachError = null;
            break;
          }
        }
      }
    }
    if (attachError) {
      if (attachError instanceof ToolError && attachError.code === 'debugger_attach_timeout') throw attachError;
      const detail = await describeAttachFailure(tabId);
      throw new ToolError('debugger_attach_failed',
        `cannot attach debugger to tab ${tabId}: ${attachError.message || attachError} | ${detail}`,
        'the tab has no attachable page target (DevTools open, or another extension page owns the tab); reload the tab and retry');
    }
    let work;
    try {
      work = Promise.resolve().then(run);
      return await withDeadline(work, remaining, 'debugger_timeout', `debugger task exceeded ${Math.round(remaining / 1000)}s`);
    } catch (error) {
      if (error instanceof ToolError && error.code === 'debugger_timeout') {
        // 先终止本次 Runtime.evaluate，再 detach；绝不能只给调用者报错却把任务留在全局队列里。
        await abortDebuggerEvaluation(tabId);
        if (work) work.catch(() => {});
      }
      throw error;
    } finally {
      try { await withDeadline(chrome.debugger.detach(target), 2000, 'debugger_detach_timeout', 'detach timed out'); }
      catch (_) { /* ignore */ }
    }
  });
  debuggerQueue = task.then(() => {}, () => {});
  return task;
}

/** 只编译、不执行；用来区分“编译期语法错误”和“运行时 SyntaxError”。 */
async function compileProbe(tabId, expression) {
  try {
    const response = await chrome.debugger.sendCommand({ tabId }, 'Runtime.compileScript', {
      expression,
      sourceURL: 'ema2://probe',
      persistScript: false,
    });
    if (response && response.exceptionDetails) {
      const details = response.exceptionDetails;
      // description 才是带 `SyntaxError: Unexpected token ...` 的完整文本；
      // text 往往只有 "Uncaught"。
      return {
        ok: false,
        message: (details.exception && details.exception.description)
          || details.text || 'syntax error',
      };
    }
    return { ok: true };
  } catch (error) {
    return { ok: false, message: String((error && error.message) || error) };
  }
}

function jsErrorHint(syntax) {
  if (syntax) {
    return 'syntax error: send an expression (`document.title`) or a statement body with optional top-level return/await; unbalanced brackets or a stray `{ }` block are the usual causes';
  }
  return 'runtime error: the code parsed but threw; read the page first (snapshot/read) or guard with try/catch';
}

function evaluateInTab(tabId, expression) {
  return chrome.debugger.sendCommand({ tabId }, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
}

function jsTimeoutMs(args) {
  const seconds = Number(args && args.timeout);
  const value = Number.isFinite(seconds) ? seconds * 1000 : JS_DEFAULT_TIMEOUT_MS;
  return Math.max(100, Math.min(JS_MAX_TIMEOUT_MS, Math.round(value)));
}

/** js 的统一输出：截断时给出明确标记；结构化值用可解析的 JSON 分片信封。 */
function formatJsOutput(value, budget, cursor, extraLine) {
  const isString = typeof value === 'string';
  const raw = value === undefined ? 'undefined' : stringifyValue(value);
  const slice = sliceText(raw, budget, cursor);
  const lines = [];
  if (slice.next != null && !isString && value !== undefined) {
    // 直接把 JSON 从中间切断会得到非法 JSON；把切片包进合法信封，
    // 并保留 next 让调用方用 cursor 继续。
    lines.push(JSON.stringify({
      ema2_truncated: true,
      total: slice.total,
      next: slice.next,
      chunk: slice.text,
    }));
    lines.push(footer(slice.total, slice.text.length, slice.next));
    lines.push('truncated=1');
    lines.push('note=structured value is a chunk envelope; pass cursor=<next> to continue');
  } else {
    lines.push(slice.text);
    if (slice.next != null) {
      lines.push(footer(slice.total, slice.text.length, slice.next));
      lines.push('truncated=1');
    }
  }
  if (extraLine) lines.push(extraLine);
  return lines.join('\n');
}

async function jsTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot run JS', 'open an http(s) page');
  // js 里也可能调用 alert/print；先装 guard，避免整个 tab 被同步对话框挂住。
  await ensurePageGuard(tab.id);
  // js 里的 console.* 也要能被后续 observe console 看到；open 超时跳过 hook 时尤其重要。
  try { await ensureConsoleHook(tab.id); } catch (_) { /* non-http page */ }
  try {
    return await jsViaDebugger(tab.id, args);
  } catch (error) {
    if (!(error instanceof ToolError)) throw error;
    // 无限循环/永不 resolve 的 Promise：withDebugger 已经 terminateExecution，
    // 把内部的 debugger_timeout 映射成对 AI 更有意义的 script_timeout。
    if (error.code === 'debugger_timeout') {
      fail('script_timeout',
        `JS did not finish within ${Math.round(jsTimeoutMs(args) / 1000)}s; execution was terminated`,
        'simplify the expression, add --timeout, or use read/snapshot/wait instead of a long-running loop');
    }
    // 别的扩展占住 tab 的页面目标时 attach 会持续失败；js 是原语，不能跟着一起消失。
    if (error.code !== 'debugger_attach_failed' && error.code !== 'debugger_attach_timeout') throw error;
    return jsViaScripting(tab.id, args, error);
  }
}

async function jsViaDebugger(tabId, args) {
  return withDebugger(tabId, async () => {
    // Runtime.compileScript 需要先启用 Runtime domain（evaluate 本身不需要）。
    await chrome.debugger.sendCommand({ tabId }, 'Runtime.enable');
    // 先用 Runtime.compileScript 只做编译探测：表达式能编译就只执行一次；
    // 运行时抛出的 SyntaxError 不是编译错误，绝不能重跑（否则副作用执行两次）。
    // compileScript 对标的是“脚本”，顶层 return 可能被接受；套一层括号才能确认是表达式。
    const expressionSource = String(args.code).trim().replace(/;+\s*$/, '');
    const asExpression = await compileProbe(tabId, `(${expressionSource}
)`);
    let result;
    let ranAsBody = false;
    if (asExpression.ok) {
      result = await evaluateInTab(tabId, args.code);
    } else {
      const wrapped = `(async () => {\n${args.code}\n})()`;
      const asBody = await compileProbe(tabId, wrapped);
      if (!asBody.ok) {
        fail('js_exception', oneLine(asBody.message || 'syntax error').slice(0, 300), jsErrorHint(true));
      }
      ranAsBody = true;
      result = await evaluateInTab(tabId, wrapped);
    }
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      const message = details.exception && details.exception.description || details.text || 'JS error';
      fail('js_exception', oneLine(message).slice(0, 300), jsErrorHint(false));
    }
    let value = result.result ? result.result.value : undefined;
    // returnByValue 会把 DOM 节点变成 {}；至少回一个可读的节点描述。
    if (value === undefined && result.result && result.result.objectId) {
      value = result.result.description || String(result.result.className || 'object');
    }
    const budget = normalizeBudget(args.budget === undefined ? 8000 : args.budget);
    const cursor = normalizeCursor(args.cursor);
    const extra = ranAsBody && value === undefined
      ? 'note=ran as an async function body; add `return <value>` to get a value back'
      : '';
    return formatJsOutput(value, budget, cursor, extra);
  }, { timeoutMs: jsTimeoutMs(args), queueWaitMs: jsTimeoutMs(args) + 5000 });
}

/** 在页面 MAIN world 里执行源码；函数在扩展页被序列化，eval 发生在页面侧。 */
async function injectMainWorld(tabId, source) {
  const results = await chrome.scripting.executeScript({
    target: { tabId },
    world: 'MAIN',
    args: [source],
    // executeScript 会把页面里抛出的异常退化成 null 结果，所以用信封把成功/失败都带回来。
    func: async (src) => {
      // eslint-disable-next-line no-eval
      const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      // 只编译、不执行地探测表达式；编译成功才 eval，运行时异常不再重跑。
      let expressionCompiles = false;
      try {
        // eslint-disable-next-line no-new-func
        new Function('return (' + src + '\n)');
        expressionCompiles = true;
      } catch (_) { /* statement body */ }
      if (expressionCompiles) {
        try {
          return { __ema2: 1, ok: true, value: await (0, eval)(src) };
        } catch (error) {
          return { __ema2: 1, ok: false, error: String((error && error.stack) || error) };
        }
      }
      let bodyError = null;
      try {
        new AsyncFunction(src);
      } catch (error) {
        bodyError = error;
      }
      if (bodyError) {
        return { __ema2: 1, ok: false, syntax: true, error: String((bodyError && bodyError.stack) || bodyError) };
      }
      try {
        return { __ema2: 1, ok: true, value: await new AsyncFunction(src)() };
      } catch (error) {
        return { __ema2: 1, ok: false, error: String((error && error.stack) || error) };
      }
    },
  });
  const payload = results && results[0] ? results[0].result : undefined;
  if (payload && typeof payload === 'object' && payload.__ema2 === 1) return payload;
  // 信封丢失说明注入没有真正跑（frame 切走/被 CSP 拦），不能当成 undefined 成功。
  return { ok: false, error: 'scripting fallback produced no result (frame changed?)', syntax: false };
}

/// debugger 被占时的降级：MAIN world 注入，参数/输出语义与 `js` 一致，额外给一行 via=scripting。
async function jsViaScripting(tabId, args, attachError) {
  const code = String(args.code || '');
  const budget = normalizeBudget(args.budget === undefined ? 8000 : args.budget);
  let payload;
  try {
    payload = await injectMainWorld(tabId, code);
  } catch (error) {
    // 注入本身失败（CSP 拦 eval、tab 被关）：统一报 js_exception，不伪装成 attach 问题。
    const message = oneLine(error && error.message || error).slice(0, 300);
    fail('js_exception', message,
      `scripting fallback failed after ${attachError.code}: page CSP may forbid eval; reload the tab and retry, or close what owns the debugger target`);
  }
  if (!payload.ok) {
    fail('js_exception', oneLine(payload.error || 'JS error').slice(0, 300),
      jsErrorHint(!!payload.syntax));
  }
  const value = payload.value;
  const cursor = normalizeCursor(args.cursor);
  return formatJsOutput(value, budget, cursor,
    'via=scripting (debugger unavailable; this path cannot be interrupted by --timeout)');
}

function stringifyValue(value) {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object' && value.nodeType === 1) {
    const html = value.outerHTML ? String(value.outerHTML) : String(value);
    return html.slice(0, 2000);
  }
  if (typeof value === 'function') return String(value);
  try { return JSON.stringify(value); } catch (_) { return String(value); }
}

function buildHar(items, pageUrl) {
  const now = Date.now();
  const toHeaders = (list) => (Array.isArray(list) ? list : []).map(item => ({
    name: String(item.name || ''),
    value: String(item.value || ''),
  }));
  const entries = items.map(entry => {
    let absolute = String(entry.url || '');
    try { absolute = new URL(absolute, pageUrl || 'about:blank').href; } catch (_) { /* keep raw */ }
    let queryString = [];
    try { queryString = [...new URL(absolute).searchParams].map(([name, value]) => ({ name, value })); } catch (_) { /* ignore */ }
    const requestHeaders = toHeaders(entry.requestHeaders);
    const responseHeaders = toHeaders(entry.responseHeaders);
    const contentType = responseHeaders.find(item => item.name.toLowerCase() === 'content-type');
    const mimeType = entry.mimeType
      || (contentType ? String(contentType.value).split(';')[0].trim() : '')
      || 'text/plain';
    const body = String(entry.body || '');
    const started = Number(entry.startedAt) || now;
    return {
      startedDateTime: new Date(started).toISOString(),
      time: entry.ms || 0,
      request: {
        method: entry.method || 'GET',
        url: absolute,
        httpVersion: 'HTTP/1.1',
        headers: requestHeaders,
        queryString,
        cookies: [],
        headersSize: -1,
        bodySize: -1,
      },
      response: {
        status: Number(entry.status) || 0,
        statusText: '',
        httpVersion: 'HTTP/1.1',
        headers: responseHeaders,
        cookies: [],
        content: { size: body.length, mimeType, text: body },
        redirectURL: '',
        headersSize: -1,
        bodySize: body.length,
      },
      cache: entry.fromCache ? { fromCache: true } : {},
      timings: { send: 0, wait: entry.ms || 0, receive: 0 },
      _resourceType: entry.type || 'fetch',
      ...(entry.ip ? { serverIPAddress: entry.ip } : {}),
    };
  });
  const first = items.length && Number(items[0].startedAt) ? Number(items[0].startedAt) : now;
  return {
    log: {
      version: '1.2',
      creator: { name: 'EMA2', version: chrome.runtime.getManifest().version },
      pages: [{ startedDateTime: new Date(first).toISOString(), id: 'page', title: '', pageTimings: {} }],
      entries,
    },
  };
}

function validateBatchInner(toolName, inner) {
  if (!inner || typeof inner !== 'object' || Array.isArray(inner)) {
    fail('invalid_argument', 'batch args must be an object', 'run `ema2 batch --help`; for scripts run `ema2 act --help`');
  }
  const has = (key) => inner[key] !== undefined && inner[key] !== null;
  const numberIn = (key, min, max) => {
    if (!has(key)) return null;
    const value = Number(inner[key]);
    if (!Number.isFinite(value) || value < min || value > max) {
      fail('invalid_argument', `${key} must be between ${min} and ${max}`, 'fix the inner arguments');
    }
    return value;
  };
  if (has('budget')) {
    const budget = numberIn('budget', 200, 200000);
    if (Math.round(budget) !== budget) fail('invalid_argument', 'budget must be an integer', 'fix the inner arguments');
  }
  if (has('cursor')) {
    const cursor = numberIn('cursor', 0, Number.MAX_SAFE_INTEGER);
    if (Math.round(cursor) !== cursor) fail('invalid_argument', 'cursor must be an integer', 'fix the inner arguments');
  }
  if (has('mode') && !['md', 'text', 'html'].includes(inner.mode)) {
    fail('invalid_argument', 'mode must be md|text|html', 'fix the inner arguments');
  }
  if (has('filter') && !['interactive', 'full', 'links', 'buttons', 'form'].includes(inner.filter)) {
    fail('invalid_argument', 'filter is invalid', 'fix the inner arguments');
  }
  if (toolName === 'js') {
    if (typeof inner.code !== 'string' || !inner.code.trim() || inner.code.length > 100000) {
      fail('invalid_argument', 'js requires a non-empty code string (max 100000)', 'fix the inner arguments');
    }
  }
  if (toolName === 'wait') {
    const conditions = ['text', 'textGone', 'selector', 'time', 'idle'].filter(key => {
      if (key === 'idle') return inner.idle === true;
      return has(key);
    });
    if (conditions.length !== 1) {
      fail('invalid_argument', 'wait needs exactly one of text, textGone, selector, time, idle', 'fix the inner arguments');
    }
    numberIn('time', 0, WAIT_MAX_SECONDS);
    numberIn('timeout', 0, WAIT_MAX_SECONDS);
  }
}

/**
 * 在多个标签页上并发执行 read/snapshot/js/wait/open，返回每页一块结果。
 * 传 `urls` 时先打开（新标签）再执行目标工具——「打开→读取」因此只需一次调用。
 */
async function batchTool(args) {
  const allowed = ['open', 'read', 'snapshot', 'js', 'wait'];
  const toolName = args.tool;
  if (!allowed.includes(toolName)) {
    fail('invalid_tool', `batch cannot run ${toolName}`, 'use open|read|snapshot|js|wait');
  }
  const urls = Array.isArray(args.urls) ? args.urls : [];
  const tabIds = Array.isArray(args.tabIds) ? args.tabIds : [];
  if (urls.length && tabIds.length) {
    fail('invalid_argument', 'batch takes either tabIds or urls, not both', 'use urls to open new tabs, or tabIds for tabs you already have');
  }
  if (args.reuse !== undefined && args.reuse !== 'session') {
    fail('invalid_argument', 'batch reuse must be session', 'use --reuse session with --urls');
  }
  if (args.reuse === 'session' && !urls.length) {
    fail('invalid_argument', 'batch --reuse session only applies to --urls', 'use --tab-ids for existing tabs');
  }
  const targets = urls.length ? urls : tabIds;
  if (!targets.length || targets.length > MAX_BATCH_TABS) {
    fail('invalid_argument', `batch needs 1..${MAX_BATCH_TABS} tabIds or urls`, 'fix tabIds/urls');
  }
  if (urls.length) {
    for (const [index, url] of urls.entries()) {
      if (typeof url !== 'string' || !/^https?:\/\//i.test(url)) {
        fail('invalid_argument', `urls[${index}] must be an http(s) url`, 'fix urls');
      }
    }
  } else {
    for (const [index, tabId] of tabIds.entries()) {
      if (!Number.isInteger(tabId) || tabId < 0) {
        fail('invalid_argument', `tabIds[${index}] must be a non-negative integer`, 'call tabs to list ids');
      }
    }
  }
  const inner = args.args === undefined ? {} : args.args;
  if (!inner || typeof inner !== 'object' || Array.isArray(inner)) {
    fail('invalid_argument', 'batch args must be an object', 'run `ema2 batch --help`; for scripts run `ema2 act --help`');
  }
  if (urls.length) {
    // 复合模式的内层参数属于目标工具；open 自身只接受 wait/replace/reuse。
    validateBatchInner(toolName, inner);
    if (inner.replace !== undefined || inner.reuse !== undefined || inner.tabId !== undefined) {
      fail('invalid_argument', 'urls mode always opens new tabs; replace/reuse/tabId are not allowed', 'remove those flags or use tabIds mode');
    }
    if (inner.wait !== undefined && !['load', 'dom', 'none'].includes(inner.wait)) {
      fail('invalid_argument', 'wait must be load|dom|none', 'fix the inner arguments');
    }
    if (inner.replace !== undefined && typeof inner.replace !== 'boolean') {
      fail('invalid_argument', 'replace must be a boolean', 'fix the inner arguments');
    }
    if (inner.reuse !== undefined && inner.reuse !== 'session') {
      fail('invalid_argument', 'reuse must be session', 'fix the inner arguments');
    }
  } else {
    validateBatchInner(toolName, inner);
  }
  const requested = args.concurrency === undefined ? 2 : Number(args.concurrency);
  if (!Number.isFinite(requested) || requested < 1 || requested > 4 || Math.round(requested) !== requested) {
    fail('invalid_argument', 'concurrency must be an integer between 1 and 4', 'fix concurrency');
  }
  const concurrency = requested;
  const reusedByIndex = new Map();
  if (args.reuse === 'session' && urls.length) {
    await loadSessionTabs();
    const liveTabs = await chrome.tabs.query({});
    const liveById = new Map(liveTabs.map(tab => [tab.id, tab]));
    const available = sessionTabs
      .map(id => liveById.get(id))
      .filter(Boolean);
    const used = new Set();
    const hrefOf = (value) => {
      try { return new URL(value).href; } catch (_) { return String(value || ''); }
    };
    for (const [index, url] of urls.entries()) {
      const wanted = hrefOf(url);
      const match = available.find(tab => !used.has(tab.id)
        && hrefOf(tab.url || tab.pendingUrl || '') === wanted);
      if (match) {
        used.add(match.id);
        reusedByIndex.set(index, match.id);
      }
    }
  }
  const results = new Array(targets.length);
  let cursor = 0;
  async function worker() {
    for (;;) {
      const index = cursor++;
      if (index >= targets.length) return;
      if (urls.length) {
        const url = urls[index];
        try {
          const reusedTabId = reusedByIndex.get(index);
          const openArgs = { ...inner, url };
          if (reusedTabId !== undefined) {
            openArgs.tabId = reusedTabId;
            openArgs.reuseSamePage = true;
          }
          let openedText;
          try {
            openedText = resultText(await executeTool('open', openArgs));
          } catch (error) {
            if (reusedTabId === undefined || !(error instanceof ToolError)
                || error.code !== 'tab_not_found') throw error;
            reusedByIndex.delete(index);
            delete openArgs.tabId;
            delete openArgs.reuseSamePage;
            openedText = resultText(await executeTool('open', openArgs));
          }
          const reusedLine = reusedByIndex.has(index) ? `reused=1 tab=${reusedTabId}\n` : '';
          if (toolName === 'open') {
            results[index] = `url=${url}\n${reusedLine}` + openedText;
          } else {
            const tabId = Number((/^id=(\d+)\s*$/m.exec(openedText) || [])[1]);
            if (!Number.isInteger(tabId)) {
              // open 的输出契约变了就别猜：明确失败，而不是把别的东西当 tabId。
              results[index] = `url=${url}\n${reusedLine}` + openedText
                + '\nerror=internal msg=open did not report id= hint=retry with tool=open to inspect the tab';
            } else {
              const innerArgs = { ...inner, tabId };
              delete innerArgs.replace;
              delete innerArgs.reuse;
              const value = await executeTool(toolName, innerArgs);
              results[index] = `url=${url}\n${reusedLine}tab=${tabId}\n` + resultText(value);
            }
          }
        } catch (error) {
          results[index] = `url=${url}\n` + formatError(error);
        }
        continue;
      }
      const tabId = tabIds[index];
      try {
        const value = await executeTool(toolName, { ...inner, tabId });
        results[index] = `tab=${tabId}\n` + resultText(value);
      } catch (error) {
        results[index] = `tab=${tabId}\n` + formatError(error);
      }
    }
  }
  await Promise.all(Array.from({ length: concurrency }, worker));
  return results.join('\n---\n');
}

function resultText(value) {
  if (value && Array.isArray(value.content)) {
    return value.content.filter(item => item.type === 'text').map(item => item.text).join('\n');
  }
  return String(value == null ? '' : value);
}

async function annotateScreenshot(dataUrl, marks, dpr, format) {
  const blob = await (await fetch(dataUrl)).blob();
  const bitmap = await createImageBitmap(blob);
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  const ctx = canvas.getContext('2d');
  ctx.drawImage(bitmap, 0, 0);
  ctx.scale(dpr || 1, dpr || 1);
  ctx.font = '600 12px sans-serif';
  ctx.textBaseline = 'top';
  for (const mark of marks) {
    ctx.strokeStyle = '#ff2d55';
    ctx.lineWidth = 2;
    ctx.strokeRect(mark.x, mark.y, mark.w, mark.h);
    const label = String(mark.ref).replace(/^e(\d+)@.*$/, '$1').replace(/^e/, '');
    const width = ctx.measureText(label).width + 8;
    const top = Math.max(0, mark.y - 16);
    ctx.fillStyle = '#ff2d55';
    ctx.fillRect(mark.x, top, width, 16);
    ctx.fillStyle = '#ffffff';
    ctx.fillText(label, mark.x + 4, top + 2);
  }
  const outBlob = await canvas.convertToBlob({ type: format === 'jpeg' ? 'image/jpeg' : 'image/png' });
  const bytes = new Uint8Array(await outBlob.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return { base64: btoa(binary), mime: 'image/' + (format === 'jpeg' ? 'jpeg' : 'png') };
}

async function activeTabInWindow(windowId) {
  const [active] = await chrome.tabs.query({ windowId, active: true });
  return active || null;
}

async function requireActiveShotTab(tab) {
  const active = await activeTabInWindow(tab.windowId);
  if (!active || active.id !== tab.id) {
    fail('tab_not_active',
      `tab ${tab.id} is not the active tab in window ${tab.windowId}`,
      'shot --tab auto-activates the tab; if it still fails, call tabs activate and keep Edge in the foreground');
  }
}

async function shotTool(args) {
  const tab = await resolveTab(args);
  const format = args.format === 'jpeg' ? 'jpeg' : 'png';
  // `--tab` 语义：自动把目标标签/窗口提到前台，而不是让用户猜为什么永远失败。
  let activated = false;
  const active = await activeTabInWindow(tab.windowId);
  if (!active || active.id !== tab.id) {
    try {
      await chrome.tabs.update(tab.id, { active: true });
      await chrome.windows.update(tab.windowId, { focused: true });
      activated = true;
      await sleep(120);
    } catch (_) { /* requireActiveShotTab 会给出稳定错误 */ }
  }
  await requireActiveShotTab(tab);
  let marks = [];
  let marksText = '';
  let docBefore = '';
  if (args.annotate) {
    await ensureHelpers(tab.id);
    docBefore = await runInTab(tab.id, () => window.__ema2_helpers.docEpoch(), []);
    marks = (await runInTab(tab.id, () => {
      const helpers = window.__ema2_helpers;
      helpers.snapshot('interactive', 20000, 0);
      return helpers.marks();
    }, [])) || [];
    marksText = marks.map(mark => `[${mark.ref}] ${mark.role} "${mark.name}" @${mark.x},${mark.y} ${mark.w}x${mark.h}`).join('\n');
  }
  let dataUrl;
  try {
    dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format });
  } catch (error) {
    fail('capture_failed', `captureVisibleTab failed: ${error.message}`, 'make the browser window visible/focused and retry');
  }
  // 截图过程中用户/页面可能切换活动标签页：目标变了就明确失败，不把别的页标成目标页。
  await requireActiveShotTab(tab);
  if (args.annotate) {
    const docAfter = await runInTab(tab.id, () => window.__ema2_helpers.docEpoch(), []).catch(() => null);
    if (docAfter !== docBefore) {
      fail('tab_changed', 'the target document changed while taking the screenshot', 'retry after the page stops navigating');
    }
  }
  const match = new RegExp('^data:image/(' + format + ');base64,(.+)$', 's').exec(dataUrl || '');
  if (!match) fail('capture_empty', 'screenshot payload was empty', 'retry');
  if (!args.annotate) {
    const mime = 'image/' + (format === 'png' ? 'png' : 'jpeg');
    const content = [];
    if (activated) content.push({ type: 'text', text: `activated=1 tab=${tab.id}` });
    content.push({ type: 'image', data: match[2], mimeType: mime });
    return { content };
  }
  const dpr = await runInTab(tab.id, () => window.devicePixelRatio || 1, []).catch(() => 1);
  const annotated = await annotateScreenshot(dataUrl, marks, dpr, format);
  const content = [];
  const prefix = activated ? `activated=1 tab=${tab.id}\n` : '';
  if (marks.length) content.push({ type: 'text', text: prefix + marksText + '\nmarks=' + marks.length });
  else if (prefix) content.push({ type: 'text', text: prefix.trimEnd() });
  content.push({ type: 'image', data: annotated.base64, mimeType: annotated.mime });
  return { content };
}

/** 按完整记录分页：next 只推进到已经完整交付的记录。 */
function paginateRecords(records, budget) {
  const items = [];
  let used = 0;
  for (const record of records) {
    const line = String(record.line || '');
    if (items.length && used + line.length + 1 > budget) break;
    items.push(record);
    used += line.length + 1;
  }
  return { items, next: items.length < records.length ? items[items.length - 1].seq : null };
}

const BODY_FILTER_MAX_CHARS = 512;
const BODY_FILTER_TEXT_MAX_CHARS = 2000;
// 灾难性回溯的典型来源：量词套量词 `(a+)+`、量词组内重复/前缀分支 `(a|a)+` / `(aa|a)+`。
const BODY_FILTER_NESTED_QUANTIFIER = /\((?:[^()\\]|\\.)*[+*{](?:[^()\\]|\\.)*\)\s*[+*{]/;

/** 只在同一量词组的 alternatives 相等或互为前缀时拒绝，避免误伤 `(GET|POST)+`。 */
function hasRiskyQuantifiedAlternation(pattern) {
  const group = /\(([^()]*)\)\s*[+*{]/g;
  let match;
  while ((match = group.exec(pattern))) {
    const body = match[1];
    if (!body.includes('|')) continue;
    const alternatives = body.split('|').map(part => part.trim()).filter(Boolean);
    if (alternatives.length < 2) continue;
    for (const left of alternatives) {
      for (const right of alternatives) {
        if (left === right || left.startsWith(right) || right.startsWith(left)) return true;
      }
    }
  }
  return false;
}

/**
 * 编译 `bodyFilter`，并拒绝已知会指数回溯的形态。
 *
 * JS 正则没有超时机制，`RegExp.test` 一旦进入灾难性回溯会阻塞 service worker
 * （心跳、重连和所有工具调用一起卡住）。这里做长度上限 + 形态拒绝 + 输入截断三层防护；
 * 这是启发式而非证明，剩余风险由 ≤800 字符的 body 预览兜底。
 */
function compileBodyFilter(source) {
  const pattern = String(source);
  if (pattern.length > BODY_FILTER_MAX_CHARS) {
    fail('invalid_regex', `bodyFilter is limited to ${BODY_FILTER_MAX_CHARS} chars`, 'shorten the pattern');
  }
  if (BODY_FILTER_NESTED_QUANTIFIER.test(pattern) || hasRiskyQuantifiedAlternation(pattern)) {
    fail('invalid_regex', 'bodyFilter uses a quantified group that can backtrack exponentially',
      'rewrite it without nested quantifiers, e.g. use two separate filters or a plain substring');
  }
  try {
    return new RegExp(pattern, 'i');
  } catch (error) {
    fail('invalid_regex', `bodyFilter is not a valid regex: ${error.message}`, 'fix the regex');
  }
}

/** bodyFilter 只在受控长度的预览上执行，避免把整页文本交给用户正则。 */
function bodyFilterMatches(regex, body) {
  return regex.test(String(body || '').slice(0, BODY_FILTER_TEXT_MAX_CHARS));
}

async function observeTool(args) {
  const tab = await resolveTab(args);
  const kind = args.kind;
  const since = normalizeCursor(args.since);
  const budget = normalizeBudget(args.budget === undefined ? 3000 : args.budget);

  if (kind === 'console') {
    if (args.har) {
      fail('invalid_argument', '--har only applies to network observations', 'use `observe network --har PATH`');
    }
    // 先装 hook 再等：否则 --wait 期间新产生的 console 行根本不会被记录。
    await ensureConsoleHook(tab.id);
    if (args.wait) await sleep(Math.min(10000, Math.round(args.wait * 1000)));
    const knownDoc = observedConsoleDocs.get(tab.id);
    const currentDoc = await runInTab(tab.id, () => window.__ema2_console_epoch || '', [], 'MAIN').catch(() => '');
    const forceReset = !!knownDoc && since > 0 && currentDoc !== knownDoc;
    const data = await runInTab(tab.id, (level, sinceValue, resetDoc, filterText) => {
      const min = { log: 1, warn: 3, error: 4 }[level] || 4;
      const all = window.__ema2_console || [];
      const seq = window.__ema2_console_seq || 0;
      const reset = resetDoc || sinceValue > seq;
      const start = reset ? 0 : sinceValue;
      const needle = filterText ? String(filterText).toLowerCase() : '';
      const entries = all.filter(e => e.seq > start && (e.order || 1) >= min
        && (!needle || String(e.text || '').toLowerCase().includes(needle)));
      return {
        entries,
        seq,
        epoch: window.__ema2_console_epoch || '',
        reset,
        firstSeq: all.length ? all[0].seq : seq + 1,
      };
    }, [args.level || 'error', since, forceReset, args.filter || null], 'MAIN').catch(() => null);
    if (!data) fail('page_changed', 'the page navigated while reading console entries', 'retry after the page settles');
    if (data.epoch) observedConsoleDocs.set(tab.id, data.epoch);
    const records = data.entries.map(e => ({
      seq: e.seq,
      line: `#${e.seq} ${e.level} ${oneLine(e.text).slice(0, 240)}`,
    }));
    const page = paginateRecords(records, budget);
    const next = page.next == null ? data.seq : page.next;
    const dropped = Math.max(0, (data.firstSeq || 1) - (data.reset ? 0 : since) - 1);
    const parts = [page.items.map(item => item.line).join('\n')];
    parts.push(`level=${args.level || 'error'}`);
    parts.push(footer(records.length, page.items.length, next));
    if (dropped > 0) parts.push(`dropped=${dropped}`);
    if (data.reset) parts.push('reset=1');
    return parts.filter(Boolean).join('\n');
  }

  if (kind === 'network') {
    // 先编译/校验正则，非法或高风险模式在触碰页面之前就失败。
    const bodyRegex = args.bodyFilter ? compileBodyFilter(args.bodyFilter) : null;
    await ensureNetworkHook(tab.id);
    const withBodies = args.bodies !== false || !!args.har;
    await setNetworkBodyCapture(tab.id, withBodies);
    if (args.wait) await sleep(Math.min(10000, Math.round(args.wait * 1000)));
    const knownDoc = observedNetworkDocs.get(tab.id);
    const currentDoc = await runInTab(tab.id, () => (window.__ema2_net ? window.__ema2_net.epoch || '' : ''), [], 'MAIN').catch(() => '');
    const forceReset = !!knownDoc && since > 0 && currentDoc !== knownDoc;
    const data = await runInTab(tab.id, (sinceValue, filterValue, resetDoc) => {
      const state = window.__ema2_net || { entries: [], seq: 0 };
      const seq = state.seq || 0;
      const reset = resetDoc || sinceValue > seq;
      const start = reset ? 0 : sinceValue;
      const filterText = filterValue ? String(filterValue).toLowerCase() : '';
      const list = state.entries.filter(entry =>
        entry.seq > start && (!filterText || String(entry.url).toLowerCase().includes(filterText)));
      return {
        seq,
        epoch: state.epoch || '',
        reset,
        firstSeq: state.entries.length ? state.entries[0].seq : seq + 1,
        items: list.map(entry => ({
          seq: entry.seq, type: entry.type, method: entry.method, url: entry.url,
          status: entry.status, ms: entry.ms, body: entry.body, preview: entry.preview || 'done',
          startedAt: entry.startedAt || 0, backfilled: !!entry.backfilled,
        })),
      };
    }, [since, args.filter || null, forceReset], 'MAIN').catch(() => null);
    if (!data) fail('page_changed', 'the page navigated while reading network entries', 'retry after the page settles');
    if (data.epoch) observedNetworkDocs.set(tab.id, data.epoch);
    let items = data.items;
    let pendingSeqs = [];
    if (bodyRegex) {
      // bodyFilter 只能判断已经读完的 preview；pending 条目无法凭空匹配，
      // 记录它们的 seq 让调用方精确补读，而不是把后续已就绪的条目一起丢掉。
      const matched = [];
      for (const entry of items) {
        if (entry.preview === 'pending') { pendingSeqs.push(entry.seq); continue; }
        if (bodyFilterMatches(bodyRegex, entry.body)) matched.push(entry);
      }
      items = matched;
    } else {
      pendingSeqs = items.filter(entry => entry.preview === 'pending').map(entry => entry.seq);
    }

    // webRequest 提供页面 hook 装好之前的真实 status/headers/MIME；
    // 页面 hook 提供响应体预览。按 method+URL（必要时 path+query）与时间邻近匹配。
    const webRecords = getWebRequests(tab.id);
    const pageUrl = tab.url || '';
    const absoluteNetworkUrl = (value) => {
      try { return new URL(String(value || ''), pageUrl).href; } catch (_) { return String(value || ''); }
    };
    const sameRequest = (record, entry) => {
      if (String(record.method || '').toUpperCase() !== String(entry.method || '').toUpperCase()) return false;
      if (record.url === entry.url) return true;
      try {
        const left = new URL(record.url);
        const right = new URL(entry.url);
        return left.pathname === right.pathname && left.search === right.search;
      } catch (_) {
        return false;
      }
    };
    for (const entry of items) {
      entry.url = absoluteNetworkUrl(entry.url);
      const candidates = webRecords.filter(record => sameRequest(record, entry));
      let match = null;
      if (candidates.length) {
        match = candidates[candidates.length - 1];
        if (entry.startedAt) {
          match = candidates.reduce((best, record) =>
            Math.abs(record.startedAt - entry.startedAt) < Math.abs(best.startedAt - entry.startedAt)
              ? record : best, match);
        }
      }
      if (match) {
        if (!(Number(entry.status) > 0) && Number(match.status) > 0) entry.status = match.status;
        entry.startedAt = entry.startedAt || match.startedAt;
        entry.requestHeaders = match.requestHeaders || [];
        entry.responseHeaders = match.responseHeaders || [];
        entry.mimeType = match.mimeType || '';
        entry.fromCache = !!match.fromCache;
        entry.ip = match.ip || '';
      }
      if (!entry.startedAt) entry.startedAt = Date.now();
    }

    if (args.har) {
      const har = buildHar(items, pageUrl);
      // HAR 是唯一允许超过 MAX_OUTPUT_CHARS 的 JSON 输出；必须用 content[] 返回，
      // 否则 toResult 的 capText 会把 JSON 截断成非法文件。
      return { content: [{ type: 'text', text: JSON.stringify(har) }] };
    }
    const records = items.map(entry => {
      // 完整 URL 至少保留 host，跨域请求不能只剩 /foo；过长时明确给省略号。
      const display = shortText(entry.url, 400);
      const body = entry.body ? ' body=' + oneLine(entry.body).slice(0, 200) : '';
      // 保留整条记录（method/status/url 始终有效），只把“body 还没读完”标出来。
      const streaming = entry.preview === 'pending' ? ' streaming=1' : '';
      return {
        seq: entry.seq,
        line: `#${entry.seq} ${entry.method} ${entry.status} ${entry.ms}ms ${display}${body}${streaming}`,
      };
    });
    const page = paginateRecords(records, budget);
    const next = page.next == null ? data.seq : page.next;
    const dropped = Math.max(0, (data.firstSeq || 1) - (data.reset ? 0 : since) - 1);
    const parts = [page.items.map(item => item.line).join('\n')];
    parts.push(footer(records.length, page.items.length, next));
    if (dropped > 0) parts.push(`dropped=${dropped}`);
    if (pendingSeqs.length) {
      const listed = pendingSeqs.slice(0, 20).join(',');
      parts.push(`pending=${pendingSeqs.length} seqs=${listed}${pendingSeqs.length > 20 ? ',...' : ''}`);
      parts.push('hint=body still streaming; pass since=<first seq - 1> to read those entries again');
    }
    if (data.reset) parts.push('reset=1');
    return parts.filter(Boolean).join('\n');
  }

}

// ---------------------------------------------------------------- read / snapshot

async function readTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be read', 'open an http(s) page');
  await ensureHelpers(tab.id);
  const mode = args.mode || 'md';
  const selector = args.selector;
  if (selector) await requireValidSelector(tab.id, selector);
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  const useMain = args.main !== false && !selector;
  const data = await runInTab(tab.id, (modeName, selectorArg, mainWanted) => {
    const helpers = window.__ema2_helpers;
    let root = document.body || document.documentElement;
    let mainSelector = '';
    let mainFallback = false;
    let textLength = 0;
    let bodyLength = 0;
    if (selectorArg) {
      const found = helpers.find(null, selectorArg, null);
      if (!found) return { missing: true };
      root = found;
    } else if (mainWanted) {
      const picked = helpers.mainRoot();
      root = picked.root;
      mainSelector = picked.selector;
      mainFallback = !!picked.fallback;
      textLength = picked.textLength || 0;
      bodyLength = picked.bodyLength || 0;
    }
    const text = modeName === 'visible' ? helpers.visibleText(root)
      : modeName === 'html' ? root.innerHTML
      : modeName === 'text' ? helpers.plainText(root)
      : helpers.markdown(root);
    return {
      text,
      mainFallback,
      textLength,
      bodyLength,
      header: 'url=' + location.href + '\ntitle=' + helpers.clean(document.title)
        + '\nmode=' + modeName
        + (mainSelector ? '\nmain=' + mainSelector : ''),
    };
  }, [mode, selector || null, useMain]).catch(() => null);
  if (!data) fail('page_changed', 'the page navigated while reading content', 'retry after the page settles');
  if (data.missing) {
    fail('selector_not_found', `selector not found: ${selector}`, 'call snapshot to see available refs/roles');
  }
  const slice = sliceText(data.text || '', budget, cursor);
  const parts = [data.header, slice.text, footer(slice.total, slice.text.length, slice.next)];
  if (slice.next != null) parts.push('truncated=1');
  const hint = truncationHint('read', slice.total, slice.text.length, slice.next, cursor);
  if (hint) parts.push(`hint=${hint.replace(/^hint=/, '')} (keep --mode ${mode})`);
  if (cursor > 0) parts.push(`hint=cursor is a character offset for mode=${mode}; use the same mode when continuing`);
  if (data.mainFallback) {
    parts.push(`hint=main extraction too short (${data.textLength || 0} chars); fell back to body. Use --selector or --no-main to control this.`);
  }
  if (mode === 'visible' && !slice.total) {
    parts.push('hint=visible text is empty; the page may render in canvas/PDF/cross-origin iframe; try js or shot --annotate');
  }
  return parts.join('\n');
}

async function snapshotTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be snapshotted', 'open an http(s) page');
  await ensureHelpers(tab.id);
  const filter = args.filter || 'interactive';
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  let data;
  try {
    data = await runInTab(tab.id, (filterName, budgetValue, cursorValue, nearRef, showValues) => {
      return window.__ema2_helpers.snapshot(filterName, budgetValue, cursorValue, nearRef, showValues);
    }, [filter, budget, cursor, args.near || null, args.values === true || filter === 'form']);
  } catch (_) {
    fail('page_changed', 'the page navigated while taking the snapshot', 'retry after the page settles');
  }
  if (!data) fail('snapshot_failed', 'snapshot produced no data', 'retry');
  const parts = [data.header];
  if (data.body) parts.push(data.body);
  parts.push(footer(data.total, data.shown, data.next));
  if (data.next != null) parts.push('truncated=1');
  const hint = truncationHint('snapshot', data.total, data.shown, data.next, cursor, 'refs');
  if (hint) parts.push(hint);
  const refs = Number((/refs=(\d+)/.exec(data.header) || [])[1] || 0);
  const visual = Number((/visual=(\d+)/.exec(data.header) || [])[1] || 0);
  if (refs === 0 && visual > 0) {
    parts.push('hint=page looks visual (canvas/img); try `ema2 shot --annotate`, or read the DOM with js');
  }
  return parts.join('\n');
}

// ---------------------------------------------------------------- dispatch

function requireStringArg(args, key, max, allowEmpty) {
  if (args[key] === undefined) return undefined;
  if (typeof args[key] !== 'string' || args[key].length > max || (!allowEmpty && !args[key].trim())) {
    fail('invalid_argument', `${key} must be a string (max ${max})`, 'run `ema2 <command> --help`; for scripts run `ema2 act --help`');
  }
  return args[key];
}

async function executeTool(name, args) {
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
      return withCommandGuard('open', openArgs, () => openTool(openArgs));
    }
    case 'read': return withCommandGuard('read', args, () => readTool(args));
    case 'snapshot': return withCommandGuard('snapshot', args, () => snapshotTool(args));
    case 'act': return withCommandGuard('act', args, () => actTool(args));
    case 'wait': return withCommandGuard('wait', args, () => waitTool(args));
    case 'js': {
      const code = requireStringArg(args, 'code', 100000, false);
      if (!code) fail('missing_code', 'code is required', 'pass a JS expression');
      const jsArgs = { ...args, code };
      return withCommandGuard('js', jsArgs, () => jsTool(jsArgs));
    }
    case 'observe': return withCommandGuard('observe', args, () => observeTool(args));
    case 'files': return withCommandGuard('files', args, () => filesTool(args));
    case 'batch': return withCommandGuard('batch', args, () => batchTool(args));
    case 'shot': return withCommandGuard('shot', args, () => shotTool(args));
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

registerWebRequestListeners();
chrome.tabs.onRemoved.addListener((tabId) => {
  observedConsoleDocs.delete(tabId);
  observedNetworkDocs.delete(tabId);
  webRequestLog.delete(tabId);
  commandGuardTabs.delete(tabId);
});
ensureWatchdog();
// 动态 command guard 不应跨 service worker 重生长时间残留。
// 统一走 guard 队列，避免启动清理和第一条命令的 register 交叉。
cleanupLegacyGuardOnce();
connect();
chrome.runtime.onStartup.addListener(() => { ensureWatchdog(); connect(); });
chrome.runtime.onInstalled.addListener(() => {
  ensureWatchdog();
  cleanupLegacyGuardOnce();
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
