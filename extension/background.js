'use strict';

/**
 * EMA2 Edge 扩展后台脚本 (MV3 service worker)
 * -------------------------------------
 * 连接常驻桥接服务，实现 MCP 协议，向 AI 客户端暴露浏览器控制工具。
 *
 * 通信：
 *   桥接服务 <==WebSocket==> 本脚本 <==chrome.* API==> 浏览器
 *
 * WS 地址：
 *   默认 ws://127.0.0.1:8765，可在 chrome.storage.local 中设置 "wsUrl" 覆盖。
 *   修改方式：扩展详情页 -> 服务工作者控制台执行
 *     chrome.storage.local.set({ wsUrl: 'ws://127.0.0.1:9000' })
 */

const DEFAULT_WS_URL = 'ws://127.0.0.1:8765';
const RECONNECT_ALARM = 'ema2-reconnect';
const MAX_MESSAGE_BYTES = 1024 * 1024;
const MAX_RESULT_CHARS = 200000;
const MAX_TEXT_CHARS = 100000;

let socket = null;
let connecting = false;
let retryAttempt = 0;

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

async function getWsUrl() {
  try {
    const s = await chrome.storage.local.get(['wsUrl']);
    return validateWsUrl(s.wsUrl || DEFAULT_WS_URL);
  } catch (_) {
    return validateWsUrl(DEFAULT_WS_URL);
  }
}

async function connect() {
  if (connecting || (socket && (socket.readyState === WebSocket.CONNECTING || socket.readyState === WebSocket.OPEN))) return;
  connecting = true;
  let ws;
  try {
    ws = new WebSocket(await getWsUrl());
    socket = ws;
  } catch (e) {
    connecting = false;
    console.error('[EMA2] WebSocket construct failed:', e.message);
    scheduleReconnect();
    return;
  }
  connecting = false;

  ws.onopen = () => {
    retryAttempt = 0;
    chrome.alarms.clear(RECONNECT_ALARM);
    console.log('[EMA2] connected to bridge');
    ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'bridge/hello', role: 'extension' }));
  };

  ws.onmessage = (event) => handleMCPMessage(event.data, ws);
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    console.log('[EMA2] connection closed, reconnect scheduled');
    scheduleReconnect();
  };
  ws.onerror = () => {};
}

function scheduleReconnect() {
  chrome.alarms.get(RECONNECT_ALARM, alarm => {
    if (alarm) return;
    // Chrome may clamp alarms below 30 seconds in MV3; retain exponential backoff.
    const seconds = Math.min(300, 30 * Math.pow(2, retryAttempt++));
    chrome.alarms.create(RECONNECT_ALARM, { delayInMinutes: seconds / 60 });
  });
}

chrome.alarms.onAlarm.addListener(alarm => {
  if (alarm.name === RECONNECT_ALARM) connect();
});

async function handleMCPMessage(data, ws) {
  if (typeof data !== 'string' || data.length > MAX_MESSAGE_BYTES) return;
  let req;
  try {
    req = JSON.parse(data);
  } catch (e) {
    console.error('[EMA2] non-JSON message:', data);
    return;
  }

  try {
    if (req.method === 'ping') {
      if (Object.prototype.hasOwnProperty.call(req, 'id')) sendResponse(ws, req.id, { result: {} });
      return;
    }
    if (!req.method) return;

    let result = null;
    let error = null;

    switch (req.method) {
      case 'initialize':
        result = {
          protocolVersion: req.params?.protocolVersion || '2024-11-05',
          capabilities: { tools: {} },
          serverInfo: { name: 'edge-browser-mcp', version: '1.0.0' },
        };
        break;

      case 'notifications/initialized':
        return;

      case 'tools/list':
        result = { tools: getToolList() };
        break;

      case 'tools/call':
        try {
          result = await executeTool(req.params.name, req.params.arguments || {});
        } catch (e) {
          result = { content: [{ type: 'text', text: `Tool execution failed: ${e.message}` }], isError: true };
        }
        break;

      default:
        error = { code: -32601, message: `Method not found: ${req.method}` };
    }

    if (Object.prototype.hasOwnProperty.call(req, 'id') && ws.readyState === 1) {
      sendResponse(ws, req.id, error ? { error } : { result });
    }
  } catch (err) {
    console.error('[EMA2] handle message failed:', err);
    if (Object.prototype.hasOwnProperty.call(req, 'id') && ws.readyState === 1) {
      sendResponse(ws, req.id, { error: { code: -32603, message: err.message } });
    }
  }
}

