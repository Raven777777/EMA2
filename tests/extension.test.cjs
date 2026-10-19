'use strict';

const { test } = require('node:test');
const {
  assert,
  createChrome,
  loadBackground,
  setupDom,
  pageEval,
  pageRun,
  refsIn,
  expectToolError,
} = require('./helpers.cjs');

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));

function responseFixture(options = {}) {
  const {
    status = 200,
    contentType = 'text/plain; charset=utf-8',
    cloneBody = null,
    cloneText = null,
  } = options;
  return {
    status,
    headers: { get: (name) => (String(name).toLowerCase() === 'content-type' ? contentType : null) },
    clone() {
      return {
        body: cloneBody,
        text: cloneText || (async () => ''),
      };
    },
  };
}

test('F01 fetch hook returns the Response immediately and preview never blocks the page', async () => {
  const background = loadBackground();
  const dom = setupDom('<p>page</p>');
  let cloned = 0;
  dom.window.fetch = async () => {
    const response = responseFixture({
      cloneBody: {
        getReader() {
          return {
            read: () => new Promise(() => {}), // 永不结束的流
            cancel: async () => {},
          };
        },
      },
    });
    const originalClone = response.clone;
    response.clone = () => { cloned++; return originalClone(); };
    return response;
  };
  pageEval(dom, background.installNetworkHook);

  const status = await Promise.race([
    pageRun(dom, 'window.fetch("/api/stream").then(r => r.status)'),
    sleep(300).then(() => { throw new Error('page fetch was blocked by the preview reader'); }),
  ]);
  assert.equal(status, 200);
  assert.equal(dom.window.__ema2_net.entries.length, 1);
  assert.equal(dom.window.__ema2_net.entries[0].status, 200);
  await sleep(20);
  assert.equal(dom.window.__ema2_net.entries[0].preview, 'pending');
  assert.equal(cloned, 1);
});

test('F01 response preview is bounded and cancels its reader', async () => {
  const background = loadBackground();
  const dom = setupDom('<p>page</p>');
  let canceled = false;
  let reads = 0;
  dom.window.fetch = async () => responseFixture({
    cloneBody: {
      getReader() {
        return {
          async read() {
            reads++;
            return { done: false, value: new Uint8Array(1000).fill(65) };
          },
          async cancel() { canceled = true; },
        };
      },
    },
  });
  pageEval(dom, background.installNetworkHook);
  await pageRun(dom, 'window.fetch("/api/big").then(() => 1)');
  const entry = dom.window.__ema2_net.entries[0];
  for (let i = 0; i < 100 && entry.preview === 'pending'; i++) await sleep(10);
  assert.equal(entry.preview, 'truncated');
  assert.equal(entry.body.length, 800);
  assert.equal(canceled, true);
  assert.ok(reads >= 8 && reads <= 12, `expected bounded reads, got ${reads}`);
});

test('F01 bodies=false really disables cloning and reading new bodies', async () => {
  const background = loadBackground();
  const dom = setupDom('<p>page</p>');
  let cloned = 0;
  dom.window.fetch = async () => {
    const response = responseFixture();
    const originalClone = response.clone;
    response.clone = () => { cloned++; return originalClone(); };
    return response;
  };
  pageEval(dom, background.installNetworkHook);
  dom.window.__ema2_net.captureBodies = false;
  await pageRun(dom, 'window.fetch("/api/no-body").then(() => 1)');
  entryCheck();
  await sleep(20);
  assert.equal(cloned, 0);
  assert.equal(dom.window.__ema2_net.entries[0].preview, 'off');

  function entryCheck() {
    assert.equal(dom.window.__ema2_net.entries.length, 1);
  }
});

test('F02 password values never enter names, snapshots, candidates, marks or action feedback', () => {
  const background = loadBackground();
  const dom = setupDom(
    '<label for="pw">Password</label><input id="pw" type="password" value="SYNTHETIC_PASSWORD">' +
    '<input id="q" placeholder="Search">',
    7,
    background,
  );
  const document = dom.window.document;
  const helpers = dom.window.__ema2_helpers;
  const password = document.getElementById('pw');

  assert.doesNotMatch(helpers.nameOf(password), /SYNTHETIC_PASSWORD/);
  const snapshot = helpers.snapshot('interactive', 20000, 0);
  assert.doesNotMatch(JSON.stringify(snapshot), /SYNTHETIC_PASSWORD/);
  assert.doesNotMatch(helpers.candidates(10).join('\n'), /SYNTHETIC_PASSWORD/);
  assert.doesNotMatch(helpers.marks().map(mark => mark.name).join('\n'), /SYNTHETIC_PASSWORD/);

  const ref = helpers.refOf(password);
  const result = pageEval(dom, background.performActionInPage, {
    action: 'type',
    ref,
    text: 'ANOTHER_SYNTHETIC_SECRET',
  });
  assert.equal(result.ok, true);
  assert.doesNotMatch(JSON.stringify(result), /ANOTHER_SYNTHETIC_SECRET/);
  assert.match(result.info, /wrote=1 chars=24/);
  assert.equal(password.value, 'ANOTHER_SYNTHETIC_SECRET');
});

