/**
 * act 单步/脚本、settle、await。
 */

// ---------------------------------------------------------------- act / settle / await

async function loadLastAct() {
  let stored = null;
  try {
    const store = chrome.storage && chrome.storage.session;
    if (store) {
      const values = (await store.get(LAST_ACT_KEY)) || {};
      const value = values[LAST_ACT_KEY];
      if (value && typeof value === 'object'
          && (!value.updatedAt || Date.now() - value.updatedAt <= LAST_ACT_TTL_MS)) {
        stored = value;
      }
    }
  } catch (_) { /* storage.session 不可用：用内存兜底 */ }
  const memory = lastActContext
    && (!lastActContext.updatedAt || Date.now() - lastActContext.updatedAt <= LAST_ACT_TTL_MS)
    ? lastActContext : null;
  // 并发 act 的内存写入和 storage 写入完成顺序可能不同：只取较新的上下文。
  let value = null;
  if (stored && memory) {
    value = (Number(stored.updatedAt) || 0) >= (Number(memory.updatedAt) || 0) ? stored : memory;
  } else {
    value = stored || memory;
  }
  lastActContext = value;
  return value;
}

async function saveLastAct(context) {
  const value = { ...context, updatedAt: Date.now() };
  if (!lastActContext || value.updatedAt >= (Number(lastActContext.updatedAt) || 0)) {
    lastActContext = value;
  }
  // 串行化 storage 写入，避免较早的 act 在较晚的 clear/save 之后落盘。
  const write = lastActWrite.then(async () => {
    const current = lastActContext;
    if (current && (Number(current.updatedAt) || 0) > (Number(value.updatedAt) || 0)) return;
    try {
      const store = chrome.storage && chrome.storage.session;
      if (store) await store.set({ [LAST_ACT_KEY]: value });
    } catch (_) { /* 忽略：内存上下文仍然可用 */ }
  });
  lastActWrite = write.catch(() => {});
  await write;
  return value;
}

async function clearLastAct() {
  lastActContext = null;
  const write = lastActWrite.then(async () => {
    // 清空之后才 save 的新上下文不受影响；这里只删除已经落盘的旧值。
    if (lastActContext) return;
    try {
      const store = chrome.storage && chrome.storage.session;
      if (store) await store.remove(LAST_ACT_KEY);
    } catch (_) { /* ignore */ }
  });
  lastActWrite = write.catch(() => {});
  await write;
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


/** 等页面条件（text / textGone / selector）成立；超时带最后 URL/文本。 */
async function waitForPageCondition(tabId, spec, condition) {
  const timeoutMs = Math.max(0, Math.min(300000, Math.round((typeof spec.timeout === 'number' ? spec.timeout : 10) * 1000)));
  const label = pageWaitQuotedLabel(condition);
  const startedAt = Date.now();
  if (condition.key === 'selector') {
    await ensureHelpers(tabId);
    // 非法 CSS 立即报 invalid_selector，而不是空等到 timeout。
    await requireValidSelector(tabId, condition.value);
  }
  let lastUrl = '';
  while (Date.now() - startedAt < timeoutMs) {
    try {
      const tabInfo = await chrome.tabs.get(tabId);
      if (tabInfo && tabInfo.url) lastUrl = tabInfo.url;
    } catch (_) {
      fail('tab_not_found', `tab ${tabId} was closed while waiting for ${label}`,
        'inspect tabs with `ema2 tabs list`');
    }
    let matched = false;
    try {
      matched = await pageWaitMatched(tabId, condition);
    } catch (_) { matched = false; }
    if (matched) {
      return `expect=ok condition=${label} ms=${Date.now() - startedAt}`;
    }
    await sleep(120);
  }
  let text = '';
  try {
    text = await runInTab(tabId, () => document.body ? document.body.innerText : '', []);
  } catch (_) { /* navigating or tab closed */ }
  if (!lastUrl) {
    try { lastUrl = (await chrome.tabs.get(tabId)).url || ''; } catch (_) { /* tab closed */ }
  }
  const seenText = shortText(text, 160);
  fail('wait_timeout',
    `expect=timeout condition=${label} ms=${Date.now() - startedAt}`
      + (lastUrl ? ` url=${lastUrl}` : '')
      + (seenText ? ` text=${JSON.stringify(seenText)}` : ''),
    'call `ema2 snapshot` or `ema2 read` to inspect the page, or raise timeout= (max 30s)');
}

/** 等某个响应/页面导航，或 text/textGone/selector 页面条件，命中返回一行摘要。 */
async function waitForResponse(tabId, spec, baselineOverride) {
  const condition = pageWaitCondition(spec);
  if (condition) return waitForPageCondition(tabId, spec, condition);
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
