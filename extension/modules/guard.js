/**
 * 命令期 page guard 租约、动态注册、disarm 与启动清理。
 */

/** 读取 SW 重启前登记的已 arm tab；storage.session 随浏览器重启清空。 */
async function loadCommandGuardTabs() {
  if (commandGuardTabsLoaded) return commandGuardTabs;
  if (!commandGuardTabsLoad) {
    commandGuardTabsLoad = (async () => {
      try {
        const store = chrome.storage && chrome.storage.session;
        if (!store) {
          commandGuardTabsLoaded = true;
          return;
        }
        const stored = (await store.get(COMMAND_GUARD_TABS_KEY)) || {};
        const ids = stored[COMMAND_GUARD_TABS_KEY];
        if (Array.isArray(ids)) {
          for (const id of ids) {
            if (Number.isInteger(id) && id >= 0) commandGuardTabs.add(id);
          }
        }
        commandGuardTabsLoaded = true;
      } catch (_) { /* storage.session 暂不可用：下次再试，不把失败缓存成空集合 */ }
    })();
  }
  await commandGuardTabsLoad;
  commandGuardTabsLoad = null;
  return commandGuardTabs;
}

/** 与 sessionTabs 相同的串行写入，避免并发 arm/disarm 后写覆盖先写。 */
async function persistCommandGuardTabs() {
  const snapshot = [...commandGuardTabs];
  const write = commandGuardTabsWrite.then(async () => {
    try {
      const store = chrome.storage && chrome.storage.session;
      if (store) await store.set({ [COMMAND_GUARD_TABS_KEY]: snapshot });
    } catch (_) { /* 内存集合仍可清理；SW 重启后再试 */ }
  });
  commandGuardTabsWrite = write;
  return write;
}

/**
 * 命令期间才启用 guard。动态 content script 负责覆盖导航后新文档在
 * document_start 阶段出现的对话框；已经打开的 tab 由这里立即注入并 arm。
 */
async function ensurePageGuard(tabId) {
  try {
    await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      files: COMMAND_GUARD_FILES,
      world: 'MAIN',
    });
    if (Number.isInteger(tabId)) {
      commandGuardTabs.add(tabId);
      await persistCommandGuardTabs();
    }
    return true;
  } catch (_) {
    return false;
  }
}

async function registerCommandGuard() {
  // 逐条注销：旧版本只注册过 main id，一次传不存在的 id 可能让整批失败。
  for (const id of [COMMAND_GUARD_ID, COMMAND_GUARD_BRIDGE_ID]) {
    await within(
      chrome.scripting.unregisterContentScripts({ ids: [id] }),
      COMMAND_GUARD_LIFECYCLE_MS,
      null,
    );
  }
  // bridge 先注册：这样任何被 MAIN guard arm 的新文档都已经有监听者。
  const bridgeOk = await within(
    chrome.scripting.registerContentScripts([{
      id: COMMAND_GUARD_BRIDGE_ID,
      matches: COMMAND_GUARD_MATCHES,
      js: COMMAND_GUARD_BRIDGE_FILES,
      runAt: 'document_start',
      allFrames: true,
      world: 'ISOLATED',
      persistAcrossSessions: false,
    }]).then(() => true),
    COMMAND_GUARD_LIFECYCLE_MS,
    false,
  );
  if (!bridgeOk) {
    // bridge 只是释放竞态的加固；失败时主 guard 仍然工作。
    console.warn('[EMA2] command guard bridge registration failed or timed out');
  }
  const mainOk = await within(
    chrome.scripting.registerContentScripts([{
      id: COMMAND_GUARD_ID,
      matches: COMMAND_GUARD_MATCHES,
      js: COMMAND_GUARD_FILES,
      runAt: 'document_start',
      allFrames: true,
      world: 'MAIN',
      persistAcrossSessions: false,
    }]).then(() => true),
    COMMAND_GUARD_LIFECYCLE_MS,
    false,
  );
  if (!mainOk) console.warn('[EMA2] command guard registration failed or timed out');
  return mainOk;
}

async function unregisterCommandGuard() {
  for (const id of [COMMAND_GUARD_ID, COMMAND_GUARD_BRIDGE_ID]) {
    await within(
      chrome.scripting.unregisterContentScripts({ ids: [id] }),
      COMMAND_GUARD_LIFECYCLE_MS,
      null,
    );
  }
}