test('F03 refs are document-scoped; navigation/reload/cross-tab refs fail and same-doc refs stay stable', async () => {
  const background = loadBackground();
  const html = '<button id="go">Go</button><input id="q">';

  const tab7 = setupDom(html, 7, background);
  const first = tab7.window.__ema2_helpers.snapshot('interactive', 20000, 0);
  const refA = refsIn(first.body)[0];
  assert.ok(refA, 'snapshot should produce a ref');
  const second = tab7.window.__ema2_helpers.snapshot('interactive', 20000, 0);
  assert.equal(refsIn(second.body)[0], refA, 'same document must keep a stable ref');

  // 同 tab、同 URL 的新文档（硬导航/刷新）必须得到新的 epoch。
  const reloaded = setupDom(html, 7, background);
  const reloadedSnapshot = reloaded.window.__ema2_helpers.snapshot('interactive', 20000, 0);
  const refB = refsIn(reloadedSnapshot.body)[0];
  assert.notEqual(refB, refA);
  assert.equal(reloaded.window.__ema2_helpers.find(refA, null, null), null);
  assert.equal(reloaded.window.__ema2_helpers.find(refA, '#go', null), null, 'stale ref must not fall back to selector');
  assert.equal(reloaded.window.__ema2_helpers.refIsCurrent(refA), false);
  assert.equal(tab7.window.__ema2_helpers.refIsCurrent(refA), true);

  const stale = pageEval(reloaded, background.performActionInPage, { action: 'click', ref: refA });
  assert.equal(stale.found, false);
  assert.equal(stale.stale, true);
  assert.throws(
    () => background.mapActionResult({ action: 'click', ref: refA }, stale),
    expectToolError('stale_ref'),
  );

  const otherTab = setupDom(html, 99, background);
  const otherSnapshot = otherTab.window.__ema2_helpers.snapshot('interactive', 20000, 0);
  const refC = refsIn(otherSnapshot.body)[0];
  assert.notEqual(refC, refA);
  assert.equal(otherTab.window.__ema2_helpers.find(refA, null, null), null);

  // files upload 也必须拒绝旧文档 ref，而不是落到 deepFind 的 target_not_found。
  const filesBackground = loadBackground(createChrome({
    tabs: { get: async (id) => ({ id, windowId: 1, active: true, url: 'https://example.test/', title: 'x' }) },
  }));
  filesBackground.runInTab = async (_tabId, func, args) => {
    if (func.name === 'installHelpers') return true;
    if (args.length === 1 && func.toString().includes('refIsCurrent')) return { current: false };
    return null;
  };
  await assert.rejects(
    filesBackground.filesTool({ action: 'upload', tabId: 1, ref: refA, paths: ['C:/tmp/a.pdf'] }),
    expectToolError('stale_ref'),
  );
});

test('F05 debugger queue releases after a hanging promise and the next task succeeds', async () => {
  const commands = [];
  let attaches = 0;
  let detaches = 0;
  const chrome = createChrome({
    debugger: {
      attach: async () => { attaches++; },
      detach: async () => { detaches++; },
      sendCommand: async (_target, method) => { commands.push(method); return {}; },
    },
  });
  const background = loadBackground(chrome);

  const hung = background.withDebugger(1, () => new Promise(() => {}), {
    timeoutMs: 40,
    queueWaitMs: 1000,
  });
  await assert.rejects(hung, expectToolError('debugger_timeout'));
  assert.ok(commands.includes('Runtime.terminateExecution'));
  assert.equal(attaches, 1);
  assert.equal(detaches, 1);

  const value = await background.withDebugger(2, async () => 42, {
    timeoutMs: 500,
    queueWaitMs: 1000,
  });
  assert.equal(value, 42);
  assert.equal(detaches, 2);
});

