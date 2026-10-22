/**
 * batch 参数校验、并发执行与预算。
 */

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
  if (has('mode') && !READ_MODES.includes(inner.mode)) {
    fail('invalid_argument', `mode must be ${READ_MODES.join('|')}`, 'fix the inner arguments');
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
async function batchTool(args, context) {
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
    if (new Set(tabIds).size !== tabIds.length) {
      fail('invalid_argument', 'tabIds must not contain duplicates', 'remove duplicate ids');
    }
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
            openedText = resultText(await executeTool('open', openArgs, context));
          } catch (error) {
            if (reusedTabId === undefined || !(error instanceof ToolError)
                || error.code !== 'tab_not_found') throw error;
            reusedByIndex.delete(index);
            delete openArgs.tabId;
            delete openArgs.reuseSamePage;
            openedText = resultText(await executeTool('open', openArgs, context));
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
              const value = await executeTool(toolName, innerArgs, context);
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
        const value = await executeTool(toolName, { ...inner, tabId }, context);
        results[index] = `tab=${tabId}\n` + resultText(value);
      } catch (error) {
        results[index] = `tab=${tabId}\n` + formatError(error);
      }
    }
  }
  await Promise.all(Array.from({ length: concurrency }, worker));
  return results.join('\n---\n');
}