/** 在 top frame 或 all frames 执行一次 disarm；不吞异常，由调用方验证。 */
async function runGuardDisarm(tabId, allFrames) {
  const results = await chrome.scripting.executeScript({
    target: allFrames ? { tabId, allFrames: true } : { tabId },
    world: 'MAIN',
    func: () => {
      const guard = window.__ema2_guard;
      if (!guard) return { guard: false, disarmed: false };
      // 新版本只暴露事件；页面里没有可被误调用的 guard.disarm()。
      try {
        document.dispatchEvent(new CustomEvent('ema2-guard-disarm'));
      } catch (_) { /* fall through to legacy below */ }
      // 旧版本（v3 及更早）没有事件监听，保留一次性兼容路径。
      if (guard.armed && typeof guard.disarm === 'function') {
        try { guard.disarm(); } catch (_) { /* report below */ }
      }
      return { guard: true, disarmed: guard.armed !== true };
    },
  });
  return Array.isArray(results) ? results : [];
}

function guardDisarmed(results) {
  return Array.isArray(results) && results.length > 0 && results.every(item => {
    const value = item && item.result;
    return !!value && (value.disarmed === true || value.guard === false);
  });
}

async function upgradePassiveGuard(tabId, allFrames) {
  await chrome.scripting.executeScript({
    target: allFrames ? { tabId, allFrames: true } : { tabId },
    files: ['page_guard.js'],
    world: 'MAIN',
  });
}

/**
 * 只有在确认 top frame 和所有可脚本化 frame 都已 disarm 后才返回 true。
 * top-frame 与 all-frames 分开执行：某个受限/刚导航的 frame 不能把 top frame
 * 一起拖失败，也不能让成功的结果掩盖未清理的 iframe。
 */
async function disarmPageGuard(tabId) {
  try { await chrome.tabs.get(tabId); }
  catch (_) { return true; } // tab 已关闭：没有页面可残留
  let top;
  try { top = await runGuardDisarm(tabId, false); }
  catch (_) { return false; }
  if (!guardDisarmed(top)) {
    try { await upgradePassiveGuard(tabId, true); }
    catch (_) { return false; }
    try { top = await runGuardDisarm(tabId, false); }
    catch (_) { return false; }
    if (!guardDisarmed(top)) return false;
  }
  let all;
  try { all = await runGuardDisarm(tabId, true); }
  catch (_) { return false; }
  if (!guardDisarmed(all)) {
    // 旧版本可能只在 iframe 里留下没有 disarm() 的 guard。
    try { await upgradePassiveGuard(tabId, true); }
    catch (_) { return false; }
    try { all = await runGuardDisarm(tabId, true); }
    catch (_) { return false; }
  }
  return guardDisarmed(all);
}

async function allScriptableTabIds() {
  try {
    const tabs = await chrome.tabs.query({});
    return tabs
      .filter(tab => Number.isInteger(tab.id) && !isInternalUrl(tab.url))
      .map(tab => tab.id);
  } catch (_) {
    return [];
  }
}

/**
 * 清理指定 tab。每个 tab 单独限时，整体再设硬上限：一个卡在原生对话框或
 * 死循环里的渲染进程不能把 release/启动清理拖到命令预算之外。
 * 只有验证过的 tab 才移出跟踪集合，失败的下次 release/启动继续重试。
 */
async function disarmPageGuards(tabIds, budget = COMMAND_GUARD_SWEEP_MS) {
  const ids = [...new Set(tabIds)].filter(id => Number.isInteger(id));
  if (!ids.length) return true;
  const results = await within(
    Promise.all(ids.map(async id => {
      const ok = await within(
        disarmPageGuard(id),
        COMMAND_GUARD_DISARM_MS,
        false,
      );
      if (ok === true && commandGuardTabs.delete(id)) await persistCommandGuardTabs();
      return ok === true;
    })),
    budget,
    null,
  );
  return Array.isArray(results) && results.every(Boolean);
}

async function disarmTrackedPageGuards(budget = COMMAND_GUARD_SWEEP_MS) {
  return disarmPageGuards([...commandGuardTabs], budget);
}

/**
 * guard 链只允许成功/失败都继续；任何一步 reject 都不能污染后续命令。
 * 返回的 Promise 本身永远 resolve（失败只记日志）。
 */
function enqueueCommandGuard(step) {
  const run = () => Promise.resolve().then(step).catch(error => {
    console.warn('[EMA2] command guard step failed:', error && error.message || error);
  });
  commandGuardLifecycle = commandGuardLifecycle.then(run, run);
  return commandGuardLifecycle;
}

/**
 * 登记一个命令租约。第一个租约注册 document_start guard，最后一个释放时
 * 注销注册并恢复已 arm 文档的原生对话框 API。
 */
