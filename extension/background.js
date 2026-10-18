'use strict';

/**
 * EMA2 Edge 扩展 · v1.4（AI 专用浏览器接口）
 * -------------------------------------------
 * 设计前提：这是给 AI 用的浏览器，不是给人用的自动化工具。
 *   - 输出一律行式紧凑文本（key=value / TSV / [ref]），不 pretty-print JSON
 *   - 列表都带 total/shown/next，长内容用 budget/cursor 分页
 *   - 错误统一 `error=<code> msg=... hint=...`，让模型能自我纠正
 *   - snapshot 给可引用 ref，act 按 ref 操作，避免 AI 猜 CSS 选择器
 *
 * 工具：tabs / open / read / snapshot / act / wait / js / observe / shot
 *
 * WS：默认 ws://127.0.0.1:8765，可在 chrome.storage.local 设 wsUrl 覆盖。
 * 鉴权：中继设了 EDGE_MCP_TOKEN 时，在这里设同一个 token：
 *   chrome.storage.local.set({ token: '<EDGE_MCP_TOKEN>' })
 */

const DEFAULT_WS_URL = 'ws://127.0.0.1:8765';
const RECONNECT_ALARM = 'ema2-reconnect';
const WATCHDOG_ALARM = 'ema2-watchdog';
const MAX_INCOMING_CHARS = 2 * 1024 * 1024;
const MAX_MESSAGE_CHARS = 48 * 1024 * 1024;
const MAX_OUTPUT_CHARS = 200000;
const DEFAULT_BUDGET = 4000;
const WAIT_TIMEOUT_MS = 30000;

let socket = null;
let connecting = false;
let retryAttempt = 0;
let reconnectTimer = null;
let relayReady = false;
let debuggerQueue = Promise.resolve();
let heartbeatTimer = null;

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
  try { return await chrome.storage.local.get(['wsUrl', 'token']); } catch (_) { return {}; }
}

async function getWsUrl(settings) {
  const stored = settings || await readSettings();
  let base;
  try {
    base = validateWsUrl(stored.wsUrl || DEFAULT_WS_URL);
  } catch (error) {
    console.error(`[EMA2] ${error.message}; falling back to ${DEFAULT_WS_URL}`);
    base = DEFAULT_WS_URL;
  }
  const token = typeof stored.token === 'string' ? stored.token.trim() : '';
  if (!token) return base;
  const url = new URL(base);
  url.searchParams.set('token', token);
  return validateWsUrl(url.href);
}

async function connect() {
  if (connecting) return;
  if (socket && (socket.readyState === WebSocket.CONNECTING || socket.readyState === WebSocket.OPEN)) return;
  connecting = true;
  let url;
  try { url = await getWsUrl(); } catch (error) {
    connecting = false;
    console.error('[EMA2] WebSocket URL rejected:', error.message);
    scheduleReconnect();
    return;
  }
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
    handleMCPMessage(event.data, ws).catch((error) => console.error('[EMA2] handler failed:', error));
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    relayReady = false;
    stopHeartbeat();
    scheduleReconnect();
  };
  ws.onerror = () => {};
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  const delayMs = Math.min(30000, 1000 * Math.pow(2, retryAttempt++));
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

// ---------------------------------------------------------------- mcp plumbing