function sendResponse(ws, id, payload) {
  const response = { jsonrpc: '2.0', id };
  if (payload.error) response.error = payload.error;
  else response.result = payload.result;
  if (ws.readyState !== 1) return;
  let encoded = JSON.stringify(response);
  if (encoded.length > MAX_MESSAGE_BYTES) {
    response.result = textResult('Result exceeds the maximum response size');
    encoded = JSON.stringify(response);
  }
  ws.send(encoded);
}

function getToolList() {
  return [
    {
      name: 'edge_get_tabs',
      description: 'List all open tabs in Edge (id, title, url, active)',
      inputSchema: { type: 'object', properties: {} },
    },
    {
      name: 'edge_get_active_tab',
      description: 'Get the active tab url and title',
      inputSchema: { type: 'object', properties: {} },
    },
    {
      name: 'edge_read_active_tab',
      description: 'Extract the plain text content of the active tab (for AI to read the current page)',
      inputSchema: { type: 'object', properties: {} },
    },
    {
      name: 'edge_read_tab',
      description: 'Extract plain text content of a specific tab by id',
      inputSchema: { type: 'object', properties: { tabId: { type: 'number', description: 'Tab id from edge_get_tabs' } }, required: ['tabId'] },
    },
    {
      name: 'edge_execute_js',
      description: 'Execute arbitrary JavaScript in the active tab (via chrome.debugger Runtime.evaluate) and return the serialized result',
      inputSchema: { type: 'object', properties: { code: { type: 'string', description: 'JavaScript expression (async supported via awaitPromise)' } }, required: ['code'] },
    },
    {
      name: 'edge_click_element',
      description: 'Click an element in the active tab by CSS selector',
      inputSchema: { type: 'object', properties: { selector: { type: 'string' } }, required: ['selector'] },
    },
    {
      name: 'edge_new_tab',
      description: 'Open a new tab with the given URL',
      inputSchema: { type: 'object', properties: { url: { type: 'string' } }, required: ['url'] },
    },
    {
      name: 'edge_navigate',
      description: 'Navigate the active tab to a URL',
      inputSchema: { type: 'object', properties: { url: { type: 'string' } }, required: ['url'] },
    },
    {
      name: 'edge_close_tab',
      description: 'Close a tab by id (defaults to the active tab)',
      inputSchema: { type: 'object', properties: { tabId: { type: 'number' } } },
    },
    {
      name: 'edge_wait_for',
      description: 'Wait for text to appear or disappear on the page, or wait a fixed time (default timeout 30s)',
      inputSchema: {
        type: 'object',
        properties: {
          text: { type: 'string', description: 'Wait until this text appears' },
          textGone: { type: 'string', description: 'Wait until this text disappears' },
          time: { type: 'number', description: 'Wait this many seconds instead' },
        },
      },
    },
    {
      name: 'edge_take_screenshot',
      description: 'Take a screenshot of the active tab (saved as PNG; returns file path)',
      inputSchema: { type: 'object', properties: {} },
    },
    {
      name: 'edge_console_messages',
       description: 'Return console messages captured after console capture is initialized on the page',
      inputSchema: {
        type: 'object',
        properties: { level: { type: 'string', enum: ['log', 'warn', 'error'], default: 'error' } },
      },
    },
    {
      name: 'edge_network_requests',
      description: 'List network requests the page has made (from performance entries)',
      inputSchema: {
        type: 'object',
        properties: { filter: { type: 'string', description: 'Only return URLs matching this substring' } },
      },
    },
    {
      name: 'edge_type',
      description: 'Type text into an element by CSS selector (dispatches input/change events)',
      inputSchema: {
        type: 'object',
        properties: {
          selector: { type: 'string' },
          text: { type: 'string' },
          submit: { type: 'boolean', description: 'Press Enter after typing' },
        },
        required: ['selector', 'text'],
      },
    },
    {
      name: 'edge_press_key',
      description: 'Press a key on the currently focused element (e.g. Enter, Escape, ArrowDown)',
      inputSchema: { type: 'object', properties: { key: { type: 'string' } }, required: ['key'] },
    },
    {
      name: 'edge_fill_form',
      description: 'Fill multiple form fields by CSS selector',
      inputSchema: {
        type: 'object',
        properties: {
          fields: {
            type: 'array',
            items: {
              type: 'object',
              properties: { selector: { type: 'string' }, name: { type: 'string' }, value: { type: 'string' } },
              required: ['selector', 'value'],
            },
          },
        },
        required: ['fields'],
      },
    },
    {
      name: 'edge_hover',
      description: 'Hover the mouse over an element by CSS selector',
      inputSchema: { type: 'object', properties: { selector: { type: 'string' } }, required: ['selector'] },
    },
    {
      name: 'edge_scroll',
      description: 'Scroll the page by a delta (default one viewport height)',
      inputSchema: { type: 'object', properties: { deltaY: { type: 'number' } } },
    },
  ];
}

