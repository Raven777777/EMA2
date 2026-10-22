/**
 * read 正文提取与 snapshot ref。
 */

// ---------------------------------------------------------------- read / snapshot

/** 把注入失败分成 tab 已关闭 / 受限页面 / 真的导航了，避免一律 page_changed。 */
async function failInjection(tab, error, action) {
  let current = null;
  try { current = await chrome.tabs.get(tab.id); } catch (_) { /* tab closed */ }
  if (!current) {
    fail('tab_not_found', `tab ${tab.id} was closed while ${action}`,
      'call tabs to list ids');
  }
  if (isInternalUrl(current.url)) {
    fail('internal_page', `browser internal pages cannot be ${action}`,
      'open an http(s) page');
  }
  const detail = oneLine(error && error.message || error).slice(0, 300);
  if (/cannot access|not allowed|extensions gallery|chrome web store|cannot be scripted|missing host permission/i.test(detail)) {
    fail('internal_page', `this page cannot be scripted by the extension (${detail})`,
      'reload the tab, or open a normal http(s) page');
  }
  fail('page_changed', `the page changed while ${action} (${detail || 'injection failed'})`,
    'retry after the page settles; if the tab is still loading, run `ema2 wait idle` first');
}

/**
 * read 的页面内提取（md/text/html/visible 共用选区/预算/游标），
 * 并在同一遍里识别页面状态（P0-F）：
 * - pdf → PDF viewer 里没有可遍历的正文 DOM，read 直接报错；
 * - challenge / login_required → 输出 state= + hint，退出 0（不伪装成正文成功，也不报错）；
 * - http>=400 → page_error；
 * - 公开正文仍按普通页面处理；**本期不检测 paywall**，也没有 JSON-LD / 订阅标记。
 * state 优先级：pdf → challenge → login → http_error → 正文提取 → empty。
 * state/empty 只约束 md/text 的整页读取（clean 内容模式）；html 是原始视图，
 * visible 自有兜底提示；--selector 是显式子树请求，允许为空。
 * 注意：本函数经 executeScript 注入 isolated world，必须自包含，
 * 不能引用 background 里的其他顶层函数（注入后它们不存在）。
 * 陷阱：Edge 对 func 注入内的抛错不 reject 而是 result=undefined（曾误报 helpers_missing），
 * 所以这里捕获整包异常带回页面异常，由 readTool 映射成 js_exception。
 */