async function handleMCPMessage(data, ws) {
  if (typeof data !== 'string' || data.length > MAX_INCOMING_CHARS) return;
  let req;
  try { req = JSON.parse(data); } catch (_) { return; }
  if (!req || typeof req !== 'object') return;

  const hasId = Object.prototype.hasOwnProperty.call(req, 'id');
  if (req.method === 'bridge/welcome') { relayReady = true; return; }
  if (req.method === 'bridge/reload') {
    // 开发用：中继要求扩展重新加载自身（等价于 edge://extensions 点“重新加载”）。
    if (hasId) sendResponse(ws, req.id, { result: {} });
    setTimeout(() => chrome.runtime.reload(), 250);
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
    let result;
    try {
      result = toResult(await withKeepAlive(() => executeTool(params.name, normalizeArgs(params.arguments))));
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
  return value.slice(0, MAX_OUTPUT_CHARS) + '\nnext=truncated total=' + value.length;
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
  if (cursor >= total) return { text: '', total, next: null };
  const end = Math.min(total, cursor + budget);
  return { text: text.slice(cursor, end), total, next: end < total ? end : null };
}

function footer(total, shown, next) {
  return `total=${total} shown=${shown} next=${next == null ? 'none' : next}`;
}

// ---------------------------------------------------------------- tabs

function isInternalUrl(url) {
  return /^(edge|chrome|about|devtools|view-source|chrome-extension|extension):/.test(url || '');
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
    return lines.join('\n') + '\n' + footer(lines.length, lines.length, null);
  }
  if (action === 'current') {
    const tab = await activeTab();
    return `id=${tab.id}\nwindow=${tab.windowId}\nos=${await currentOs()}\ntitle=${oneLine(tab.title || '')}\nurl=${tab.url || ''}`;
  }
  if (action === 'new') {
    const url = args.url;
    if (!url) fail('missing_url', 'tabs new requires url', 'call open instead');
    const tab = await chrome.tabs.create({ url, active: args.active !== false });
    return `id=${tab.id}\nurl=${tab.pendingUrl || tab.url || url}\nwindow=${tab.windowId}`;
  }
  if (action === 'close') {
    const tab = await resolveTab(args);
    await chrome.tabs.remove(tab.id);
    return `closed=${tab.id}`;
  }
  if (action === 'activate') {
    const tab = await resolveTab(args);
    await chrome.tabs.update(tab.id, { active: true });
    return `active=${tab.id}`;
  }
  fail('invalid_action', `unknown tabs action: ${action}`, 'use list|current|new|close|activate');
}

// ---------------------------------------------------------------- open / wait

async function openTool(args) {
  const wait = args.wait || 'load';
  let tab;
  if (args.tabId === undefined) {
    tab = await chrome.tabs.create({ url: args.url, active: true });
  } else {
    tab = await resolveTab(args);
    await chrome.tabs.update(tab.id, { url: args.url });
  }
  if (wait !== 'none') {
    await waitForReady(tab.id, wait === 'load' ? 'complete' : 'interactive', 15000);
  }
  const current = await chrome.tabs.get(tab.id);
  // 等页面稳定后自动装网络 hook，这样后续 fetch/XHR 都能被 observe network 看到。
  await sleep(250);
  let hook = false;
  try { hook = await ensureNetworkHook(tab.id); } catch (_) { hook = false; }
  try { await ensureConsoleHook(tab.id); } catch (_) { /* ignore */ }
  return `id=${current.id}\nurl=${current.url || args.url}\ntitle=${oneLine(current.title || '')}\nnet_hook=${hook ? 'on' : 'off'}`;
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
  const tab = await resolveTab(args);
  const timeoutMs = args.timeout === undefined ? WAIT_TIMEOUT_MS : Math.round(args.timeout * 1000);
  const start = Date.now();
  if (args.time !== undefined) {
    await sleep(Math.round(args.time * 1000));
    return `ok=time elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.idle) {
    const ready = await waitForReady(tab.id, 'complete', timeoutMs);
    if (!ready) fail('wait_timeout', `page did not become idle in ${timeoutMs / 1000}s`, 'retry, or wait for a specific selector/text');
    return `ok=idle elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.selector !== undefined) await ensureHelpers(tab.id);
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

/** 页面内 helper（作为 executeScript 的 func 注入，必须自包含）。 */
function installHelpers() {
  if (window.__ema2_helpers) return true;
  const clean = (value) => String(value == null ? '' : value).replace(/\s+/g, ' ').trim();
  const shortUrl = (url) => {
    try { const u = new URL(url); return (u.pathname + u.search).slice(0, 80); } catch (_) { return String(url).slice(0, 80); }
  };
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
    const labelled = el.getAttribute('aria-labelledby');
    const fromLabel = labelled ? (document.getElementById(labelled) || {}).innerText : '';
    return clean(el.getAttribute('aria-label') || fromLabel || el.getAttribute('placeholder') || el.getAttribute('alt') || el.getAttribute('title') || el.innerText || el.value || '').slice(0, 60);
  };
  const extraOf = (el) => {
    const parts = [];
    if (el.tagName === 'A' && el.getAttribute('href')) parts.push('href=' + shortUrl(el.href));
    if (el.tagName === 'INPUT') {
      const t = (el.type || 'text').toLowerCase();
      if (t !== 'text') parts.push('type=' + t);
      if (el.name) parts.push('name=' + clean(el.name).slice(0, 24));
      if (el.value && t !== 'password') parts.push('value=' + clean(el.value).slice(0, 30));
      if (el.checked) parts.push('checked');
      if (t === 'file') parts.push('accept=' + clean(el.getAttribute('accept') || '*').slice(0, 30));
    }
    if (el.tagName === 'TEXTAREA' && el.name) parts.push('name=' + clean(el.name).slice(0, 24));
    if (el.disabled) parts.push('disabled');
    return parts.join(' ');
  };
  const visible = (el) => {
    if (!el.getClientRects().length) return false;
    const style = getComputedStyle(el);
    return style.visibility !== 'hidden' && style.display !== 'none';
  };
  const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role],[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';

  const collectRoots = () => {
    const roots = [];
    let frameSeq = 0;
    const seen = new Set();
    const visit = (root, frame) => {
      if (!root || seen.has(root)) return;
      seen.add(root);
      roots.push({ root, frame });
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) visit(el.shadowRoot, frame);
      }
      for (const frameEl of root.querySelectorAll('iframe,frame')) {
        let doc = null;
        try { doc = frameEl.contentDocument; } catch (_) { /* cross-origin */ }
        if (doc) visit(doc, frame ? frame + '.f' + (++frameSeq) : 'f' + (++frameSeq));
      }
    };
    visit(document, '');
    return roots;
  };

  const refOf = (el) => {
    let ref = el.getAttribute('data-ema2-ref');
    if (!ref) {
      const seq = (window.__ema2_ref_seq || 0) + 1;
      window.__ema2_ref_seq = seq;
      ref = 'e' + seq;
      el.setAttribute('data-ema2-ref', ref);
    }
    return ref;
  };

  const find = (ref, selector, target) => {
    const roots = collectRoots();
    if (ref) {
      const wanted = String(ref).replace(/"/g, '');
      for (const { root } of roots) {
        const el = root.querySelector('[data-ema2-ref="' + wanted + '"]');
        if (el) return el;
      }
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
            if (roleOf(el).toLowerCase() !== role) continue;
            candidates.push(el);
          }
        }
        const exact = candidates.find(el => nameOf(el).toLowerCase() === name);
        if (exact) return exact;
        const partial = candidates.find(el => nameOf(el).toLowerCase().includes(name));
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

  /** snapshot 过滤器：links / buttons / form。 */
  const filterFor = (name) => {
    if (name === 'links') return (el) => el.tagName === 'A' && !!el.getAttribute('href');
    if (name === 'buttons') return (el) => roleOf(el) === 'button';
    if (name === 'form') {
      return (el) => ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON', 'LABEL', 'FIELDSET'].includes(el.tagName)
        || roleOf(el) === 'combobox' || roleOf(el) === 'textbox' || roleOf(el) === 'checkbox' || roleOf(el) === 'radio';
    }
    return null;
  };

  const snapshot = (filter, budget, cursor, nearRef) => {
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
        refs++;
        const extra = extraOf(el);
        lines.push(describe(el) + (extra ? ' ' + extra : '') + (frame ? ' frame=' + frame : ''));
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
    window.__ema2_snapshot_url = location.href;
    return {
      header: 'url=' + location.href + '\ntitle=' + clean(document.title)
        + '\nscroll=' + Math.round(window.scrollY) + '/'
        + Math.max(0, document.documentElement.scrollHeight - window.innerHeight)
        + ' refs=' + refs + ' roots=' + collectRoots().length
        + ' visual=' + document.querySelectorAll('canvas,video,img[width],img[height]').length
        + (container ? ' near=' + (container.getAttribute('data-ema2-ref') || container.tagName.toLowerCase()) : ''),
      body: out.join('\n'),
      total,
      shown: out.length,
      next: index < total ? index : null,
    };
  };

  /** 正文容器启发式：取文本最多、链接密度最低的候选。 */
  const mainRoot = () => {
    const selectors = ['main', '[role="main"]', 'article', '#content', '#main', '.content', '.main',
      '.article', '.post', '.markdown-body', '.rich_media_content', '.article-content', '.post-content'];
    let best = null;
    let bestScore = 0;
    for (const selector of selectors) {
      for (const el of document.querySelectorAll(selector)) {
        const text = el.innerText || '';
        if (text.length < 200) continue;
        const linkLength = [...el.querySelectorAll('a')].reduce((sum, link) => sum + (link.innerText || '').length, 0);
        const score = text.length - linkLength * 1.5;
        if (score > bestScore) { bestScore = score; best = el; }
      }
    }
    if (best) return { root: best, selector: best.tagName.toLowerCase() + (best.id ? '#' + best.id : '') };
    return { root: document.body || document.documentElement, selector: 'body' };
  };

  const markdown = (root) => {
    const SKIP = /^(SCRIPT|STYLE|NOSCRIPT|IFRAME|SVG|CANVAS|VIDEO|AUDIO|TEMPLATE|NAV|HEADER|FOOTER|ASIDE|FORM)$/;
    const BLOCK = /^(DIV|SECTION|ARTICLE|P|UL|OL|LI|TABLE|THEAD|TBODY|TR|TD|TH|H1|H2|H3|H4|H5|H6|PRE|BLOCKQUOTE|DL|DT|DD|FIGURE|FIGCAPTION|MAIN|HEADER|FOOTER|ASIDE|FORM|NAV)$/;
    const out = [];
    const walk = (element) => {
      for (const child of element.children) {
        const tag = child.tagName;
        if (SKIP.test(tag)) continue;
        if (/^H[1-4]$/.test(tag)) {
          const text = clean(child.innerText);
          if (text) out.push('#'.repeat(Number(tag[1])) + ' ' + text);
          continue;
        }
        if (tag === 'PRE') {
          const code = (child.innerText || '').replace(/^\n+/, '').replace(/\s+$/, '');
          if (code.trim()) out.push('```\n' + code + '\n```');
          continue;
        }
        if (tag === 'UL' || tag === 'OL') {
          for (const li of child.children) {
            if (li.tagName !== 'LI') continue;
            const text = clean(li.innerText);
            if (text) out.push('- ' + text);
          }
          continue;
        }
        if (tag === 'TABLE') {
          const rows = (child.innerText || '').split('\n').map(clean).filter(Boolean);
          if (rows.length) out.push(rows.join(' | '));
          continue;
        }
        const hasBlock = [...child.children].some(grand => BLOCK.test(grand.tagName));
        if (!hasBlock) {
          const text = clean(child.innerText);
          if (text) out.push(text);
          continue;
        }
        walk(child);
      }
    };
    walk(root);
    return out.join('\n\n');
  };
  const plainText = (root) => (root.innerText || '').split('\n').map(clean).filter(Boolean).join('\n');

  /** 当前视口内的 ref 标记（用于截图标注，set-of-marks）。 */
  const marks = () => {
    const out = [];
    for (const { root } of collectRoots()) {
      for (const el of root.querySelectorAll('[data-ema2-ref]')) {
        const rect = el.getBoundingClientRect();
        if (rect.width < 2 || rect.height < 2) continue;
        if (rect.bottom < 0 || rect.right < 0 || rect.top > innerHeight || rect.left > innerWidth) continue;
        out.push({
          ref: el.getAttribute('data-ema2-ref'),
          role: roleOf(el),
          name: nameOf(el).slice(0, 30),
          x: Math.round(rect.left),
          y: Math.round(rect.top),
          w: Math.round(rect.width),
          h: Math.round(rect.height),
        });
      }
    }
    return out;
  };

  window.__ema2_helpers = {
    snapshot, markdown, plainText, mainRoot, find, refOf, clean, candidates,
    collectRoots, roleOf, nameOf, marks,
  };
  return true;
}

