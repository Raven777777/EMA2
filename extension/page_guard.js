/**
 * EMA2 page guard · 只在 EMA2 命令租约期间改变页面对话框行为。
 *
 * 默认状态是 disarmed：页面保留原生 alert/confirm/prompt/print/window.open。
 * 命令执行前由 page_guard_armed.js 或 modules/guard.js 调用 guard.arm() 拿到
 * 随机 lease，把同步阻塞的对话框变成非阻塞记录；命令结束后由后台派发带同一
 * lease 的 `ema2-guard-disarm` 文档事件恢复原生 API。
 *
 * 暴露对象只有只读探针（version/seq/events/armed/probe）和受控方法
 * （arm/disarm/disarmWith）；原生 API 引用只存在于本闭包。
 *
 * 这个文件必须是自包含 IIFE，因为既会作为动态注册的 document_start content
 * script，也会由 chrome.scripting.executeScript 注入。
 */
(function ema2PageGuard() {
  // 修改 wrappers/restore 逻辑时递增；旧版本页面会在下一次命令注入时被替换。
  // 同步提升 contract/runtime.json 的 guardProtocolVersion，让升级后的首次启动全量清理。
  const GUARD_VERSION = 5;
  const prior = window.__ema2_guard;
  // 页面与本脚本共享同一个 realm，版本号不是安全边界，只防误撞/误伪造：
  // 旧实例必须同时提供 probe() 才认可，否则照样重装（v1-v4 都没有 probe）。
  if (prior && prior.version >= GUARD_VERSION && typeof prior.probe === 'function') return;

  const MAX_EVENTS = 50;
  // 原生 API 只留在本 IIFE 闭包里，绝不再挂到 window.__ema2_guard。
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
    seq: 0,
    events: [],
  };
  // armed / activeLease 只存在于闭包；state 上只暴露一个只读 getter。
  let armed = false;
  let activeLease = '';
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
        handled: armed ? 'dismissed' : 'passed',
      });
      if (!armed) return original.alert.call(window, message);
    },
    confirm(message) {
      const result = armed ? false : original.confirm.call(window, message);
      record('dialog', {
        dialog: 'confirm',
        message: text(message),
        handled: armed ? 'dismissed' : 'passed',
        result,
      });
      return result;
    },
    prompt(message, defaultValue) {
      const result = armed ? null : original.prompt.call(window, message, defaultValue);
      record('dialog', {
        dialog: 'prompt',
        message: text(message),
        defaultValue: text(defaultValue),
        handled: armed ? 'dismissed' : 'passed',
        result,
      });
      return result;
    },
    print() {
      record('dialog', {
        dialog: 'print',
        message: 'window.print()',
        handled: armed ? 'blocked' : 'passed',
      });
      if (!armed) return original.print.call(window);
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
        handled: armed ? 'observed' : 'passed',
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

  const randomLease = () => {
    try {
      const bytes = new Uint8Array(16);
      crypto.getRandomValues(bytes);
      let value = '';
      for (const byte of bytes) value += byte.toString(16).padStart(2, '0');
      return value;
    } catch (_) {
      // 非安全上下文/老浏览器没有 crypto.getRandomValues：退回 Math.random。
      return `l${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
    }
  };

  const disarmNow = () => {
    armed = false;
    activeLease = '';
    restore();
  };

  const armWith = (lease) => {
    activeLease = lease ? String(lease) : randomLease();
    armed = true;
    install();
    return activeLease;
  };

  // disarm() 是安全方向（恢复原生 API），允许页面调用；disarmWith() 必须回显
  // 本次 arm 的租约，伪造的布尔值无法通过。
  const disarmWith = (lease) => {
    if (!activeLease || String(lease) !== activeLease) return false;
    disarmNow();
    return true;
  };

  const probe = () => {
    try {
      return {
        version: GUARD_VERSION,
        armed,
        wrapped: window.alert === wrappers.alert
          || window.confirm === wrappers.confirm
          || window.prompt === wrappers.prompt
          || window.print === wrappers.print
          || window.open === wrappers.open,
      };
    } catch (_) {
      // 页面给 window 装了会抛异常的 accessor：保守地当作仍被包装。
      return { version: GUARD_VERSION, armed, wrapped: true };
    }
  };

  const onDisarmEvent = (event) => {
    // 只接受带匹配租约的事件；旧的无 detail 事件在 v5 不再生效。
    const detail = event && event.detail;
    if (!detail || !activeLease || detail.lease !== activeLease) return;
    disarmNow();
  };
  try {
    document.addEventListener('ema2-guard-disarm', onDisarmEvent, true);
  } catch (_) { /* ignore */ }

  // 只读探针 + 受控方法：armed 只有 getter，方法不可枚举、不可重定义。
  try {
    Object.defineProperty(state, 'armed', {
      configurable: false, enumerable: false, get: () => armed,
    });
  } catch (_) { /* ignore */ }
  const expose = (key, value) => {
    try {
      Object.defineProperty(state, key, { value, enumerable: false, configurable: false });
    } catch (_) { /* ignore */ }
  };
  expose('arm', armWith);
  expose('disarm', disarmNow);
  expose('disarmWith', disarmWith);
  expose('probe', probe);

  // 从 v1-v4 升级：旧 armed 文档上仍是旧 wrapper，且旧对象保存着原生引用。
  // 先恢复原生 API，避免 v5 probe 看不到旧 wrapper 而误报已 disarm。
  if (legacy || priorArmed) {
    try { window.alert = original.alert; } catch (_) { /* ignore */ }
    try { window.confirm = original.confirm; } catch (_) { /* ignore */ }
    try { window.prompt = original.prompt; } catch (_) { /* ignore */ }
    try { window.print = original.print; } catch (_) { /* ignore */ }
    try { window.open = original.open; } catch (_) { /* ignore */ }
    try { delete window.onbeforeunload; } catch (_) { /* ignore */ }
  }

  window.__ema2_guard = state;
  if (prior && prior !== state) {
    // 旧对象可能仍被页面持有：删掉它保存的原生引用。
    try { delete prior.original; } catch (_) { /* ignore */ }
  }
})();
