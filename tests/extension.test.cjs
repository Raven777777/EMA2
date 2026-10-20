'use strict';

const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const {
  assert,
  createChrome,
  loadBackground,
  setupDom,
  pageEval,
  pageRun,
  refsIn,
  expectToolError,
  ROOT,
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

  // 新实现只有一次动作注入（ensureHelpers + performActionInPage），不存在二次 find 后假成功。
  // ensureHelpers 在「真的装了 helper」时会多一次注入，用来下发文档句柄。
  const other = loadBackground();
  const calls = [];
  other.runInTab = async (_tabId, func) => {
    calls.push(func.name || 'setDocHandle');
    if (func.name === 'installHelpers') return { installed: true };
    if (!func.name) return true;
    return { found: true, ok: true, info: 'ref=e1@test' };
  };
  const info = await other.runStep({ id: 1 }, { action: 'click', ref: 'e1@test' });
  assert.deepEqual(calls, ['installHelpers', 'setDocHandle', 'performActionInPage']);
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

test('P2-4 network observe keeps ready records that follow a streaming preview', async () => {
  const background = loadBackground();
  background.runInTab = async (_tabId, func, args) => {
    const source = func.toString();
    if (source.includes('__ema2_net') && args.length === 0) return 'n-test';
    if (source.includes('__ema2_net') && args.length === 3) {
      return {
        seq: 3,
        epoch: 'n-test',
        reset: false,
        firstSeq: 1,
        items: [
          { seq: 1, type: 'fetch', method: 'GET', url: 'https://example.test/a', status: 200, ms: 1, body: '', preview: 'pending' },
          { seq: 2, type: 'fetch', method: 'GET', url: 'https://example.test/b', status: 201, ms: 2, body: 'ok', preview: 'done' },
          { seq: 3, type: 'fetch', method: 'GET', url: 'https://example.test/c', status: 204, ms: 3, body: '', preview: 'done' },
        ],
      };
    }
    return true;
  };

  const output = await background.observeTool({ kind: 'network', budget: 4000, tabId: 1, since: 0 });
  assert.match(output, /#2 GET 201 2ms \/b body=ok/, 'ready records after a pending one must not be dropped');
  assert.match(output, /#3 GET 204 3ms \/c/);
  assert.match(output, /#1 GET 200 1ms \/a streaming=1/, 'the streaming record is reported without inventing a body');
  assert.match(output, /pending=1 seqs=1/);
  assert.match(output, /next=3/, 'pagination must advance, not stall on the streaming entry');
  assert.match(output, /hint=body still streaming/);
});

test('P2-1 bodyFilter rejects exponential backtracking and still matches safe patterns', async () => {
  const background = loadBackground();
  background.runInTab = async (_tabId, func, args) => {
    const source = func.toString();
    if (source.includes('__ema2_net') && args.length === 0) return 'n-test';
    if (source.includes('__ema2_net') && args.length === 3) {
      return {
        seq: 2,
        epoch: 'n-test',
        reset: false,
        firstSeq: 1,
        items: [
          { seq: 1, type: 'fetch', method: 'GET', url: 'https://example.test/a', status: 200, ms: 1, body: 'ok', preview: 'done' },
          { seq: 2, type: 'fetch', method: 'GET', url: 'https://example.test/b', status: 500, ms: 2, body: 'boom', preview: 'done' },
        ],
      };
    }
    return true;
  };

  await assert.rejects(
    background.observeTool({ kind: 'network', bodyFilter: '(a+)+$', tabId: 1 }),
    expectToolError('invalid_regex'),
  );
  await assert.rejects(
    background.observeTool({ kind: 'network', bodyFilter: '(a|a)+$', tabId: 1 }),
    expectToolError('invalid_regex'),
  );
  await assert.rejects(
    background.observeTool({ kind: 'network', bodyFilter: '('.repeat(1) + 'a'.repeat(600) + ')', tabId: 1 }),
    expectToolError('invalid_regex'),
  );

  const filtered = await background.observeTool({ kind: 'network', bodyFilter: 'boom', budget: 4000, tabId: 1, since: 0 });
  assert.match(filtered, /#2 GET 500/);
  assert.doesNotMatch(filtered, /#1 GET 200/);
});

test('extension icons cover every manifest size and are valid RGBA PNGs', () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, 'extension', 'manifest.json'), 'utf8'));
  const declared = { ...manifest.icons, ...manifest.action.default_icon };
  for (const size of ['16', '48', '128']) {
    assert.ok(declared[size], `manifest must declare a ${size}px icon`);
  }

  const byFile = new Map();
  for (const [size, file] of Object.entries(declared)) {
    byFile.set(file, Number(size));
  }
  for (const [file, size] of byFile) {
    const bytes = fs.readFileSync(path.join(ROOT, 'extension', file));
    assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', `${file} is not a PNG`);
    assert.equal(bytes.readUInt32BE(16), size, `${file} width must be ${size}px`);
    assert.equal(bytes.readUInt32BE(20), size, `${file} height must be ${size}px`);
    assert.equal(bytes[25], 6, `${file} must be RGBA so the toolbar background shows through`);
  }
});

test('pairing: ws URL carries token or the pairing flag', async () => {
  const background = loadBackground();
  const anon = await background.getWsUrl({ wsUrl: 'ws://127.0.0.1:8765' });
  assert.match(anon, /\?pair=1$/, `expected pairing URL, got ${anon}`);
  const paired = await background.getWsUrl({ wsUrl: 'ws://127.0.0.1:8765', token: 'tok' });
  assert.match(paired, /\?token=tok$/, `expected token URL, got ${paired}`);
});

test('pairing: welcome proof is verified against the shared token', async () => {
  const nodeCrypto = require('node:crypto');
  const background = loadBackground();
  const token = 's3cret-token';
  const challenge = '0123456789abcdef';
  const proof = nodeCrypto
    .createHmac('sha256', token)
    .update(`ema2-welcome|${challenge}`)
    .digest('hex');

  background.authToken = token;
  background.currentChallenge = challenge;
  assert.equal(await background.verifyWelcome({ auth: 'hmac-v1', proof }), true);
  assert.equal(await background.verifyWelcome({ auth: 'hmac-v1', proof: '00'.repeat(32) }), false);
  assert.equal(await background.verifyWelcome({ auth: 'hmac-v1' }), false, 'missing proof must fail');
  assert.equal(await background.verifyWelcome({ proof }), false, 'missing auth marker must fail');

  // 匿名中继没有共享密钥，保持旧的“信任 welcome”行为。
  background.authToken = null;
  assert.equal(await background.verifyWelcome({}), true);
});

test('pairing: authorize stores the granted token and reconnects with it', async () => {
  const grantedToken = 'a'.repeat(64);
  const sent = [];

  class FakeSocket {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.sent = sent;
      setTimeout(() => {
        this.readyState = 1;
        if (this.onopen) this.onopen();
      }, 0);
    }

    send(payload) {
      sent.push(JSON.parse(payload));
      const message = JSON.parse(payload);
      if (message.method === 'bridge/hello') {
        setTimeout(() => this.onmessage && this.onmessage({
          data: JSON.stringify({ jsonrpc: '2.0', method: 'bridge/welcome', pairing: true }),
        }), 0);
      }
      if (message.method === 'bridge/pair-claim') {
        setTimeout(() => this.onmessage && this.onmessage({
          data: JSON.stringify({
            jsonrpc: '2.0',
            method: 'bridge/pair-granted',
            token: grantedToken,
            port: 8765,
            selection: ['opencode', 'codex'],
            clients: [{ id: 'opencode', label: 'opencode', path: 'C:/x/opencode.json', action: 'updated' }],
          }),
        }), 0);
      }
    }

    close() { this.readyState = 3; if (this.onclose) this.onclose(); }
  }

  const store = {};
  const chrome = createChrome({
    storage: {
      local: {
        get: async () => ({ ...store }),
        set: async (values) => { Object.assign(store, values); },
        remove: async (key) => { delete store[key]; },
      },
    },
  });
  const background = loadBackground(chrome);
  background.WebSocket = FakeSocket;
  // 配对成功后扩展会尝试重连；本测试只验证授权结果，不需要真的跑重连循环。
  background.connect = async () => {};
  background.fetch = async () => ({
    ok: true,
    json: async () => ({ reachable: true, pairing: true, paired: false }),
  });

  const result = await background.pairWithRelay(8765, ['opencode', 'codex']);
  assert.equal(result.ok, true);
  assert.equal(result.clients[0].id, 'opencode');
  assert.equal(store.token, grantedToken);
  assert.equal(store.wsUrl, 'ws://127.0.0.1:8765');
  assert.deepEqual(Array.from(store.pairedClients), ['opencode', 'codex'], 'selection must be remembered');
  assert.equal(background.authToken, grantedToken);
  const claim = sent.find((message) => message.method === 'bridge/pair-claim');
  assert.deepEqual(claim.clients, ['opencode', 'codex'], 'selection must be sent with the claim');
});

test('pairing: configure posts only the selected client ids', async () => {
  const background = loadBackground();
  background.authToken = 'tok';
  let captured = null;
  background.fetch = async (url, options) => {
    captured = { url, options };
    return { ok: true, json: async () => ({ clients: [], selection: ['codex'] }) };
  };

  const result = await background.configureClients(8765, ['codex']);
  assert.match(captured.url, /\/admin\/clients$/);
  assert.equal(captured.options.method, 'POST');
  assert.equal(captured.options.headers['content-type'], 'application/json');
  assert.deepEqual(JSON.parse(captured.options.body), { clients: ['codex'] });
  assert.equal(result.ok, true);
});

// ---------------------------------------------------------------- v1.11：tab 复用 / 状态码 / 截断 / js 函数体

function tabWorld() {
  const state = {
    nextId: 91,
    tabs: new Map([[7, { id: 7, windowId: 1, active: true, url: 'https://user.example/', title: 'User tab' }]]),
    removed: [],
    navigated: [],
  };
  const chrome = createChrome({
    tabs: {
      query: async () => [...state.tabs.values()],
      get: async (id) => {
        const tab = state.tabs.get(id);
        if (!tab) throw new Error(`no tab ${id}`);
        return tab;
      },
      create: async ({ url }) => {
        const tab = { id: state.nextId++, windowId: 1, active: true, url, title: 'T' };
        state.tabs.set(tab.id, tab);
        return tab;
      },
      update: async (id, patch) => {
        const tab = state.tabs.get(id);
        if (!tab) throw new Error(`no tab ${id}`);
        if (patch.url) { tab.url = patch.url; state.navigated.push({ id, url: patch.url }); }
        return tab;
      },
      remove: async (id) => {
        if (!state.tabs.has(id)) throw new Error('no tab ' + id);
        state.removed.push(id);
        state.tabs.delete(id);
      },
    },
  });
  return { chrome, state };
}

/**
 * open 需要「文档身份 / 导航计时 / hook」三类注入结果。
 * 文档身份必须跟着 tab.url 走：open 会等人真正落到目标文档后才回读 url/title。
 */
function stubOpenRuntime(background, state, status = 200) {
  background.runInTab = async (tabId, func) => {
    const source = func.toString();
    const tab = state ? state.tabs.get(tabId) : null;
    if (source.includes('getEntriesByType')) return { status, redirects: 0, type: 'navigate' };
    if (source.includes('document.readyState')) {
      return { href: (tab && tab.url) || '', ready: 'complete' };
    }
    return true;
  };
}

test('open reports the real document status, reuse mode and keeps user tabs out of the session list', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  stubOpenRuntime(background, state, 200);

  const fresh = await background.executeTool('open', { url: 'https://a.example/' });
  assert.match(fresh, /^id=91$/m);
  assert.match(fresh, /^status=200$/m);
  assert.match(fresh, /^reuse=new$/m);
  assert.equal(state.tabs.size, 2, 'default open must create exactly one tab');

  // replace：导航当前活动 tab，不新建，也不把用户的 tab 记进会话。
  const replaced = await background.executeTool('open', { url: 'https://b.example/', replace: true });
  assert.match(replaced, /^id=7$/m);
  assert.match(replaced, /^reuse=replace$/m);
  assert.equal(state.tabs.size, 2, 'replace must not create a tab');
  assert.deepEqual(state.navigated.at(-1), { id: 7, url: 'https://b.example/' });

  const list = await background.tabsTool({ action: 'list' });
  assert.match(list, /session_tabs=91/, 'only tabs EMA2 opened belong to the session');
});