async function ensureHelpers(tabId) {
  return runInTab(tabId, installHelpers, []);
}

/** 页面内：安装 console 采集钩子（MAIN world）。 */
function installConsoleHook() {
  if (window.__ema2_console) return true;
  window.__ema2_console = [];
  window.__ema2_console_seq = 0;
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

/** 页面内：记录 fetch/XHR 的真实状态码与响应体（MAIN world）。 */
function installNetworkHook() {
  if (window.__ema2_net) return true;
  const state = { seq: 0, entries: [] };
  window.__ema2_net = state;
  const push = (entry) => {
    entry.seq = ++state.seq;
    state.entries.push(entry);
    if (state.entries.length > 300) state.entries.splice(0, 100);
  };
  // 安装前的请求无法拿到状态码/响应体，用 performance entries 回填，至少能看到发生过什么。
  try {
    for (const entry of performance.getEntriesByType('resource').slice(-100)) {
      push({
        type: 'resource', method: 'GET', url: entry.name,
        status: -1, ms: Math.round(entry.duration || 0), body: '',
      });
    }
  } catch (_) { /* ignore */ }
  const preview = (text) => String(text == null ? '' : text).slice(0, 800);
  const originalFetch = window.fetch;
  window.fetch = async function (...args) {
    const start = performance.now();
    const input = args[0];
    const init = args[1] || {};
    let url = '';
    try { url = typeof input === 'string' ? input : (input && input.url) || String(input); } catch (_) { url = ''; }
    const method = String(init.method || (input && input.method) || 'GET').toUpperCase();
    try {
      const response = await originalFetch.apply(this, args);
      let body = '';
      try { body = preview(await response.clone().text()); } catch (_) { /* binary */ }
      push({ type: 'fetch', method, url, status: response.status, ms: Math.round(performance.now() - start), body });
      return response;
    } catch (error) {
      push({ type: 'fetch', method, url, status: 0, ms: Math.round(performance.now() - start), body: 'ERR ' + (error && error.message) });
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
    this.addEventListener('loadend', () => {
      let body = '';
      try {
        if (!this.responseType || this.responseType === 'text') body = preview(this.responseText);
      } catch (_) { /* ignore */ }
      push({
        type: 'xhr',
        method: meta.method || 'GET',
        url: this.responseURL || meta.url || '',
        status: this.status || 0,
        ms: Math.round(performance.now() - (meta.start || performance.now())),
        body,
      });
    });
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

// ---------------------------------------------------------------- act

async function actTool(args) {
  const tab = await resolveTab(args);
  const batch = Array.isArray(args.steps) && args.steps.length > 0;
  const steps = batch ? args.steps : [args];
  if (steps.length > 20) fail('too_many_steps', 'at most 20 steps', 'split the batch');
  const before = await pageState(tab.id);
  // 动作前装 hook，并记录网络序号基线（waitFor 只看这之后的新请求）。
  try { await ensureNetworkHook(tab.id); } catch (_) { /* non-http page */ }
  const netBaseline = await runInTab(tab.id, () => (window.__ema2_net ? window.__ema2_net.seq : 0), [], 'MAIN');
  try { await ensureConsoleHook(tab.id); } catch (_) { /* ignore */ }
  const results = [];

  for (let index = 0; index < steps.length; index++) {
    const step = steps[index];
    if (!step || !step.action) fail('invalid_step', `steps[${index}] has no action`, 'each step needs action');
    try {
      await ensureHelpers(tab.id);
      const info = await runStepWithRetry(tab, step);
      results.push(`step${index + 1} ok=${step.action}${info ? ` ${info}` : ''}`);
    } catch (error) {
      if (error instanceof ToolError && batch) {
        error.message = `steps[${index + 1}] ${step.action}: ${error.message}`;
      }
      throw error;
    }
    await sleep(step.action === 'click' || step.action === 'submit' ? 350 : 150);
  }

  let waitInfo = '';
  if (args.waitFor) {
    waitInfo = await waitForResponse(tab.id, args.waitFor, netBaseline);
  }
  try { await ensureNetworkHook(tab.id); } catch (_) { /* non-http page */ }
  const after = await pageState(tab.id);
  const changed = [];
  if (before.url !== after.url) changed.push('url');
  if (before.title !== after.title) changed.push('title');
  const lines = [...results, `ok=${results.length}/${steps.length}`];
  if (before.url !== after.url && after.url) lines.push(`url=${after.url}`);
  if (after.title) lines.push(`title=${after.title}`);
  if (waitInfo) lines.push(waitInfo);
  if (changed.length) lines.push(`changed=${changed.join(',')}`);
  const newRefs = await newRefsSince(tab.id, before.refSeq);
  if (newRefs.length) lines.push('new=' + newRefs.join(' | '));
  const consoleErrors = await consoleErrorCount(tab.id);
  if (consoleErrors > 0) lines.push(`console_errors=${consoleErrors}`);
  return lines.join('\n');
}

function shortPath(url) {
  try {
    const parsed = new URL(url);
    return (parsed.pathname + parsed.search).slice(0, 100);
  } catch (_) {
    return String(url).slice(0, 100);
  }
}

/** 等某个响应出现（url 子串 + 可选状态码），命中返回一行摘要。 */
async function waitForResponse(tabId, spec, baselineOverride) {
  await ensureNetworkHook(tabId);
  const wantUrl = spec.url ? String(spec.url).toLowerCase() : '';
  const wantStatus = typeof spec.status === 'number' ? spec.status : null;
  const timeoutMs = Math.round((typeof spec.timeout === 'number' ? spec.timeout : 10) * 1000);
  const current = await runInTab(tabId, () => (window.__ema2_net ? window.__ema2_net.seq : 0), [], 'MAIN');
  const baseline = typeof baselineOverride === 'number'
    ? Math.min(baselineOverride, current)
    : current;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const hit = await runInTab(tabId, (since, url, status) => {
      const state = window.__ema2_net;
      if (!state) return null;
      for (const entry of state.entries) {
        if (entry.seq <= since) continue;
        if (url && !String(entry.url).toLowerCase().includes(url)) continue;
        if (status != null && entry.status !== status) continue;
        return { method: entry.method, status: entry.status, url: entry.url, ms: entry.ms };
      }
      return null;
    }, [baseline, wantUrl, wantStatus], 'MAIN');
    if (hit) return `response=${hit.status} ${hit.method} ${shortPath(hit.url)} ${hit.ms}ms`;
    await sleep(120);
  }
  fail('response_timeout',
    `no response matching ${wantUrl || '*'} ${wantStatus == null ? '' : wantStatus} within ${timeoutMs / 1000}s`,
    'call observe network to inspect requests, or raise waitFor.timeout');
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
      refSeq: window.__ema2_ref_seq || 0,
    }), []);
  } catch (_) {
    return { url: '', title: '', refSeq: 0 };
  }
}

