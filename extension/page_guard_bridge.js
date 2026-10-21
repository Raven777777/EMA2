/**
 * ISOLATED world bridge：MAIN world 的 page_guard_armed.js 没有 chrome.* API，
 * 由这里把“新文档已 arm”的事件转给 service worker。
 * 若命令租约已经释放（lease=0），立即通知 MAIN world disarm，堵住释放竞态。
 */
(function ema2PageGuardBridge() {
  const askLease = () => {
    try {
      chrome.runtime.sendMessage({ type: 'ema2/guard-armed' }, (reply) => {
        if (chrome.runtime.lastError) return;
        if (!reply || reply.lease !== 0) return;
        try { document.dispatchEvent(new CustomEvent('ema2-guard-force-disarm')); } catch (_) { /* ignore */ }
      });
    } catch (_) { /* extension context invalidated */ }
  };

  try { document.addEventListener('ema2-guard-armed', askLease, true); } catch (_) { /* ignore */ }
  // 两侧脚本执行顺序不确定：先广播 ready，MAIN 侧收到后会重发 armed。
  try { document.dispatchEvent(new CustomEvent('ema2-guard-bridge-ready')); } catch (_) { /* ignore */ }
})();
