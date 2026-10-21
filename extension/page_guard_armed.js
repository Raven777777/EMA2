/** document_start 动态 guard 的第二个文件：在页面脚本运行前立即 arm。 */
(function ema2ArmPageGuard() {
  const guard = window.__ema2_guard;
  if (!guard || typeof guard.arm !== 'function') return;
  guard.arm();

  // MAIN world 没有扩展 API；通过 ISOLATED world 的 bridge 询问 SW 是否还有租约。
  // 释放竞态中新文档若被 arm，lease=0 的回复会让它立即恢复原生 API。
  const signalArmed = () => {
    try { document.dispatchEvent(new CustomEvent('ema2-guard-armed')); } catch (_) { /* ignore */ }
  };
  const forceDisarm = () => {
    try {
      if (typeof guard.disarm === 'function') guard.disarm();
    } catch (_) { /* ignore */ }
  };
  // 同一文档可能被多次注入（每个命令一次）；监听器只装一次，避免长驻 SPA 叠加。
  if (!guard.__ema2ArmedListeners) {
    guard.__ema2ArmedListeners = true;
    try {
      document.addEventListener('ema2-guard-force-disarm', forceDisarm, true);
      // bridge 可能比本脚本晚执行：它 ready 后重发一次 armed。
      document.addEventListener('ema2-guard-bridge-ready', signalArmed, true);
    } catch (_) { /* ignore */ }
  }
  signalArmed();
})();
