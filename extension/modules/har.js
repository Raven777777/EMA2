/**
 * HAR 构建与凭证头脱敏。
 */

function buildHar(items, pageUrl, options = {}) {
  const now = Date.now();
  const redact = options.headers !== 'all';
  // HAR 会落盘并经常被贴进 AI 上下文；凭证头默认脱敏，需要原值才显式 --headers=all。
  const toHeaders = (list) => (Array.isArray(list) ? list : []).map(item => {
    const name = String(item.name || '');
    const value = String(item.value || '');
    return {
      name,
      value: redact && SENSITIVE_HAR_HEADERS.has(name.toLowerCase()) ? '<redacted>' : value,
    };
  });
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