test('F05 queued task starts after the hung task is terminated', async () => {
  const chrome = createChrome({
    debugger: {
      attach: async () => {},
      detach: async () => {},
      sendCommand: async () => ({}),
    },
  });
  const background = loadBackground(chrome);
  const first = background.withDebugger(1, () => new Promise(() => {}), {
    timeoutMs: 30,
    queueWaitMs: 1000,
  });
  const second = background.withDebugger(2, async () => 'second', {
    timeoutMs: 500,
    queueWaitMs: 1000,
  });
  await assert.rejects(first, expectToolError('debugger_timeout'));
  assert.equal(await second, 'second');
});

test('F06 shot rejects inactive targets and detects a tab switch during capture', async () => {
  let activeId = 1;
  const chrome = createChrome({
    tabs: {
      get: async (id) => ({ id, windowId: 5, active: id === activeId, url: 'https://example.test/', title: 'x' }),
      query: async () => [{ id: activeId, windowId: 5, active: true }],
      captureVisibleTab: async () => 'data:image/png;base64,AAAA',
    },
  });
  const background = loadBackground(chrome);

  await assert.rejects(background.shotTool({ tabId: 2 }), expectToolError('tab_not_active'));

  activeId = 2;
  const shot = await background.shotTool({ tabId: 2 });
  assert.equal(shot.content[0].type, 'image');
  assert.equal(shot.content[0].data, 'AAAA');

  // 截图期间切换活动 tab：必须失败，不能把前台页图片当目标页返回。
  activeId = 2;
  chrome.tabs.captureVisibleTab = async () => {
    activeId = 1;
    return 'data:image/png;base64,AAAA';
  };
  await assert.rejects(background.shotTool({ tabId: 2 }), expectToolError('tab_not_active'));
});

