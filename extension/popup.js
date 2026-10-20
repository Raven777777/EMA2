'use strict';

const DEFAULT_PORT = 7744;
const ui = {
  dot: document.getElementById('dot'),
  status: document.getElementById('status'),
  detail: document.getElementById('detail'),
  port: document.getElementById('port'),
  save: document.getElementById('save'),
  reconnect: document.getElementById('reconnect'),
  hint: document.getElementById('hint'),
};

let busy = false;

function setHint(text, kind) {
  ui.hint.className = `hint${kind ? ` ${kind}` : ''}`;
  ui.hint.textContent = text || '';
}

function setStatus(kind, text, detail) {
  ui.dot.className = `dot${kind ? ` ${kind}` : ''}`;
  ui.status.textContent = text;
  ui.detail.textContent = detail || '';
}

async function send(message) {
  const response = await chrome.runtime.sendMessage(message);
  if (!response) throw new Error('扩展后台无响应；在 edge://extensions 里重新加载扩展');
  return response;
}

function storedPort(stored) {
  const direct = Number(stored && stored.port);
  if (Number.isInteger(direct) && direct > 0 && direct < 65536) return direct;
  try {
    const parsed = Number(new URL(stored && stored.wsUrl).port);
    if (Number.isInteger(parsed) && parsed > 0 && parsed < 65536) {
      return parsed === 8765 ? DEFAULT_PORT : parsed;
    }
  } catch (_) { /* ignore */ }
  return DEFAULT_PORT;
}

async function readPort() {
  try {
    return storedPort(await chrome.storage.local.get(['port', 'wsUrl']));
  } catch (_) {
    return DEFAULT_PORT;
  }
}

function render(response) {
  const port = response.port || Number(ui.port.value) || DEFAULT_PORT;
  const relay = response.relay || {};
  const health = relay.health || {};
  const relayPid = health.pid ? ` pid=${health.pid}` : '';
  const extensionVersion = response.extensionVersion ? ` v${response.extensionVersion}` : '';
  if (response.connected) {
    setStatus('ok', `已连接 ${port}`, `中继${relayPid} · 扩展${extensionVersion}`);
    setHint('');
    return;
  }
  if (relay.reachable) {
    setStatus('warn', '中继在运行，扩展未连接',
      `端口 ${port}${relayPid}；保存端口后会自动重连`);
    setHint('若一直不连：确认端口与 ema2 status 的 port= 一致，再点“重新连接”。', '');
    return;
  }
  setStatus('err', `未连接 ${port}`, '本机中继没有运行');
  setHint('先在 shell 里运行：ema2 server', 'err');
}

async function refresh() {
  if (busy) return;
  try {
    const port = Number(ui.port.value) || DEFAULT_PORT;
    const response = await send({ type: 'ema2/state', port });
    if (!response.ok) throw new Error(response.error || '状态读取失败');
    render(response);
  } catch (error) {
    setStatus('err', '无法联系扩展后台', String((error && error.message) || error));
    setHint('在 edge://extensions 里重新加载 EMA2。', 'err');
  }
}

async function save() {
  if (busy) return;
  const port = Number(ui.port.value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    setHint('端口必须是 1–65535 的整数。', 'err');
    return;
  }
  busy = true;
  ui.save.disabled = true;
  ui.reconnect.disabled = true;
  setHint('正在保存并重连…', '');
  try {
    await chrome.storage.local.set({ port });
    await chrome.storage.local.remove(['wsUrl', 'token', 'pairedClients']);
    await send({ type: 'ema2/reconnect' });
    setHint(`已保存端口 ${port}；等待中继连接。`, 'ok');
    setTimeout(refresh, 400);
    setTimeout(refresh, 1600);
  } catch (error) {
    setHint(String((error && error.message) || error), 'err');
  } finally {
    busy = false;
    ui.save.disabled = false;
    ui.reconnect.disabled = false;
  }
}

async function reconnect() {
  if (busy) return;
  busy = true;
  ui.reconnect.disabled = true;
  setHint('正在重连…', '');
  try {
    await send({ type: 'ema2/reconnect' });
    setTimeout(refresh, 400);
    setTimeout(refresh, 1600);
  } catch (error) {
    setHint(String((error && error.message) || error), 'err');
  } finally {
    busy = false;
    ui.reconnect.disabled = false;
  }
}

ui.port.addEventListener('change', () => {
  const value = Number(ui.port.value);
  if (!Number.isInteger(value) || value < 1 || value > 65535) {
    ui.port.value = String(DEFAULT_PORT);
  }
  refresh();
});
ui.save.addEventListener('click', save);
ui.reconnect.addEventListener('click', reconnect);

(async () => {
  const port = await readPort();
  ui.port.value = String(port);
  await refresh();
  setInterval(() => { if (!document.hidden && !busy) refresh(); }, 1500);
})();
