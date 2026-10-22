/**
 * console / network 页面 hook 与 lastAct 上下文。
 */

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

/**
 * 网络水位用 {epoch, seq, installed} 表示；不同文档的 seq 不可比较。
 * installed=false 表示 hook 从未安装：此时 epoch 为空，seq 不能当作基线使用。
 */
async function networkMark(tabId) {
  try {
    return await runInTab(tabId, () => {
      const state = window.__ema2_net;
      return state
        ? { epoch: state.epoch || '', seq: state.seq || 0, installed: true }
        : { epoch: '', seq: 0, installed: false };
    }, [], 'MAIN');
  } catch (_) {
    return { epoch: '', seq: 0, installed: false };
  }
}

/** console 水位用 {epoch, seq} 表示；无 hook 时 epoch 为空、seq 为 0。 */
async function consoleMark(tabId) {
  try {
    return await runInTab(tabId, () => (window.__ema2_console
      ? { epoch: window.__ema2_console_epoch || '', seq: window.__ema2_console_seq || 0 }
      : { epoch: '', seq: 0 }), [], 'MAIN');
  } catch (_) {
    return { epoch: '', seq: 0 };
  }
}