async function newRefsSince(tabId, seq) {
  try {
    return await runInTab(tabId, (since) => {
      const out = [];
      document.querySelectorAll('[data-ema2-ref]').forEach((el) => {
        const n = Number(String(el.getAttribute('data-ema2-ref')).slice(1));
        if (Number.isFinite(n) && n > since) {
          out.push('[' + el.getAttribute('data-ema2-ref') + '] ' + el.tagName.toLowerCase() + ' "' + (el.innerText || el.getAttribute('aria-label') || '').replace(/\s+/g, ' ').trim().slice(0, 50) + '"');
        }
      });
      return out.slice(0, 12);
    }, [seq]);
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

async function runStep(tab, step) {
  const action = step.action;
  if (action === 'back' || action === 'forward' || action === 'reload') {
    await runInTab(tab.id, (kind) => {
      if (kind === 'back') history.back();
      else if (kind === 'forward') history.forward();
      else location.reload();
      return true;
    }, [action]);
    return '';
  }
  if (action === 'scroll') {
    const info = await runInTab(tab.id, (delta) => {
      const before = window.scrollY;
      const dy = typeof delta === 'number' ? delta : Math.round(window.innerHeight * 0.8);
      if (dy !== 0) window.scrollBy(0, dy);
      return 'scroll=' + before + '->' + window.scrollY;
    }, [typeof step.deltaY === 'number' ? step.deltaY : null]);
    return info || '';
  }

  await ensureHelpers(tab.id);
  if (action === 'fill') {
    const fields = step.fields || [];
    if (!Array.isArray(fields) || !fields.length) fail('missing_fields', 'fill requires fields[]', 'pass [{ref|selector,value}]');
    const result = await runInTab(tab.id, (items) => {
      const helpers = window.__ema2_helpers;
      const out = [];
      for (const item of items) {
        const el = helpers.find(item.ref || null, item.selector || null, item.target || null);
        if (!el) { out.push({ ok: false, target: item.ref || item.selector || item.target }); continue; }
        const setter = el instanceof HTMLInputElement ? Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set
          : el instanceof HTMLTextAreaElement ? Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set
          : null;
        if (el.isContentEditable) el.textContent = item.value;
        else if (setter) setter.call(el, item.value);
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        out.push({ ok: true, target: item.ref || item.selector || item.target });
      }
      return out;
    }, [fields]);
    const failed = result.filter(item => !item.ok);
    if (failed.length) {
      fail('target_not_found', 'some fill targets were not found: ' + failed.map(f => f.target).join(','), 'call snapshot and use fresh refs');
    }
    return `fields=${result.length}`;
  }

  const resolved = await runInTab(tab.id, (ref, selector, targetSpec) => {
    const helpers = window.__ema2_helpers;
    const el = helpers.find(ref, selector, targetSpec);
    if (!el) {
      return {
        found: false,
        stale: !!ref && !!window.__ema2_snapshot_url && window.__ema2_snapshot_url !== location.href,
        hadSnapshot: !!window.__ema2_snapshot_url,
        candidates: helpers.candidates(5),
      };
    }
    el.scrollIntoView({ block: 'center', inline: 'center' });
    return { found: true };
  }, [step.ref || null, step.selector || null, step.target || null]);

  if (!resolved.found) {
    if (resolved.stale) fail('stale_ref', `ref ${step.ref} is stale (page navigated)`, 'call snapshot again');
    if (step.ref && resolved.hadSnapshot) {
      fail('ref_not_found', `ref ${step.ref} is not in the current page`, 'refs are page-scoped; call snapshot again', resolved.candidates);
    }
    fail('target_not_found', `target not found: ${step.ref || step.selector || step.target || '(none)'}`, 'use snapshot refs, or target="role:name"', resolved.candidates);
  }

  switch (action) {
    case 'click': {
      await runInTab(tab.id, (ref, selector, targetSpec) => {
        const helpers = window.__ema2_helpers;
        const el = helpers.find(ref, selector, targetSpec);
        if (!el) return false;
        el.click();
        return true;
      }, [step.ref || null, step.selector || null, step.target || null]);
      return step.ref ? `ref=${step.ref}` : step.target ? `target=${step.target}` : `selector=${step.selector}`;
    }
    case 'type': {
      await runInTab(tab.id, (ref, selector, targetSpec, text, submit) => {
        const helpers = window.__ema2_helpers;
        const el = helpers.find(ref, selector, targetSpec);
        if (!el) return false;
        el.focus();
        if (el.isContentEditable) el.textContent = text;
        else {
          const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
            : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype
            : HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(proto, 'value') && Object.getOwnPropertyDescriptor(proto, 'value').set;
          if (setter) setter.call(el, text);
        }
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        if (submit) {
          el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));
          const form = el.closest('form');
          if (form && form.requestSubmit) form.requestSubmit();
        }
        return true;
      }, [step.ref || null, step.selector || null, step.target || null, step.text === undefined ? '' : String(step.text), !!step.submit]);
      return `text=${JSON.stringify(step.text === undefined ? '' : String(step.text)).slice(0, 60)}`;
    }
    case 'select': {
      await runInTab(tab.id, (ref, selector, targetSpec, value) => {
        const helpers = window.__ema2_helpers;
        const el = helpers.find(ref, selector, targetSpec);
        if (!el) return false;
        el.value = value;
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      }, [step.ref || null, step.selector || null, step.target || null, step.value === undefined ? '' : String(step.value)]);
      return `value=${String(step.value || '').slice(0, 40)}`;
    }
    case 'press': {
      const key = String(step.key || '');
      await runInTab(tab.id, (keyName) => {
        const el = document.activeElement || document.body;
        const opts = { key: keyName, code: keyName, bubbles: true, cancelable: true };
        el.dispatchEvent(new KeyboardEvent('keydown', opts));
        el.dispatchEvent(new KeyboardEvent('keyup', opts));
        return true;
      }, [key]);
      return `key=${key}`;
    }
    case 'hover': {
      await runInTab(tab.id, (ref, selector, targetSpec) => {
        const helpers = window.__ema2_helpers;
        const el = helpers.find(ref, selector, targetSpec);
        if (!el) return false;
        const rect = el.getBoundingClientRect();
        const opts = { bubbles: true, clientX: rect.x + rect.width / 2, clientY: rect.y + rect.height / 2 };
        el.dispatchEvent(new MouseEvent('mouseover', opts));
        el.dispatchEvent(new MouseEvent('mouseenter', opts));
        return true;
      }, [step.ref || null, step.selector || null, step.target || null]);
      return '';
    }
    case 'submit': {
      await runInTab(tab.id, (ref, selector, targetSpec) => {
        const helpers = window.__ema2_helpers;
        const el = helpers.find(ref, selector, targetSpec) || document.activeElement;
        const form = el && el.closest ? el.closest('form') : null;
        if (form && form.requestSubmit) { form.requestSubmit(); return true; }
        if (el) {
          el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));
          return true;
        }
        return false;
      }, [step.ref || null, step.selector || null, step.target || null]);
      return '';
    }
    default:
      fail('invalid_action', `unsupported action: ${action}`, 'use click|type|fill|select|press|hover|scroll|submit|back|forward|reload');
  }
}

