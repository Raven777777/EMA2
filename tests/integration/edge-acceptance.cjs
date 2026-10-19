'use strict';

/**
 * Real Edge / MV3 acceptance runner.
 *
 * Requirements:
 *   - Windows with Microsoft Edge installed
 *   - bin/ema2.exe built from the current source
 *   - no other process on the chosen relay/debug ports
 *
 * It uses an isolated Edge user-data-dir and loads extension/ unpacked.
 * It never attaches to the user's normal Edge profile.
 *
 * Run:
 *   npm run test:edge
 * Optional env:
 *   EMA2_EDGE_EXE, EMA2_RELAY_EXE, EMA2_EDGE_PORT, EMA2_EDGE_DEBUG_PORT
 *   EMA2_EDGE_HEADLESS=1     (headless new; shot test will likely be skipped/fail)
 *   EMA2_EDGE_SKIP_SLOW=1    (skip the 60s hanging-JS test)
 *   EMA2_EDGE_KEEP=1         (keep profile/output on failure)
 */

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '..', '..');
const EDGE = process.env.EMA2_EDGE_EXE
  || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const RELAY = process.env.EMA2_RELAY_EXE || path.join(ROOT, 'bin', 'ema2.exe');
const EDGE_EXTENSION = path.join(ROOT, 'extension');
const HEADLESS = process.env.EMA2_EDGE_HEADLESS === '1';
const SKIP_SLOW = process.env.EMA2_EDGE_SKIP_SLOW === '1';
const KEEP = process.env.EMA2_EDGE_KEEP === '1';
const SECRET = 'REAL_SYNTHETIC_PASSWORD_9f3c';

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));

function log(message) {
  process.stdout.write(`${message}\n`);
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

async function waitFor(fn, timeoutMs, label, intervalMs = 200) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) {
      last = error;
    }
    await sleep(intervalMs);
  }
  throw new Error(`${label} timed out${last ? `: ${last.message}` : ''}`);
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

async function fetchJson(url, options = {}) {
  const response = await fetch(url, options);
  const text = await response.text();
  let body = null;
  try { body = JSON.parse(text); } catch (_) { body = text; }
  return { status: response.status, headers: response.headers, body, text };
}

class McpClient {
  constructor(url) {
    this.url = url;
    this.sessionId = null;
    this.nextId = 1;
  }

  async request(method, params) {
    const body = {
      jsonrpc: '2.0',
      id: this.nextId++,
      method,
      params,
    };
    const headers = { 'content-type': 'application/json' };
    if (this.sessionId) headers['mcp-session-id'] = this.sessionId;
    const response = await fetch(this.url, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });
    this.sessionId = response.headers.get('mcp-session-id') || this.sessionId;
    const text = await response.text();
    let data;
    try { data = JSON.parse(text); } catch (_) {
      throw new Error(`MCP returned invalid JSON (HTTP ${response.status}): ${text.slice(0, 300)}`);
    }
    if (data.error) throw new Error(`MCP protocol error: ${data.error.message || JSON.stringify(data.error)}`);
    return data;
  }

  async initialize() {
    await this.request('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'ema2-edge-acceptance', version: '1.0.0' },
    });
  }

  async tool(name, args = {}) {
    const data = await this.request('tools/call', { name, arguments: args });
    const result = data.result || {};
    const content = Array.isArray(result.content) ? result.content : [];
    const text = content.filter(item => item.type === 'text').map(item => item.text).join('\n');
    const images = content.filter(item => item.type === 'image');
    return { text, images, isError: !!result.isError, result };
  }

  async ok(name, args = {}) {
    const value = await this.tool(name, args);
    if (value.isError) throw new Error(`${name} failed: ${value.text}`);
    return value.text;
  }
}

function field(text, key) {
  const match = new RegExp(`^${key}=(.*)$`, 'm').exec(text || '');
  return match ? match[1].trim() : undefined;
}

function tabRefs(text) {
  return [...String(text).matchAll(/^\[(e\d+@[^\]\s]+)\]\s+(.*)$/gm)].map(match => ({
    ref: match[1],
    line: match[2],
  }));
}

function findRef(text, predicate, description) {
  const found = tabRefs(text).find(item => predicate(item.line));
  if (!found) throw new Error(`cannot find ref for ${description} in snapshot:\n${text.slice(0, 1200)}`);
  return found.ref;
}

