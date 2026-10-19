'use strict';

const { test } = require('node:test');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const { assert, ROOT } = require('./helpers.cjs');

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: ROOT, windowsHide: true, ...options });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

test('F04 bridge scripts do not use netstat/findstr/taskkill string matching', () => {
  for (const name of ['bridge.ps1', 'start-bridge.cmd', 'stop-bridge.cmd']) {
    const source = fs.readFileSync(path.join(ROOT, 'scripts', name), 'utf8');
    assert.doesNotMatch(source, /\bnetstat\b/i, `${name} must not parse netstat`);
    assert.doesNotMatch(source, /\bfindstr\b/i, `${name} must not substring-match ports`);
    assert.doesNotMatch(source, /\btaskkill\b/i, `${name} must not blind-kill PIDs`);
  }
  for (const name of ['start-bridge.cmd', 'stop-bridge.cmd']) {
    const source = fs.readFileSync(path.join(ROOT, 'scripts', name), 'utf8');
    assert.match(source, /bridge\.ps1/, `${name} must delegate to the structured helper`);
    assert.match(source, /exit \/b %errorlevel%/, `${name} must propagate the real exit code`);
  }
  const setup = fs.readFileSync(path.join(ROOT, 'scripts', 'setup.cmd'), 'utf8');
  assert.doesNotMatch(setup, /D:\\Rust\\cargo\.cmd/i, 'setup.cmd must not hard-code one machine path');
  assert.match(setup, /EMA2_CARGO/, 'setup.cmd should expose an explicit cargo override');
  assert.match(setup, /copy \/Y[\s\S]*if errorlevel 1/, 'setup.cmd must check the copy result');
});

test('F04 bridge.ps1 self-test covers exact port and executable identity', { skip: process.platform !== 'win32' }, async () => {
  const result = await run('powershell', [
    '-NoProfile', '-ExecutionPolicy', 'Bypass',
    '-File', path.join(ROOT, 'scripts', 'bridge.ps1'), '-SelfTest',
  ]);
  assert.equal(result.code, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /self-test ok/);
});

test('F04 stop refuses a foreign listener on the exact port and leaves it alive', { skip: process.platform !== 'win32' }, async () => {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const port = server.address().port;
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ema2-bridge-test-'));
  const fakeExe = path.join(tempDir, 'ema2.exe');
  fs.writeFileSync(fakeExe, 'not a real executable');

  try {
    const result = await run('powershell', [
      '-NoProfile', '-ExecutionPolicy', 'Bypass',
      '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
      '-Action', 'stop', '-Port', String(port), '-Exe', fakeExe,
    ]);
    assert.equal(result.code, 2, `foreign listener should exit 2, got ${result.code}: ${result.stderr}`);
    assert.match(result.stderr, /refusing to stop/i);
    assert.equal(server.listening, true, 'foreign listener must not be terminated');

    const start = await run('powershell', [
      '-NoProfile', '-ExecutionPolicy', 'Bypass',
      '-File', path.join(ROOT, 'scripts', 'bridge.ps1'),
      '-Action', 'start', '-Port', String(port), '-Exe', fakeExe,
    ]);
    assert.equal(start.code, 2, `start must refuse a foreign listener, got ${start.code}: ${start.stderr}`);
    assert.match(start.stderr, /refusing to stop/i);
    assert.equal(server.listening, true, 'start must not launch a second service');

    const stillThere = await new Promise((resolve) => {
      const probe = net.createConnection({ port, host: '127.0.0.1' }, () => {
        probe.end();
        resolve(true);
      });
      probe.on('error', () => resolve(false));
      probe.setTimeout(1000, () => { probe.destroy(); resolve(false); });
    });
    assert.equal(stillThere, true, 'the foreign listener should still accept connections');
  } finally {
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});
