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

let socket = null;
let retryTimer = null;

async function getWsUrl() {
  try {
    const s = await chrome.storage.local.get(['wsUrl']);
    return s.wsUrl || DEFAULT_WS_URL;
  } catch (_) {
    return DEFAULT_WS_URL;
  }
}

async function connect() {
  try {
    socket = new WebSocket(await getWsUrl());
  } catch (e) {
    console.error('[EMA2] WebSocket construct failed:', e.message);
    scheduleReconnect();
    return;
  }

  socket.onopen = () => {
    console.log('[EMA2] connected to bridge');
    socket.send(JSON.stringify({ jsonrpc: '2.0', method: 'bridge/hello', role: 'extension' }));
  };

  socket.onmessage = (event) => handleMCPMessage(event.data);
  socket.onclose = () => {
    console.log('[EMA2] connection closed, reconnect in 3s...');
    scheduleReconnect();
  };
  socket.onerror = () => {};
}

function scheduleReconnect() {
  if (retryTimer) return;
  retryTimer = setTimeout(() => {
    retryTimer = null;
    connect();
  }, 3000);
}

async function handleMCPMessage(data) {
  let req;
  try {
    req = JSON.parse(data);
  } catch (e) {
    console.error('[EMA2] non-JSON message:', data);
    return;
  }

  try {
    if (req.method === 'ping') {
      if (req.id) sendResponse(req.id, { result: 'pong' });
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

    if (req.id && socket && socket.readyState === 1) {
      sendResponse(req.id, error ? { error } : { result });
    }
  } catch (err) {
    console.error('[EMA2] handle message failed:', err);
    if (req.id && socket && socket.readyState === 1) {
      sendResponse(req.id, { error: { code: -32603, message: err.message } });
    }
  }
}

function sendResponse(id, payload) {
  const response = { jsonrpc: '2.0', id };
  if (payload.error) response.error = payload.error;
  else response.result = payload.result;
  socket.send(JSON.stringify(response));
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
      description: 'Return console messages captured on the page since the first call',
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
  return { content: [{ type: 'text', text }] };
}

async function executeTool(name, args) {
  switch (name) {
    case 'edge_get_tabs': {
      const tabs = await chrome.tabs.query({});
      return textResult(JSON.stringify(tabs.map(t => ({ id: t.id, title: t.title, url: t.url, active: t.active })), null, 2));
    }

    case 'edge_get_active_tab': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      return textResult(JSON.stringify({ id: tab.id, title: tab.title, url: tab.url, active: tab.active }));
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
        return textResult(JSON.stringify(res.result?.value ?? null, null, 2));
      } finally {
        await chrome.debugger.detach({ tabId: tab.id });
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
      if (time) {
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
      const dataUrl = await chrome.tabs.captureVisibleTab(win.id, { format: 'png' });
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
            const orig = {};
            for (const lvl of ['log', 'warn', 'error', 'info', 'debug']) {
              orig[lvl] = console[lvl];
              console[lvl] = (...a) => {
                window.__ema2_console.push({
                  level: lvl,
                  text: a.map(x => (typeof x === 'object' ? (x && x.message) || JSON.stringify(x) : String(x))).join(' '),
                });
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
      return textResult(JSON.stringify(msgs.slice(-50), null, 2));
    }

    case 'edge_network_requests': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: () =>
          performance.getEntriesByType('resource').map(e => ({
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
      return textResult(JSON.stringify(list.slice(-100), null, 2));
    }

    case 'edge_type': {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab) throw new Error('No active tab');
      const res = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (selector, text, submit) => {
          const el = document.querySelector(selector);
          if (!el) return 'NOT_FOUND';
          el.focus();
          el.value = text;
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
          if (submit) el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
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
          const opts = { key, code: key, bubbles: true, cancelable: true };
          el.dispatchEvent(new KeyboardEvent('keydown', opts));
          el.dispatchEvent(new KeyboardEvent('keyup', opts));
          return 'PRESSED ' + key;
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
            el.value = f.value;
            el.dispatchEvent(new Event('input', { bubbles: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
            out.push({ name: f.name || f.selector, status: 'OK' });
          }
          return out;
        },
        args: [args.fields],
      });
      return textResult(JSON.stringify(res[0]?.result || []));
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
          return 'HOVERED';
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
          const d = deltaY || window.innerHeight * 0.8;
          window.scrollBy({ top: d, behavior: 'smooth' });
          return { before, delta: d };
        },
        args: [args.deltaY],
      });
      return textResult(JSON.stringify(res[0]?.result || {}));
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