/** 在 MAIN world 里按 ref / selector / "role:name" 深度查找（含 shadow root / 同源 iframe）。 */
function deepFindForDebugger(ref, selector, target) {
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
  const nameOf = (el) => clean(el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.getAttribute('alt') || el.getAttribute('title') || el.innerText || el.value || '').slice(0, 60);
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
    const wanted = String(ref).replace(/"/g, '');
    for (const root of roots) {
      const el = root.querySelector('[data-ema2-ref="' + wanted + '"]');
      if (el) return el;
    }
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
      let partial = null;
      for (const root of roots) {
        for (const el of root.querySelectorAll(INTERACTIVE)) {
          if (roleOf(el).toLowerCase() !== role) continue;
          const current = nameOf(el).toLowerCase();
          if (current === name) return el;
          if (!partial && current.includes(name)) partial = el;
        }
      }
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
    const paths = Array.isArray(args.paths) ? args.paths.filter(p => typeof p === 'string' && p.trim()) : [];
    if (!paths.length) fail('missing_files', 'upload requires paths[]', 'pass absolute file paths');
    if (paths.length > 20) fail('too_many_files', 'at most 20 paths per upload', 'split into multiple calls');
    const os = await currentOs();
    const bad = paths.find(p => !looksAbsolute(p, os));
    if (bad) {
      fail('invalid_path', `not an absolute path for ${os}: ${bad}`,
        os === 'win' ? 'use a Windows path like C:/dir/file.pdf' : 'use a path starting with /');
    }
    if (!args.ref && !args.selector && !args.target) {
      fail('missing_target', 'upload requires ref, selector or target', 'call snapshot and pass the file input ref');
    }
    await ensureHelpers(tab.id);
    return withDebugger(tab.id, async () => {
      await chrome.debugger.sendCommand({ tabId: tab.id }, 'DOM.enable');
      const expression = '(' + deepFindForDebugger.toString() + ')('
        + [args.ref || null, args.selector || null, args.target || null].map(v => JSON.stringify(v)).join(',')
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
      return `uploaded=${paths.length}\nfiles=${paths.map(p => p.split(/[\\/]/).pop()).join(',')}`;
    });
  }
  if (action === 'download-dir') {
    const dir = String(args.dir || '').trim();
    if (!dir) fail('missing_dir', 'download-dir requires dir', 'pass an absolute directory path');
    return withDebugger(tab.id, async () => {
      try {
        await chrome.debugger.sendCommand({ tabId: tab.id }, 'Page.setDownloadBehavior', {
          behavior: 'allow', downloadPath: dir,
        });
        return `download_dir=${dir}`;
      } catch (_) { /* deprecated, fall through */ }
      try {
        await chrome.debugger.sendCommand({ tabId: tab.id }, 'Browser.setDownloadBehavior', {
          behavior: 'allow', downloadPath: dir, eventsEnabled: false,
        });
        return `download_dir=${dir}`;
      } catch (_) { /* page targets don't expose the Browser domain */ }
      fail('download_dir_unsupported', 'this browser build does not allow an extension to change the download directory', 'files will land in the default Downloads folder; use chrome.downloads or set it manually');
    });
  }
  fail('invalid_action', `unknown files action: ${action}`, 'use upload|download-dir');
}

