'use strict';

const ui = {
  dot: document.getElementById('dot'),
  status: document.getElementById('status'),
  detail: document.getElementById('detail'),
  authCard: document.getElementById('authCard'),
  port: document.getElementById('port'),
  authorize: document.getElementById('authorize'),
  retry: document.getElementById('retry'),
  hint: document.getElementById('hint'),
  done: document.getElementById('done'),
  doneText: document.getElementById('doneText'),
  doneHint: document.getElementById('doneHint'),
  clientsCard: document.getElementById('clientsCard'),
  clientList: document.getElementById('clientList'),
  clientHint: document.getElementById('clientHint'),
  detailsCard: document.getElementById('detailsCard'),
  details: document.getElementById('details'),
  configure: document.getElementById('configure'),
};

let port = 8765;
let busy = false;
let selected = new Set();
let userPicked = false;
let lastStatus = {};
let lastResults = {};   // id -> 最近一次写入/移除结果（含备份路径）

async function send(message) {
  const response = await chrome.runtime.sendMessage(message);
  if (!response) throw new Error('扩展后台无响应，请重新加载扩展');
  return response;
}

function setStatus(kind, text, detail) {
  ui.dot.className = `dot ${kind}`;
  ui.status.textContent = text;
  ui.detail.textContent = detail || '';
}

function showAuth(visible) {
  ui.authCard.hidden = !visible;
}

function showDone(visible) {
  ui.done.hidden = !visible;
}

function setHint(text, kind) {
  ui.hint.className = `hint${kind ? ` ${kind}` : ''}`;
  ui.hint.textContent = text || '';
}

function tag(text, kind) {
  const span = document.createElement('span');
  span.className = `tag${kind ? ` ${kind}` : ''}`;
  span.textContent = text;
  return span;
}