function parseNext(text) {
  const matches = [...String(text).matchAll(/next=(\S+)/g)];
  const value = matches.length ? matches[matches.length - 1][1] : undefined;
  if (value === undefined || value === 'none') return null;
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

async function cdpEvaluate(webSocketUrl, expression, options = {}) {
  const { awaitPromise = true, timeoutMs = 5000 } = options;
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketUrl);
    const id = 1;
    let settled = false;
    const finish = (fn, value) => {
      if (settled) return;
      settled = true;
      try { socket.close(); } catch (_) { /* ignore */ }
      fn(value);
    };
    const timer = setTimeout(() => finish(reject, new Error('CDP evaluate timed out')), timeoutMs);
    socket.onopen = () => {
      socket.send(JSON.stringify({
        id,
        method: 'Runtime.evaluate',
        params: { expression, returnByValue: true, awaitPromise },
      }));
    };
    socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      clearTimeout(timer);
      if (message.error) return finish(reject, new Error(message.error.message));
      if (message.result && message.result.exceptionDetails) {
        const details = message.result.exceptionDetails;
        return finish(reject, new Error(details.exception?.description || details.text || 'CDP exception'));
      }
      finish(resolve, message.result?.result?.value);
    };
    socket.onerror = () => finish(reject, new Error('CDP WebSocket error'));
  });
}

function createTestServer() {
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (url.pathname === '/stream') {
      response.writeHead(200, {
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-store',
        connection: 'keep-alive',
      });
      response.write('first-chunk|');
      setTimeout(() => response.write('second-chunk|'), 700);
      setTimeout(() => response.write('third-chunk|'), 1400);
      setTimeout(() => response.end('end'), 2200);
      return;
    }
    if (url.pathname.startsWith('/api/')) {
      setTimeout(() => {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ ok: true, path: url.pathname }));
      }, 15);
      return;
    }
    if (url.pathname === '/next') {
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      response.end(`<!doctype html><title>Next</title><div id="done">next</div>
        <script>
          setTimeout(() => fetch('/api/save').then(r => {
            document.getElementById('done').textContent = 'saved ' + r.status;
            window.__saved = r.status;
          }), 800);
        </script>`);
      return;
    }
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    response.end(`<!doctype html><title>EMA2 Test App</title>
      <label for="pw">Password</label>
      <input id="pw" name="pw" type="password" value="REAL_SYNTHETIC_PASSWORD_9f3c">
      <input id="text" name="text" value="hello">
      <input id="ro" name="ro" readonly value="readonly">
      <input id="check" name="check" type="checkbox">
      <input id="file" name="file" type="file">
      <input id="p1" name="p1"><input id="p2" name="p2">
      <button id="go" onclick="location.href='/next'">Go</button>
      <button id="nav" onclick="location.href='/next?fast=1'">Nav</button>
      <button id="remove" onclick="this.remove(); document.getElementById('removed').textContent='clicked'">Remove</button>
      <span id="removed"></span>
      <button id="disabled" disabled>Disabled</button>
      <button id="stream">Stream</button>
      <button id="burst">Burst</button>
      <button id="logs">Logs</button>
      <div id="root">前言<p>正文</p><span>强调</span>结尾</div>
      <script>
        window.__keyTarget = null;
        for (const id of ['p1', 'p2']) {
          document.getElementById(id).addEventListener('keydown', event => {
            window.__keyTarget = event.target.id;
          });
        }
        document.getElementById('stream').addEventListener('click', async () => {
          const start = performance.now();
          try {
            const response = await fetch('/stream');
            window.__headersMs = Math.round(performance.now() - start);
            const reader = response.body.getReader();
            const first = await reader.read();
            window.__firstChunk = new TextDecoder().decode(first.value);
            window.__firstChunkMs = Math.round(performance.now() - start);
            while (!(await reader.read()).done) { /* drain */ }
            window.__streamDoneMs = Math.round(performance.now() - start);
          } catch (error) {
            window.__streamError = String(error);
          }
        });
        document.getElementById('burst').addEventListener('click', () => {
          window.__burstDone = 0;
          for (let i = 0; i < 100; i++) {
            fetch('/api/' + i).then(() => { window.__burstDone++; });
          }
        });
        document.getElementById('logs').addEventListener('click', () => {
          for (let i = 0; i < 120; i++) console.log('log-' + i + '-' + 'x'.repeat(180));
        });
      </script>`);
  });
  return server;
}