function withDebugger(tabId, run) {
  const task = debuggerQueue.then(async () => {
    try {
      await chrome.debugger.attach({ tabId }, '1.3');
    } catch (error) {
      throw new ToolError('debugger_attach_failed', `cannot attach debugger to tab ${tabId}: ${error.message}`, 'close DevTools on that tab and retry');
    }
    try { return await run(); }
    finally { try { await chrome.debugger.detach({ tabId }); } catch (_) {} }
  });
  debuggerQueue = task.then(() => {}, () => {});
  return task;
}

async function jsTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot run JS', 'open an http(s) page');
  return withDebugger(tab.id, async () => {
    const result = await chrome.debugger.sendCommand({ tabId: tab.id }, 'Runtime.evaluate', {
      expression: args.code,
      returnByValue: true,
      awaitPromise: true,
    });
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      const message = details.exception && details.exception.description || details.text || 'JS error';
      fail('js_exception', oneLine(message).slice(0, 300), 'fix the expression; read the page with snapshot/read first');
    }
    const value = result.result ? result.result.value : undefined;
    let text;
    if (typeof value === 'string') text = value;
    else {
      try { text = JSON.stringify(value); } catch (_) { text = String(value); }
    }
    const budget = normalizeBudget(args.budget === undefined ? 8000 : args.budget);
    const slice = sliceText(text == null ? 'null' : text, budget, 0);
    return slice.text + (slice.next == null ? '' : '\n' + footer(slice.total, slice.text.length, slice.next));
  });
}

