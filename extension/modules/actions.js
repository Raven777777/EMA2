/**
 * 页面内查找/动作、焦点、键盘与可信输入。
 */

/** 在 MAIN/isolated world 中执行一次完整的“查找 + 校验 + 动作”，失败结果由调用方映射为错误。 */
function performActionInPage(spec) {
  const helpers = window.__ema2_helpers;
  if (!helpers) return { found: false, code: 'helpers_missing', message: 'page helpers are not installed', hint: 'reload the page and retry' };
  const action = spec.action;
  const ref = spec.ref || null;
  const selector = spec.selector || null;
  const target = spec.target || null;
  const hasLocator = !!(ref || selector || target);
  const missing = () => ({
    found: false,
    stale: !!ref && !helpers.refIsCurrent(ref),
    hadSnapshot: !!helpers.docEpoch(),
    candidates: helpers.candidates(5),
  });

  /** 穿透 shadow root 与同源 iframe 的命中测试。坐标相对传入 document 的 viewport。 */
  const hitTest = (doc, x, y) => {
    if (!doc) return null;
    let hit = doc.elementFromPoint(x, y);
    for (let guard = 0; hit && hit.shadowRoot && guard < 10; guard++) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    if (hit && /^(IFRAME|FRAME)$/.test(hit.tagName)) {
      try {
        const innerDoc = hit.contentDocument;
        if (innerDoc) {
          const rect = hit.getBoundingClientRect();
          const inner = hitTest(
            innerDoc,
            x - rect.left - (Number(hit.clientLeft) || 0),
            y - rect.top - (Number(hit.clientTop) || 0),
          );
          if (inner) return inner;
        }
      } catch (_) { /* cross-origin */ }
    }
    return hit;
  };

  const relatedTo = (el, hit) => {
    if (!el || !hit) return false;
    if (el === hit || el.contains(hit) || hit.contains(el)) return true;
    // shadow DOM：沿 host 链向上找目标元素。
    let node = hit;
    for (let guard = 0; node && guard < 20; guard++) {
      if (node === el) return true;
      node = node.parentNode || node.host || null;
    }
    return false;
  };

  /**
   * 点击前检查真实可点性：隐藏/ inert / pointer-events:none / 被 overlay 覆盖。
   * 返回 null 表示可点；否则返回可直接映射为 ToolError 的结果。
   */
  const clickBlocker = (el) => {
    if (!helpers.isActionableRole(el, helpers.roleOf(el))) {
      return {
        code: 'not_clickable',
        message: 'target is a non-interactive container',
        hint: 'snapshot again and click the real button/link/input ref',
      };
    }
    const doc = el.ownerDocument || document;
    const view = doc.defaultView || window;
    let style;
    try { style = view.getComputedStyle(el); } catch (_) { style = null; }
    if (style && (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0')) {
      return {
        code: 'not_clickable',
        message: 'target is not visible',
        hint: 'the element may have been hidden after the snapshot; take a fresh snapshot',
      };
    }
    let inert = false;
    try { inert = !!el.closest('[inert]'); } catch (_) { inert = false; }
    if (inert) {
      return {
        code: 'not_clickable',
        message: 'target is inside an inert subtree',
        hint: 'close the modal/overlay that made the page inert, then retry',
      };
    }
    if (style && style.pointerEvents === 'none') {
      return {
        code: 'not_clickable',
        message: 'target has pointer-events:none',
        hint: 'click the element that actually receives pointer events',
      };
    }
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return { code: 'not_clickable', message: 'target has an empty box', hint: 'snapshot again' };
    }
    const x = Math.round(rect.left + rect.width / 2);
    const y = Math.round(rect.top + rect.height / 2);
    let hit = null;
    try { hit = hitTest(doc, x, y); } catch (_) { hit = null; }
    if (!hit) {
      return {
        code: 'not_clickable',
        message: 'target is outside the viewport',
        hint: 'scroll it into view (act scroll) and snapshot again',
      };
    }
    if (!relatedTo(el, hit)) {
      const tag = String(hit.tagName || 'element').toLowerCase();
      return {
        code: 'obscured',
        message: `target is covered by <${tag}>`,
        hint: 'close the overlay/modal/cookie banner first, or click the element that actually receives the pointer',
      };
    }
    return null;
  };

  const formValidationFailed = (form, submitter) => {
    if (!form || typeof form.checkValidity !== 'function') return false;
    if (form.noValidate) return false;
    if (submitter && submitter.formNoValidate) return false;
    try { return !form.checkValidity(); } catch (_) { return false; }
  };

  // press 的 key 支持 "Control+Shift+L"：最后一段是键，其余是修饰位。
  const parseKeyCombo = (raw) => {
    const source = String(raw || '').trim();
    // '+' 本身无法用分隔符解析，单独处理（真实事件是 Shift+Equal）。
    if (source === '+') {
      return { key: '+', code: 'Equal', ctrlKey: false, shiftKey: true, altKey: false, metaKey: false };
    }
    const parts = source.split('+').map(part => part.trim()).filter(Boolean);
    const token = parts.pop() || '';
    const combo = { key: '', code: '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
    for (const part of parts) {
      const lower = part.toLowerCase();
      if (lower === 'ctrl' || lower === 'control') combo.ctrlKey = true;
      else if (lower === 'shift') combo.shiftKey = true;
      else if (lower === 'alt' || lower === 'option') combo.altKey = true;
      else if (lower === 'meta' || lower === 'cmd' || lower === 'command' || lower === 'win') combo.metaKey = true;
    }
    if (!token) return combo;
    const alias = {
      esc: 'Escape', escape: 'Escape', enter: 'Enter', return: 'Enter', tab: 'Tab', space: ' ',
      backspace: 'Backspace', delete: 'Delete', del: 'Delete', insert: 'Insert', home: 'Home',
      end: 'End', pageup: 'PageUp', pagedown: 'PageDown', arrowup: 'ArrowUp', up: 'ArrowUp',
      arrowdown: 'ArrowDown', down: 'ArrowDown', arrowleft: 'ArrowLeft', left: 'ArrowLeft',
      arrowright: 'ArrowRight', right: 'ArrowRight',
    };
    const lower = token.toLowerCase();
    if (alias[lower] !== undefined) {
      combo.key = alias[lower];
      combo.code = alias[lower] === ' ' ? 'Space' : alias[lower];
      return combo;
    }
    if (token.length === 1) {
      combo.key = token;
      combo.code = /[a-z]/i.test(token) ? `Key${token.toUpperCase()}`
        : (/[0-9]/.test(token) ? `Digit${token}` : '');
      return combo;
    }
    combo.key = token;
    combo.code = /^f\d{1,2}$/i.test(token) ? token.toUpperCase() : token;
    return combo;
  };

  if (action === 'fill') {
    const fields = Array.isArray(spec.fields) ? spec.fields : [];
    const results = [];
    for (const item of fields) {
      const label = item.ref || item.selector || item.target || '(none)';
      const el = helpers.find(item.ref || null, item.selector || null, item.target || null);
      if (!el) {
        results.push({ ok: false, target: label, code: item.ref && !helpers.refIsCurrent(item.ref) ? 'stale_ref' : 'target_not_found' });
        continue;
      }
      const written = helpers.setValue(el, item.value);
      if (!written.ok) {
        results.push({ ok: false, target: label, code: written.code, message: written.message });
        continue;
      }
      if (!written.selfDispatched) {
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      results.push({ ok: true, target: label });
    }
    const failed = results.filter(item => !item.ok);
    if (failed.length) {
      return {
        found: true,
        ok: false,
        code: failed[0].code || 'target_not_found',
        message: 'some fill targets failed: ' + failed.map(item => item.target).join(','),
        fields: results,
      };
    }
    return { found: true, ok: true, info: `fields=${results.length}` };
  }

  let el;
  if (action === 'press' && !hasLocator) {
    el = document.activeElement || document.body;
  } else if (action === 'submit' && !hasLocator) {
    el = document.activeElement || document.body;
  } else {
    el = helpers.find(ref, selector, target);
    if (!el) return missing();
  }
  if (!el) return missing();
  try { el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); } catch (_) { /* ignore */ }
  const disabled = !!el.disabled || el.getAttribute('aria-disabled') === 'true';

  switch (action) {
    case 'click': {
      if (disabled) return { found: true, ok: false, code: 'element_disabled', message: 'target is disabled' };
      const blocker = clickBlocker(el);
      if (blocker) return { found: true, ok: false, ...blocker };
      const tag = el.tagName.toLowerCase();
      const form = el.closest && el.closest('form');
      const anchor = tag === 'a' && !!(el.href || el.getAttribute('href'));
      const submitter = (tag === 'button' && (!el.type || el.type === 'submit'))
        || (tag === 'input' && (el.type === 'submit' || el.type === 'image'));
      const rawHref = anchor ? String(el.getAttribute('href') || '') : '';
      const lowerHref = rawHref.trim().toLowerCase();
      // 只有 http(s)/相对导航链接才值得按“导航”等待/重试；
      // javascript:、mailto:、纯 # 锚点、download 都不是页面导航。
      const navigable = anchor
        && !el.hasAttribute('download')
        && !lowerHref.startsWith('javascript:')
        && !lowerHref.startsWith('mailto:')
        && !lowerHref.startsWith('tel:')
        && !lowerHref.startsWith('#');
      const download = anchor && el.hasAttribute('download');
      const validationFailed = submitter && formValidationFailed(form, el);
      const effect = validationFailed ? 'none'
        : download ? 'download'
          : navigable || (submitter && !!form) ? 'nav' : 'page';
      const nextUrl = anchor ? (el.href || el.getAttribute('href') || '') : (submitter && form ? form.action || '' : '');
      // 纯 el.click() 只产生一个 click 事件：没有 pointer/mouse 序列，聚焦还是静默的，
      // 密码管理器这类靠 focusin/pointerdown 出浮层的扩展完全看不到这次点击。
      // 按真实浏览器顺序补事件，真正的激活仍然交给 el.click()，只发生一次。
      try {
        const rect = el.getBoundingClientRect();
        const x = Math.round(rect.left + rect.width / 2);
        const y = Math.round(rect.top + rect.height / 2);
        const base = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window };
        const pointer = (type, buttons) => el.dispatchEvent(new PointerEvent(type, {
          ...base, button: 0, buttons, pointerId: 1, pointerType: 'mouse', isPrimary: true,
        }));
        const mouse = (type, buttons) => el.dispatchEvent(new MouseEvent(type, { ...base, button: 0, buttons, detail: 1 }));
        pointer('pointerover', 0);
        pointer('pointerenter', 0);
        pointer('pointermove', 0);
        pointer('pointerdown', 1);
        mouse('mousedown', 1);
        try { el.focus({ preventScroll: true }); } catch (_) { try { el.focus(); } catch (_) { /* ignore */ } }
        if (typeof el.setSelectionRange === 'function') {
          try {
            const length = String(el.value == null ? '' : el.value).length;
            el.setSelectionRange(length, length);
          } catch (_) { /* 非文本控件不支持选区，忽略 */ }
        }
        pointer('pointerup', 0);
        mouse('mouseup', 0);
      } catch (_) { /* 补事件是尽力而为，不影响下面的真实激活 */ }
      try { el.click(); } catch (error) {
        return { found: true, ok: false, code: 'click_failed', message: String(error && error.message || error) };
      }
      const info = ref ? `ref=${ref}` : target ? `target=${target}` : `selector=${selector}`;
      // canRetry 只影响 settle 后的重试策略：只有可导航链接允许安全重试，
      // javascript:/download/# 链接和按钮都不重试，避免副作用执行两次。
      const kind = navigable ? 'anchor' : submitter ? 'submit' : 'node';
      return {
        found: true, ok: true, info, effect, nextUrl, kind, canRetry: navigable,
        download, validation: validationFailed ? 'failed' : '',
      };
    }
    case 'type': {
      const text = spec.text === undefined ? '' : String(spec.text);
      // type 在 caret/selection 处插入；fill 才覆盖整个值。
      const written = helpers.setValue(el, text, { mode: 'insert' });
      if (!written.ok) return { found: true, ...written };
      try { el.focus(); } catch (_) { /* ignore */ }
      if (!written.selfDispatched) {
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      let effect = 'none';
      let nextUrl = '';
      let submitted = false;
      let validation = '';
      if (spec.submit) {
        // 页面已经接管 Enter（preventDefault，例如自己提交过）时不再重复 requestSubmit。
        const notCanceled = el.dispatchEvent(new KeyboardEvent('keydown', {
          key: 'Enter', code: 'Enter', bubbles: true, cancelable: true,
        }));
        const form = el.closest && el.closest('form');
        if (formValidationFailed(form, null)) {
          // HTML5 校验失败时浏览器不会提交；不要伪装成 nav。
          validation = 'failed';
        } else if (notCanceled && form && form.requestSubmit) {
          form.requestSubmit();
          submitted = true;
          effect = 'nav';
          nextUrl = form.action || '';
        }
      }
      // 只返回写入长度；输入内容（尤其是密码）绝不进入工具输出。
      return {
        found: true,
        ok: true,
        info: `wrote=1 chars=${text.length} trusted=0${spec.submit ? ` submit=${submitted ? 1 : 0}` : ''}`,
        effect,
        nextUrl,
        validation,
      };
    }
    case 'select': {
      if (el.tagName !== 'SELECT') {
        return { found: true, ok: false, code: 'not_select', message: 'select action requires a <select> element' };
      }
      const value = spec.value === undefined ? '' : String(spec.value);
      const written = helpers.setValue(el, value);
      if (!written.ok) return { found: true, ...written };
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return { found: true, ok: true, info: `value=${value.slice(0, 40)}` };
    }
    case 'press': {
      const combo = parseKeyCombo(String(spec.key || ''));
      if (!combo.key) return { found: true, ok: false, code: 'missing_key', message: 'press requires key' };
      if (hasLocator) { try { el.focus(); } catch (_) { /* ignore */ } }
      const pressTarget = el || document.activeElement || document.body;
      if (!pressTarget) return { found: true, ok: false, code: 'target_not_found', message: 'no element to receive the key' };
      const opts = {
        key: combo.key,
        code: combo.code,
        bubbles: true,
        cancelable: true,
        ctrlKey: combo.ctrlKey,
        shiftKey: combo.shiftKey,
        altKey: combo.altKey,
        metaKey: combo.metaKey,
      };
      const notCanceled = pressTarget.dispatchEvent(new KeyboardEvent('keydown', opts));
      pressTarget.dispatchEvent(new KeyboardEvent('keyup', opts));
      let effect = 'page';
      let nextUrl = '';
      let validation = '';
      if (combo.key === 'Enter' && notCanceled) {
        const form = pressTarget.closest && pressTarget.closest('form');
        if (formValidationFailed(form, null)) {
          validation = 'failed';
          effect = 'none';
        } else if (form && form.requestSubmit) {
          form.requestSubmit();
          effect = 'nav';
          nextUrl = form.action || '';
        }
      }
      const where = ref ? `ref=${ref}` : selector ? `selector=${selector}` : target ? `target=${target}` : 'target=active';
      return { found: true, ok: true, info: `key=${spec.key} ${where} trusted=0`, effect, nextUrl, validation };
    }
    case 'hover': {
      const rect = el.getBoundingClientRect();
      const x = rect.x + rect.width / 2;
      const y = rect.y + rect.height / 2;
      const opts = { bubbles: true, clientX: x, clientY: y };
      const pointerOpts = { ...opts, pointerId: 1, pointerType: 'mouse', isPrimary: true };
      el.dispatchEvent(new PointerEvent('pointerover', pointerOpts));
      el.dispatchEvent(new PointerEvent('pointerenter', pointerOpts));
      el.dispatchEvent(new PointerEvent('pointermove', pointerOpts));
      el.dispatchEvent(new MouseEvent('mouseover', opts));
      el.dispatchEvent(new MouseEvent('mouseenter', opts));
      el.dispatchEvent(new MouseEvent('mousemove', opts));
      return { found: true, ok: true, info: '' };
    }
    case 'submit': {
      const form = el.closest && el.closest('form');
      if (!form || !form.requestSubmit) {
        return { found: true, ok: false, code: 'no_form', message: 'target is not inside a form', hint: 'use click on the submit button or press Enter on the field' };
      }
      if (formValidationFailed(form, null)) {
        return { found: true, ok: true, info: 'submitted=0 validation=failed', effect: 'none', validation: 'failed' };
      }
      form.requestSubmit();
      return { found: true, ok: true, info: 'submitted=1', effect: 'nav', nextUrl: form.action || '' };
    }
    default:
      return { found: true, ok: false, code: 'invalid_action', message: `unsupported action: ${action}` };
  }
}

/**
 * 在页面内定位并聚焦目标，供 CDP Input.* 可信按键使用。
 * 只做校验/聚焦，不修改值；type 还需要知道控件是否支持 Input.insertText。
 */
function focusInPage(spec) {
  const helpers = window.__ema2_helpers;
  if (!helpers) {
    return { found: false, code: 'helpers_missing', message: 'page helpers are not installed', hint: 'reload the page and retry' };
  }
  const ref = spec.ref || null;
  const selector = spec.selector || null;
  const target = spec.target || null;
  const el = helpers.find(ref, selector, target);
  if (!el) {
    return {
      found: false,
      stale: !!ref && !helpers.refIsCurrent(ref),
      candidates: helpers.candidates(5),
    };
  }
  if (el.disabled) return { found: true, ok: false, code: 'element_disabled', message: 'target is disabled' };
  const activeInfo = (node) => {
    if (!node) return { tag: '', role: '', name: '' };
    return {
      tag: String(node.tagName || '').toLowerCase(),
      role: helpers.roleOf(node),
      name: helpers.nameOf(node).slice(0, 80),
    };
  };
  const before = activeInfo(document.activeElement);
  try { el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' }); } catch (_) { /* ignore */ }
  let focused = false;
  try {
    el.focus({ preventScroll: true });
    focused = document.activeElement === el;
  } catch (_) {
    try { el.focus(); focused = document.activeElement === el; } catch (_) { /* ignore */ }
  }
  if (!focused && el.tagName !== 'BODY') {
    return {
      found: true,
      ok: false,
      code: 'not_clickable',
      message: 'target cannot receive keyboard focus',
      hint: 'focus a nearby input/button, or use click instead',
    };
  }
  const tag = el.tagName;
  const inputType = tag === 'INPUT' ? String(el.type || 'text').toLowerCase() : '';
  const editable = el.isContentEditable
    || tag === 'TEXTAREA'
    || (tag === 'INPUT' && !['file', 'checkbox', 'radio', 'hidden', 'submit', 'reset', 'button', 'image'].includes(inputType));
  const insertable = el.isContentEditable
    || tag === 'TEXTAREA'
    || (tag === 'INPUT' && ['text', 'search', 'email', 'url', 'tel', 'password'].includes(inputType));
  const form = el.closest && el.closest('form');
  const formInvalid = !!form && !form.noValidate && !el.formNoValidate
    && typeof form.checkValidity === 'function' && !form.checkValidity();
  return {
    found: true,
    ok: true,
    editable,
    insertable,
    readOnly: !!el.readOnly,
    formInvalid,
    nextUrl: form ? (form.action || '') : '',
    before,
    after: activeInfo(document.activeElement),
  };
}

/**
 * 背景页侧的键名解析（页面内的 parseKeyCombo 只负责合成事件）。
 * 与页面侧保持同一套别名；CDP 需要 windowsVirtualKeyCode。
 */
function parseKeyComboBackground(raw) {
  const source = String(raw || '').trim();
  if (source === '+') {
    return { key: '+', code: 'Equal', ctrlKey: false, shiftKey: true, altKey: false, metaKey: false };
  }
  const parts = source.split('+').map(part => part.trim()).filter(Boolean);
  const token = parts.pop() || '';
  const combo = { key: '', code: '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
  for (const part of parts) {
    const lower = part.toLowerCase();
    if (lower === 'ctrl' || lower === 'control') combo.ctrlKey = true;
    else if (lower === 'shift') combo.shiftKey = true;
    else if (lower === 'alt' || lower === 'option') combo.altKey = true;
    else if (lower === 'meta' || lower === 'cmd' || lower === 'command' || lower === 'win') combo.metaKey = true;
  }
  if (!token) return combo;
  const alias = {
    esc: 'Escape', escape: 'Escape', enter: 'Enter', return: 'Enter', tab: 'Tab', space: ' ',
    backspace: 'Backspace', delete: 'Delete', del: 'Delete', insert: 'Insert', home: 'Home',
    end: 'End', pageup: 'PageUp', pagedown: 'PageDown', arrowup: 'ArrowUp', up: 'ArrowUp',
    arrowdown: 'ArrowDown', down: 'ArrowDown', arrowleft: 'ArrowLeft', left: 'ArrowLeft',
    arrowright: 'ArrowRight', right: 'ArrowRight',
  };
  const lower = token.toLowerCase();
  if (alias[lower] !== undefined) {
    combo.key = alias[lower];
    combo.code = alias[lower] === ' ' ? 'Space' : alias[lower];
    return combo;
  }
  if (token.length === 1) {
    combo.key = token;
    combo.code = /[a-z]/i.test(token) ? `Key${token.toUpperCase()}`
      : (/[0-9]/.test(token) ? `Digit${token}` : '');
    return combo;
  }
  combo.key = token;
  combo.code = /^f\d{1,2}$/i.test(token) ? token.toUpperCase() : token;
  return combo;
}

function modifierMask(combo) {
  return (combo.altKey ? 1 : 0) | (combo.ctrlKey ? 2 : 0)
    | (combo.metaKey ? 4 : 0) | (combo.shiftKey ? 8 : 0);
}

function virtualKeyCode(combo) {
  const map = {
    Enter: 13, Tab: 9, Backspace: 8, Delete: 46, Escape: 27, ' ': 32,
    ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
    Home: 36, End: 35, PageUp: 33, PageDown: 34, Insert: 45,
    Shift: 16, Control: 17, Alt: 18, Meta: 91,
  };
  if (map[combo.key] != null) return map[combo.key];
  if (combo.code === 'Equal') return 187;
  if (/^Key[A-Z]$/.test(combo.code)) return combo.code.charCodeAt(3);
  if (/^Digit\d$/.test(combo.code)) return 48 + Number(combo.code.slice(5));
  if (/^F\d{1,2}$/.test(combo.key)) return 111 + Number(combo.key.slice(1));
  const upper = String(combo.key).toUpperCase();
  return upper.length === 1 ? upper.charCodeAt(0) : 0;
}

function commandsFor(combo) {
  if ((!combo.ctrlKey && !combo.metaKey) || combo.altKey) return [];
  const key = String(combo.key).toLowerCase();
  if (key === 'a') return ['selectAll'];
  if (key === 'c') return ['copy'];
  if (key === 'x') return ['cut'];
  if (key === 'v') return ['paste'];
  if (key === 'z') return [combo.shiftKey ? 'redo' : 'undo'];
  return [];
}

async function dispatchTrustedKey(tabId, combo, focusedTag) {
  let key = combo.key;
  if (key.length === 1 && /[a-z]/i.test(key)) key = combo.shiftKey ? key.toUpperCase() : key.toLowerCase();
  const vk = virtualKeyCode(combo);
  const base = {
    key,
    code: combo.code || '',
    modifiers: modifierMask(combo),
    windowsVirtualKeyCode: vk,
    nativeVirtualKeyCode: vk,
  };
  const isEnter = combo.key === 'Enter';
  const isSpace = combo.key === ' ' && /^(BUTTON|A|SUMMARY)$/.test(String(focusedTag || '').toUpperCase());
  const plainText = key.length === 1 && !combo.ctrlKey && !combo.metaKey && !combo.altKey && !isSpace;
  const down = { ...base, type: plainText || isEnter ? 'keyDown' : 'rawKeyDown' };
  if (plainText) { down.text = key; down.unmodifiedText = key; }
  if (isEnter) { down.text = '\r'; down.unmodifiedText = '\r'; }
  const commands = commandsFor(combo);
  if (commands.length) down.commands = commands;
  await chrome.debugger.sendCommand({ tabId }, 'Input.dispatchKeyEvent', down);
  await chrome.debugger.sendCommand({ tabId }, 'Input.dispatchKeyEvent', { ...base, type: 'keyUp' });
  await sleep(35);
}

async function activeElementInfo(tabId) {
  return runInTab(tabId, () => {
    const helpers = window.__ema2_helpers;
    const active = document.activeElement;
    if (!active) return { tag: '', role: '', name: '', formAction: '', invalid: false };
    const form = active.closest && active.closest('form');
    return {
      tag: String(active.tagName || '').toLowerCase(),
      role: helpers ? helpers.roleOf(active) : '',
      name: helpers ? helpers.nameOf(active).slice(0, 80) : '',
      formAction: form ? (form.action || '') : '',
      invalid: !!form && !form.noValidate && typeof form.checkValidity === 'function' && !form.checkValidity(),
    };
  }, [], undefined, 1).catch(() => ({ tag: '', role: '', name: '', formAction: '', invalid: false }));
}

/** 超时回读：目标控件（或当前焦点）的当前值，只用于判断输入是否已经落地；绝不把值回传给调用方。 */
async function readControlValue(tabId, spec) {
  return runInTab(tabId, (locator) => {
    const helpers = window.__ema2_helpers;
    let el = null;
    try {
      if (helpers) el = helpers.find(locator.ref || null, locator.selector || null, locator.target || null);
    } catch (_) { el = null; }
    if (!el) el = document.activeElement;
    if (!el) return '';
    if (typeof el.value === 'string') return el.value;
    return el.isContentEditable ? String(el.textContent || '') : '';
  }, [spec], undefined, 1).catch(() => '');
}

/** 可信按键：CDP Input.dispatchKeyEvent 会触发浏览器默认行为（Tab/Backspace/Enter/Ctrl+A）。 */
async function pressViaDebugger(tab, step) {
  const spec = {
    ref: step.ref || null,
    selector: step.selector || null,
    target: step.target || null,
  };
  const hasLocator = !!(spec.ref || spec.selector || spec.target);
  let prepared;
  if (hasLocator) {
    prepared = await runInTab(tab.id, focusInPage, [spec], undefined, 1);
    if (!prepared) return null;
    if (prepared.found === false) return prepared;
    if (!prepared.ok) return prepared;
  } else {
    const before = await activeElementInfo(tab.id);
    prepared = { ok: true, before, after: null, formInvalid: !!before.invalid, nextUrl: '' };
  }
  const combo = parseKeyComboBackground(String(step.key || ''));
  if (!combo.key) return { found: true, ok: false, code: 'missing_key', message: 'press requires key' };
  const where = spec.ref ? `ref=${spec.ref}` : spec.selector ? `selector=${spec.selector}` : spec.target ? `target=${spec.target}` : 'target=active';
  let timedOutAfter = null;
  try {
    await withDebugger(tab.id, async () => {
      await dispatchTrustedKey(tab.id, combo, prepared.after ? prepared.after.tag : prepared.before.tag);
    }, { timeoutMs: 8000, queueWaitMs: 15000 });
  } catch (error) {
    if (error instanceof ToolError && error.code === 'debugger_timeout') {
      // 超时是「结果未知」而不是「没有投递」：按键可能已经生效，重放会二次派发
      // （Enter 可能提交两次表单）。回读焦点，相对按键前有变化就按已生效处理。
      const after = await activeElementInfo(tab.id);
      const beforeTag = prepared.after ? prepared.after.tag : prepared.before.tag;
      const beforeName = prepared.after ? prepared.after.name : prepared.before.name;
      if (beforeTag === after.tag && beforeName === after.name) {
        fail('debugger_timeout', 'press result is unknown: the key may or may not have been delivered',
          'inspect the page (snapshot/read) before retrying; do not blindly repeat');
      }
      timedOutAfter = after;
    } else if (error instanceof ToolError
      && ['debugger_attach_failed', 'debugger_attach_timeout', 'debugger_busy'].includes(error.code)) {
      // 这几类错误确定没有投递（attach 没成功 / 队列没轮到），可以安全降级到页面合成事件。
      return null;
    } else {
      throw error;
    }
  }
  const after = timedOutAfter || await activeElementInfo(tab.id);
  const focusChanged = (prepared.after ? prepared.after.tag : prepared.before.tag) !== after.tag
    || (prepared.after ? prepared.after.name : prepared.before.name) !== after.name;
  const validation = (prepared.formInvalid || after.invalid) && combo.key === 'Enter' ? 'failed' : '';
  // 按键可能已经触发导航，after 里的 form 可能已经消失；用按键前的 form action 判定。
  const formAction = prepared.nextUrl || (prepared.before && prepared.before.formAction) || after.formAction || '';
  const effect = combo.key === 'Enter' && ((formAction && !validation) || after.tag === 'a')
    ? 'nav'
    : 'page';
  return {
    found: true,
    ok: true,
    info: `key=${step.key} ${where} trusted=1 focus_changed=${focusChanged ? 1 : 0} active=${after.tag || 'none'}${timedOutAfter ? ' result=unknown' : ''}`,
    effect,
    nextUrl: effect === 'nav' ? formAction : '',
    validation,
  };
}

/** 可信输入：CDP Input.insertText 在 caret/selection 处插入，不再总是追加到末尾。 */
async function typeViaDebugger(tab, step) {
  const spec = {
    ref: step.ref || null,
    selector: step.selector || null,
    target: step.target || null,
  };
  if (!spec.ref && !spec.selector && !spec.target) return null;
  const prepared = await runInTab(tab.id, focusInPage, [spec], undefined, 1);
  if (!prepared) return null;
  if (prepared.found === false) return prepared;
  if (!prepared.ok) return prepared;
  if (prepared.readOnly) return { found: true, ok: false, code: 'element_readonly', message: 'target is read-only' };
  if (!prepared.editable) return { found: true, ok: false, code: 'not_writable', message: 'target is not an editable control' };
  // number/date/range 等不支持 Input.insertText：交给页面 setValue（同样会校验并回滚）。
  if (!prepared.insertable) return null;
  const text = step.text === undefined ? '' : String(step.text);
  let timedOut = false;
  try {
    await withDebugger(tab.id, async () => {
      await chrome.debugger.sendCommand({ tabId: tab.id }, 'Input.insertText', { text });
      if (step.submit) {
        await dispatchTrustedKey(tab.id, parseKeyComboBackground('Enter'), 'INPUT');
      }
    }, {
      timeoutMs: Math.max(8000, Math.min(60000, text.length * 5 + 3000)),
      queueWaitMs: 20000,
    });
  } catch (error) {
    if (error instanceof ToolError && error.code === 'debugger_timeout') {
      // insertText 超时同样是「结果未知」：回读控件当前值，已包含待输入文本就按生效处理，不再重放。
      // 字段本来就有同样文本时可能误判为成功，但重复插入的代价更高。
      const current = text ? await readControlValue(tab.id, spec) : '';
      if (!current || !current.includes(text)) {
        fail('debugger_timeout', 'type result is unknown: the text may or may not have been inserted',
          'inspect the field value (snapshot/read) before retrying; do not blindly repeat');
      }
      timedOut = true;
    } else if (error instanceof ToolError
      && ['debugger_attach_failed', 'debugger_attach_timeout', 'debugger_busy'].includes(error.code)) {
      // 确定没有投递：退回页面 setValue 路径。
      return null;
    } else {
      throw error;
    }
  }
  const after = await activeElementInfo(tab.id);
  const validation = step.submit && after.invalid ? 'failed' : '';
  const formAction = prepared.nextUrl || after.formAction || '';
  const effect = step.submit && formAction && !validation ? 'nav' : 'none';
  return {
    found: true,
    ok: true,
    info: `wrote=1 chars=${text.length} trusted=1${step.submit ? ` submit=${validation ? 0 : 1}` : ''}${timedOut ? ' result=unknown' : ''}`,
    effect,
    nextUrl: effect === 'nav' ? formAction : '',
    validation,
  };
}