function formatUptime(seconds) {
  const value = Number(seconds) || 0;
  if (value < 60) return `${value}s`;
  const minutes = Math.floor(value / 60);
  if (minutes < 60) return `${minutes}m ${value % 60}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function formatTime(unix) {
  if (!unix) return '—';
  return new Date(unix * 1000).toLocaleString();
}

function selectedIds(clients) {
  return clients.filter(client => client.detected && selected.has(client.id)).map(client => client.id);
}

function updateSummary(clients) {
  if (!Array.isArray(clients)) return;
  const installed = clients.filter(client => client.detected);
  const count = selectedIds(clients).length;
  if (!installed.length) {
    ui.clientHint.className = 'hint err';
    ui.clientHint.textContent = '没有检测到可配置的 AI 终端。先安装 opencode / Claude Code / Codex / Pi，再点“重新检测”。';
    return;
  }
  ui.clientHint.className = 'hint';
  ui.clientHint.textContent = `已选 ${count} / ${installed.length} 个已安装终端；未勾选的不写入配置。`;
}

function renderClients(clients) {
  ui.clientList.textContent = '';
  const list = Array.isArray(clients) ? clients : [];
  if (!list.length) {
    const item = document.createElement('li');
    item.textContent = '正在检测已安装的终端…';
    ui.clientList.appendChild(item);
    return;
  }
  if (!userPicked) {
    selected = new Set(list.filter(client => client.selected).map(client => client.id));
  }

  for (const client of list) {
    const item = document.createElement('li');
    if (!client.detected) item.className = 'off';

    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.checked = client.detected && selected.has(client.id);
    checkbox.disabled = busy || !client.detected;
    checkbox.addEventListener('change', () => {
      userPicked = true;
      if (checkbox.checked) selected.add(client.id);
      else selected.delete(client.id);
      updateSummary(list);
    });

    const body = document.createElement('div');
    body.className = 'body';
    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = client.label || client.id || 'client';
    name.appendChild(tag(client.format || 'json'));

    const statusTag = document.createElement('span');
    const renderStatus = () => {
      let text = '未选择';
      let kind = '';
      if (!client.detected) {
        text = '未安装';
      } else if (checkbox.checked) {
        text = client.configured ? '已配置' : '待写入';
        kind = client.configured ? 'ok' : 'warn';
      } else if (client.configured) {
        text = '将移除';
        kind = 'warn';
      }
      statusTag.className = `tag${kind ? ` ${kind}` : ''}`;
      statusTag.textContent = text;
    };
    renderStatus();
    name.appendChild(statusTag);

    const path = document.createElement('code');
    path.textContent = client.path || '';
    body.append(name, path);
    if (client.note) {
      const note = document.createElement('div');
      note.className = 'note';
      note.textContent = client.note;
      body.appendChild(note);
    }
    const result = lastResults[client.id];
    if (result && result.backup) {
      const backup = document.createElement('div');
      backup.className = 'note';
      backup.textContent = `backup: ${result.backup}${result.action === 'created' ? ' (empty: no previous file)' : ''}`;
      body.appendChild(backup);
    }

    checkbox.addEventListener('change', () => {
      userPicked = true;
      if (checkbox.checked) selected.add(client.id);
      else selected.delete(client.id);
      renderStatus();
      updateSummary(list);
    });

    item.append(checkbox, body);
    ui.clientList.appendChild(item);
  }
  updateSummary(list);
}

function renderDetails(status, response) {
  const rows = [
    ['中继', `${status.version || '?'} · 端口 ${status.port || port}`],
    ['运行时长', formatUptime(status.uptime_secs)],
    ['扩展', `${response.extensionVersion || '?'} · ${response.connected ? '已连接' : '未连接'}`],
    ['配对时间', formatTime(status.paired_at)],
    ['token 指纹', status.token_fingerprint || '尚未生成'],
    ['中继状态', status.anonymous ? '匿名模式（无鉴权）' : status.paired ? '已配对' : '待授权'],
  ];
  ui.details.textContent = '';
  for (const [key, value] of rows) {
    const dt = document.createElement('dt');
    dt.textContent = key;
    const dd = document.createElement('dd');
    dd.textContent = value;
    ui.details.append(dt, dd);
  }
  ui.detailsCard.hidden = false;
}

async function refresh() {
  if (busy) return;
  let response;
  try {
    response = await send({ type: 'ema2/state', port });
  } catch (error) {
    setStatus('err', '无法联系扩展后台', error.message);
    return;
  }
  if (!response.ok) {
    setStatus('err', '状态读取失败', response.error || '');
    return;
  }
  const status = response.status || {};
  lastStatus = status;
  const version = status.version ? `EMA2 v${status.version}` : 'EMA2';

  if (!status.reachable) {
    setStatus('err', 'EMA2 中继未运行', `未连接到 127.0.0.1:${port}。先执行 bin\\ema2.exe install。`);
    showAuth(true);
    showDone(false);
    ui.clientsCard.hidden = true;
    ui.detailsCard.hidden = true;
    ui.authorize.disabled = true;
    setHint('中继启动后点“重新检测”。', '');
    return;
  }

  ui.clientsCard.hidden = false;
  renderClients(status.clients);
  renderDetails(status, response);

  if (status.anonymous) {
    setStatus('warn', '中继以匿名模式运行', '该模式不需要授权，也不校验 token。');
    showAuth(false);
    showDone(false);
    return;
  }
  if (status.pairing) {
    setStatus('warn', '检测到 EMA2，等待授权', `${version} · 127.0.0.1:${port}`);
    showAuth(true);
    showDone(false);
    ui.authorize.disabled = false;
    setHint('授权后 AI 客户端即可直接使用。', '');
    return;
  }
  if (status.connected && response.connected) {
    setStatus('ok', '已连接', `${version} · 扩展已连接，AI 可以直接使用`);
    showAuth(false);
    showDone(true);
    ui.doneText.textContent = 'AI 客户端可以直接使用 EMA2 了。';
    return;
  }
  if (response.hasToken) {
    setStatus('busy', '已授权，正在连接中继…', `${version} · 127.0.0.1:${port}`);
    showAuth(false);
    showDone(true);
    return;
  }
  setStatus('err', 'EMA2 已绑定其他扩展', '当前浏览器没有绑定的 token。');
  showAuth(false);
  showDone(false);
}

async function authorize() {
  if (busy) return;
  busy = true;
  ui.authorize.disabled = true;
  setHint('');
  setStatus('busy', '正在授权…', '请保持此页面打开');
  try {
    const clients = Array.isArray((lastStatus || {}).clients)
      ? selectedIds(lastStatus.clients)
      : [...selected];
    const result = await send({ type: 'ema2/authorize', port, clients });
    if (!result.ok) throw new Error(result.error || '授权失败');
    userPicked = false;
    lastResults = Object.fromEntries(
      (result.clients || []).filter(entry => entry.backup).map(entry => [entry.id, entry]),
    );
    setStatus('ok', '已授权', '正在建立扩展连接…');
    showAuth(false);
    showDone(true);
    ui.doneHint.className = 'hint ok';
    const removed = (result.clients || []).filter(entry => entry.action === 'removed').length;
    ui.doneHint.textContent = clients.length
      ? `已写入 ${clients.length} 个终端的 MCP 配置${removed ? `，移除 ${removed} 个未勾选项` : ''}。`
      : (removed
        ? `已移除 ${removed} 个终端的 EMA2 配置。`
        : '未选择终端：只完成浏览器授权，没有改任何配置。');
    busy = false;
    await refresh();
    return;
  } catch (error) {
    setStatus('err', '授权失败', '');
    setHint(error.message, 'err');
    ui.authorize.disabled = false;
  } finally {
    busy = false;
  }
}

async function configure() {
  if (busy) return;
  busy = true;
  ui.configure.disabled = true;
  ui.doneHint.className = 'hint';
  ui.doneHint.textContent = '正在写入…';
  try {
    const clients = Array.isArray((lastStatus || {}).clients)
      ? selectedIds(lastStatus.clients)
      : [...selected];
    const result = await send({ type: 'ema2/configure', port, clients });
    if (!result.ok) throw new Error(result.error || '写入失败');
    userPicked = false;
    lastResults = Object.fromEntries(
      (result.clients || []).filter(entry => entry.backup).map(entry => [entry.id, entry]),
    );
    const removed = (result.clients || []).filter(entry => entry.action === 'removed').length;
    ui.doneHint.className = 'hint ok';
    ui.doneHint.textContent = clients.length
      ? `已应用 ${clients.length} 个终端${removed ? `，移除 ${removed} 个未勾选项` : ''}；每个被修改的文件旁都有 .ema2.bak。`
      : (removed ? `已移除 ${removed} 个终端的 EMA2 配置。` : '未选择终端，配置没有变化。');
    await refresh();
  } catch (error) {
    ui.doneHint.className = 'hint err';
    ui.doneHint.textContent = error.message;
  } finally {
    ui.configure.disabled = false;
    busy = false;
  }
}

function attach() {
  ui.port.addEventListener('change', () => {
    const value = Number(ui.port.value);
    port = Number.isFinite(value) && value > 0 && value < 65536 ? Math.round(value) : 8765;
    ui.port.value = String(port);
    userPicked = false;
    refresh();
  });
  ui.authorize.addEventListener('click', authorize);
  ui.retry.addEventListener('click', () => { userPicked = false; refresh(); });
  ui.configure.addEventListener('click', configure);
}

(async () => {
  attach();
  try {
    const stored = await chrome.storage.local.get('wsUrl');
    if (stored && stored.wsUrl) {
      const parsed = new URL(stored.wsUrl);
      if (parsed.port) port = Number(parsed.port) || port;
    }
  } catch (_) { /* keep default */ }
  ui.port.value = String(port);
  await refresh();
  setInterval(() => { if (!document.hidden && !busy) refresh(); }, 2000);
})();