async function acquireCommandGuard() {
  const lease = { id: ++commandGuardLeaseSeq, released: false };
  // guard 是优化而不是前置条件：这里失败也必须放行工具调用。
  try {
    await enqueueCommandGuard(async () => {
      if (lease.released) return;
      commandGuardLeases += 1;
      commandGuardLeaseTokens.add(lease);
      if (commandGuardLeases === 1) await registerCommandGuard();
    });
  } catch (_) { /* fail open */ }
  return lease;
}

async function releaseCommandGuard(lease) {
  // 同步标记，watchdog 兜底和 finally 谁先到都不会二次扣减别人的租约。
  if (!lease || lease.released) return;
  lease.released = true;
  try {
    await enqueueCommandGuard(async () => {
      if (!commandGuardLeaseTokens.delete(lease)) return;
      if (commandGuardLeases > 0) commandGuardLeases -= 1;
      if (commandGuardLeases !== 0) return;
      // 顺序很重要：先注销动态脚本，再 disarm。反序会在注销窗口里把正在
      // 加载的新文档 arm 掉，而 sweep 已经结束，guard 会永久残留。
      await unregisterCommandGuard();
      await disarmTrackedPageGuards();
    });
  } catch (_) { /* guard failure must not fail the command */ }
}

/**
 * watchdog 超时后的兜底：工具可能永远不会 settle，给正常收尾一个宽限期后
 * 强制释放该 RPC 持有的全部租约。context.closed 会让迟到的新租约立即释放。
 */
function abandonCommandGuard(context) {
  if (!context || context.abandoned) return;
  context.closed = true;
  context.abandoned = true;
  setTimeout(() => {
    for (const lease of [...context.leases]) {
      context.leases.delete(lease);
      void releaseCommandGuard(lease);
    }
  }, COMMAND_GUARD_ABANDON_GRACE_MS);
}

async function cleanupLegacyGuardOnce(forceAll = false) {
  let legacy = forceAll;
  let protocolStale = forceAll;
  try {
    const stored = await chrome.storage.local.get(GUARD_PROTOCOL_KEY);
    const stale = Number(stored && stored[GUARD_PROTOCOL_KEY]) < GUARD_PROTOCOL_VERSION;
    protocolStale = protocolStale || stale;
    legacy = legacy || stale;
  } catch (_) { /* storage may be unavailable during startup */ }
  await loadCommandGuardTabs();

  let attempted = false;
  await enqueueCommandGuard(async () => {
    if (commandGuardLeases !== 0) return;
    await unregisterCommandGuard();
    // 协议升级/扩展更新要扫所有 tab（清 v1/v2 或重载前的遗留）；普通 SW
    // 启动只清理已经记录/持久化的 tab。
    const ids = legacy ? await allScriptableTabIds() : [...commandGuardTabs];
    if (legacy) {
      // 未验证成功的 tab 留在集合里，下次 release/启动只重试这些，不再全量扫描。
      for (const id of ids) commandGuardTabs.add(id);
      await persistCommandGuardTabs();
    }
    await disarmPageGuards(ids, legacy ? LEGACY_GUARD_SWEEP_MS : COMMAND_GUARD_SWEEP_MS);
    attempted = true;
  });
  // 即使整批 sweep 超时，已验证的 tab 已移除、未验证的已持久化；标记协议完成，
  // 避免慢机器每次 SW 启动都重复全量扫描。
  if (protocolStale && attempted) {
    try { await chrome.storage.local.set({ [GUARD_PROTOCOL_KEY]: GUARD_PROTOCOL_VERSION }); } catch (_) { /* ignore */ }
  }
}

/**
 * 返回本次命令实际 arm 的 tabId。调用方把它写回 args.tabId，保证 guard 与
 * 真正执行的 tab 是同一个，避免 arm 后用户快速切换活动标签。
 */
async function guardTargetForTool(name, args) {
  // open 不在这里：它先导航，再由 openTool 对最终文档 ensurePageGuard。
  // act 只在这里 arm 一次；actTool 不要在动作前重复注入。
  const guarded = new Set([
    'act', 'wait', 'read', 'snapshot', 'js', 'observe', 'files', 'shot',
  ]);
  if (!guarded.has(name)) return undefined;
  if (name === 'files' && args.action === 'download-dir') return undefined;
  let tabId = args.tabId;
  if (name === 'wait' && args.await === true && tabId === undefined) {
    const context = await loadLastAct().catch(() => null);
    tabId = context && (context.targetTabId || context.tabId);
  }
  if (tabId === undefined) {
    try {
      const tab = await activeTab();
      tabId = tab.id;
    } catch (_) {
      return undefined;
    }
  }
  if (!Number.isInteger(tabId)) return undefined;
  await ensurePageGuard(tabId);
  return tabId;
}