async function runPowerShell(args, env) {
  return new Promise((resolve) => {
    const child = spawn('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], {
      cwd: ROOT,
      env: { ...process.env, ...env },
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('close', code => resolve({ code, stdout, stderr }));
  });
}

async function focusEdgeWindow(processId) {
  if (!processId) return;
  const script = `
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Ema2Win32 {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
'@
$p = Get-Process -Id ${processId} -ErrorAction SilentlyContinue
if ($p -and $p.MainWindowHandle -ne 0) {
  [Ema2Win32]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
  [Ema2Win32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
}
`;
  await runPowerShell(['-Command', script], {});
  await sleep(500);
}

async function main() {
  if (process.platform !== 'win32') throw new Error('Edge acceptance runner currently supports Windows only');
  if (!fs.existsSync(EDGE)) throw new Error(`Edge not found: ${EDGE}`);
  if (!fs.existsSync(RELAY)) throw new Error(`relay not found: ${RELAY}. Run scripts\\setup.cmd first.`);

  const relayPort = Number(process.env.EMA2_EDGE_PORT) || await freePort();
  let debugPort = Number(process.env.EMA2_EDGE_DEBUG_PORT) || 0;
  const profileDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ema2-edge-profile-'));
  const outputDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ema2-edge-output-'));
  const relayLogPath = path.join(outputDir, 'relay.log');
  const relayLogFd = fs.openSync(relayLogPath, 'a');
  const edgeLogPath = path.join(outputDir, 'edge.log');
  const edgeLogFd = fs.openSync(edgeLogPath, 'a');
  const testServer = createTestServer();
  let relay = null;
  let edge = null;
  let edgePid = null;
  let intentionalStop = false;
  const createdTabs = [];
  const results = [];

  const closeTab = async (tabId) => {
    if (!tabId) return;
    try {
      await mcp.tool('tabs', { action: 'close', tabId });
    } catch (_) { /* ignore */ }
    const index = createdTabs.indexOf(tabId);
    if (index >= 0) createdTabs.splice(index, 1);
  };

  let mcp;
  try {
    log(`profile=${profileDir}`);

    relay = spawn(RELAY, ['run', '--port', String(relayPort)], {
      cwd: ROOT,
      env: { ...process.env, EDGE_MCP_OUTPUT_DIR: outputDir },
      stdio: ['ignore', relayLogFd, relayLogFd],
      windowsHide: true,
    });
    relay.on('exit', (code, signal) => {
      if (!intentionalStop && code && code !== 0) log(`relay exited early: code=${code} signal=${signal || ''}`);
    });

    await waitFor(async () => {
      const status = await fetchJson(`http://127.0.0.1:${relayPort}/status`);
      return status.status === 200 && status.body && status.body.name === 'ema2';
    }, 15000, 'relay /status');

    testServer.listen(0, '127.0.0.1');
    await new Promise(resolve => testServer.once('listening', resolve));
    const baseUrl = `http://127.0.0.1:${testServer.address().port}`;
    log(`relay port=${relayPort} test server=${testServer.address().port}`);

    const edgeArgs = [
      `--user-data-dir=${profileDir}`,
      `--load-extension=${EDGE_EXTENSION}`,
      `--disable-extensions-except=${EDGE_EXTENSION}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-sync',
      '--disable-background-networking',
      '--disable-component-update',
      '--disable-gpu',
      '--disable-features=CalculateNativeWinOcclusion',
      '--disable-backgrounding-occluded-windows',
      '--disable-renderer-backgrounding',
      '--disable-background-timer-throttling',
      '--force-device-scale-factor=1',
      '--window-size=1280,900',
      `--remote-debugging-port=${debugPort || 0}`,
      ...(HEADLESS ? ['--headless=new'] : []),
      'about:blank',
    ];
    edge = spawn(EDGE, edgeArgs, {
      cwd: ROOT,
      stdio: ['ignore', edgeLogFd, edgeLogFd],
      windowsHide: false,
      detached: false,
    });
    edgePid = edge.pid;
    edge.on('exit', (code) => log(`Edge exited: ${code}`));

    if (!debugPort) {
      const devtoolsPortFile = path.join(profileDir, 'DevToolsActivePort');
      debugPort = await waitFor(() => {
        try {
          const rawPort = fs.readFileSync(devtoolsPortFile, 'utf8');
          const port = Number(rawPort.trim().split(String.fromCharCode(10))[0]);
          return Number.isFinite(port) && port > 0 ? port : null;
        } catch (_) { return null; }
      }, 30000, 'Edge DevToolsActivePort');
    }
    log(`Edge debug port=${debugPort}`);

    const configuredWorker = await waitFor(async () => {
      const response = await fetchJson(`http://127.0.0.1:${debugPort}/json/list`);
      if (response.status !== 200 || !Array.isArray(response.body)) return null;
      const worker = response.body.find(target =>
        target.type === 'service_worker' && /\/background\.js$/.test(target.url || ''));
      if (!worker || !worker.webSocketDebuggerUrl) return null;
      try {
        const configured = await cdpEvaluate(worker.webSocketDebuggerUrl,
          `new Promise((resolve) => chrome.storage.local.set({ wsUrl: 'ws://127.0.0.1:${relayPort}' }, () => resolve(true)))`,
          { timeoutMs: 5000 });
        return configured === true ? worker : null;
      } catch (_) {
        return null;
      }
    }, 30000, 'configure extension WebSocket URL');

    try {
      await cdpEvaluate(configuredWorker.webSocketDebuggerUrl,
        "(() => { try { if (typeof socket !== 'undefined' && socket) socket.close(); } catch (_) {}; relayReady = false; connect(); return true; })()",
        { awaitPromise: false, timeoutMs: 3000 });
    } catch (_) { /* reconnect is also retried by the extension watchdog */ }

    await waitFor(async () => {
      const status = await fetchJson(`http://127.0.0.1:${relayPort}/status`);
      return status.status === 200 && status.body && status.body.extension_connected === true;
    }, 30000, 'extension connection to isolated relay');
    log('extension connected to isolated relay');
    await focusEdgeWindow(edgePid);

    mcp = new McpClient(`http://127.0.0.1:${relayPort}/mcp`);
    await mcp.initialize();

    const scenario = async (name, run) => {
      const started = Date.now();
      try {
        const detail = await run();
        results.push({ name, ok: true, detail, ms: Date.now() - started });
        log(`PASS ${name}${detail ? ` (${detail})` : ''}`);
      } catch (error) {
        results.push({ name, ok: false, error, ms: Date.now() - started });
        log(`FAIL ${name}: ${error.message}`);
        throw error;
      }
    };

    let appTab = null;
    await scenario('tabs/open + snapshot document-scoped refs', async () => {
      const opened = await mcp.ok('open', { url: baseUrl + '/app', wait: 'load' });
      appTab = Number(field(opened, 'id'));
      check(Number.isFinite(appTab), `open did not return a tab id: ${opened}`);
      createdTabs.push(appTab);
      const snapshot = await mcp.ok('snapshot', { tabId: appTab });
      check(/doc=[a-z0-9-]+/i.test(snapshot), 'snapshot header is missing doc= epoch');
      check(tabRefs(snapshot).length > 0, 'snapshot produced no refs');
      return `tab=${appTab}`;
    });

    await scenario('F02 password redaction in snapshot and type feedback', async () => {
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'form' });
      check(!snapshot.includes(SECRET), 'password value leaked into snapshot');
      const ref = findRef(snapshot, line => /type=password/.test(line), 'password input');
      const result = await mcp.tool('act', { tabId: appTab, action: 'type', ref, text: SECRET });
      check(!result.isError, `type failed: ${result.text}`);
      check(!result.text.includes(SECRET), 'type feedback leaked password value');
      check(/wrote=1 chars=\d+/.test(result.text), `type feedback missing write summary: ${result.text}`);
      const after = await mcp.ok('snapshot', { tabId: appTab, filter: 'form' });
      check(!after.includes(SECRET), 'password value leaked into a later snapshot');
      return 'redacted';
    });

    await scenario('F09 Markdown keeps root direct text and mixed blocks', async () => {
      const text = await mcp.ok('read', { tabId: appTab, selector: '#root', mode: 'md' });
      for (const expected of ['前言', '正文', '强调', '结尾']) {
        check(text.includes(expected), `Markdown lost ${expected}: ${text.slice(0, 500)}`);
      }
      return 'mixed text preserved';
    });

    await scenario('F10 action result correctness', async () => {
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'full' });
      const disabled = findRef(snapshot, line => /button "Disabled"/.test(line), 'disabled button');
      const disabledResult = await mcp.tool('act', { tabId: appTab, action: 'click', ref: disabled });
      check(disabledResult.isError && /element_disabled/.test(disabledResult.text), 'disabled click was reported as success');

      const readonly = findRef(snapshot, line => /name=ro/.test(line), 'readonly input');
      const readonlyResult = await mcp.tool('act', { tabId: appTab, action: 'type', ref: readonly, text: 'x' });
      check(readonlyResult.isError && /element_readonly/.test(readonlyResult.text), 'readonly type was reported as success');

      const checkbox = findRef(snapshot, line => /type=checkbox/.test(line), 'checkbox');
      const fillResult = await mcp.tool('act', {
        tabId: appTab,
        action: 'fill',
        fields: [{ ref: checkbox, value: 'yes' }],
      });
      check(fillResult.isError && /unsupported_control/.test(fillResult.text), 'checkbox fill was reported as success');

      const remove = findRef(snapshot, line => /button "Remove"/.test(line), 'remove button');
      const clicked = await mcp.tool('act', { tabId: appTab, action: 'click', ref: remove });
      check(!clicked.isError, `remove click failed: ${clicked.text}`);
      const after = await mcp.ok('snapshot', { tabId: appTab, filter: 'full' });
      check(!/button "Remove"/.test(after), 'removed button still present in snapshot');
      return 'disabled/readonly/unsupported/remove';
    });

    await scenario('F13 press targets the located ref and active element modes', async () => {
      await mcp.ok('js', { tabId: appTab, code: "document.getElementById('p1').focus(); 'ok'" });
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'form' });
      const p2 = findRef(snapshot, line => /name=p2/.test(line), 'p2 input');
      const targeted = await mcp.tool('act', { tabId: appTab, action: 'press', ref: p2, key: 'a' });
      check(!targeted.isError, `targeted press failed: ${targeted.text}`);
      const target = (await mcp.ok('js', { tabId: appTab, code: 'window.__keyTarget' })).trim();
      check(target === 'p2', `press went to ${target}, expected p2`);

      await mcp.ok('js', { tabId: appTab, code: "document.getElementById('p1').focus(); 'ok'" });
      const active = await mcp.tool('act', { tabId: appTab, action: 'press', key: 'b' });
      check(!active.isError, `active press failed: ${active.text}`);
      const activeTarget = (await mcp.ok('js', { tabId: appTab, code: 'window.__keyTarget' })).trim();
      check(activeTarget === 'p1', `active press went to ${activeTarget}, expected p1`);
      return 'p2 then p1';
    });

    await scenario('F01 streaming fetch is not blocked by observation preview', async () => {
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const stream = findRef(snapshot, line => /button "Stream"/.test(line), 'stream button');
      const clicked = await mcp.tool('act', { tabId: appTab, action: 'click', ref: stream });
      check(!clicked.isError, `stream click failed: ${clicked.text}`);
      const stateText = await waitFor(async () => {
        const value = await mcp.ok('js', {
          tabId: appTab,
          code: "window.__headersMs === undefined ? '' : JSON.stringify({headersMs:window.__headersMs,first:window.__firstChunk,firstMs:window.__firstChunkMs,done:window.__streamDoneMs,error:window.__streamError})",
        });
        return value.trim() ? value : null;
      }, 5000, 'page stream state');
      const state = JSON.parse(stateText);
      check(!state.error, `page stream failed: ${state.error}`);
      check(state.headersMs < 1200, `page waited ${state.headersMs}ms for headers; hook likely blocked on body`);
      check(String(state.first).startsWith('first-chunk'), `unexpected first chunk: ${state.first}`);
      const observed = await mcp.ok('observe', {
        kind: 'network', tabId: appTab, filter: '/stream', wait: 1, budget: 4000,
      });
      check(/#\d+ GET 200\b/.test(observed), `network observation did not record a real stream response:\n${observed}`);
      return `headers=${state.headersMs}ms first=${state.firstMs}ms`;
    });

    await scenario('F07 network pagination covers more than 80 entries and HAR saves', async () => {
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const burst = findRef(snapshot, line => /button "Burst"/.test(line), 'burst button');
      const clicked = await mcp.tool('act', { tabId: appTab, action: 'click', ref: burst });
      check(!clicked.isError, `burst click failed: ${clicked.text}`);
      await sleep(1800);
      const burstDone = await mcp.ok('js', { tabId: appTab, code: 'window.__burstDone || 0' });
      log(`F07 burst completed=${burstDone.trim()}`);

      const seen = new Set();
      let since = 0;
      for (let round = 0; round < 12; round++) {
        const output = await mcp.ok('observe', {
          kind: 'network', tabId: appTab, filter: '/api/', since, budget: 1200,
        });
        for (const match of output.matchAll(/^#(\d+)\s+GET\s+200\b/gm)) seen.add(Number(match[1]));
        const next = parseNext(output);
        log(`F07 network round=${round} since=${since} next=${next} totalSeen=${seen.size} footer=${(output.match(/total=\d+ shown=\d+ next=\S+/)||[''])[0]}`);
        if (next === null || next === since) break;
        since = next;
      }
      check(seen.size >= 100, `network pagination saw ${seen.size}/100 entries`);

      const har = await mcp.ok('observe', {
        kind: 'network', tabId: appTab, filter: '/api/', har: true, budget: 2000,
      });
      const harPath = field(har, 'path');
      check(harPath && fs.existsSync(harPath), `HAR was not saved: ${har}`);
      check(/\bentries=\d+/.test(har), `HAR result missing entries count: ${har}`);
      return `${seen.size} entries, HAR saved`;
    });

    await scenario('F07 console pagination covers 120 records', async () => {
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const logs = findRef(snapshot, line => /button "Logs"/.test(line), 'logs button');
      const clicked = await mcp.tool('act', { tabId: appTab, action: 'click', ref: logs });
      check(!clicked.isError, `logs click failed: ${clicked.text}`);
      await sleep(300);

      const seen = new Set();
      let since = 0;
      for (let round = 0; round < 20; round++) {
        const output = await mcp.ok('observe', {
          kind: 'console', tabId: appTab, level: 'log', since, budget: 1200,
        });
        for (const match of output.matchAll(/^#(\d+)\s+log\b/gm)) seen.add(Number(match[1]));
        const next = parseNext(output);
        if (next === null || next === since) break;
        since = next;
      }
      check(seen.size >= 120, `console pagination saw ${seen.size}/120 records`);
      return `${seen.size} records`;
    });

    await scenario('F03 stale refs after navigation, reload and across tabs', async () => {
      await mcp.ok('open', { tabId: appTab, url: baseUrl + '/app', wait: 'load' });
      let snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const go = findRef(snapshot, line => /button "Go"/.test(line), 'Go button');
      await mcp.ok('act', { tabId: appTab, action: 'click', ref: go });
      await waitFor(async () => {
        const path = await mcp.ok('js', { tabId: appTab, code: 'location.pathname' });
        return path.trim() === '/next';
      }, 5000, 'navigation to /next');
      const staleNav = await mcp.tool('act', { tabId: appTab, action: 'click', ref: go });
      check(staleNav.isError && /stale_ref/.test(staleNav.text), `old ref after navigation did not fail: ${staleNav.text}`);

      await mcp.ok('open', { tabId: appTab, url: baseUrl + '/app', wait: 'load' });
      snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const remove = findRef(snapshot, line => /button "Remove"/.test(line), 'Remove button');
      await mcp.ok('act', { tabId: appTab, action: 'reload' });
      await mcp.ok('wait', { tabId: appTab, idle: true, timeout: 8 });
      const staleReload = await mcp.tool('act', { tabId: appTab, action: 'click', ref: remove });
      check(staleReload.isError && /stale_ref/.test(staleReload.text), `old ref after reload did not fail: ${staleReload.text}`);

      const tab1 = Number(field(await mcp.ok('open', { url: baseUrl + '/app', wait: 'load' }), 'id'));
      const tab2 = Number(field(await mcp.ok('open', { url: baseUrl + '/app', wait: 'load' }), 'id'));
      createdTabs.push(tab1, tab2);
      const snap1 = await mcp.ok('snapshot', { tabId: tab1, filter: 'buttons' });
      const go1 = findRef(snap1, line => /button "Go"/.test(line), 'Go button in tab1');
      const cross = await mcp.tool('act', { tabId: tab2, action: 'click', ref: go1 });
      check(cross.isError && /stale_ref/.test(cross.text), `cross-tab ref did not fail: ${cross.text}`);
      await closeTab(tab2);
      await closeTab(tab1);
      return 'navigation/reload/cross-tab';
    });

    await scenario('F11 waitFor matches a response in the new document', async () => {
      await mcp.ok('open', { tabId: appTab, url: baseUrl + '/app', wait: 'load' });
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'buttons' });
      const nav = findRef(snapshot, line => /button "Nav"/.test(line), 'Nav button');
      const acted = await mcp.ok('act', {
        tabId: appTab,
        action: 'click',
        ref: nav,
        waitFor: { url: 'api/save', status: 200, timeout: 8 },
      });
      check(/response=200 GET \/api\/save/.test(acted), `waitFor did not match new-document response:\n${acted}`);
      check(!/observation_gap/.test(acted), `waitFor used history gap as success: ${acted}`);
      return 'response=200 /api/save';
    });

    await scenario('F12 batch rejects inner time=3600', async () => {
      const result = await mcp.tool('batch', {
        tool: 'wait', tabIds: [appTab], args: { time: 3600 },
      });
      check(result.isError && /invalid_argument/.test(result.text), `batch accepted time=3600: ${result.text}`);
      return 'rejected before execution';
    });

    await scenario('F08 batch budget covers 8 tabs x 4s at concurrency 1', async () => {
      const tabs = [];
      for (let i = 0; i < 8; i++) {
        const opened = await mcp.ok('tabs', { action: 'new', url: `${baseUrl}/next?batch=${i}`, active: false });
        const id = Number(field(opened, 'id'));
        check(Number.isFinite(id), `tabs new did not return id: ${opened}`);
        tabs.push(id);
        createdTabs.push(id);
      }
      const started = Date.now();
      const output = await mcp.ok('batch', {
        tool: 'wait', tabIds: tabs, args: { time: 4 }, concurrency: 1,
      });
      const elapsed = Date.now() - started;
      const blocks = [...output.matchAll(/^tab=(\d+)$/gm)].map(match => Number(match[1]));
      check(blocks.length === 8, `batch returned ${blocks.length}/8 blocks:\n${output.slice(0, 1500)}`);
      check(!/error=/.test(output), `batch had a tab error:\n${output.slice(0, 1500)}`);
      check(elapsed >= 30000, `batch returned too quickly (${elapsed}ms), waits may not have run`);
      for (const id of tabs) await closeTab(id);
      return `8 waves, ${(elapsed / 1000).toFixed(1)}s`;
    });

    await scenario('F05 hanging JS releases debugger and next js succeeds', async () => {
      if (SKIP_SLOW) return 'skipped by EMA2_EDGE_SKIP_SLOW=1';
      const hung = await mcp.tool('js', { tabId: appTab, code: 'new Promise(() => {})' });
      check(hung.isError && /debugger_timeout/.test(hung.text), `hanging js did not time out: ${hung.text}`);
      const next = await mcp.ok('js', { tabId: appTab, code: '1 + 1' });
      check(next.trim() === '2', `js after timeout returned ${next}`);
      return 'timeout + recovery';
    });

    await scenario('F06 shot rejects background tab, then captures/annotates active tab', async () => {
      await focusEdgeWindow(edgePid);
      const opened = await mcp.ok('tabs', { action: 'new', url: baseUrl + '/app', active: false });
      const backgroundTab = Number(field(opened, 'id'));
      check(Number.isFinite(backgroundTab), `tabs new did not return id: ${opened}`);
      createdTabs.push(backgroundTab);
      const inactive = await mcp.tool('shot', { tabId: backgroundTab, inline: true });
      check(inactive.isError && /tab_not_active/.test(inactive.text), `background shot did not fail: ${inactive.text}`);
      await mcp.ok('tabs', { action: 'activate', tabId: backgroundTab });
      let shot = await mcp.tool('shot', { tabId: backgroundTab, inline: true });
      if (shot.isError && /capture_failed/.test(shot.text)) {
        await focusEdgeWindow(edgePid);
        await sleep(700);
        shot = await mcp.tool('shot', { tabId: backgroundTab, inline: true });
      }
      check(!shot.isError, `active shot failed: ${shot.text}`);
      check(shot.images.length === 1, 'active shot returned no image');
      const annotated = await mcp.tool('shot', { tabId: backgroundTab, inline: true, annotate: true });
      check(!annotated.isError, `annotated shot failed: ${annotated.text}`);
      check(annotated.images.length === 1, 'annotated shot returned no image');
      check(/marks=/.test(annotated.text), `annotated shot missing marks list: ${annotated.text}`);
      await closeTab(backgroundTab);
      return 'background rejected; active + annotate captured';
    });

    await scenario('files upload sets a real file input', async () => {
      await mcp.ok('open', { tabId: appTab, url: baseUrl + '/app', wait: 'load' });
      await mcp.ok('tabs', { action: 'activate', tabId: appTab });
      const uploadPath = path.join(outputDir, 'upload-fixture.txt');
      fs.writeFileSync(uploadPath, 'ema2 integration upload');
      const windowsPath = uploadPath.replace(/\\/g, '/');
      const snapshot = await mcp.ok('snapshot', { tabId: appTab, filter: 'form' });
      const fileRef = findRef(snapshot, line => /type=file/.test(line), 'file input');
      const uploaded = await mcp.ok('files', {
        action: 'upload', tabId: appTab, ref: fileRef, paths: [windowsPath],
      });
      check(/uploaded=1/.test(uploaded), `upload did not report success: ${uploaded}`);
      const name = (await mcp.ok('js', {
        tabId: appTab,
        code: "document.getElementById('file').files[0] ? document.getElementById('file').files[0].name : ''",
      })).trim();
      check(name === 'upload-fixture.txt', `file input contains ${name}`);
      return 'upload-fixture.txt';
    });

    await scenario('F04 real bridge.ps1 status/stop/start/health', async () => {
      const env = { EDGE_MCP_OUTPUT_DIR: outputDir };
      const status = await runPowerShell([
        '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
        '-Action', 'status', '-Port', String(relayPort),
      ], env);
      check(status.code === 0, `status failed (${status.code}): ${status.stderr || status.stdout}`);

      intentionalStop = true;
      const stop = await runPowerShell([
        '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
        '-Action', 'stop', '-Port', String(relayPort),
      ], env);
      check(stop.code === 0, `stop failed (${stop.code}): ${stop.stderr || stop.stdout}`);
      await waitFor(async () => {
        try {
          await fetchJson(`http://127.0.0.1:${relayPort}/status`);
          return false;
        } catch (_) { return true; }
      }, 5000, 'relay port closed after stop');

      const start = await runPowerShell([
        '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
        '-Action', 'start', '-Port', String(relayPort),
      ], env);
      check(start.code === 0, `start failed (${start.code}): ${start.stderr || start.stdout}`);
      await waitFor(async () => {
        const response = await fetchJson(`http://127.0.0.1:${relayPort}/status`);
        return response.status === 200 && response.body && response.body.extension_connected === true;
      }, 20000, 'extension reconnect after bridge start');

      const finalStop = await runPowerShell([
        '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
        '-Action', 'stop', '-Port', String(relayPort),
      ], env);
      check(finalStop.code === 0, `final stop failed (${finalStop.code}): ${finalStop.stderr || finalStop.stdout}`);
      return 'status/stop/start/reconnect/stop';
    });

    log('');
    log('Acceptance summary:');
    for (const result of results) {
      log(`  ${result.ok ? 'PASS' : 'FAIL'} ${result.name} (${(result.ms / 1000).toFixed(1)}s)`);
    }
    const failed = results.filter(result => !result.ok);
    if (failed.length) throw new Error(`${failed.length} acceptance scenario(s) failed`);
  } finally {
    try { for (const tabId of createdTabs) await closeTab(tabId); } catch (_) { /* ignore */ }
    if (relay && !relay.killed) {
      try { relay.kill(); } catch (_) { /* ignore */ }
    }
    // Stop anything bridge.ps1 may have started.
    await runPowerShell([
      '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
      '-Action', 'stop', '-Port', String(relayPort),
    ], { EDGE_MCP_OUTPUT_DIR: outputDir });

    if (edgePid) {
      await new Promise((resolve) => {
        const child = spawn('taskkill', ['/PID', String(edgePid), '/T', '/F'], { windowsHide: true });
        child.on('close', resolve);
        child.on('error', resolve);
      });
    }
    await new Promise(resolve => testServer.close(resolve));
    try { fs.closeSync(relayLogFd); } catch (_) { /* ignore */ }
    const keep = KEEP && results.some(result => !result.ok);
    if (results.some(result => !result.ok)) {
      try { log(`edge log tail:
${fs.readFileSync(edgeLogPath, 'utf8').slice(-2000)}`); } catch (_) { /* ignore */ }
    }
    try { fs.closeSync(edgeLogFd); } catch (_) { /* ignore */ }
    if (keep) {
      log(`relay log: ${relayLogPath}`);
    }
    await sleep(700);
    if (!keep) {
      try { fs.rmSync(profileDir, { recursive: true, force: true }); } catch (_) { /* ignore */ }
      try { fs.rmSync(outputDir, { recursive: true, force: true }); } catch (_) { /* ignore */ }
    } else {
      log(`kept profile=${profileDir}`);
      log(`kept output=${outputDir}`);
    }
  }
}

main().then(() => {
  log('EDGE ACCEPTANCE PASSED');
  process.exit(0);
}).catch((error) => {
  console.error('EDGE ACCEPTANCE FAILED');
  console.error(error && error.stack || error);
  process.exit(1);
});
