/** document_start 动态 guard 的第二个文件：在页面脚本运行前立即 arm。 */
(function ema2ArmPageGuard() {
  const guard = window.__ema2_guard;
  if (!guard || typeof guard.arm !== 'function') return;
  // v5 的 arm() 返回本次租约；force-disarm 必须回显它才能解开，
  // 页面伪造的无 detail 事件不再生效。
  let lease;
  try { lease = guard.arm(); } catch (_) { return; }

  // MAIN world 没有扩展 API；通过 ISOLATED world 的 bridge 询问 SW 是否还有租约。
  // 释放竞态中新文档若被 arm，lease=0 的回复会让它立即恢复原生 API。
  const signalArmed = () => {
    try { document.dispatchEvent(new CustomEvent('ema2-guard-armed')); } catch (_) { /* ignore */ }
  };
  const forceDisarm = () => {
    try { guard.disarmWith(lease); } catch (_) { /* ignore */ }
  };
  // 同一文档每个命令注入一次：用 guard 上的句柄替换旧监听器，避免长驻 SPA 叠加，
  // 也保证监听器回显的是最新一次 arm 的 lease。
  try {
    if (typeof guard.__ema2GuardForceDisarm === 'function') {
      document.removeEventListener('ema2-guard-force-disarm', guard.__ema2GuardForceDisarm, true);
    }
    if (typeof guard.__ema2GuardBridgeReady === 'function') {
      document.removeEventListener('ema2-guard-bridge-ready', guard.__ema2GuardBridgeReady, true);
    }
  } catch (_) { /* ignore */ }
  try {
    guard.__ema2GuardForceDisarm = forceDisarm;
    guard.__ema2GuardBridgeReady = signalArmed;
    document.addEventListener('ema2-guard-force-disarm', forceDisarm, true);
    // bridge 可能比本脚本晚执行：它 ready 后重发一次 armed。
    document.addEventListener('ema2-guard-bridge-ready', signalArmed, true);
  } catch (_) { /* ignore */ }
  signalArmed();
})();