function readPageContent(modeName, selectorArg, mainWanted) {
  try {
    const helpers = window.__ema2_helpers;
  if (!helpers) return { kind: 'helpers_missing', trace: String(Object.keys(window)).filter(k => k.includes('ema2')).join(',') };
  const clean = helpers.clean;
  const navEntry = (typeof performance.getEntriesByType === 'function'
    ? (performance.getEntriesByType('navigation') || [])[0] || null
    : null);
  const http = navEntry ? (Number(navEntry.responseStatus) || 0) : 0;
  const contentType = String(document.contentType || '').trim().toLowerCase();
  const title = clean(document.title);
  const scoped = !!selectorArg;
  const contentMode = modeName === 'md' || modeName === 'text';

  const isPdf = /^application\/pdf/i.test(contentType)
    || (/(?:^|\.)pdf$/i.test(String(location.pathname || ''))
      && !/^text\/html|^application\/xhtml/i.test(contentType));

  /** 人机验证页（Cloudflare / 滑块 / Turnstile 中间页）。小页面才扫 body 文本，避免拖慢大页 read。 */
  const challengeState = () => {
    if (/just a moment|attention required|checking your browser|verify you (?:are |)human|are you (?:a )?robot|press &?\s*hold|请稍候|正在验证|人机验证|安全验证/i.test(title)) return true;
    if (document.querySelector('#challenge-form,.challenge-form,#cf-chl-widget,.cf-turnstile-wrapper,#cf-chl-runner,.cf-browser-verification')) return true;
    const body = document.body;
    if (!body || body.childElementCount > 60) return false;
    const preview = String(body.innerText || '').slice(0, 400);
    return preview.length > 0
      && /verify you (?:are )?human|checking your browser|progress&|press (?:and|&) hold|确认您是真人/i.test(preview);
  };

  /** 登录墙：页面是「有密码框 + 登录语义的标题/一级标题」的表单页，不是弹层。 */
  const loginState = () => {
    if (!document.querySelector('input[type="password"]')) return false;
    const heading = document.querySelector('h1,h2');
    const context = title + ' ' + (heading ? String(heading.textContent || '') : '');
    return /log ?in|sign ?in|sign ?up|登录|登入|密码|pass ?word|passcode|two-?step|two-?factor|verification code|authenticat|authorize/i.test(context);
  };

  let kind = 'ok';
  if (isPdf) kind = 'pdf';
  else if (!scoped && modeName !== 'html') {
    if (contentMode && challengeState()) kind = 'challenge';
    else if (contentMode && loginState()) kind = 'login_required';
    else if (http >= 400) kind = 'http_error';
  }

  let text = '';
  let mainSelector = '';
  let mainFallback = false;
  let textLength = 0;
  let bodyLength = 0;
  if (kind === 'ok') {
    let root = document.body || document.documentElement;
    if (selectorArg) {
      const found = helpers.find(null, selectorArg, null);
      if (!found) return { missing: true };
      root = found;
    } else if (mainWanted) {
      const picked = helpers.mainRoot();
      root = picked.root;
      mainSelector = picked.selector;
      mainFallback = !!picked.fallback;
      textLength = picked.textLength || 0;
      bodyLength = picked.bodyLength || 0;
    }
    text = modeName === 'visible' ? helpers.visibleText(root)
      : modeName === 'html' ? root.innerHTML
      : modeName === 'text' ? helpers.plainText(root)
      : helpers.markdown(root);
    // md/text 的整页读取不允许 "0 字却成功"；--selector 子树是显式请求，允许为空。
    if (contentMode && !scoped && !String(text || '').trim()) kind = 'empty';
  }

  const headerLines = [
    'url=' + location.href,
    'title=' + title,
    'mode=' + modeName,
  ];
  if (mainSelector) headerLines.push('main=' + mainSelector);
  if (kind === 'challenge' || kind === 'login_required') headerLines.push('state=' + kind);
  if (http > 0) headerLines.push('http=' + http);
  if (contentType) headerLines.push('content_type=' + contentType);
  return {
    kind,
    header: headerLines.join('\n'),
    text,
    mainFallback,
    textLength,
    bodyLength,
    http,
    contentType,
  };
  } catch (error) {
    return { kind: 'page_exception', message: String((error && error.stack) || error).slice(0, 500) };
  }
}