function textResult(text) {
  const value = String(text);
  return { content: [{ type: 'text', text: value.length > MAX_RESULT_CHARS ? value.slice(0, MAX_RESULT_CHARS) + '\n[truncated]' : value }] };
}

function safeJsonStringify(value) {
  const seen = new WeakSet();
  try {
    return JSON.stringify(value, (_, item) => {
      if (typeof item === 'bigint') return `${item}n`;
      if (item instanceof Error) return { name: item.name, message: item.message, stack: item.stack };
      if (item && typeof item === 'object') {
        if (seen.has(item)) return '[Circular]';
        seen.add(item);
      }
      return item;
    }, 2);
  } catch (_) {
    return '[Unserializable value]';
  }
}

function requireString(args, key, max = MAX_TEXT_CHARS, nonEmpty = true) {
  if (typeof args[key] !== 'string' || args[key].length > max || (nonEmpty && !args[key].trim())) {
    throw new Error(`Invalid ${key}`);
  }
  return args[key];
}

function validateToolArgs(name, args) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('Tool arguments must be an object');
  if (['edge_click_element', 'edge_hover'].includes(name)) requireString(args, 'selector', 2048);
  if (name === 'edge_read_tab' && (!Number.isInteger(args.tabId) || args.tabId < 0)) throw new Error('Invalid tabId');
  if (name === 'edge_close_tab' && args.tabId !== undefined && (!Number.isInteger(args.tabId) || args.tabId < 0)) throw new Error('Invalid tabId');
  if (name === 'edge_new_tab' || name === 'edge_navigate') {
    const url = requireString(args, 'url', 8192);
    let parsed;
    try { parsed = new URL(url); } catch (_) { throw new Error('Invalid url'); }
    if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('url must use http or https');
  }
  if (name === 'edge_execute_js') requireString(args, 'code', MAX_TEXT_CHARS);
  if (name === 'edge_press_key') requireString(args, 'key', 100);
  if (name === 'edge_type') { requireString(args, 'selector', 2048); requireString(args, 'text', MAX_TEXT_CHARS, false); }
  if (name === 'edge_wait_for') {
    if (args.text !== undefined) requireString(args, 'text', MAX_TEXT_CHARS);
    if (args.textGone !== undefined) requireString(args, 'textGone', MAX_TEXT_CHARS);
    if (args.text !== undefined && args.textGone !== undefined) throw new Error('Use text or textGone, not both');
    if (args.time !== undefined && (args.text !== undefined || args.textGone !== undefined)) throw new Error('Use time or text condition, not both');
    if (args.time === undefined && args.text === undefined && args.textGone === undefined) throw new Error('Specify time, text, or textGone');
    if (args.time !== undefined && (!Number.isFinite(args.time) || args.time < 0 || args.time > 300)) throw new Error('Invalid time');
  }
  if (name === 'edge_console_messages' && args.level !== undefined && !['log', 'warn', 'error'].includes(args.level)) throw new Error('Invalid level');
  if (name === 'edge_network_requests' && args.filter !== undefined) requireString(args, 'filter', 2048);
  if (name === 'edge_scroll' && args.deltaY !== undefined && (!Number.isFinite(args.deltaY) || Math.abs(args.deltaY) > 100000)) throw new Error('Invalid deltaY');
  if (name === 'edge_fill_form') {
    if (!Array.isArray(args.fields) || args.fields.length > 100) throw new Error('Invalid fields');
    for (const field of args.fields) {
      if (!field || typeof field !== 'object') throw new Error('Invalid field');
      requireString(field, 'selector', 2048);
      requireString(field, 'value', MAX_TEXT_CHARS, false);
      if (field.name !== undefined) requireString(field, 'name', 200, false);
    }
  }
}

