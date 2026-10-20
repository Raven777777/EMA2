'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { JSDOM } = require('jsdom');

const ROOT = path.resolve(__dirname, '..');
const BACKGROUND = fs.readFileSync(path.join(ROOT, 'extension', 'background.js'), 'utf8');
const AUTO_START = BACKGROUND.search(/\r?\nensureWatchdog\(\);\r?\nconnect\(\);/);

function createChrome(overrides = {}) {
  const chrome = {
    alarms: {
      onAlarm: { addListener() {} },
      create() {},
      clear() {},
    },
    runtime: {
      getManifest: () => ({ version: '1.12.1' }),
      getPlatformInfo: async () => ({ os: 'win' }),
      onStartup: { addListener() {} },
      onInstalled: { addListener() {} },
      onMessage: { addListener() {} },
      lastError: null,
      reload() {},
    },
    action: { onClicked: { addListener() {} } },
    storage: {
      local: { get: async () => ({}), set: async () => {} },
      session: { get: async () => ({}), set: async () => {} },
    },
    tabs: {
      query: async () => [],
      get: async (id) => ({ id, windowId: 1, active: true, url: 'https://example.test/', title: 'Example' }),
      create: async () => ({ id: 1, windowId: 1, url: 'https://example.test/', title: 'Example' }),
      update: async () => ({}),
      remove: async () => {},
      captureVisibleTab: async () => 'data:image/png;base64,AAAA',
    },
    scripting: { executeScript: async () => [{ result: undefined }] },
    debugger: { attach: async () => {}, detach: async () => {}, sendCommand: async () => ({}) },
  };
  for (const [group, values] of Object.entries(overrides)) {
    if (chrome[group] && typeof chrome[group] === 'object' && values && typeof values === 'object') {
      chrome[group] = { ...chrome[group], ...values };
    } else {
      chrome[group] = values;
    }
  }
  return chrome;
}

function loadBackground(chrome = createChrome()) {
  assert.ok(AUTO_START > 0, 'auto-connect marker not found in background.js');
  const source = BACKGROUND.slice(0, AUTO_START);
  const context = {
    chrome,
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    URL,
    URLSearchParams,
    TextEncoder,
    TextDecoder,
    WebSocket: class FakeWebSocket {},
    performance,
    crypto: globalThis.crypto,
    fetch: globalThis.fetch,
    btoa: globalThis.btoa,
    atob: globalThis.atob,
  };
  vm.createContext(context);
  vm.runInContext(source, context, { filename: 'extension/background.js' });
  return context;
}

function polyfillDom(window) {
  // jsdom 不实现 layout/innerText；测试只需要稳定的文本与可见性语义。
  if (!Object.getOwnPropertyDescriptor(window.HTMLElement.prototype, 'innerText')) {
    Object.defineProperty(window.HTMLElement.prototype, 'innerText', {
      configurable: true,
      get() { return this.textContent || ''; },
      set(value) { this.textContent = String(value); },
    });
  }
  window.Element.prototype.getClientRects = function () {
    return [{ width: 10, height: 10 }];
  };
  window.Element.prototype.getBoundingClientRect = function () {
    return { x: 0, y: 0, left: 0, top: 0, right: 10, bottom: 10, width: 10, height: 10 };
  };
  if (!window.Element.prototype.scrollIntoView) {
    window.Element.prototype.scrollIntoView = function () {};
  }
  window.getComputedStyle = () => ({ visibility: 'visible', display: 'block' });
  window.TextDecoder = TextDecoder;
  window.TextEncoder = TextEncoder;
}

let docSeq = 0;

/**
 * 建一个 jsdom 文档并装好 helper。
 * 文档句柄（d1、d2…）由扩展分配，这里模拟 ensureHelpers：每建一个文档就发一个新号。
 */
function setupDom(html, tabId = 1, background = null) {
  const dom = new JSDOM(html, {
    url: 'https://example.test/page',
    runScripts: 'outside-only',
    pretendToBeVisual: true,
  });
  polyfillDom(dom.window);
  if (background) {
    const installed = pageEval(dom, background.installHelpers);
    assert.equal(installed && installed.installed, true);
    docSeq += 1;
    pageRun(dom, `window.__ema2_helpers.setDocHandle('d${docSeq}')`);
  }
  return dom;
}

function pageEval(dom, fn, ...args) {
  const ctx = dom.getInternalVMContext();
  const values = args.map(value => (value === undefined ? 'undefined' : JSON.stringify(value)));
  return vm.runInContext(`(${fn.toString()})(${values.join(',')})`, ctx);
}

function pageRun(dom, source) {
  return vm.runInContext(source, dom.getInternalVMContext());
}

function refsIn(text) {
  return [...String(text).matchAll(/\[(e\d+@[a-z0-9-]+)\]/gi)].map(match => match[1]);
}

function expectToolError(code) {
  return (error) => {
    assert.equal(error && error.code, code, `expected error.code=${code}, got ${error && error.code}: ${error && error.message}`);
    return true;
  };
}

module.exports = {
  ROOT,
  assert,
  createChrome,
  loadBackground,
  setupDom,
  pageEval,
  pageRun,
  refsIn,
  expectToolError,
};