/** 正文地标 selector：未命中时回退到默认 main/body，避免多一次往返。 */
function isMainLandmarkSelector(selector) {
  return /^(?:main|article|\[role\s*=\s*['"]?main['"]?\])$/i.test(String(selector || '').trim());
}

/** challenge / login 是页面状态而不是错误：state= + hint，退出 0。 */
function readStateOutput(data, kind) {
  const hint = kind === 'challenge'
    ? 'complete the human-verification challenge in the browser, then re-run read; snapshot works after it passes'
    : 'page looks like a sign-in wall (visible password field + login-style title); use snapshot to find the form and act to log in, or open a page that is publicly readable';
  return [data.header, footer(0, 0, null), 'hint=' + hint].join('\n');
}

async function readTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be read', 'open an http(s) page');
  const mode = args.mode || 'md';
  const selector = args.selector;
  // PDF 由页面内 contentType / 扩展名判定：webRequest 只在 observe 期间开启，
  // 不再为 read 常驻记录主文档 MIME。
  await ensureHelpers(tab.id);
  if (selector) await requireValidSelector(tab.id, selector);
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  const useMain = args.main !== false && !selector;
  let data;
  let selectorFallback = '';
  try {
    data = await runInTab(tab.id, readPageContent, [mode, selector || null, useMain]);
  } catch (error) {
    await failInjection(tab, error, 'reading content');
  }
  if (data && data.missing) {
    if (!isMainLandmarkSelector(selector)) {
      fail('selector_not_found', `selector not found: ${selector}`, 'call snapshot to see available refs/roles');
    }
    selectorFallback = selector;
    try {
      data = await runInTab(tab.id, readPageContent, [mode, null, args.main !== false]);
    } catch (error) {
      await failInjection(tab, error, 'reading fallback content');
    }
  }
  if (!data || data.kind === 'helpers_missing') {
    fail('helpers_missing', 'page helpers are not installed', 'reload the tab and retry');
  }
  if (data.kind === 'page_exception') {
    fail('js_exception', `read's page helper threw: ${oneLine(data.message)}`,
      "the page may be hostile or mid-navigation; check the exception, retry after `ema2 await 2s`, then file a bug if it repeats");
  }
  if (selectorFallback) {
    data.header += `\nselector_fallback=1 selector=${oneLine(selectorFallback)}`;
  }
  if (data.kind === 'pdf') {
    fail('pdf_page', `the document is a PDF (content_type=${data.contentType || 'application/pdf'})`,
      'read cannot extract PDF text; open the html version, or let the user save the pdf and read it with a file tool');
  }
  if (data.kind === 'http_error') {
    fail('page_error', `the page responded http=${data.http} (content_type=${data.contentType || 'unknown'})`,
      'the url is dead or blocked; verify it, or check `ema2 status` / the tab title for what actually loaded');
  }
  if (data.kind === 'empty') {
    fail('empty_page', `read produced no text for mode=${mode}`,
      'the tab may still be loading; wait (`ema2 wait idle`) or wait selector, then retry; canvas/PDF-like pages need shot --annotate');
  }
  if (data.kind === 'challenge' || data.kind === 'login_required') {
    return readStateOutput(data, data.kind);
  }
  const slice = sliceText(data.text || '', budget, cursor);
  const parts = [data.header, slice.text, footer(slice.total, slice.text.length, slice.next)];
  if (slice.next != null) parts.push('truncated=1');
  const hint = truncationHint('read', slice.total, slice.text.length, slice.next, cursor);
  if (hint) {
    let message = hint.replace(/^hint=/, '');
    if (!cursor && slice.total <= MAX_OUTPUT_CHARS) {
      message += `, or --budget ${slice.total} --cursor 0 for all ${slice.total} chars`;
    }
    parts.push(`hint=${message} (keep --mode ${mode})`);
  }
  if (cursor > 0) parts.push(`hint=cursor is a character offset for mode=${mode}; use the same mode when continuing`);
  if (selectorFallback) {
    parts.push(`hint=selector "${oneLine(selectorFallback)}" not found; used default main/body instead`);
  } else if (data.mainFallback) {
    parts.push(`hint=main extraction too short (${data.textLength || 0} chars); fell back to body. Use --selector or --no-main to control this.`);
  }
  if (mode === 'visible' && !slice.total) {
    parts.push('hint=visible text is empty; the page may render in canvas/PDF/cross-origin iframe; try js or shot --annotate');
  }
  return parts.join('\n');
}

async function snapshotTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot be snapshotted', 'open an http(s) page');
  await ensureHelpers(tab.id);
  const filter = args.filter || 'interactive';
  const budget = normalizeBudget(args.budget);
  const cursor = normalizeCursor(args.cursor);
  let data;
  try {
    data = await runInTab(tab.id, (filterName, budgetValue, cursorValue, nearRef, showValues) => {
      return window.__ema2_helpers.snapshot(filterName, budgetValue, cursorValue, nearRef, showValues);
    }, [filter, budget, cursor, args.near || null, args.values === true || filter === 'form']);
  } catch (error) {
    await failInjection(tab, error, 'taking the snapshot');
  }
  if (!data) fail('snapshot_failed', 'snapshot produced no data', 'retry');
  const parts = [data.header];
  if (data.body) parts.push(data.body);
  parts.push(footer(data.total, data.shown, data.next));
  if (data.next != null) parts.push('truncated=1');
  const hint = truncationHint('snapshot', data.total, data.shown, data.next, cursor, 'refs');
  if (hint) parts.push(hint);
  const refs = Number((/refs=(\d+)/.exec(data.header) || [])[1] || 0);
  const visual = Number((/visual=(\d+)/.exec(data.header) || [])[1] || 0);
  if (refs === 0 && visual > 0) {
    parts.push('hint=page looks visual (canvas/img); try `ema2 shot --annotate`, or read the DOM with js');
  }
  return parts.join('\n');
}
