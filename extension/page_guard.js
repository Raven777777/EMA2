/**
 * EMA2 page guard · 只在 EMA2 命令租约期间改变页面对话框行为。
 *
 * 默认状态是 disarmed：页面保留原生 alert/confirm/prompt/print/window.open。
 * 命令执行前由 page_guard_armed.js 或 modules/guard.js 调用 guard.arm()，
 * 把同步阻塞的对话框变成非阻塞记录；命令结束后由后台派发 `ema2-guard-disarm`
 * 文档事件恢复原生 API。
 *
 * 这个文件必须是自包含 IIFE，因为既会作为动态注册的 document_start content
 * script，也会由 chrome.scripting.executeScript 注入。
 */
(function ema2PageGuard() {
  // 修改 wrappers/restore 逻辑时递增；旧版本页面会在下一次命令注入时被替换。
  // 同步提升 contract/runtime.json 的 guardProtocolVersion，让升级后的首次启动全量清理。
  const GUARD_VERSION = 4;
  const prior = window.__ema2_guard;
  if (prior && prior.version >= GUARD_VERSION) return;

  const MAX_EVENTS = 50;
  // 旧 v1 会永久覆盖页面 API；优先复用它保存的原生引用，避免包装套包装。
  const original = prior && prior.original ? prior.original : {
    alert: window.alert,
    confirm: window.confirm,
    prompt: window.prompt,
    print: window.print,
    open: window.open,
  };
  const legacy = !!(prior && prior.version < 2);
  const priorArmed = !!(prior && prior.armed);
  const ownBeforeUnload = Object.getOwnPropertyDescriptor(window, 'onbeforeunload');
  const prototypeBeforeUnload = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(window), 'onbeforeunload',
  );
  // v1 的 own descriptor 是吞掉事件的假 accessor；旧版本 armed 时 own 也可能
  // 是旧 wrapper。两种情况都不能当作原生引用，直接取原型上的 descriptor。
  const usePrototypeBeforeUnload = legacy || priorArmed;
  const originalBeforeUnload = usePrototypeBeforeUnload
    ? prototypeBeforeUnload
    : (ownBeforeUnload || prototypeBeforeUnload);
  const originalBeforeUnloadOwn = !usePrototypeBeforeUnload && !!ownBeforeUnload;

  const state = {
    version: GUARD_VERSION,
    armed: false,
    lease: '',
    seq: 0,
    events: [],
    original,
  };
  let wrapped = false;
  let beforeUnloadHandler = null;

  const record = (kind, detail) => {
    state.seq += 1;
    const event = Object.assign({ seq: state.seq, kind, at: Date.now() }, detail || {});
    state.events.push(event);
    if (state.events.length > MAX_EVENTS) {
      state.events.splice(0, state.events.length - MAX_EVENTS);
    }
    return event;
  };

  const text = (value) => {
    if (value == null) return '';
    try {
      return String(value).replace(/\s+/g, ' ').trim().slice(0, 300);
    } catch (_) {
      return '';
    }
  };

  const setOriginalBeforeUnload = (handler) => {
    const value = typeof handler === 'function' ? handler : null;
    if (!value) {
      // 先用原生 setter 清掉内部 handler，再删除 own 属性：既不会让
      // beforeunload 继续生效，也不会留下 window.onbeforeunload=null 的脏状态。
      try {
        if (originalBeforeUnload && typeof originalBeforeUnload.set === 'function') {
          originalBeforeUnload.set.call(window, null);
        }
      } catch (_) { /* ignore */ }
      try { delete window.onbeforeunload; } catch (_) { /* ignore */ }
      return;
    }
    if (originalBeforeUnload && typeof originalBeforeUnload.set === 'function') {
      originalBeforeUnload.set.call(window, value);
      return;
    }
    try {
      delete window.onbeforeunload;
      window.onbeforeunload = value;
    } catch (_) { /* ignore */ }
  };

  const readBeforeUnload = () => {
    try {
      if (originalBeforeUnload && typeof originalBeforeUnload.get === 'function') {
        return originalBeforeUnload.get.call(window);
      }
      return window.onbeforeunload;
    } catch (_) {
      return null;
    }
  };

  const installBeforeUnload = () => {
    beforeUnloadHandler = readBeforeUnload();
    // 先让原生事件暂时不注册 handler；disarm 时再把页面自己的 handler 装回去。
    setOriginalBeforeUnload(null);
    try {
      Object.defineProperty(window, 'onbeforeunload', {
        configurable: true,
        get() { return beforeUnloadHandler; },
        set(handler) {
          beforeUnloadHandler = typeof handler === 'function' ? handler : null;
          if (beforeUnloadHandler) {
            record('dialog', {
              dialog: 'beforeunload',
              message: 'beforeunload handler suppressed',
              handled: 'suppressed',
            });
          }
        },
      });
    } catch (_) { /* some pages lock the property */ }
  };

  const restoreBeforeUnload = () => {
    const handler = beforeUnloadHandler || readBeforeUnload();
    try {
      // 原生 descriptor 在原型上时删除 own 属性即可；只有页面本来就有 own
      // descriptor 才需要重新定义，避免留下可检测的 DOM 差异。
      if (originalBeforeUnloadOwn && originalBeforeUnload) {
        Object.defineProperty(window, 'onbeforeunload', originalBeforeUnload);
      } else {
        delete window.onbeforeunload;
      }
    } catch (_) { /* ignore */ }
    setOriginalBeforeUnload(handler);
    beforeUnloadHandler = null;
  };

  const wrappers = {
    alert(message) {
      record('dialog', {
        dialog: 'alert',
        message: text(message),
        handled: state.armed ? 'dismissed' : 'passed',
      });
      if (!state.armed) return original.alert.call(window, message);
    },
    confirm(message) {
      const result = state.armed ? false : original.confirm.call(window, message);
      record('dialog', {
        dialog: 'confirm',
        message: text(message),
        handled: state.armed ? 'dismissed' : 'passed',
        result,
      });
      return result;
    },
    prompt(message, defaultValue) {
      const result = state.armed ? null : original.prompt.call(window, message, defaultValue);
      record('dialog', {
        dialog: 'prompt',
        message: text(message),
        defaultValue: text(defaultValue),
        handled: state.armed ? 'dismissed' : 'passed',
        result,
      });
      return result;
    },
    print() {
      record('dialog', {
        dialog: 'print',
        message: 'window.print()',
        handled: state.armed ? 'blocked' : 'passed',
      });
      if (!state.armed) return original.print.call(window);
    },
    open(url, target, features) {
      let opened = null;
      let error = '';
      try {
        opened = original.open.call(window, url, target, features);
      } catch (cause) {
        error = text(cause && cause.message || cause);
      }
      record('popup', {
        url: text(url),
        target: text(target),
        blocked: !opened,
        error,
        handled: state.armed ? 'observed' : 'passed',
      });
      return opened;
    },
  };

  const install = () => {
    if (wrapped) return;
    wrapped = true;
    try { window.alert = wrappers.alert; } catch (_) { /* ignore */ }
    try { window.confirm = wrappers.confirm; } catch (_) { /* ignore */ }
    try { window.prompt = wrappers.prompt; } catch (_) { /* ignore */ }
    try { window.print = wrappers.print; } catch (_) { /* ignore */ }
    try { window.open = wrappers.open; } catch (_) { /* ignore */ }
    installBeforeUnload();
  };

  const restore = () => {
    if (!wrapped) return;
    try { if (window.alert === wrappers.alert) window.alert = original.alert; } catch (_) { /* ignore */ }
    try { if (window.confirm === wrappers.confirm) window.confirm = original.confirm; } catch (_) { /* ignore */ }
    try { if (window.prompt === wrappers.prompt) window.prompt = original.prompt; } catch (_) { /* ignore */ }
    try { if (window.print === wrappers.print) window.print = original.print; } catch (_) { /* ignore */ }
    try { if (window.open === wrappers.open) window.open = original.open; } catch (_) { /* ignore */ }
    restoreBeforeUnload();
    wrapped = false;
  };

  const disarm = () => {
    state.armed = false;
    state.lease = '';
    restore();
  };
  state.arm = (lease) => {
    state.armed = true;
    state.lease = String(lease || '');
    install();
  };
  // 不向页面暴露可调用的 disarm()：后台通过同一个文档事件触发释放。
  // 页面仍能伪造事件（它拥有同一个 JS realm），但不会因为一个公开方法被误调用。
  try {
    document.addEventListener('ema2-guard-disarm', disarm, true);
  } catch (_) { /* ignore */ }

  // 从永久覆盖的 v1 升级时，立即恢复原生 API；真正的 arm 由 armed helper 执行。
  if (legacy) {
    try { window.alert = original.alert; } catch (_) { /* ignore */ }
    try { window.confirm = original.confirm; } catch (_) { /* ignore */ }
    try { window.prompt = original.prompt; } catch (_) { /* ignore */ }
    try { window.print = original.print; } catch (_) { /* ignore */ }
    try { window.open = original.open; } catch (_) { /* ignore */ }
    try { delete window.onbeforeunload; } catch (_) { /* ignore */ }
  }

  window.__ema2_guard = state;
})();