test('open reuse=session navigates the last tab this session opened instead of piling up tabs', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  stubOpenRuntime(background, state);

  await background.executeTool('open', { url: 'https://a.example/' });
  const second = await background.executeTool('open', { url: 'https://c.example/', reuse: 'session' });
  assert.match(second, /^id=91$/m, 'reuse=session must land on tab 91');
  assert.match(second, /^reuse=session$/m);
  assert.equal(state.tabs.size, 2, 'reuse must not open a second tab');

  // 会话 tab 被用户关掉后，复用要退回新建，而不是失败。
  state.tabs.delete(91);
  const third = await background.executeTool('open', { url: 'https://d.example/', reuse: 'session' });
  assert.match(third, /^reuse=new$/m);
});

test('tabs close scope=session closes only the tabs EMA2 opened', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  stubOpenRuntime(background, state);

  await background.executeTool('open', { url: 'https://a.example/' });
  await background.executeTool('open', { url: 'https://b.example/' });
  const closed = await background.tabsTool({ action: 'close', scope: 'session' });
  assert.match(closed, /^closed=91,92$/m);
  assert.match(closed, /^count=2$/m);
  assert.deepEqual(state.removed, [91, 92]);
  assert.ok(state.tabs.has(7), 'the user tab must survive');

  const again = await background.tabsTool({ action: 'close', scope: 'session' });
  assert.match(again, /^closed=none$/m);
  assert.match(again, /^count=0$/m);
});

