/**
 * EMA2 page guard · 在页面主世界（MAIN world）里最早执行。
 *
 * 目的：真实站点上的 alert/confirm/prompt/print 会阻塞渲染进程，
 * `chrome.scripting.executeScript` 和 CDP `Runtime.evaluate` 都会一起挂住，
 * 一个对话框就能让整个 tab 不可操作。这里把这几类同步阻塞 API 变成
 * 非阻塞记录 + 默认关闭，并记录 window.open 是否被浏览器拦截。
 *
 * 行为：
 *   - alert(msg)      → 记录，立即返回
 *   - confirm(msg)    → 记录，返回 false（默认不确认有副作用的操作）
 *   - prompt(msg, d)  → 记录，返回 null
 *   - print()         → 记录，不打开打印预览
 *   - window.open()   → 调用原始实现；返回 null 时记录 popup_blocked
 *
 * 记录只放在页面内存 `window.__ema2_guard.events`，snapshot/act 读取后不会
 * 写回 DOM。该文件同时作为 manifest 的 document_start content script 和
 * `chrome.scripting.executeScript({ files: ['page_guard.js'], world: 'MAIN' })`
 * 的注入源使用，因此必须是自包含的 IIFE。
 */
(function ema2PageGuard() {
  if (window.__ema2_guard && window.__ema2_guard.version >= 1) return;

  const MAX_EVENTS = 50;
  const state = {
    version: 1,
    seq: 0,
    events: [],
  };
  window.__ema2_guard = state;

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

  // 原生函数可能被页面替换/绑定；保留原始实现的引用，避免递归。
  const original = {
    alert: window.alert,
    confirm: window.confirm,
    prompt: window.prompt,
    print: window.print,
    open: window.open,
  };

  try {
    window.alert = function ema2Alert(message) {
      record('dialog', { dialog: 'alert', message: text(message), handled: 'dismissed' });
    };
  } catch (_) { /* 某些页面用不可写属性锁定，忽略 */ }

  try {
    window.confirm = function ema2Confirm(message) {
      record('dialog', {
        dialog: 'confirm', message: text(message), handled: 'dismissed', result: false,
      });
      return false;
    };
  } catch (_) { /* ignore */ }

  try {
    window.prompt = function ema2Prompt(message, defaultValue) {
      record('dialog', {
        dialog: 'prompt',
        message: text(message),
        defaultValue: text(defaultValue),
        handled: 'dismissed',
        result: null,
      });
      return null;
    };
  } catch (_) { /* ignore */ }

  try {
    window.print = function ema2Print() {
      record('dialog', { dialog: 'print', message: 'window.print()', handled: 'blocked' });
    };
  } catch (_) { /* ignore */ }

  // beforeunload 是浏览器原生确认框，不能靠 window.alert 覆盖；把赋值型
  // handler 吞掉并记录。addEventListener('beforeunload') 仍由页面自己注册。
  try {
    Object.defineProperty(window, 'onbeforeunload', {
      configurable: true,
      get() { return null; },
      set(handler) {
        if (typeof handler === 'function') {
          record('dialog', {
            dialog: 'beforeunload',
            message: 'beforeunload handler suppressed',
            handled: 'suppressed',
          });
        }
      },
    });
  } catch (_) { /* some pages lock the property */ }

  try {
    window.open = function ema2Open(url, target, features) {
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
      });
      return opened;
    };
  } catch (_) { /* ignore */ }

  // 只保留这些原始引用，避免页面脚本通过 window.open.toString() 等检测时崩溃。
  state.original = original;
})();
