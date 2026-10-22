/**
 * 按 tab / 命令租约采集 webRequest，补齐网络记录。
 */

// ---------------------------------------------------------------- webRequest
//
// 页面 hook 只能看到安装之后的 fetch/XHR；首屏文档/资源的状态码、响应头只能靠
// webRequest。监听器只在 `observe network` 期间按目标 tab 注册（RequestFilter.tabId），
// 命令结束立即移除，避免常驻观察所有标签的流量。响应体仍由页面 hook 采样。
const WEBREQ_PER_TAB = 500;
const webRequestLog = new Map(); // tabId -> Map<requestId, record>
const webRequestCaptures = new Map(); // tabId -> { refs, listeners, scrub }

function webRequestBucket(tabId, create) {
  let bucket = webRequestLog.get(tabId);
  if (!bucket && create) {
    bucket = new Map();
    webRequestLog.set(tabId, bucket);
  }
  return bucket || null;
}

function scrubHeaders(headers, scrub) {
  if (!scrub || !Array.isArray(headers)) return Array.isArray(headers) ? headers : [];
  return headers.map(item => {
    const name = String(item && item.name || '');
    if (!SENSITIVE_HAR_HEADERS.has(name.toLowerCase())) return item;
    return { name, value: '<redacted>' };
  });
}

function webRequestAdd(bucket, details) {
  if (!details || details.tabId < 0) return;
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

function webRequestPatch(bucket, details, patch) {
  if (!details || details.tabId < 0) return;
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

/**
 * 只在目标 tab 上开始记录。`scrub` 为 true 时凭证头在进入内存前就替换掉；
 * 并发 observe 中只要有一个要求脱敏，就保留更保守的行为。
 */
function startWebRequestCapture(tabId, scrub) {
  const existing = webRequestCaptures.get(tabId);
  if (existing) {
    existing.refs += 1;
    existing.scrub = existing.scrub || !!scrub;
    return tabId;
  }
  const capture = { refs: 1, scrub: !!scrub, listeners: [] };
  const bucket = webRequestBucket(tabId, true);
  try {
    const api = chrome.webRequest;
    if (!api || !api.onBeforeRequest) {
      webRequestCaptures.set(tabId, capture);
      return tabId;
    }
    const filter = { urls: ['<all_urls>'], tabId };
    const add = (method, listener, extra) => {
      if (!api[method] || typeof api[method].addListener !== 'function') return;
      api[method].addListener(listener, filter, extra);
      capture.listeners.push({ method, listener });
    };
    add('onBeforeRequest', details => webRequestAdd(bucket, details));
    add('onBeforeSendHeaders', details => webRequestPatch(bucket, details, {
      requestHeaders: scrubHeaders(details.requestHeaders, capture.scrub),
    }), ['requestHeaders', 'extraHeaders']);
    add('onHeadersReceived', details => webRequestPatch(bucket, details, {
      status: Number(details.statusCode) || 0,
      responseHeaders: scrubHeaders(details.responseHeaders, capture.scrub),
      mimeType: headerMime(details.responseHeaders),
      fromCache: !!details.fromCache,
    }), ['responseHeaders', 'extraHeaders']);
    add('onCompleted', details => webRequestPatch(bucket, details, {
      status: Number(details.statusCode) || 0,
      fromCache: !!details.fromCache,
      ip: String(details.ip || ''),
      timeMs: Math.max(0, Math.round((Number(details.timeStamp) || Date.now())
        - (bucket.get(details.requestId)?.startedAt || Date.now()))),
    }));
    add('onErrorOccurred', details => webRequestPatch(bucket, details, {
      status: 0,
      error: String(details.error || ''),
      timeMs: Math.max(0, Math.round((Number(details.timeStamp) || Date.now())
        - (bucket.get(details.requestId)?.startedAt || Date.now()))),
    }));
  } catch (error) {
    console.warn('[EMA2] webRequest listeners unavailable:', error && error.message || error);
  }
  webRequestCaptures.set(tabId, capture);
  return tabId;
}

/** 最后一个 observe 结束后注销监听器并丢弃该 tab 的记录。 */
function stopWebRequestCapture(tabId) {
  const capture = webRequestCaptures.get(tabId);
  if (!capture) return;
  capture.refs -= 1;
  if (capture.refs > 0) return;
  webRequestCaptures.delete(tabId);
  try {
    const api = chrome.webRequest;
    for (const { method, listener } of capture.listeners) {
      try { api[method].removeListener(listener); } catch (_) { /* ignore */ }
    }
  } catch (_) { /* ignore */ }
  webRequestLog.delete(tabId);
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
      if (new Set(args.tabIds).size !== args.tabIds.length) {
        fail('invalid_argument', 'tabIds must not contain duplicates', 'remove duplicate ids');
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