test('tabs close tabIds[] closes a batch and ignores ids that are already gone', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  const closed = await background.tabsTool({ action: 'close', tabIds: [7, 404] });
  assert.match(closed, /^closed=7$/m);
  assert.match(closed, /^count=1$/m);
  assert.deepEqual(state.removed, [7]);
  await assert.rejects(
    background.tabsTool({ action: 'close', tabIds: ['7'] }),
    expectToolError('invalid_argument'),
  );
});

test('open surfaces an http error status instead of pretending the page is fine', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  stubOpenRuntime(background, state, 404);
  const out = await background.executeTool('open', { url: 'https://a.example/missing' });
  assert.match(out, /^status=404$/m);
  assert.match(out, /hint=http 404/);
});

test('open rejects conflicting navigation targets before touching the browser', async () => {
  const { chrome, state } = tabWorld();
  const background = loadBackground(chrome);
  stubOpenRuntime(background, state);
  await assert.rejects(
    background.executeTool('open', { url: 'https://a.example/', replace: true, tabId: 7 }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.executeTool('open', { url: 'https://a.example/', reuse: 'all' }),
    expectToolError('invalid_argument'),
  );
  assert.equal(state.tabs.size, 1, 'no tab may be created when arguments are rejected');
});

test('read says explicitly that the text was truncated and which cursor continues it', async () => {
  const background = loadBackground(createChrome());
  background.runInTab = async () => ({ text: 'x'.repeat(1000), header: 'url=https://x.example/ title=T' });

  const out = await background.readTool({ budget: 400, tabId: 1 });
  assert.match(out, /^total=1000 shown=400 next=400$/m);
  assert.match(out, /hint=truncated: returned 400 of 1000 chars; call read with cursor=400 to continue/);

  const tail = await background.readTool({ budget: 400, cursor: 400, tabId: 1 });
  assert.match(tail, /^total=1000 shown=400 next=800$/m);
  assert.match(tail, /from cursor=400/);

  const done = await background.readTool({ budget: 2000, cursor: 0, tabId: 1 });
  assert.doesNotMatch(done, /hint=truncated/, 'no hint when everything fits');
});

test('js accepts a function body with top-level return, like the codemode sandbox', async () => {
  const background = loadBackground(createChrome());
  const seen = [];
  background.chrome.debugger.sendCommand = async (_target, _method, params) => {
    seen.push(params.expression);
    if (params.expression.startsWith('(async () =>')) {
      return { result: { type: 'string', value: 'https://detail.example/42' } };
    }
    return {
      exceptionDetails: {
        text: 'Uncaught',
        exception: { className: 'SyntaxError', description: 'SyntaxError: Illegal return statement' },
      },
    };
  };

  const code = 'const a = 1;\nreturn location.href;';
  const out = await background.jsTool({ code, tabId: 1 });
  assert.equal(out.trim(), 'https://detail.example/42');
  assert.equal(seen.length, 2, 'expression attempt first, then the async function body');
  assert.ok(seen[1].startsWith('(async () => {'), seen[1]);
  assert.ok(seen[1].includes(code), 'the wrapped body must keep the original code');
});

test('js flags a body that produced no value instead of silently returning nothing', async () => {
  const background = loadBackground(createChrome());
  let first = true;
  background.chrome.debugger.sendCommand = async () => {
    if (first) {
      first = false;
      return {
        exceptionDetails: {
          text: 'Uncaught',
          exception: { className: 'SyntaxError', description: 'SyntaxError: Illegal return statement' },
        },
      };
    }
    return { result: { type: 'undefined' } };
  };

  const out = await background.jsTool({ code: 'document.title = "x";', tabId: 1 });
  assert.match(out, /^undefined$/m);
  assert.match(out, /note=ran as an async function body/);
});

test('js keeps a real runtime error and gives a hint that matches the failure kind', async () => {
  const background = loadBackground(createChrome());
  background.chrome.debugger.sendCommand = async () => ({
    exceptionDetails: {
      text: 'Uncaught',
      exception: { className: 'TypeError', description: 'TypeError: Cannot read properties of null' },
    },
  });

  await assert.rejects(
    background.jsTool({ code: 'document.querySelector("x").click()', tabId: 1 }),
    (error) => {
      assert.equal(error.code, 'js_exception');
      assert.match(error.hint, /runtime error/);
      return true;
    },
  );
});

test('js reports a syntax error after the body fallback and points at the real cause', async () => {
  const background = loadBackground(createChrome());
  let calls = 0;
  background.chrome.debugger.sendCommand = async () => {
    calls++;
    return {
      exceptionDetails: {
        text: 'Uncaught',
        exception: { className: 'SyntaxError', description: 'SyntaxError: Unexpected token }' },
      },
    };
  };

  await assert.rejects(background.jsTool({ code: 'if (a) {', tabId: 1 }), (error) => {
    assert.equal(error.code, 'js_exception');
    assert.match(error.hint, /statement body/);
    return true;
  });
  assert.equal(calls, 2);
});

test('batch urls opens each url and then runs the tool on that new tab', async () => {
  const background = loadBackground(createChrome());
  const order = [];
  background.openTool = async ({ url }) => {
    order.push(`open:${url}`);
    const id = url.includes('a.example') ? 11 : 12;
    return { content: [{ type: 'text', text: `id=${id}\nurl=${url}\ntitle=A\nstatus=200\nreuse=new\nnet_hook=on` }] };
  };
  background.readTool = async ({ tabId }) => {
    order.push(`read:${tabId}`);
    return `url=https://x/${tabId}\nbody of ${tabId}`;
  };

  const out = await background.batchTool({
    tool: 'read',
    urls: ['https://a.example/', 'https://b.example/'],
    concurrency: 2,
    args: { mode: 'md' },
  });
  assert.match(out, /^url=https:\/\/a\.example\/\ntab=11\nurl=https:\/\/x\/11$/m);
  assert.match(out, /^url=https:\/\/b\.example\/\ntab=12\nurl=https:\/\/x\/12$/m);
  assert.deepEqual(order.sort(), [
    'open:https://a.example/',
    'open:https://b.example/',
    'read:11',
    'read:12',
  ]);
});

test('batch open accepts urls without tabIds and rejects mixing the two', async () => {
  const background = loadBackground(createChrome());
  background.openTool = async ({ url }) => ({
    content: [{ type: 'text', text: `id=5\nurl=${url}\nstatus=200\nreuse=new\nnet_hook=on` }],
  });
  const out = await background.batchTool({ tool: 'open', urls: ['https://a.example/'] });
  assert.match(out, /^url=https:\/\/a\.example\/\nid=5/m);

  await assert.rejects(
    background.batchTool({ tool: 'open', urls: ['https://a.example/'], tabIds: [1] }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.batchTool({ tool: 'read', urls: ['file:///etc/passwd'] }),
    expectToolError('invalid_argument'),
  );
  await assert.rejects(
    background.batchTool({ tool: 'act', urls: ['https://a/'] }),
    expectToolError('invalid_tool'),
  );
});

test('open waits for the new document instead of reporting the page it just left', async () => {
  // 真实浏览器里 tabs.update 之后旧文档还活着且 readyState=complete，
  // 只判 readyState 会让 open 立刻返回上一页的 url/title。
  const tab = { id: 7, windowId: 1, active: true, url: 'https://user.example/', title: 'User tab', status: 'complete' };
  let committed = 'https://user.example/';
  let docHref = 'https://user.example/';
  let probes = 0;
  const chrome = createChrome({
    tabs: {
      query: async () => [tab],
      get: async () => ({ ...tab, url: committed }),
      update: async (_id, patch) => {
        tab.pendingUrl = patch.url;
        return { ...tab, url: committed };
      },
      create: async () => { throw new Error('replace must not create a tab'); },
    },
  });
  const background = loadBackground(chrome);
  background.runInTab = async (_tabId, func) => {
    const source = func.toString();
    if (source.includes('getEntriesByType')) return { status: 200, redirects: 0, type: 'navigate' };
    if (source.includes('document.readyState')) {
      probes += 1;
      if (probes > 3) {
        docHref = tab.pendingUrl;
        committed = tab.pendingUrl;
      }
      return { href: docHref, ready: 'complete' };
    }
    return true;
  };

  const out = await background.executeTool('open', { url: 'https://b.example/', replace: true });
  assert.ok(out.includes('url=https://b.example/'), 'url must be the new document, got: ' + out);
  assert.ok(out.includes('reuse=replace'));
  assert.ok(probes > 3, 'must keep polling until the document really changed');
});

// ---------------------------------------------------------------- v1.12：快照瘦身

test('snapshot drops nameless containers, clips long hrefs and stays lossless for filter=full', () => {
  const background = loadBackground();
  const dom = setupDom(
    '<div role="presentation"></div>' +
    '<div role="banner"></div>' +
    '<a href="/short">Short</a>' +
    '<a href="/a/very/long/path/that/exceeds/forty/chars/for/sure">Long</a>' +
    '<button id="icon"></button>',
    1,
    background,
  );
  const helpers = dom.window.__ema2_helpers;

  const body = helpers.snapshot('interactive', 8000, 0).body;
  assert.doesNotMatch(body, /presentation/, 'nameless non-actionable nodes are noise');
  assert.doesNotMatch(body, /banner/);
  assert.match(body, /link "Short" href=\/short/);
  assert.match(body, /…/, 'long href must be clipped with an ellipsis');
  assert.match(body, /button ""/, 'an actionable control survives even without a name');

  const full = helpers.snapshot('full', 8000, 0).body;
  assert.match(full, /presentation/, 'filter=full keeps the lossless view');

  const links = helpers.snapshot('links', 8000, 0).body;
  assert.match(links, /\/a\/very\/long\/path\/that\/exceeds\/forty\/chars\/for\/sure/, 'filter=links keeps full urls');
});

test('refs carry a short document handle and long-form refs are rejected', () => {
  const background = loadBackground();
  const dom = setupDom('<button>Go</button>', 7, background);
  const helpers = dom.window.__ema2_helpers;
  const ref = refsIn(helpers.snapshot('interactive', 8000, 0).body)[0];

  assert.match(ref, /^e\d+@d\d+$/, 'ref must be short, got ' + ref);
  assert.ok(helpers.refIsCurrent(ref));
  assert.equal(helpers.refIsCurrent('e1@t7-k3a9'), false, 'the old tab-random form is gone');
  assert.equal(helpers.find('e1@t7-k3a9', null, null), null);
  assert.equal(helpers.refIsCurrent('e1'), false, 'a bare ref is not enough to identify a document');
});

test('each new document gets its own handle and repeat injections do not burn numbers', async () => {
  const background = loadBackground();
  const handles = [];
  let pendingInstall = true;
  background.runInTab = async (_tabId, func, args) => {
    if (func && func.name === 'installHelpers') {
      const installed = pendingInstall;
      pendingInstall = false;
      return { installed };
    }
    handles.push(args[0]);
    return true;
  };

  await background.ensureHelpers(1);
  await background.ensureHelpers(1);
  assert.deepEqual(handles, ['d1'], 'a page that is already wired must not consume a handle');

  pendingInstall = true; // 同一个 tab 上换了一个文档
  await background.ensureHelpers(1);
  assert.deepEqual(handles, ['d1', 'd2']);
});

test('two tabs installing at the same moment never share a document handle', async () => {
  const background = loadBackground(createChrome({
    storage: {
      local: { get: async () => ({}), set: async () => {} },
      session: {
        // 慢一点，让两个 tab 的首次读取真正重叠
        get: async () => { await new Promise(resolve => setTimeout(resolve, 5)); return { ema2_doc_seq: 0 }; },
        set: async () => {},
      },
    },
  }));
  const handles = [];
  background.runInTab = async (_tabId, func, args) => {
    if (func && func.name === 'installHelpers') return { installed: true };
    handles.push(args[0]);
    return true;
  };

  await Promise.all([background.ensureHelpers(1), background.ensureHelpers(2)]);
  assert.equal(new Set(handles).size, 2, 'handles must be unique, got ' + handles.join(','));
});

test('document handles resume from the persisted counter instead of restarting at d1', async () => {
  // 扩展重载会清空 session storage；计数器必须落 local，否则新文档的 d1 会和旧页面撞号。
  const background = loadBackground(createChrome({
    storage: {
      local: { get: async () => ({ ema2_doc_seq: 41 }), set: async () => {} },
      session: { get: async () => ({}), set: async () => {} },
    },
  }));
  const handles = [];
  background.runInTab = async (_tabId, func, args) => {
    if (func && func.name === 'installHelpers') return { installed: true };
    handles.push(args[0]);
    return true;
  };

  await background.ensureHelpers(1);
  assert.deepEqual(handles, ['d42'], 'must continue from the stored counter');
});
