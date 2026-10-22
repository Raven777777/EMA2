/**
 * 页面执行桥 runInTab、文档句柄、helper 安装。
 */

// ---------------------------------------------------------------- page runs

async function runInTab(tabId, func, args, world, attempts = 2) {
  const options = { target: { tabId }, func, args };
  if (world) options.world = world;
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const results = await chrome.scripting.executeScript(options);
      return results && results[0] ? results[0].result : undefined;
    } catch (error) {
      lastError = error;
      // debugger attach/detach 之后偶尔会瞬时拿不到页面访问权，重试一次通常就好。
      if (attempt + 1 < attempts) await sleep(180);
    }
  }
  throw lastError;
}

// 必须用 local 而不是 session：session 在扩展重载/更新时会被清空，
// 计数器归零后新文档会重新拿到 d1、d2…，和仍然开着的旧页面撞号，
// 于是旧 ref 就能在另一个文档上通过校验。
const DOC_SEQ_KEY = 'ema2_doc_seq';
let docHandleSeq = 0;
let docSeqReady = null;
let docSeqLoaded = false;

/**
 * 只从 storage.session 读一次。
 * 必须缓存「读取过程」而不是「读取结果」：两个 tab 同时首次取号时，
 * 各自 await 一次 get() 会把已经自增过的值重新覆盖回 0，于是两边都拿到 d1。
 */
function ensureDocSeq() {
  if (!docSeqReady) {
    docSeqReady = (async () => {
      try {
        const stored = await chrome.storage.local.get(DOC_SEQ_KEY);
        // 内存里可能已经发过号（上一次写入失败）；不能回退到更小的持久值。
        docHandleSeq = Math.max(docHandleSeq, Number(stored && stored[DOC_SEQ_KEY]) || 0);
        docSeqLoaded = true;
      } catch (_) {
        // 读取失败不能永久缓存成 0：下次调用重试；内存里已发过的号必须保留。
        docSeqReady = null;
        docSeqLoaded = false;
      }
    })();
  }
  return docSeqReady;
}

/**
 * 下一个文档句柄（d1、d2…）。初始化之后的自增是同步的，两个文档不会拿到同一个号。
 * 读取或写入 storage 失败时拒绝发号：发一个无法持久化的号会在 SW 重启后回绕，
 * 让旧 ref 在新文档上通过校验 —— 这是 @dN 检查存在的意义。
 */
async function nextDocHandle() {
  await ensureDocSeq();
  if (!docSeqLoaded) {
    fail('doc_handle_failed', 'cannot read the document handle counter from storage',
      'reload the extension; if it persists, check that chrome.storage.local is available');
  }
  docHandleSeq += 1;
  try {
    // await：service worker 在写入前被回收会让序号回绕，旧 ref 就可能在新文档上通过校验。
    await chrome.storage.local.set({ [DOC_SEQ_KEY]: docHandleSeq });
  } catch (_) {
    fail('doc_handle_failed', 'cannot persist the document handle counter',
      'reload the extension; if it persists, check that chrome.storage.local is available');
  }
  return 'd' + docHandleSeq;
}

/**
 * 注入页面 helper，并给新文档分配一个短句柄。
 * 句柄只在「本次真的装了 helper」时分配：已装好的页面不消耗号，号就不会被空调用推高。
 */
async function ensureHelpers(tabId) {
  const result = await runInTab(tabId, installHelpers, []);
  // 注入结果丢了（帧刚切走）时按「装了」处理：宁可变废旧 ref，也不能让页面带着空句柄跑。
  const installed = result == null || result === true || result.installed === true;
  // 已安装且句柄还在：不消耗新号。
  if (!installed && result && result.handle) return true;
  // 新安装，或旧页面丢了句柄（上次 setDocHandle 失败）：补一个。
  const handle = await nextDocHandle();
  try {
    await runInTab(tabId, (value) => window.__ema2_helpers.setDocHandle(value), [handle]);
  } catch (_) { /* 途中导航：新文档会重新安装并拿到新句柄 */ }
  return true;
}