function buildHar(items, pageUrl) {
  const now = new Date().toISOString();
  const entries = items.map(entry => ({
    startedDateTime: now,
    time: entry.ms || 0,
    request: {
      method: entry.method || 'GET',
      url: entry.url,
      httpVersion: 'HTTP/1.1',
      headers: [],
      queryString: [],
      cookies: [],
      headersSize: -1,
      bodySize: -1,
    },
    response: {
      status: entry.status,
      statusText: '',
      httpVersion: 'HTTP/1.1',
      headers: [],
      cookies: [],
      content: { size: (entry.body || '').length, mimeType: 'text/plain', text: entry.body || '' },
      redirectURL: '',
      headersSize: -1,
      bodySize: -1,
    },
    cache: {},
    timings: { send: 0, wait: entry.ms || 0, receive: 0 },
    _resourceType: entry.type || 'fetch',
  }));
  return {
    log: {
      version: '1.2',
      creator: { name: 'EMA2', version: chrome.runtime.getManifest().version },
      pages: [{ startedDateTime: now, id: 'page', title: '', pageTimings: {} }],
      entries,
    },
  };
}

/** 在多个标签页上并发执行 read/snapshot/js/wait，返回每页一块结果。 */
async function batchTool(args) {
  const allowed = ['read', 'snapshot', 'js', 'wait'];
  const toolName = args.tool;
  if (!allowed.includes(toolName)) {
    fail('invalid_tool', `batch cannot run ${toolName}`, 'use read|snapshot|js|wait');
  }
  const tabIds = Array.isArray(args.tabIds) ? args.tabIds : [];
  const inner = args.args && typeof args.args === 'object' ? args.args : {};
  const concurrency = Math.max(1, Math.min(4, Math.round(args.concurrency || 2)));
  const results = new Array(tabIds.length);
  let cursor = 0;
  async function worker() {
    for (;;) {
      const index = cursor++;
      if (index >= tabIds.length) return;
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
    const label = String(mark.ref).replace(/^e/, '');
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

async function shotTool(args) {
  const tab = await resolveTab(args);
  const format = args.format === 'jpeg' ? 'jpeg' : 'png';
  let marks = [];
  let marksText = '';
  if (args.annotate) {
    await ensureHelpers(tab.id);
    marks = await runInTab(tab.id, () => {
      const helpers = window.__ema2_helpers;
      helpers.snapshot('interactive', 20000, 0);
      return helpers.marks();
    }, []);
    marksText = marks.map(mark => `[${mark.ref}] ${mark.role} "${mark.name}" @${mark.x},${mark.y} ${mark.w}x${mark.h}`).join('\n');
  }
  let dataUrl;
  try {
    dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format });
  } catch (error) {
    fail('capture_failed', `captureVisibleTab failed: ${error.message}`, 'make the browser window visible/focused and retry');
  }
  const match = new RegExp('^data:image/(' + format + ');base64,(.+)$', 's').exec(dataUrl || '');
  if (!match) fail('capture_empty', 'screenshot payload was empty', 'retry');
  if (!args.annotate) {
    const mime = 'image/' + (format === 'png' ? 'png' : 'jpeg');
    return { content: [{ type: 'image', data: match[2], mimeType: mime }] };
  }
  const dpr = await runInTab(tab.id, () => window.devicePixelRatio || 1, []);
  const annotated = await annotateScreenshot(dataUrl, marks, dpr, format);
  const content = [];
  if (marks.length) content.push({ type: 'text', text: marksText + '\nmarks=' + marks.length });
  content.push({ type: 'image', data: annotated.base64, mimeType: annotated.mime });
  return { content };
}

async function observeTool(args) {
  const tab = await resolveTab(args);
  const kind = args.kind;
  const since = normalizeCursor(args.since);
  const budget = normalizeBudget(args.budget === undefined ? 3000 : args.budget);

  if (kind === 'console') {
    await ensureConsoleHook(tab.id);
    const data = await runInTab(tab.id, (level, sinceValue) => {
      const min = { log: 1, warn: 3, error: 4 }[level] || 4;
      const entries = (window.__ema2_console || [])
        .filter(e => e.seq > sinceValue && (e.order || 1) >= min);
      return { entries, next: window.__ema2_console_seq || 0 };
    }, [args.level || 'error', since], 'MAIN');
    const lines = data.entries.map(e => `#${e.seq} ${e.level} ${oneLine(e.text).slice(0, 240)}`);
    const slice = sliceText(lines.join('\n'), budget, 0);
    const shown = slice.text ? slice.text.split('\n').length : 0;
    return [slice.text, footer(lines.length, shown, data.next)].filter(Boolean).join('\n');
  }

  if (kind === 'network') {
    await ensureNetworkHook(tab.id);
    if (args.wait) await sleep(Math.min(10000, Math.round(args.wait * 1000)));
    const withBodies = args.bodies !== false || !!args.har;
    let bodyRegex = null;
    if (args.bodyFilter) {
      try { bodyRegex = new RegExp(String(args.bodyFilter), 'i'); }
      catch (_) { fail('invalid_regex', `bodyFilter is not a valid regex: ${args.bodyFilter}`, 'fix the regex'); }
    }
    const data = await runInTab(tab.id, (sinceValue, filterValue, wantBodies) => {
      const state = window.__ema2_net || { entries: [], seq: 0 };
      const filterText = filterValue ? String(filterValue).toLowerCase() : '';
      const list = state.entries.filter(entry =>
        entry.seq > sinceValue && (!filterText || String(entry.url).toLowerCase().includes(filterText)));
      return {
        next: state.seq,
        total: list.length,
        items: list.slice(-80).map(entry => ({
          seq: entry.seq, type: entry.type, method: entry.method, url: entry.url,
          status: entry.status, ms: entry.ms,
          body: wantBodies ? entry.body : '',
        })),
      };
    }, [since, args.filter || null, withBodies], 'MAIN');
    let items = data.items;
    if (bodyRegex) items = items.filter(entry => bodyRegex.test(entry.body || ''));
    if (args.har) {
      const har = buildHar(items, tab.url);
      return JSON.stringify(har);
    }
    const lines = items.map(entry => {
      let path = entry.url;
      try {
        const parsed = new URL(entry.url);
        path = (parsed.pathname + parsed.search).slice(0, 100);
      } catch (_) { /* keep raw */ }
      const body = entry.body ? ' body=' + oneLine(entry.body).slice(0, 200) : '';
      return `#${entry.seq} ${entry.method} ${entry.status} ${entry.ms}ms ${path}${body}`;
    });
    const slice = sliceText(lines.join('\n'), budget, 0);
    const shown = slice.text ? slice.text.split('\n').length : 0;
    return [slice.text, footer(lines.length, shown, data.next)].filter(Boolean).join('\n');
  }

}

// ---------------------------------------------------------------- read / snapshot

async function readTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be read', 'open an http(s) page');
  await ensureHelpers(tab.id);
  const mode = args.mode || 'md';
  const selector = args.selector;
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  const useMain = args.main !== false && !selector;
  const data = await runInTab(tab.id, (modeName, selectorArg, mainWanted) => {
    const helpers = window.__ema2_helpers;
    let root = document.body || document.documentElement;
    let mainSelector = '';
    if (selectorArg) {
      const found = helpers.find(null, selectorArg, null);
      if (!found) return { missing: true };
      root = found;
    } else if (mainWanted) {
      const picked = helpers.mainRoot();
      root = picked.root;
      mainSelector = picked.selector;
    }
    const text = modeName === 'html' ? root.innerHTML
      : modeName === 'text' ? helpers.plainText(root)
      : helpers.markdown(root);
    return {
      text,
      header: 'url=' + location.href + '\ntitle=' + helpers.clean(document.title)
        + (mainSelector ? '\nmain=' + mainSelector : ''),
    };
  }, [mode, selector || null, useMain]);
  if (data.missing) {
    fail('selector_not_found', `selector not found: ${selector}`, 'call snapshot to see available refs/roles');
  }
  const slice = sliceText(data.text || '', budget, cursor);
  return [data.header, slice.text, footer(slice.total, slice.text.length, slice.next)].join('\n');
}

