/**
 * open 导航、文档接位等待、wait 条件。
 */

// ---------------------------------------------------------------- open / wait

/** 主文档的导航计时：真实 HTTP 状态与重定向次数（Chromium 109+ 的 responseStatus）。 */
async function navigationTiming(tabId) {
  try {
    return await runInTab(tabId, () => {
      const entries = performance.getEntriesByType ? performance.getEntriesByType('navigation') : [];
      const nav = entries && entries[0];
      if (!nav) return null;
      return {
        status: typeof nav.responseStatus === 'number' ? nav.responseStatus : 0,
        redirects: nav.redirectCount || 0,
        type: nav.type || '',
      };
    }, []);
  } catch (_) {
    return null;
  }
}

async function openTool(args) {
  await clearLastAct();
  const wait = args.wait || 'load';
  let tab;
  let reuse = 'new';
  let previousUrl = '';
  let previousMarker = 0;
  if (args.tabId !== undefined) {
    // 显式导航用户已有的标签：不改会话登记，scope=session 不会关掉用户的标签。
    tab = await resolveTab(args);
    const currentUrl = String(tab.url || tab.pendingUrl || '');
    // batch --reuse session 命中精确 URL 时用内部标记续用文档，不重载。
    if (args.reuseSamePage === true
        && samePageTarget(currentUrl, args.url) && !samePageHashOnly(currentUrl, args.url)) {
      previousUrl = currentUrl;
      reuse = 'session';
    } else {
      const nav = await navigateTab(tab, args.url, wait !== 'none');
      previousUrl = nav.before;
      previousMarker = nav.marker;
      reuse = 'tabId';
    }
  } else if (args.replace === true) {
    tab = await activeTab();
    const nav = await navigateTab(tab, args.url, wait !== 'none');
    previousUrl = nav.before;
    previousMarker = nav.marker;
    reuse = 'replace';
  } else if (args.reuse === 'session') {
    const live = await lastLiveSessionTab();
    if (live) {
      tab = live;
      const currentUrl = String(tab.url || tab.pendingUrl || '');
      if (samePageTarget(currentUrl, args.url) && !samePageHashOnly(currentUrl, args.url)) {
        // “复用”同一个文档时不要重载：保留滚动位置/已填表单。
        previousUrl = currentUrl;
      } else {
        const nav = await navigateTab(tab, args.url, wait !== 'none');
        previousUrl = nav.before;
        previousMarker = nav.marker;
      }
      reuse = 'session';
    }
  }
  if (!tab) {
    tab = await chrome.tabs.create({ url: args.url, active: true });
    await rememberSessionTab(tab.id);
    // 新标签必须让用户看得见：窗口提到前台（测试也依赖前台语义）。
    try { await chrome.windows.update(tab.windowId, { focused: true }); } catch (_) { /* ignore */ }
  }
  let loaded = true;
  let docTimeout = 0;
  // --timeout 覆盖“打开标签 + 等文档 + 装 hook + 读 timing/focus”全流程。
  // 只给 waitForDocument 设 deadline 会让后面的 hook/timing/focus 再各花几秒，
  // 慢页面/挂起页面因此把 3s 拉成 15s。
  const openBudget = wait === 'none' ? 5000 : Math.min(60000, Math.max(1000, Number(args.waitTimeout || 15) * 1000));
  if (wait !== 'none') docTimeout = openBudget;
  const deadline = Date.now() + openBudget;
  const remaining = () => Math.max(0, deadline - Date.now());
  if (wait !== 'none') {
    // 单个 executeScript 也可能卡住；外层硬截止保证 open 一定按 budget 返回。
    loaded = await within(
      waitForDocument(
        tab.id, args.url, previousUrl,
        wait === 'load' ? 'complete' : 'interactive', docTimeout, previousMarker,
      ),
      docTimeout + 250,
      false,
    );
    // 超时后再做一次单步探测：页面可能刚好在 deadline 后完成，
    // 不要把已经加载好的页面误报成 load=pending。
    if (!loaded) {
      loaded = await within(
        waitForDocument(
          tab.id, args.url, previousUrl,
          wait === 'load' ? 'complete' : 'interactive', 1, previousMarker,
        ),
        Math.min(1000, remaining() + 400),
        false,
      );
    }
  }
  let current;
  try { current = await chrome.tabs.get(tab.id); }
  catch (_) { fail('tab_not_found', `tab ${tab.id} was closed during open`, 'call tabs to list ids'); }
  // 等页面稳定后自动装网络 hook，这样后续 fetch/XHR 都能被 observe network 看到。
  if (remaining() > 0) await sleep(Math.min(180, remaining()));
  let hook = false;
  try {
    hook = remaining() > 0
      ? await within(ensureNetworkHook(tab.id), Math.min(2500, remaining()), false)
      : false;
  } catch (_) { hook = false; }
  try {
    if (remaining() > 0) await within(ensureConsoleHook(tab.id), Math.min(1500, remaining()), false);
  } catch (_) { /* ignore */ }
  try {
    if (remaining() > 0) await within(ensurePageGuard(tab.id), Math.min(1000, remaining()), false);
  } catch (_) { /* ignore */ }
  const timing = wait === 'none' || remaining() <= 0
    ? null
    : await within(navigationTiming(tab.id), Math.min(1500, remaining()), null);
  const status = !timing ? (wait === 'none' ? 'pending' : loaded ? 'unknown' : 'pending')
    : timing.status > 0 ? String(timing.status) : 'unknown';
  // hook/timing 期间可能又发生重定向；用最新 tab 元数据，而不是早先的快照。
  let latest = current;
  try { latest = await chrome.tabs.get(tab.id); } catch (_) { /* keep current */ }
  // 导航尚未提交时 tab.url/title 仍是旧文档；不要把旧标题当成新页面。
  const pendingNav = !loaded && !samePageTarget(latest.url || '', args.url);
  const shownUrl = pendingNav ? (latest.pendingUrl || args.url) : (latest.url || args.url);
  const shownTitle = pendingNav ? '' : (latest.title || '');
  const shownStatus = pendingNav ? 'pending' : status;
  const lines = [
    `id=${latest.id}`,
    `url=${shownUrl}`,
    `title=${oneLine(shownTitle)}`,
    `status=${shownStatus}`,
    `reuse=${reuse}`,
    `net_hook=${hook ? 'on' : 'off'}`,
  ];
  if (!loaded) {
    lines.push(`load=pending timeout=${Math.round(docTimeout / 1000)}s timed_out=1`);
    lines.push('hint=the document did not reach the requested readyState before --timeout; the tab is open, read/snapshot may still work (try `ema2 wait idle`)');
  }
  if (timing && timing.redirects > 0) lines.push(`redirects=${timing.redirects}`);
  const guardEvents = (!loaded || pendingNav)
    ? []
    : await within(
      guardSince(tab.id, 0),
      Math.min(800, Math.max(1, remaining())),
      [],
    );
  const guardDialogs = (Array.isArray(guardEvents) ? guardEvents : [])
    .filter(event => event.kind === 'dialog');
  for (const event of guardDialogs.slice(0, 3)) {
    lines.push(`dialog_opened=${event.dialog || 'dialog'} handled=${event.handled || 'dismissed'}`
      + (event.message ? ` msg=${oneLine(event.message).slice(0, 120)}` : ''));
  }
  const blockedPopups = (Array.isArray(guardEvents) ? guardEvents : [])
    .filter(event => event.kind === 'popup' && event.blocked);
  if (blockedPopups.length) lines.push(`popup_blocked=1 url=${shortText(blockedPopups[0].url || '', 120)}`);
  const focused = await within(
    runInTab(tab.id, () => document.hasFocus(), []),
    Math.min(1200, Math.max(1, remaining())),
    false,
  );
  lines.push(`focused=${focused ? 1 : 0}`);
  if (!focused) {
    lines.push('hint=the browser window is not focused; call `ema2 tabs activate ' + tab.id
      + '` (or bring Edge to front) before shot/press/password-manager flows');
  }
  if (timing && timing.status >= 400) {
    lines.push(`hint=http ${timing.status}: the page is probably the site's own error page, not the content you asked for`);
  } else if (timing && timing.status > 0 && timing.status >= 300 && timing.status < 400) {
    lines.push(`hint=http ${timing.status}: redirect left in place (page may need a click to follow)`);
  }
  return lines.join('\n');
}