async function executeTool(name, args) {
  validateToolArgs(name, args);
  switch (name) {
    case 'edge_get_tabs': {
      const tabs = await chrome.tabs.query({});
        return textResult(safeJsonStringify(tabs.map(t => ({ id: t.id, title: t.title, url: t.url, active: t.active }))));
    }

    case 'edge_get_active_tab': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      return textResult(safeJsonStringify({ id: tab.id, title: tab.title, url: tab.url, active: tab.active }));
    }

    case 'edge_read_active_tab':
    case 'edge_read_tab': {
      let tabId = args.tabId;
      if (tabId === undefined) {
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        tabId = tab && tab.id;
      }
      if (tabId === undefined) throw new Error('No active tab');
      return textResult(await extractTabText(tabId));
    }

    case 'edge_execute_js': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      if (/^(edge|chrome|about|devtools|chrome-extension):/.test(tab.url)) {
        throw new Error('Browser internal pages cannot run arbitrary JS');
      }
      await chrome.debugger.attach({ tabId: tab.id }, '1.3');
      try {
        const res = await chrome.debugger.sendCommand({ tabId: tab.id }, 'Runtime.evaluate', {
          expression: args.code || '',
          returnByValue: true,
          awaitPromise: true,
        });
        if (res.exceptionDetails) {
          throw new Error(res.exceptionDetails.exception?.description || res.exceptionDetails.text || 'JS error');
        }
        return textResult(safeJsonStringify(res.result?.value ?? null));
      } finally {
        try { await chrome.debugger.detach({ tabId: tab.id }); } catch (_) { /* already detached */ }
      }
    }

    case 'edge_click_element': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (selector) => {
          const el = document.querySelector(selector);
          if (!el) return 'NOT_FOUND';
          el.click();
          return 'CLICKED';
        },
        args: [args.selector],
      });
      return textResult(res[0]?.result || 'clicked');
    }

    case 'edge_new_tab': {
      const tab = await chrome.tabs.create({ url: args.url });
      return textResult(JSON.stringify({ id: tab.id, url: tab.url }));
    }

    case 'edge_navigate': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      await chrome.tabs.update(tab.id, { url: args.url });
      return textResult(`Navigated active tab to ${args.url}`);
    }

    case 'edge_close_tab': {
      let tabId = args.tabId;
      if (tabId === undefined) {
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        tabId = tab && tab.id;
      }
      if (tabId === undefined) throw new Error('No tab to close');
      await chrome.tabs.remove(tabId);
      return textResult(`Closed tab ${tabId}`);
    }

    case 'edge_wait_for': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const { text, textGone, time } = args;
      if (time !== undefined) {
        await new Promise(r => setTimeout(r, time * 1000));
        return textResult(`waited ${time}s`);
      }
      const target = textGone || text || '';
      const gone = !!textGone;
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        const res = await chrome.scripting.executeScript({
          target: { tabId: tab.id },
          func: (t, g) => {
            const b = document.body ? document.body.innerText : '';
            return g ? !b.includes(t) : b.includes(t);
          },
          args: [target, gone],
        });
        if (res[0]?.result) {
          return textResult(gone ? `disappeared: ${target}` : `appeared: ${target}`);
        }
        await new Promise(r => setTimeout(r, 300));
      }
      throw new Error(`timeout (30s) waiting for ${gone ? 'disappearance' : 'appearance'}: ${target}`);
    }

    case 'edge_take_screenshot': {
      const win = await chrome.windows.getLastFocused();
      if (!win || win.id === undefined) throw new Error('No focused window');
      const dataUrl = await chrome.tabs.captureVisibleTab(win.id, { format: 'png' });
      if (typeof dataUrl !== 'string' || !dataUrl) throw new Error('Screenshot is empty');
      return textResult(dataUrl);
    }

    case 'edge_console_messages': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: () => {
          if (!window.__ema2_console) {
            window.__ema2_console = [];
            const stringify = (value) => {
              const seen = new WeakSet();
              try {
                return JSON.stringify(value, (key, item) => {
                  if (typeof item === 'bigint') return `${item}n`;
                  if (item instanceof Error) return { name: item.name, message: item.message, stack: item.stack };
                  if (item && typeof item === 'object') {
                    if (seen.has(item)) return '[Circular]';
                    seen.add(item);
                  }
                  return item;
                });
              } catch (_) { return '[Unserializable object]'; }
            };
            const orig = {};
            for (const lvl of ['log', 'warn', 'error', 'info', 'debug']) {
              orig[lvl] = console[lvl];
              console[lvl] = (...a) => {
                const entry = {
                  level: lvl,
                  text: a.map(x => (x && typeof x === 'object' ? stringify(x) : String(x))).join(' ').slice(0, 2000),
                };
                window.__ema2_console.push(entry);
                if (window.__ema2_console.length > 200) window.__ema2_console.splice(0, window.__ema2_console.length - 200);
                orig[lvl].apply(console, a);
              };
            }
          }
          return window.__ema2_console;
        },
      });
      const level = args.level || 'error';
      const order = { log: 1, info: 2, warn: 3, error: 4 };
      const msgs = (res[0]?.result || []).filter(m => (order[m.level] || 0) >= (order[level] || 4));
      return textResult(safeJsonStringify(msgs.slice(-50)));
    }

    case 'edge_network_requests': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: () =>
          performance.getEntriesByType('resource').slice(-500).map(e => ({
            name: e.name.slice(0, 300),
            duration: Math.round(e.duration),
            size: e.transferSize,
            type: e.initiatorType,
          })),
      });
      let list = res[0]?.result || [];
      if (args.filter) {
        const f = String(args.filter).toLowerCase();
        list = list.filter(r => r.name.toLowerCase().includes(f));
      }
      return textResult(safeJsonStringify(list.slice(-100)));
    }

    case 'edge_type': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (selector, text, submit) => {
          const el = document.querySelector(selector);
          if (!el) return 'NOT_FOUND';
          if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || el.isContentEditable)) return 'UNSUPPORTED_ELEMENT';
          el.focus();
          if (el.isContentEditable) el.textContent = text;
          else {
            const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype :
              el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
            const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
            if (!setter) return 'UNSUPPORTED_ELEMENT';
            setter.call(el, text);
          }
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
          if (submit) el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));
          return 'TYPED';
        },
        args: [args.selector, args.text, !!args.submit],
      });
      return textResult(res[0]?.result || 'typed');
    }

    case 'edge_press_key': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (key) => {
          const el = document.activeElement || document.body;
          if (!el || !(el instanceof HTMLElement)) return 'NO_FOCUS';
          const opts = { key, code: key, bubbles: true, cancelable: true };
          el.dispatchEvent(new KeyboardEvent('keydown', opts));
          el.dispatchEvent(new KeyboardEvent('keyup', opts));
          return 'DISPATCHED ' + key + ' (synthetic; not a real key press)';
        },
        args: [args.key],
      });
      return textResult(res[0]?.result || 'pressed');
    }

    case 'edge_fill_form': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (fields) => {
          const out = [];
          for (const f of fields || []) {
            const el = document.querySelector(f.selector);
            if (!el) { out.push({ name: f.name || f.selector, status: 'NOT_FOUND' }); continue; }
            if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || el.isContentEditable)) {
              out.push({ name: f.name || f.selector, status: 'UNSUPPORTED_ELEMENT' }); continue;
            }
            if (el.isContentEditable) el.textContent = f.value;
            else {
              const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype :
                el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
              const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
              if (!setter) { out.push({ name: f.name || f.selector, status: 'UNSUPPORTED_ELEMENT' }); continue; }
              setter.call(el, f.value);
            }
            el.dispatchEvent(new Event('input', { bubbles: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
            out.push({ name: f.name || f.selector, status: 'OK' });
          }
          return out;
        },
        args: [args.fields],
      });
      return textResult(safeJsonStringify(res[0]?.result || []));
    }

    case 'edge_hover': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (selector) => {
          const el = document.querySelector(selector);
          if (!el) return 'NOT_FOUND';
          const r = el.getBoundingClientRect();
          const x = r.x + r.width / 2;
          const y = r.y + r.height / 2;
          el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX: x, clientY: y }));
          el.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true, clientX: x, clientY: y }));
          return 'HOVERED (synthetic DOM events; not a real mouse hover)';
        },
        args: [args.selector],
      });
      return textResult(res[0]?.result || 'hovered');
    }

    case 'edge_scroll': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (deltaY) => {
          const before = window.scrollY;
          const d = deltaY === undefined ? window.innerHeight * 0.8 : deltaY;
          if (d === 0) return { before, delta: 0 };
          window.scrollBy({ top: d, behavior: 'smooth' });
          return { before, delta: d };
        },
        args: [args.deltaY],
      });
      return textResult(safeJsonStringify(res[0]?.result || {}));
    }

    default:
      throw new Error(`Unknown tool: ${name}`);
  }
}

async function extractTabText(tabId) {
  const tab = await chrome.tabs.get(tabId);
  if (/^(edge|chrome|about|devtools|view-source|chrome-extension):/.test(tab.url)) {
    throw new Error('Browser internal pages cannot be scripted');
  }
  const res = await chrome.scripting.executeScript({
    target: { tabId },
    func: () => (document.body ? document.body.innerText.substring(0, 50000) : ''),
  });
  return res[0]?.result || 'Page has no text content';
}

connect();
chrome.runtime.onStartup.addListener(() => connect());
chrome.runtime.onInstalled.addListener(() => connect());