async function snapshotTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be snapshotted', 'open an http(s) page');
  await ensureHelpers(tab.id);
  const filter = args.filter || 'interactive';
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  const data = await runInTab(tab.id, (filterName, budgetValue, cursorValue, nearRef) => {
    return window.__ema2_helpers.snapshot(filterName, budgetValue, cursorValue, nearRef);
  }, [filter, budget, cursor, args.near || null]);
  if (!data) fail('snapshot_failed', 'snapshot produced no data', 'retry');
  const parts = [data.header];
  if (data.body) parts.push(data.body);
  parts.push(footer(data.total, data.shown, data.next));
  const refs = Number((/refs=(\d+)/.exec(data.header) || [])[1] || 0);
  const visual = Number((/visual=(\d+)/.exec(data.header) || [])[1] || 0);
  if (refs === 0 && visual > 0) {
    parts.push('hint=page looks visual (canvas/img); try shot annotate=true, or read the DOM with js');
  }
  return parts.join('\n');
}

// ---------------------------------------------------------------- dispatch

function requireStringArg(args, key, max, allowEmpty) {
  if (args[key] === undefined) return undefined;
  if (typeof args[key] !== 'string' || args[key].length > max || (!allowEmpty && !args[key].trim())) {
    fail('invalid_argument', `${key} must be a string (max ${max})`, 'see tools/list');
  }
  return args[key];
}

async function executeTool(name, args) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) {
    fail('invalid_arguments', 'tool arguments must be an object', 'see tools/list');
  }
  switch (name) {
    case 'tabs': return tabsTool(args);
    case 'open': {
      const url = requireStringArg(args, 'url', 8192, false);
      if (!url) fail('missing_url', 'url is required', 'pass an http(s) url');
      if (!/^https?:\/\//i.test(url)) fail('invalid_url', 'url must use http or https', 'fix the url');
      return openTool({ ...args, url });
    }
    case 'read': return readTool(args);
    case 'snapshot': return snapshotTool(args);
    case 'act': return actTool(args);
    case 'wait': return waitTool(args);
    case 'js': {
      const code = requireStringArg(args, 'code', 100000, false);
      if (!code) fail('missing_code', 'code is required', 'pass a JS expression');
      return jsTool({ ...args, code });
    }
    case 'observe': return observeTool(args);
    case 'files': return filesTool(args);
    case 'batch': return batchTool(args);
    case 'shot': return shotTool(args);
    default: fail('unknown_tool', `unknown tool: ${name}`, 'call tools/list');
  }
}

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

ensureWatchdog();
connect();
chrome.runtime.onStartup.addListener(() => { ensureWatchdog(); connect(); });
chrome.runtime.onInstalled.addListener(() => { ensureWatchdog(); connect(); });