async function withCommandGuard(name, args, run, context) {
  const lease = await acquireCommandGuard();
  if (context && context.closed) {
    // 调用方已经因 watchdog 放弃这次 RPC；迟到的内层工具不再新增租约。
    await releaseCommandGuard(lease);
    return run();
  }
  if (context) context.leases.add(lease);
  try {
    const targetTabId = await guardTargetForTool(name, args);
    if (targetTabId !== undefined && args.tabId === undefined) args.tabId = targetTabId;
    return await run();
  } finally {
    if (context) context.leases.delete(lease);
    await releaseCommandGuard(lease);
  }
}

async function runInAllFrames(tabId, func, args, world) {
  const options = { target: { tabId, allFrames: true }, func, args };
  if (world) options.world = world;
  const results = await chrome.scripting.executeScript(options);
  return Array.isArray(results) ? results : [];
}

const guardProbe = () => ({
  seq: window.__ema2_guard ? window.__ema2_guard.seq || 0 : 0,
  origin: Number(performance.timeOrigin) || 0,
});

const guardEventPayload = () => {
  const guard = window.__ema2_guard;
  if (!guard) return { seq: 0, origin: 0, events: [] };
  return {
    seq: guard.seq || 0,
    origin: Number(performance.timeOrigin) || 0,
    events: guard.events || [],
  };
};

/**
 * 当前每个 frame 的 guard 事件水位。必须 allFrames：iframe 里的 confirm()
 * 也会被 guard 吞掉，只看 top frame 会漏报 dialog_opened。
 * 水位带 performance.timeOrigin，frame 导航换代后自动从 0 开始计。
 */
async function guardMark(tabId) {
  try {
    const results = await runInAllFrames(tabId, guardProbe, [], 'MAIN');
    const frames = {};
    for (const item of results) {
      if (!item || !Number.isInteger(item.frameId)) continue;
      const value = item.result || {};
      frames[item.frameId] = {
        seq: Number(value.seq) || 0,
        origin: Number(value.origin) || 0,
      };
    }
    if (Object.keys(frames).length) return { frames };
  } catch (_) { /* fall back to the top frame below */ }
  try {
    const value = await runInTab(tabId, guardProbe, [], 'MAIN');
    return {
      frames: {
        0: { seq: Number(value && value.seq) || 0, origin: Number(value && value.origin) || 0 },
      },
    };
  } catch (_) {
    return { frames: {} };
  }
}

async function guardSince(tabId, mark) {
  const frames = mark && typeof mark === 'object' && mark.frames ? mark.frames : null;
  const merge = (results) => {
    const events = [];
    for (const item of results || []) {
      const payload = item && item.result;
      if (!payload || !Array.isArray(payload.events)) continue;
      const frameId = Number.isInteger(item.frameId) ? item.frameId : 0;
      const prior = frames ? frames[frameId] : null;
      const since = prior && Number(prior.origin) === Number(payload.origin)
        ? Number(prior.seq) || 0
        : (frames ? 0 : Number(mark) || 0);
      for (const event of payload.events) {
        if ((event.seq || 0) > since) events.push(event);
      }
    }
    events.sort((left, right) => (left.at || 0) - (right.at || 0)
      || (left.seq || 0) - (right.seq || 0));
    return events;
  };
  try {
    const results = await runInAllFrames(tabId, guardEventPayload, [], 'MAIN');
    if (results.length) return merge(results);
  } catch (_) { /* fall back to the top frame below */ }
  try {
    const value = await runInTab(tabId, guardEventPayload, [], 'MAIN');
    return merge([{ frameId: 0, result: value }]);
  } catch (_) {
    return [];
  }
}

/** 返回空字符串表示 selector 语法合法；否则返回浏览器给出的 SyntaxError。 */
async function selectorError(tabId, selector) {
  if (!selector) return '';
  try {
    return await runInTab(tabId, (value) => {
      try {
        document.documentElement.matches(value);
        return '';
      } catch (error) {
        return String((error && error.message) || error);
      }
    }, [String(selector)], undefined, 1) || '';
  } catch (_) {
    return '';
  }
}

async function requireValidSelector(tabId, selector) {
  const error = await selectorError(tabId, selector);
  if (error) {
    fail('invalid_selector', `invalid CSS selector: ${selector} (${oneLine(error)})`,
      'fix the selector syntax, or use a snapshot ref/target instead');
  }
}