/** 两个 URL 是否指向同一页面（忽略 hash 与结尾斜杠）。 */
function samePageTarget(a, b) {
  const normalize = (value) => {
    try {
      const url = new URL(value);
      url.hash = '';
      return url.href.replace(/\/$/, '');
    } catch (_) {
      return String(value || '').replace(/#.*$/, '').replace(/\/$/, '');
    }
  };
  return normalize(a) === normalize(b);
}

/** 只改了 hash 的同一页面不会触发 reload。 */
function samePageHashOnly(a, b) {
  try {
    const left = new URL(a);
    const right = new URL(b);
    const stripHash = (url) => url.href.slice(0, url.href.length - url.hash.length);
    return left.hash !== right.hash && stripHash(left) === stripHash(right);
  } catch (_) {
    return false;
  }
}

/**
 * 等目标文档真正接位再判定就绪。
 * 只判 readyState 会被旧文档骗过：tabs.update 之后新文档尚未提交时，
 * 旧文档仍是 complete，于是 open 返回的 url/title/正文都停留在上一页。
 * 这里同时看 document 的 location.href：只要确认已经离开旧文档（或落到目标/重定向后的地址）
 * 且 readyState 达标，才算加载完成。
 */
async function waitForDocument(tabId, targetUrl, previousUrl, state, timeoutMs, previousMarker = 0) {
  const deadline = Date.now() + timeoutMs;
  const reloadingSamePage = samePageTarget(previousUrl, targetUrl);
  // 同 URL 的 open 必须等到新文档，否则旧文档的 complete 会立刻骗过 readyState 检查。
  const requireReload = reloadingSamePage && !samePageHashOnly(previousUrl, targetUrl)
    && previousMarker > 0;
  while (Date.now() < deadline) {
    let info = null;
    try {
      info = await runInTab(
        tabId,
        () => ({
          href: location.href,
          ready: document.readyState,
          origin: Number(performance.timeOrigin) || 0,
        }),
        [],
      );
    } catch (_) { /* 导航中：旧文档已销毁、新文档未就绪 */ }
    if (info) {
      let arrived;
      if (!previousUrl) {
        // 新标签的初始 about:blank / edge://newtab 不能算“已到达”，否则 open 会提前返回。
        arrived = !!info.href && info.href !== 'about:blank' && !info.href.startsWith('about:');
      } else {
        arrived = reloadingSamePage
          ? samePageTarget(info.href, previousUrl || targetUrl)
          : !samePageTarget(info.href, previousUrl);
      }
      // 浏览器自己的错误页/新标签页不是目标文档；宁可等超时并报 load=pending。
      if (arrived && isInternalUrl(info.href)) arrived = false;
      if (arrived && requireReload && Number(info.origin) === previousMarker) arrived = false;
      const readyOk = state === 'interactive'
        ? info.ready === 'interactive' || info.ready === 'complete'
        : info.ready === 'complete';
      if (arrived && readyOk) return true;
    }
    await sleep(120);
  }
  return false;
}

/**
 * 导航到指定 URL，返回导航前的地址与文档时间原点。
 * timeOrigin 用来区分“旧文档仍是 complete”和 reload 后的新文档。
 */
async function navigateTab(tab, url, wantMarker) {
  const before = tab.url || '';
  let marker = 0;
  if (wantMarker) {
    // 只用于等待新文档；--wait none 不该为了读 marker 而多等。
    marker = await within(
      runInTab(tab.id, () => Number(performance.timeOrigin) || 0, []),
      1000,
      0,
    );
  }
  await chrome.tabs.update(tab.id, { url });
  return { before, marker: Number(marker) || 0 };
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
  if (args.await) return awaitPendingAction(args);
  const tab = await resolveTab(args);
  const timeoutSeconds = args.timeout === undefined ? WAIT_TIMEOUT_MS / 1000 : Number(args.timeout);
  if (!Number.isFinite(timeoutSeconds) || timeoutSeconds < 0 || timeoutSeconds > WAIT_MAX_SECONDS) {
    fail('invalid_argument', `timeout must be between 0 and ${WAIT_MAX_SECONDS} seconds`, 'fix timeout');
  }
  const timeoutMs = Math.round(timeoutSeconds * 1000);
  const start = Date.now();
  if (args.time !== undefined) {
    const seconds = Number(args.time);
    if (!Number.isFinite(seconds) || seconds < 0 || seconds > WAIT_MAX_SECONDS) {
      fail('invalid_argument', `time must be between 0 and ${WAIT_MAX_SECONDS} seconds`, 'fix time');
    }
    await sleep(Math.round(seconds * 1000));
    return `ok=time elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.idle) {
    const ready = await waitForReady(tab.id, 'complete', timeoutMs);
    if (!ready) fail('wait_timeout', `page did not become idle in ${timeoutMs / 1000}s`, 'retry, or wait for a specific selector/text');
    return `ok=idle elapsed=${((Date.now() - start) / 1000).toFixed(1)}s`;
  }
  if (args.selector !== undefined) {
    await ensureHelpers(tab.id);
    // 非法 CSS 应立即返回 invalid_selector，而不是等到 timeout 后报 wait_timeout。
    await requireValidSelector(tab.id, args.selector);
  }
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