test('F07 console pagination uses the delivered record as next and reports dropped history', async () => {
  const consoleEntries = [1, 2, 3].map(seq => ({
    seq,
    level: 'log',
    order: 1,
    text: `${seq}:` + 'x'.repeat(240),
  }));
  const background = loadBackground();
  background.runInTab = async (_tabId, func, args) => {
    const source = func.toString();
    if (source.includes('__ema2_console') && args.length === 0) return 'c-test';
    if (source.includes('__ema2_console') && args.length === 3) {
      const since = args[1];
      return {
        entries: consoleEntries.filter(entry => entry.seq > (args[2] ? 0 : since)),
        seq: 3,
        epoch: 'c-test',
        firstSeq: 1,
        reset: !!args[2],
      };
    }
    return true;
  };

  const seen = [];
  let since = 0;
  for (let round = 0; round < 5; round++) {
    const output = await background.observeTool({ kind: 'console', level: 'log', budget: 250, since, tabId: 1 });
    seen.push(...[...output.matchAll(/^#(\d+) /gm)].map(match => Number(match[1])));
    const next = /next=(\d+|none)/.exec(output);
    assert.ok(next, `missing next footer in: ${output}`);
    if (next[1] === 'none') break;
    const value = Number(next[1]);
    if (value === since) break;
    since = value;
  }
  assert.deepEqual(seen, [1, 2, 3], 'every retained console record must be reachable via next');

  background.runInTab = async (_tabId, func, args) => {
    if (func.toString().includes('__ema2_console') && args.length === 0) return 'c-test';
    if (func.toString().includes('__ema2_console') && args.length === 3) {
      return {
        entries: [{ seq: 5, level: 'log', order: 1, text: 'kept' }],
        seq: 7,
        epoch: 'c-test',
        firstSeq: 5,
        reset: false,
      };
    }
    return true;
  };
  const dropped = await background.observeTool({ kind: 'console', level: 'log', budget: 400, since: 0, tabId: 1 });
  assert.match(dropped, /dropped=4/);
  assert.match(dropped, /#5 /);
  assert.doesNotMatch(dropped, /#1 /);
});

test('F07 observe resets on document epoch change even when new seq exceeds the old since', async () => {
  const background = loadBackground();
  let epoch = 'c-one';
  let all = [{ seq: 1, level: 'log', order: 1, text: 'one' }];
  background.runInTab = async (_tabId, func, args) => {
    const source = func.toString();
    if (source.includes('__ema2_console') && args.length === 0) return epoch;
    if (source.includes('__ema2_console') && args.length === 3) {
      const start = args[2] ? 0 : args[1];
      return {
        entries: all.filter(entry => entry.seq > start),
        seq: all.length,
        epoch,
        firstSeq: 1,
        reset: !!args[2] || args[1] > all.length,
      };
    }
    return true;
  };
  const first = await background.observeTool({ kind: 'console', level: 'log', budget: 400, tabId: 1, since: 0 });
  assert.match(first, /#1 /);

  epoch = 'c-two';
  all = Array.from({ length: 5 }, (_, index) => ({ seq: index + 1, level: 'log', order: 1, text: `n${index + 1}` }));
  const second = await background.observeTool({ kind: 'console', level: 'log', budget: 400, tabId: 1, since: 2 });
  assert.match(second, /#1 /, 'new document must be read from its own start');
  assert.match(second, /reset=1/);
});

test('F07 network pagination does not drop the first 80 entries', async () => {
  const background = loadBackground();
  background.runInTab = async (_tabId, func, args) => {
    const source = func.toString();
    if (source.includes('__ema2_net') && args.length === 0) return 'n-test';
    if (source.includes('__ema2_net') && args.length === 3) {
      const since = args[0];
      const entries = Array.from({ length: 100 }, (_, index) => ({
        seq: index + 1,
        type: 'fetch',
        method: 'GET',
        url: `https://example.test/api/${index + 1}`,
        status: 200,
        ms: 5,
        body: '',
        preview: 'done',
      })).filter(entry => entry.seq > (args[2] ? 0 : since));
      return { seq: 100, epoch: 'n-test', reset: !!args[2], firstSeq: 1, items: entries };
    }
    return true;
  };

  const seen = [];
  let since = 0;
  for (let round = 0; round < 10; round++) {
    const output = await background.observeTool({ kind: 'network', budget: 2000, since, tabId: 1 });
    seen.push(...[...output.matchAll(/^#(\d+) /gm)].map(match => Number(match[1])));
    const next = /next=(\d+|none)/.exec(output);
    assert.ok(next, `missing next footer in: ${output}`);
    if (next[1] === 'none') break;
    const value = Number(next[1]);
    if (value === since) break;
    since = value;
  }
  assert.deepEqual(seen, Array.from({ length: 100 }, (_, index) => index + 1));
  assert.equal(new Set(seen).size, 100);
});

test('F09 markdown preserves root text and mixed text/block children without duplication', () => {
  const background = loadBackground();
  const mixed = setupDom('<div id="root">前言<p>正文</p><span>强调</span>结尾</div>', 1, background);
  const markdown = mixed.window.__ema2_helpers.markdown(mixed.window.document.getElementById('root'));
  assert.deepEqual(markdown.split('\n\n'), ['前言', '正文', '强调', '结尾']);

  const paragraph = setupDom('<p id="root">只有直接文本</p>', 2, background);
  assert.equal(
    paragraph.window.__ema2_helpers.markdown(paragraph.window.document.getElementById('root')),
    '只有直接文本',
  );

  const nested = setupDom('<div id="root"><h1>标题</h1><pre>a\nb</pre><ul><li>一</li><li>二</li></ul></div>', 3, background);
  const output = nested.window.__ema2_helpers.markdown(nested.window.document.getElementById('root'));
  assert.match(output, /^# 标题/m);
  assert.match(output, /```\na\nb\n```/);
  assert.match(output, /- 一/);
  assert.match(output, /- 二/);
});

test('F10 actions fail on disabled/read-only/unsupported controls and use one injection', async () => {
  const background = loadBackground();
  const dom = setupDom(
    '<button id="disabled" disabled>Disabled</button>' +
    '<input id="ro" readonly value="x">' +
    '<input id="check" type="checkbox">' +
    '<button id="go">Go</button>',
    1,
    background,
  );
  const helpers = dom.window.__ema2_helpers;
  const document = dom.window.document;

  const disabled = pageEval(dom, background.performActionInPage, {
    action: 'click',
    ref: helpers.refOf(document.getElementById('disabled')),
  });
  assert.equal(disabled.found, true);
  assert.equal(disabled.ok, false);
  assert.equal(disabled.code, 'element_disabled');

  const readonly = pageEval(dom, background.performActionInPage, {
    action: 'type',
    ref: helpers.refOf(document.getElementById('ro')),
    text: 'nope',
  });
  assert.equal(readonly.ok, false);
  assert.equal(readonly.code, 'element_readonly');

  const unsupported = pageEval(dom, background.performActionInPage, {
    action: 'fill',
    fields: [{ ref: helpers.refOf(document.getElementById('check')), value: 'yes' }],
  });
  assert.equal(unsupported.found, true);
  assert.equal(unsupported.ok, false);
  assert.equal(unsupported.code, 'unsupported_control');

  // 新实现只有一次页面注入（ensureHelpers + performActionInPage），不存在二次 find 后假成功。
  const other = loadBackground();
  const calls = [];
  other.runInTab = async (_tabId, func) => {
    calls.push(func.name);
    if (func.name === 'installHelpers') return true;
    return { found: true, ok: true, info: 'ref=e1@test' };
  };
  const info = await other.runStep({ id: 1 }, { action: 'click', ref: 'e1@test' });
  assert.deepEqual(calls, ['installHelpers', 'performActionInPage']);
  assert.match(info, /ref=e1@test/);
});

test('F13 press targets the located ref, or the active element when no locator is given', () => {
  const background = loadBackground();
  const dom = setupDom('<input id="i1"><input id="i2">', 1, background);
  const helpers = dom.window.__ema2_helpers;
  const document = dom.window.document;
  const first = document.getElementById('i1');
  const second = document.getElementById('i2');
  const events = [];
  first.addEventListener('keydown', event => events.push(event.target.id));
  second.addEventListener('keydown', event => events.push(event.target.id));

  first.focus();
  const ref = helpers.refOf(second);
  const targeted = pageEval(dom, background.performActionInPage, { action: 'press', ref, key: 'a' });
  assert.equal(targeted.ok, true);
  assert.deepEqual(events, ['i2']);
  assert.match(targeted.info, /ref=/);

  events.length = 0;
  first.focus();
  const active = pageEval(dom, background.performActionInPage, { action: 'press', key: 'b' });
  assert.equal(active.ok, true);
  assert.deepEqual(events, ['i1']);
  assert.match(active.info, /target=active/);
});

test('F11 waitForResponse starts at the new document epoch instead of using min(seq) across documents', async () => {
  const background = loadBackground();
  background.ensureNetworkHook = async () => true;
  let scanArgs = null;
  background.runInTab = async (_tabId, _func, args) => {
    if (args.length === 0) return { epoch: 'new-document', seq: 1 };
    if (args.length === 3) {
      scanArgs = args;
      return {
        hit: { method: 'GET', status: 200, url: 'https://example.test/api/save', ms: 5, seq: 1 },
        uncertain: false,
      };
    }
    return null;
  };
  const output = await background.waitForResponse(
    1,
    { url: 'api/save', status: 200, timeout: 0.05 },
    { epoch: 'old-document', seq: 20 },
  );
  assert.equal(scanArgs[0], 0, 'a new document must be scanned from seq 0');
  assert.match(output, /response=200 GET \/api\/save 5ms doc=reset/);

  background.runInTab = async (_tabId, _func, args) => {
    if (args.length === 0) return { epoch: 'new-document', seq: 1 };
    return { hit: null, uncertain: true };
  };
  await assert.rejects(
    background.waitForResponse(1, { url: 'api/save', timeout: 0.02 }, { epoch: 'new-document', seq: 0 }),
    (error) => {
      assert.equal(error.code, 'response_timeout');
      assert.match(error.message, /observation_gap=status_unavailable/);
      return true;
    },
  );
});

test('F12 batch and wait reject resource-range bypasses before doing browser work', async () => {
  const background = loadBackground(createChrome({
    tabs: { get: async (id) => ({ id, windowId: 1, active: true, url: 'https://example.test/', title: 'x' }) },
  }));
  await assert.rejects(
    background.batchTool({ tool: 'wait', tabIds: [1], args: { time: 3600 } }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.batchTool({ tool: 'js', tabIds: [1], args: {} }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.batchTool({ tool: 'wait', tabIds: [1, '2'], args: { time: 1 } }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.waitTool({ tabId: 1, time: 3600 }),
    expectToolError('invalid_argument'),
  );

  background.executeTool = async (name, args) => ({
    content: [{ type: 'text', text: `${name}:${args.tabId}` }],
  });
  const output = await background.batchTool({
    tool: 'wait',
    tabIds: [1, 2],
    concurrency: 2,
    args: { time: 0 },
  });
  assert.match(output, /tab=1\nwait:1/);
  assert.match(output, /tab=2\nwait:2/);
});
