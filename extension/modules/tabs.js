/**
 * 标签页会话跟踪与关闭。
 */

// ---------------------------------------------------------------- tabs

/** 会话标签登记：open/batch 打开的标签按顺序记下来，供复用与批量关闭使用。 */
async function loadSessionTabs() {
  if (sessionTabsLoaded) return sessionTabs;
  if (!sessionTabsLoad) {
    sessionTabsLoad = (async () => {
      try {
        const store = chrome.storage && chrome.storage.session;
        if (!store) {
          sessionTabsLoaded = true;
          return;
        }
        const stored = (await store.get(SESSION_TABS_KEY)) || {};
        const ids = stored[SESSION_TABS_KEY];
        if (Array.isArray(ids)) sessionTabs = ids.filter(id => Number.isInteger(id) && id >= 0);
        sessionTabsLoaded = true;
      } catch (_) { /* storage.session 暂时不可用：下次调用再试，不把失败缓存成空数组 */ }
    })();
  }
  await sessionTabsLoad;
  sessionTabsLoad = null;
  return sessionTabs;
}

/** 写入串行化：open/act/batch 并发登记时，后写不能覆盖先写。 */
async function persistSessionTabs() {
  const snapshot = [...sessionTabs];
  const write = sessionWriteChain.then(async () => {
    try {
      const store = chrome.storage && chrome.storage.session;
      if (store) await store.set({ [SESSION_TABS_KEY]: snapshot });
    } catch (_) { /* 忽略：内存登记仍然可用 */ }
  });
  sessionWriteChain = write;
  return write;
}

async function rememberSessionTab(tabId) {
  await loadSessionTabs();
  sessionTabs = sessionTabs.filter(id => id !== tabId).concat(tabId);
  await persistSessionTabs();
}

async function forgetSessionTab(tabId) {
  await loadSessionTabs();
  if (!sessionTabs.includes(tabId)) return;
  sessionTabs = sessionTabs.filter(id => id !== tabId);
  await persistSessionTabs();
}

/** 最近一个仍然存在的会话标签；不存在时返回 null（调用方新建标签）。 */
async function lastLiveSessionTab() {
  await loadSessionTabs();
  for (let index = sessionTabs.length - 1; index >= 0; index--) {
    try { return await chrome.tabs.get(sessionTabs[index]); } catch (_) { /* 已被关掉 */ }
  }
  return null;
}

async function closeTabIds(ids) {
  const closed = [];
  for (const id of ids) {
    try {
      await chrome.tabs.remove(id);
      observedConsoleDocs.delete(id);
      observedNetworkDocs.delete(id);
      dropWebRequestCapture(id);
      closed.push(id);
    } catch (_) { /* 已经不存在：跳过，不算失败 */ }
  }
  await loadSessionTabs();
  sessionTabs = sessionTabs.filter(id => !closed.includes(id));
  await persistSessionTabs();
  return closed;
}
