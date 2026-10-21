/** document_start 动态 guard 的第二个文件：在页面脚本运行前立即 arm。 */
(function ema2ArmPageGuard() {
  const guard = window.__ema2_guard;
  if (guard && typeof guard.arm === 'function') guard.arm();
})();
