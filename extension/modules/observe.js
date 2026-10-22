/**
 * observe console/network、分页、bodyFilter。
 */

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
    // webRequest 只在本命令期间、只对目标 tab 开启；凭证头默认在进入内存前脱敏。
    startWebRequestCapture(tab.id, args.harHeaders !== 'all');
    try {
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
      const har = buildHar(items, pageUrl, {
        headers: args.harHeaders === 'all' ? 'all' : 'redact',
      });
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
    } finally {
      stopWebRequestCapture(tab.id);
    }
  }

}
