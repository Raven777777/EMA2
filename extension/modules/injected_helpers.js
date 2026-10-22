/**
 * 页面内 helper（executeScript 注入，必须自包含）。
 */

/** 页面内 helper（作为 executeScript 的 func 注入，必须自包含）。 */
function installHelpers() {
  // 版本只在这里声明一次：rust/src/tools.rs 的契约测试会解析下面这行
  // `const NAME = N` 常量声明取版本号；页面里的判定与写入也都引用它，避免字面量漂移。
  const HELPERS_VERSION = 21;
  if (window.__ema2_helpers && window.__ema2_helpers_version === HELPERS_VERSION) {
    return { installed: false, handle: String(window.__ema2_doc_handle || '') };
  }
  const clean = (value) => String(value == null ? '' : value).replace(/\s+/g, ' ').trim();

  const roleOf = (el) => {
    const role = el.getAttribute('role'); if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'summary') return 'summary';
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio') return t;
      if (['submit', 'button', 'reset', 'image'].includes(t)) return 'button';
      if (t === 'file') return 'file';
      if (t === 'search') return 'searchbox';
      return 'textbox';
    }
    if (el.isContentEditable) return 'textbox';
    return 'element';
  };
  const labelText = (el) => {
    const doc = el.ownerDocument || document;
    const labelled = el.getAttribute('aria-labelledby');
    if (labelled) {
      const text = String(labelled).split(/\s+/).map(id => {
        const node = doc.getElementById ? doc.getElementById(id) : null;
        return node ? (node.innerText || node.textContent || '') : '';
      }).filter(Boolean).join(' ');
      if (clean(text)) return text;
    }
    if (el.labels && el.labels.length) {
      const text = [...el.labels].map(label => label.innerText || label.textContent || '').join(' ');
      if (clean(text)) return text;
    }
    const wrapping = el.closest ? el.closest('label') : null;
    if (wrapping) {
      const clone = wrapping.cloneNode ? wrapping.cloneNode(true) : null;
      if (clone) {
        for (const control of clone.querySelectorAll ? clone.querySelectorAll('input,textarea,select,button') : []) control.remove();
        const text = clone.innerText || clone.textContent || '';
        if (clean(text)) return text;
      }
    }
    return '';
  };
  /** 名称只来自标签/ARIA/占位符等元数据；绝不使用用户输入的值（密码、textarea、富文本）。 */
  const nameOf = (el) => {
    const tag = el.tagName;
    const type = tag === 'INPUT' ? String(el.type || 'text').toLowerCase() : '';
    const safeValue = tag === 'INPUT' && ['button', 'submit', 'reset'].includes(type) ? el.value : '';
    // textarea / contenteditable 的 innerText 就是用户输入；input 的 value 同理。
    const userValueControl = tag === 'TEXTAREA' || el.isContentEditable === true
      || (tag === 'INPUT' && !['button', 'submit', 'reset', 'image'].includes(type));
    const text = el.getAttribute('aria-label')
      || labelText(el)
      || el.getAttribute('placeholder')
      || el.getAttribute('alt')
      || el.getAttribute('title')
      || safeValue
      || (userValueControl ? '' : el.innerText)
      || '';
    const clipped = clean(text);
    return clipped.length > 80 ? clipped.slice(0, 79) + '…' : clipped;
  };
  const clip = (value, max) => {
    const text = clean(value);
    return text.length > max ? text.slice(0, max - 1) + '…' : text;
  };
  const absoluteHref = (el) => {
    const raw = el.getAttribute('href') || '';
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  const extraOf = (el) => {
    const parts = [];
    // 表单当前值默认不输出（密码/textarea/富文本可能含敏感输入）；
    // 显式 snapshot --values 时才附加。read --selector 仍可直接读单个控件。
    const showValues = () => !!window.__ema2_show_values;
    // href 必须是完整绝对 URL：scheme/host/query/fragment 都是定位信息，
    // 截断成 path 会让 AI 跳到同源错误地址或分不清 mailto/tel。
    // 2048 只是防御性上限（data:/超长追踪参数），正常 URL 远达不到，不受影响。
    if (el.tagName === 'A' && el.getAttribute('href')) {
      parts.push('href=' + clip(absoluteHref(el), 2048));
      if (el.hasAttribute('download')) parts.push('download');
    }
    if (el.tagName === 'INPUT') {
      const t = (el.type || 'text').toLowerCase();
      if (t !== 'text') parts.push('type=' + t);
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (el.checked) parts.push('checked');
      if (t === 'file') {
        parts.push('accept=' + clip(el.getAttribute('accept') || '*', 30));
        const names = el.files ? [...el.files].map(file => file.name).slice(0, 5) : [];
        if (names.length) parts.push('files=' + clip(names.join(','), 100));
      } else if (t === 'password') {
        // 密码值永远不进入快照，只确认“已填写”。
        if (showValues() && el.value) parts.push('value=<hidden>');
      } else if (showValues()
          && !['checkbox', 'radio', 'hidden', 'submit', 'reset', 'button', 'image'].includes(t)
          && el.value) {
        parts.push('value=' + clip(el.value, 80));
      }
    }
    if (el.tagName === 'TEXTAREA') {
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (showValues() && el.value) parts.push('value=' + clip(el.value, 80));
    }
    if (el.tagName === 'SELECT') {
      if (el.name) parts.push('name=' + clip(el.name, 24));
      if (el.multiple) parts.push('multiple');
      const selected = [...el.options].filter(option => option.selected).map(option => option.value);
      if (showValues() && selected.length) parts.push('value=' + clip(selected.join(','), 100));
    }
    // contenteditable 的当前文本只作为 extra 展示，不参与 nameOf，避免把用户输入当成标签。
    if (showValues() && el.isContentEditable && el.textContent) parts.push('value=' + clip(el.textContent, 80));
    if (el.disabled) parts.push('disabled');
    if (el.getAttribute('aria-expanded')) parts.push('expanded=' + el.getAttribute('aria-expanded'));
    return parts.join(' ');
  };
  const visible = (el) => {
    if (!el || el.hidden) return false;
    try {
      if (typeof el.checkVisibility === 'function') {
        if (!el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true })) return false;
      } else {
        if (!el.getClientRects().length) return false;
        const view = (el.ownerDocument && el.ownerDocument.defaultView) || window;
        const style = view.getComputedStyle(el);
        if (style.visibility === 'hidden' || style.display === 'none') return false;
      }
    } catch (_) {
      if (!el.getClientRects().length) return false;
    }
    return true;
  };
  const INTERACTIVE = 'a[href],button,input,textarea,select,summary,[role],[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';

  const collectRoots = () => {
    const roots = [];
    let frameSeq = 0;
    const seen = new Set();
    const visit = (root, frame, frameEl, win) => {
      if (!root || seen.has(root)) return;
      seen.add(root);
      const owner = win || (root.ownerDocument && root.ownerDocument.defaultView) || window;
      roots.push({ root, frame, frameEl: frameEl || null, win: owner });
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) visit(el.shadowRoot, frame, frameEl, owner);
      }
      for (const frameNode of root.querySelectorAll('iframe,frame')) {
        let doc = null;
        try { doc = frameNode.contentDocument; } catch (_) { /* cross-origin */ }
        if (doc) {
          visit(doc, frame ? frame + '.f' + (++frameSeq) : 'f' + (++frameSeq),
            frameNode, doc.defaultView || owner);
        }
      }
    };
    visit(document, '', null, window);
    return roots;
  };

  // 文档句柄（d1、d2…）由扩展侧统一分配：页面自己生成会撞 id，跨文档的 ref 就失去校验意义。
  // 页面只负责持有它，导航后的新文档由扩展重新下发。
  const docEpoch = () => window.__ema2_doc_handle || '';
  const setDocHandle = (handle) => { window.__ema2_doc_handle = String(handle || ''); };
  const parseRef = (value) => {
    const text = String(value == null ? '' : value).trim().replace(/^\[|\]$/g, '').replace(/"/g, '');
    // 全写 e12@d1 带文档校验；CLI 示例里的短写 e12 视作“当前文档”，方便手敲。
    const match = /^e(\d+)(?:@(d\d+))?$/i.exec(text);
    return match ? { seq: Number(match[1]), epoch: match[2] || '' } : null;
  };
  const refIsCurrent = (value) => {
    const parsed = parseRef(value);
    return !!parsed && !!docEpoch() && (!parsed.epoch || parsed.epoch === docEpoch());
  };

  // ref 只存在页面内存：不再写 data-ema2-ref，避免触发 MutationObserver /
  // 框架 hydration / CSS [data-*] 选择器。WeakRef 让长页面反复 snapshot 也不会
  // 把已移除的节点永久留在表里。
  const refs = window.__ema2_refs instanceof Map ? window.__ema2_refs : new Map();
  window.__ema2_refs = refs;
  const elRefs = window.__ema2_el_refs instanceof WeakMap ? window.__ema2_el_refs : new WeakMap();
  window.__ema2_el_refs = elRefs;
  const deref = (entry) => {
    if (typeof WeakRef === 'function' && entry instanceof WeakRef) return entry.deref();
    return entry;
  };
  const refElement = (ref) => {
    const entry = refs.get(ref);
    if (!entry) return null;
    const el = deref(entry);
    if (!el || !el.isConnected) {
      refs.delete(ref);
      return null;
    }
    return el;
  };
  // refs 是长驻 SPA 里的长期表：反复 snapshot 只增不减，所以设一个软上限。
  // 超限时先清掉已失效的条目（元素已移除或已被回收）；仍然超限就按 Map 插入顺序
  // 丢掉最旧的约 10%。elRefs 是 WeakMap，元素被回收后条目自然消失，不用手动清理。
  // 被删掉的旧 ref 会变成 stale，调用方应重新 snapshot；这是长驻 SPA 的预期取舍。
  const REF_SOFT_LIMIT = 5000;
  const pruneRefs = () => {
    if (refs.size <= REF_SOFT_LIMIT) return;
    for (const [ref, entry] of refs) {
      const el = deref(entry);
      if (!el || !el.isConnected) refs.delete(ref);
    }
    if (refs.size <= REF_SOFT_LIMIT) return;
    const drop = Math.ceil(refs.size * 0.1);
    let removed = 0;
    for (const ref of refs.keys()) {
      if (removed >= drop) break;
      refs.delete(ref);
      removed++;
    }
  };
  const setRef = (el, ref) => {
    refs.set(ref, typeof WeakRef === 'function' ? new WeakRef(el) : el);
    elRefs.set(el, ref);
    pruneRefs();
    if (el.getAttribute && el.getAttribute('data-ema2-ref')) el.removeAttribute('data-ema2-ref');
  };
  const refEntries = (since) => {
    const floor = Number.isFinite(Number(since)) ? Number(since) : -1;
    const out = [];
    for (const [ref, entry] of refs) {
      const parsed = parseRef(ref);
      if (!parsed || parsed.seq <= floor) continue;
      const el = deref(entry);
      if (!el || !el.isConnected) {
        refs.delete(ref);
        continue;
      }
      out.push({ ref, el });
    }
    out.sort((left, right) => (parseRef(left.ref) || { seq: 0 }).seq - (parseRef(right.ref) || { seq: 0 }).seq);
    return out;
  };
  const refOf = (el) => {
    const existing = elRefs.get(el);
    if (existing && refIsCurrent(existing)) {
      // 软上限裁剪可能已把这条 ref 从 refs 表里删掉：补回去，否则重新 snapshot
      // 仍会输出解析不了的 ref（elRefs 是 WeakMap，条目本身还在）。
      if (!refs.has(existing)) refs.set(existing, typeof WeakRef === 'function' ? new WeakRef(el) : el);
      return existing;
    }
    // 旧版本写进 DOM 的 data-ema2-ref：迁移到内存表，然后清掉属性。
    const legacy = el.getAttribute('data-ema2-ref');
    if (refIsCurrent(legacy)) {
      const parsed = parseRef(legacy);
      const full = parsed && !parsed.epoch ? 'e' + parsed.seq + '@' + docEpoch() : legacy;
      setRef(el, full);
      return full;
    }
    const seq = (window.__ema2_ref_seq || 0) + 1;
    window.__ema2_ref_seq = seq;
    const ref = 'e' + seq + '@' + docEpoch();
    setRef(el, ref);
    return ref;
  };

  const find = (ref, selector, target) => {
    const roots = collectRoots();
    if (ref) {
      const wanted = String(ref).trim().replace(/^\[|\]$/g, '').replace(/"/g, '');
      if (!refIsCurrent(wanted)) return null;
      const parsed = parseRef(wanted);
      const full = parsed && !parsed.epoch ? `e${parsed.seq}@${docEpoch()}` : wanted;
      const fromMap = refElement(full);
      if (fromMap) return fromMap;
      // 兼容上一个 helper 版本遗留的 DOM 属性；找到后迁移进内存表。
      for (const { root } of roots) {
        const el = root.querySelector('[data-ema2-ref="' + full + '"]');
        if (el) { setRef(el, full); return el; }
      }
      return null;
    }
    if (selector) {
      for (const { root } of roots) {
        try {
          const el = root.querySelector(selector);
          if (el) return el;
        } catch (_) { /* invalid selector in this root */ }
      }
    }
    if (target) {
      const match = /^([a-zA-Z]+)\s*:\s*(.+)$/.exec(String(target).trim());
      if (match) {
        const role = match[1].toLowerCase();
        const name = match[2].trim().toLowerCase();
        const candidates = [];
        for (const { root } of roots) {
          for (const el of root.querySelectorAll(INTERACTIVE)) {
            const currentRole = roleOf(el).toLowerCase();
            if (currentRole !== role) continue;
            // 地标容器（role=search 等）不能作为 target 命中；真正的控件在后代里。
            if (!isActionableRole(el, currentRole)) continue;
            candidates.push(el);
          }
        }
        // 每一层都先取可见控件；只有整层都不可见时才回退隐藏控件。
        const pick = (test) => {
          const matching = candidates.filter(test);
          return matching.find(visible) || matching[0] || null;
        };
        const exact = pick(el => nameOf(el).toLowerCase() === name);
        if (exact) return exact;
        // role:name 同时接受 HTML name 属性；可访问名称精确匹配仍优先。
        const named = pick(el => String(el.getAttribute('name') || '').toLowerCase() === name);
        if (named) return named;
        const partial = pick(el => nameOf(el).toLowerCase().includes(name));
        if (partial) return partial;
      }
    }
    return null;
  };

  const describe = (el) => '[' + refOf(el) + '] ' + roleOf(el) + ' "' + nameOf(el) + '"';

  const candidates = (limit) => {
    const out = [];
    for (const { root } of collectRoots()) {
      for (const el of root.querySelectorAll(INTERACTIVE)) {
        if (!visible(el)) continue;
        out.push(describe(el));
        if (out.length >= (limit || 5)) return out;
      }
    }
    return out;
  };

  /** 能承载动作的角色；其余角色只在有名称时才值得占一行。 */
  const ACTIONABLE_ROLES = new Set([
    'link', 'button', 'textbox', 'searchbox', 'combobox', 'listbox', 'checkbox', 'radio',
    'switch', 'slider', 'spinbutton', 'tab', 'option', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'treeitem', 'file', 'summary', 'input', 'textarea', 'select',
  ]);
  // 真正可操作的 ARIA widget role。`role=search/banner/main/...` 是地标容器，
  // 不是控件：把 div[role=search] 当 searchbox 会让 AI 对着不可写的容器 fill。
  const WIDGET_ROLES = new Set([
    'button', 'link', 'textbox', 'searchbox', 'combobox', 'listbox', 'option', 'checkbox',
    'radio', 'switch', 'slider', 'spinbutton', 'tab', 'menuitem', 'menuitemcheckbox',
    'menuitemradio', 'treeitem', 'gridcell', 'columnheader', 'rowheader', 'scrollbar',
  ]);
  const ACTIONABLE_MATCH = 'a[href],button,input,textarea,select,summary,[contenteditable=""],[contenteditable="true"],[tabindex],[onclick]';
  const isActionableRole = (el, role) => {
    if (WIDGET_ROLES.has(String(role || '').toLowerCase())) return true;
    try { if (el.matches(ACTIONABLE_MATCH)) return true; } catch (_) { /* ignore */ }
    // label 本身可点击（for= 或包着控件），但它是“激活代理”而不是输入控件。
    if (el.tagName === 'LABEL' && (el.htmlFor || el.querySelector('input,select,textarea,button'))) return true;
    return ['A', 'BUTTON', 'INPUT', 'TEXTAREA', 'SELECT', 'SUMMARY'].includes(el.tagName)
      && (el.tagName !== 'A' || el.hasAttribute('href'));
  };

  /**
   * 额外属性（主要是 href）是所有字段里最占预算的：实测占 snapshot 字符的 39%，p90 达 84 字符。
   * 截到 40 字符保留路径主干，需要完整 URL 时用 read/js；filter=links 是专门取链接的场景，不截。
   */
  const clipExtra = (extra, keepFull) => {
    if (!extra || keepFull) return extra;
    // href 必须完整；value/accept/files 等也在 extraOf 里单独限制了长度。
    // 这里不再做 40 字符的总截断，否则 value=<hidden> 会被切成误导性的半截文本。
    return extra;
  };

  /** snapshot 过滤器：links / buttons / form。 */
  const filterFor = (name) => {
    if (name === 'links') return (el) => {
      if (el.tagName !== 'A') return false;
      const href = String(el.getAttribute('href') || '').trim();
      // `#`/`javascript:` 是浏览器操作入口，不是可读链接；Bing 的 skip/feedback
      // 按钮就是这种 <a role=button href="#">，别让它们混进 links 结果。
      return !!href && href !== '#' && !/^javascript:/i.test(href);
    };
    if (name === 'buttons') return (el) => roleOf(el) === 'button';
    if (name === 'form') {
      return (el) => ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON', 'LABEL', 'FIELDSET'].includes(el.tagName)
        || roleOf(el) === 'combobox' || roleOf(el) === 'textbox' || roleOf(el) === 'checkbox' || roleOf(el) === 'radio';
    }
    return null;
  };

  const snapshot = (filter, budget, cursor, nearRef, values) => {
    window.__ema2_show_values = !!values;
    const predicate = filterFor(filter);
    let container = null;
    if (nearRef) {
      const anchor = find(nearRef, null, null);
      if (anchor) {
        container = anchor.closest('form,section,article,dialog,[role="dialog"],table,ul,ol,main,nav,header,footer') || anchor.parentElement || anchor;
      }
    }
    const seen = new Set();
    const lines = [];
    let refs = 0;
    for (const { root, frame } of collectRoots()) {
      for (const el of root.querySelectorAll('h1,h2,h3,' + INTERACTIVE)) {
        if (seen.has(el)) continue;
        seen.add(el);
        if (container && el !== container && !container.contains(el)) continue;
        if (!visible(el)) continue;
        const tag = el.tagName;
        if (tag === 'H1' || tag === 'H2' || tag === 'H3') {
          if (filter === 'full') {
            const text = clean(el.innerText);
            if (text) lines.push('#' + tag[1] + ' ' + text.slice(0, 100) + (frame ? ' frame=' + frame : ''));
          }
          continue;
        }
        if (predicate && !predicate(el)) continue;
        // filter=links 时按链接展示：Bing 等站点会给 <a href> 加 role=button，
        // 直接信 role 会让“只看链接”的结果里出现 button 行。
        const role = filter === 'links' ? 'link' : roleOf(el);
        const name = nameOf(el);
        // 地标容器（例如 Wikipedia 的 div[role=search]）不是控件；只有 filter=full
        // 才保留。实际输入框/链接会作为各自的后代单独出现。
        if (filter !== 'full' && !predicate && !isActionableRole(el, role)) continue;
        // label 只在 --filter form 里出现；默认 interactive 不把表单标签当控件，
        // 否则大表单的 snapshot 会被标签文本占满。
        if (filter !== 'full' && !predicate && tag === 'LABEL') continue;
        // 无名称又不可操作：点了也没意义，只占预算（x.com 这类站点能到 20%）。
        // filter=full 是「无损视图」，保持原样。
        if (filter !== 'full' && !name && !ACTIONABLE_ROLES.has(role)) continue;
        refs++;
        const extra = clipExtra(extraOf(el), filter === 'links');
        lines.push('[' + refOf(el) + '] ' + role + ' "' + name + '"'
          + (extra ? ' ' + extra : '') + (frame ? ' frame=' + frame : ''));
      }
    }
    const total = lines.length;
    const from = Math.min(cursor, total);
    const out = [];
    let used = 0;
    let index = from;
    while (index < total) {
      const line = lines[index];
      if (used > 0 && used + line.length + 1 > budget) break;
      // 首行同样受 budget 约束：单行就超预算时只能截断，否则一次超长记录会突破调用方的预算。
      // index 照常推进，保证 next 不会重复交付本行。
      if (used === 0 && line.length + 1 > budget) {
        out.push(line.slice(0, Math.max(1, budget - 1)) + '…');
        used = budget;
        index++;
        break;
      }
      out.push(line);
      used += line.length + 1;
      index++;
    }
    return {
      header: 'url=' + location.href + '\ntitle=' + clean(document.title)
        + '\nscroll=' + Math.round(window.scrollY) + '/'
        + Math.max(0, document.documentElement.scrollHeight - window.innerHeight)
        + ' refs=' + refs + ' roots=' + collectRoots().length
        + ' doc=' + docEpoch()
        + ' visual=' + document.querySelectorAll('canvas,video,img,picture,object,embed').length
        + (container ? ' near=' + (container.getAttribute('data-ema2-ref') || container.tagName.toLowerCase()) : ''),
      body: out.join('\n'),
      total,
      shown: out.length,
      next: index < total ? index : null,
    };
  };

  /**
   * read 专用可见性（只影响 md/text 的清洁遍历；snapshot/click 的 visible() 不变，见 P0-A/P0-E）：
   * - 隐藏元素（display:none / visibility:hidden / content-visibility）整棵跳过；
   * - `display:contents` 没有自己的盒，Edge 的 checkVisibility 会判 false，但子树照常渲染：
   *   它有自有文本或可见盒后代时就继续遍历，`--no-main` 才不会丢掉整段正文；
   * - 1×1 / clip(0) 的 sr-only 元素对正文无贡献，按隐藏过滤；
   * - BUTTON / [role=button] 是交互噪声，不进入正文（看按钮找 snapshot）。
   */
  const readStyleOf = (el) => {
    const view = (el.ownerDocument && el.ownerDocument.defaultView) || window;
    return view.getComputedStyle(el);
  };
  /** display:contents 的可见性看子树：自有文本节点落在匿名盒里；后代有可见盒就算可见。 */
  const contentsRendered = (el) => {
    try {
      for (const node of el.childNodes) {
        if (node.nodeType === 3 && /\S/.test(String(node.nodeValue || ''))) return true;
      }
      for (const child of el.children) {
        if (child.checkVisibility
          && child.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true })) return true;
        if (readStyleOf(child).display === 'contents' && contentsRendered(child)) return true;
      }
    } catch (_) {
      return true; // 子树算不了就按可见处理，宁可多读不可漏读
    }
    return false;
  };
  /** 1×1 / clip(0) 的视觉隐藏（Bootstrap `.visually-hidden`、`.sr-only` 的常见模式）。 */
  const srOnlyForRead = (el, rect) => {
    if (rect.width <= 1 && rect.height <= 1) return true;
    // 无 1px 尺寸约束时只看内联 clip，不给每个元素额外做一次 getComputedStyle；
    // class 形式的 clip sr-only 绝大多数同时带 1px 尺寸，上面已过滤。
    return /^rect\(\s*0/.test(String((el.style && el.style.clip) || ''));
  };
  const isReadVisible = (el) => {
    if (!el || el.hidden) return false;
    if (el.getAttribute && el.getAttribute('aria-hidden') === 'true') return false;
    if (el.tagName === 'BUTTON' || el.getAttribute('role') === 'button') return false;
    try {
      if (typeof el.checkVisibility === 'function') {
        if (el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true })) {
          return !srOnlyForRead(el, el.getBoundingClientRect());
        }
        // 唯一的例外:display:contents 被 checkVisibility 判 false 但子树照常渲染。
        if (readStyleOf(el).display === 'contents') return contentsRendered(el);
        return false;
      }
      const style = readStyleOf(el);
      if (style.display === 'none' || style.visibility === 'hidden') return false;
      if (style.display === 'contents') return contentsRendered(el);
      return !srOnlyForRead(el, el.getBoundingClientRect());
    } catch (_) {
      return true;
    }
  };

  /** 正文容器启发式：取文本最多、链接密度最低的候选；太短就回退 body。 */
  const mainRoot = () => {
    const selectors = ['main', '[role="main"]', 'article', '#content', '#main', '.content', '.main',
      '.article', '.post', '.markdown-body', '.rich_media_content', '.article-content', '.post-content'];
    let best = null;
    let bestScore = 0;
    let bestLength = 0;
    let candidateLength = 0;
    let sawCandidate = false;
    let evaluated = 0;
    for (const selector of selectors) {
      for (const el of document.querySelectorAll(selector)) {
        sawCandidate = true;
        // 先用便宜的 textContent 长度做门槛：大页面上候选可能几十个，
        // 每个都跑一遍 plainText（整棵子树遍历）会把 read 拖成秒级。
        if ((el.textContent || '').length < 200) continue;
        if (evaluated >= 12) break;
        evaluated++;
        // 用清洁后的文本评分，避免 script/style 把候选撑大。
        const text = plainText(el);
        candidateLength = Math.max(candidateLength, text.length);
        if (text.length < 200) continue;
        const linkLength = [...el.querySelectorAll('a')].reduce((sum, link) => sum + (link.innerText || '').length, 0);
        const score = text.length - linkLength * 1.5;
        if (score > bestScore) { bestScore = score; best = el; bestLength = text.length; }
      }
    }
    const body = document.body || document.documentElement;
    const bodyLength = body ? plainText(body).length : 0;
    if (!best) {
      return {
        root: body,
        selector: sawCandidate ? 'body(fallback)' : 'body',
        fallback: sawCandidate,
        bodyLength,
        textLength: candidateLength,
      };
    }
    const tooShort = bestLength < 200 || (bodyLength > 2000 && bestLength < bodyLength * 0.05);
    if (tooShort) {
      return { root: body, selector: 'body(fallback)', fallback: true, bodyLength, textLength: bestLength };
    }
    return {
      root: best,
      selector: best.tagName.toLowerCase() + (best.id ? '#' + best.id : ''),
      fallback: false,
      bodyLength,
      textLength: bestLength,
    };
  };

  // ---------------------------------------------------------------- read 渲染
  // md / text 共用一次 DOM 遍历，产出轻量 block/inline 结构；html 走 innerHTML，
  // visible 直接使用浏览器渲染后的 innerText。三种来源共用 read 的选区、预算和游标。
  const READ_BLOCK = /^(ADDRESS|ARTICLE|ASIDE|BLOCKQUOTE|CENTER|DIV|DL|FIELDSET|FIGCAPTION|FIGURE|FOOTER|FORM|H[1-6]|HEADER|HR|LI|MAIN|OL|P|PRE|SECTION|TABLE|TBODY|TD|TFOOT|TH|THEAD|TR|UL)$/;

  const READ_SKIP_TAGS = /^(SCRIPT|STYLE|NOSCRIPT|IFRAME|SVG|CANVAS|VIDEO|AUDIO|TEMPLATE|TEXTAREA|INPUT|SELECT|OPTION)$/;

  /**
   * read 直接跳过的元素（P0-B）：
   * - script/style/svg/canvas… 这些本就不该进正文；
   * - nav/form 永久跳过；
   * - header/footer/aside 在 role=banner/contentinfo/complementary 时跳过；
   * - 其余只在「不在 article/main/[role=main] 内且没有标题后代」时跳过：
   *   article > header > h1 必须保留；像 Guardian 首页那样把内容放进页面级
   *   header 的情况也不能只剩一个标题。`section` 不算正文容器语境。
   */
  const LANDMARK_HIDE_ROLES = /^(banner|contentinfo|complementary)$/;
  const readSkip = (el) => {
    const tag = el.tagName;
    if (!tag || READ_SKIP_TAGS.test(tag)) return true;
    if (tag === 'NAV' || tag === 'FORM') return true;
    if (tag !== 'HEADER' && tag !== 'FOOTER' && tag !== 'ASIDE') return false;
    if (LANDMARK_HIDE_ROLES.test(String(el.getAttribute('role') || '').trim())) return true;
    const inContent = el.closest && el.closest('article,main,[role="main"]');
    const hasHeading = el.querySelector && el.querySelector('h1,h2,h3,h4,h5,h6');
    return !(inContent || hasHeading);
  };

  const hrefOf = (el) => {
    const raw = el.getAttribute('href') || '';
    if (!raw || /^javascript:/i.test(raw) || raw === '#') return '';
    // 同源 iframe 里的相对链接要按 iframe 的 baseURI 解析，不能用顶层 location。
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  // 链接文字里的 ] 会提前闭合 markdown；URL 里的括号/空格会截断目标。
  const escapeMdText = (value) => String(value).replace(/([\\\[\]])/g, '\\$1');
  const escapeMdUrl = (value) => String(value)
    .replace(/ /g, '%20')
    .replace(/\(/g, '%28')
    .replace(/\)/g, '%29')
    .replace(/</g, '%3C')
    .replace(/>/g, '%3E');
  const srcOf = (el) => {
    const raw = el.currentSrc || el.getAttribute('src') || el.getAttribute('data-src') || '';
    if (!raw) return '';
    const base = (el.ownerDocument && el.ownerDocument.baseURI) || location.href;
    try { return new URL(raw, base).href; } catch (_) { return raw; }
  };
  const controlValue = (root) => {
    if (!root) return null;
    if (root.tagName === 'INPUT') return String(root.value == null ? '' : root.value);
    if (root.tagName === 'TEXTAREA') return String(root.value == null ? '' : root.value);
    if (root.tagName === 'SELECT') {
      return [...root.selectedOptions].map(option => option.value).join(',');
    }
    return null;
  };
  const cellText = (cell) => clean(cell.innerText || cell.textContent || '');
  /** 含嵌套表的 td：host 单元格只留自己的文字，嵌套表交给后面的独立表格块。 */
  const cellTextStripped = (cell) => {
    const clone = cell.cloneNode(true);
    for (const inner of clone.querySelectorAll('table')) inner.remove();
    return clean(clone.textContent || '');
  };
  /**
   * `table.rows` 只含本表 thead/tbody/tfoot 里的行；嵌套表不是本表的行——
   * host 单元格剥离嵌套表文本，嵌套表作为独立表格块在当前表之后输出
   * （WeakSet 去重：同一嵌套表不会被相邻层级重复展开）。
   */
  const tableRows = (table, nestedSink) => [...table.rows]
    .map(row => [...row.children]
      .filter(cell => /^(TD|TH)$/.test(cell.tagName))
      .map(cell => {
        if (nestedSink && cell.querySelector('table')) {
          for (const inner of cell.querySelectorAll('table')) {
            if (!nestedSink.seen.has(inner)) { nestedSink.seen.add(inner); nestedSink.queue.push(inner); }
          }
          return cellTextStripped(cell);
        }
        return cellText(cell);
      }))
    .filter(cells => cells.length);
  const mdCell = (value) => String(value).replace(/\|/g, '\\|');

  /**
   * 链接标签只信 innerText（P0-D）：textContent 会把 `<a><script>…</script></a>`
   * 的脚本泄漏进正文。没有文字但有图时用图片 alt；两者都没有就没有链接标签，
   * 上层按空文本链接丢弃，不编造裸 URL。
   */
  const linkLabel = (el) => {
    const text = clean(el.innerText || '');
    if (text) return text;
    const img = el.querySelector ? el.querySelector('img') : null;
    return img ? clean(img.getAttribute('alt') || img.getAttribute('aria-label') || '') : '';
  };

  /** 单个不可再拆的行内节点；A/CODE/STRONG/EM 保持和旧 inline() 一样的原子语义。 */
  const inlineNode = (el) => {
    const tag = el.tagName;
    if (tag === 'A') {
      return {
        t: 'link',
        text: linkLabel(el),
        href: hrefOf(el),
      };
    }
    if (tag === 'IMG') {
      return {
        t: 'img',
        alt: clean(el.getAttribute('alt') || el.getAttribute('aria-label') || el.getAttribute('title') || ''),
        src: srcOf(el),
      };
    }
    if (tag === 'CODE') return { t: 'code', text: clean(el.innerText || '') };
    if (tag === 'STRONG' || tag === 'B') return { t: 'strong', text: clean(el.innerText || '') };
    if (tag === 'EM' || tag === 'I') return { t: 'em', text: clean(el.innerText || '') };
    return { t: 'text', v: '' };
  };

  /** 行内层：文本节点、BR、链接、图片、行内代码/强调；其他元素递归。 */
  const inlineIn = (parent) => {
    const out = [];
    for (const node of parent.childNodes || []) {
      if (node.nodeType === 3) {
        out.push({ t: 'text', v: node.nodeValue || '' });
        continue;
      }
      if (node.nodeType !== 1) continue;
      const el = node;
      const tag = el.tagName;
      if (readSkip(el)) continue;
      if (!isReadVisible(el)) continue;
      if (el.shadowRoot) { out.push(...inlineIn(el.shadowRoot)); continue; }
      if (tag === 'BR') { out.push({ t: 'br' }); continue; }
      if (['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(tag)) {
        out.push(inlineNode(el));
        continue;
      }
      out.push(...inlineIn(el));
    }
    return out;
  };

  const listBlock = (el) => {
    const ordered = el.tagName === 'OL';
    const reversed = ordered && el.hasAttribute('reversed');
    const parsedStart = Number.parseInt(el.getAttribute('start') || '', 10);
    const items = [...el.children]
      .filter(item => item.tagName === 'LI')
      .map(li => {
        const ownValue = Number.parseInt(li.getAttribute('value') || '', 10);
        return {
          inlines: inlineIn(li),
          value: Number.isFinite(ownValue) ? ownValue : null,
        };
      });
    return {
      b: 'list',
      ordered,
      reversed,
      start: Number.isFinite(parsedStart) ? parsedStart : null,
      items,
    };
  };

  /** 块级层：段落、标题、代码块、表格、列表；段落内部用 inlineIn。 */
  const readBlocks = (root) => {
    const blocks = [];
    let pending = [];
    /** hard=1 表示这个块被 <br> 截开、下面还有紧跟的一行（md 里按单换行连接）。 */
    const flush = (viaBr) => {
      if (pending.length) {
        blocks.push({ b: 'p', inlines: pending, hard: !!viaBr });
        pending = [];
      }
    };
    const walk = (parent) => {
      for (const node of parent.childNodes || []) {
        if (node.nodeType === 3) {
          pending.push({ t: 'text', v: node.nodeValue || '' });
          continue;
        }
        if (node.nodeType !== 1) continue;
        const el = node;
        const tag = el.tagName;
        if (readSkip(el)) continue;
        if (!isReadVisible(el)) continue;
        if (el.shadowRoot) { flush(); walk(el.shadowRoot); flush(); continue; }
        if (tag === 'BR') {
          // BR 结束当前物理行：md/text 都按块边界渲染换行，
          // 否则 clean() 会把 md 段落里的 \n 又折叠回空格。
          flush(true);
          continue;
        }
        if (['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(tag)) {
          pending.push(inlineNode(el));
          continue;
        }
        if (!el.innerText && !el.querySelector('img,picture,video')) continue;
        if (/^H[1-6]$/.test(tag)) {
          flush();
          const text = clean(el.innerText);
          if (text) blocks.push({ b: 'h', level: Number(tag[1]), text });
          continue;
        }
        if (tag === 'PRE') {
          flush();
          const code = (el.innerText || '').replace(/^\n+/, '').replace(/\s+$/, '');
          if (code.trim()) blocks.push({ b: 'pre', text: code });
          continue;
        }
        if (tag === 'TABLE') {
          const nested = { seen: new Set(), queue: [] };
          const rows = tableRows(el, nested);
          if (rows.length) {
            flush();
            blocks.push({ b: 'table', rows });
            // 嵌套表按文档顺序跟在后面对齐语义：外层表先写完，嵌套表再展开。
            while (nested.queue.length) {
              const inner = nested.queue.shift();
              const innerRows = tableRows(inner, nested);
              if (innerRows.length) blocks.push({ b: 'table', rows: innerRows });
            }
          }
          continue;
        }
        if (tag === 'UL' || tag === 'OL') {
          flush();
          const list = listBlock(el);
          if (list.items.length) blocks.push(list);
          continue;
        }
        // DL 的 dt/dd 每项独立成段，不再被拼进一个长段落。
        if (tag === 'DT' || tag === 'DD') {
          flush();
          pending.push(...inlineIn(el));
          flush();
          continue;
        }
        if (tag === 'SUMMARY') {
          // <details> 的第一行是标题语义，先单独成段再展开内容。
          flush();
          pending.push(...inlineIn(el));
          flush();
          continue;
        }
        if (tag === 'DETAILS') {
          flush();
          walk(el);
          flush();
          continue;
        }
        // 自定义元素（如 <query-result>、<x-wrap>）通常整段包裹正文，按块展开。
        if (tag.includes && tag.includes('-')) {
          flush();
          walk(el);
          flush();
          continue;
        }
        if (READ_BLOCK.test(tag)) {
          flush();
          walk(el);
          flush();
          continue;
        }
        pending.push(...inlineIn(el));
      }
    };
    if (root && ['A', 'IMG', 'CODE', 'STRONG', 'B', 'EM', 'I'].includes(root.tagName)) {
      return [{ b: 'p', inlines: [inlineNode(root)] }];
    }
    if (root && root.shadowRoot) walk(root.shadowRoot);
    else walk(root);
    flush();
    return blocks;
  };

  const markdownInline = (nodes, separator = '') => nodes.map(node => {
    switch (node.t) {
      case 'text': return node.v;
      case 'br': return '\n';
      case 'link':
        // 空文本链接默认丢弃：不为它编造裸 URL（需要链接结构用 snapshot）。
        if (!node.text) return '';
        return node.href
          ? '[' + escapeMdText(node.text) + '](' + escapeMdUrl(node.href) + ') '
          : escapeMdText(node.text);
      case 'img':
        if (!node.src) return node.alt ? escapeMdText(node.alt) + ' ' : '';
        return '![' + escapeMdText(node.alt) + '](' + escapeMdUrl(node.src) + ') ';
      case 'code': return node.text ? '`' + node.text + '`' : '';
      case 'strong': return node.text ? '**' + node.text + '**' : '';
      case 'em': return node.text ? '*' + node.text + '*' : '';
      default: return '';
    }
  }).join(separator);

  const markdown = (root) => {
    // 直接 read 一个控件时，md 也应返回当前值；否则 --selector "#text" 会是空。
    const control = controlValue(root);
    if (control !== null) return control;
    const out = [];
    for (const block of readBlocks(root)) {
      if (block.b === 'p') {
        const text = clean(markdownInline(block.inlines));
        if (text) out.push({ p: true, hard: !!block.hard, text });
      } else if (block.b === 'h') {
        out.push({ text: '#'.repeat(block.level) + ' ' + block.text });
      } else if (block.b === 'pre') {
        out.push({ text: '```\n' + block.text + '\n```' });
      } else if (block.b === 'table') {
        out.push({ text: markdownTable(block.rows) });
      } else if (block.b === 'list') {
        out.push({ text: markdownList(block) });
      }
    }
    // 被 <br> 截开的段落段之间只换一行，其他块都是空行分段；
    // 跨块型（p→h/table/…）不继承 hard。
    let md = '';
    for (let index = 0; index < out.length; index++) {
      const segment = out[index];
      if (!index) { md = segment.text; continue; }
      const previous = out[index - 1];
      md += (previous.p && segment.p && previous.hard ? '\n' : '\n\n') + segment.text;
    }
    return md;
  };

  const markdownTable = (rows) => {
    const width = Math.max(...rows.map(row => row.length));
    const pad = (cells) => [...cells, ...Array(Math.max(0, width - cells.length)).fill('')];
    const head = pad(rows[0]).map(mdCell);
    const lines = [
      '| ' + head.join(' | ') + ' |',
      '| ' + head.map(() => '---').join(' | ') + ' |',
    ];
    for (const row of rows.slice(1)) {
      lines.push('| ' + pad(row).map(mdCell).join(' | ') + ' |');
    }
    return lines.join('\n');
  };

  const markdownList = (block) => {
    let number = block.start != null
      ? block.start
      : (block.reversed ? block.items.length : 1);
    const lines = [];
    for (const item of block.items) {
      if (block.ordered && item.value != null) number = item.value;
      // 列表项沿用旧行为：每个直接行内节点之间补一个空格。
      const text = clean(markdownInline(item.inlines, ' '));
      if (text) lines.push((block.ordered ? number + '. ' : '- ') + text);
      if (block.ordered) number += block.reversed ? -1 : 1;
    }
    return lines.join('\n');
  };

  const textInline = (nodes) => {
    const lines = [];
    let parts = [];
    const flush = () => {
      const text = clean(parts.join(' '));
      if (text) lines.push(text);
      parts = [];
    };
    for (const node of nodes) {
      if (node.t === 'br') { flush(); continue; }
      const value = node.t === 'text' ? node.v
        : node.t === 'link' ? node.text
        : node.t === 'img' ? node.alt
        : (node.text || '');
      if (value) parts.push(value);
    }
    flush();
    return lines;
  };

  const plainText = (root) => {
    // `read --selector "#input"` 应该给出控件的当前值，而不是空字符串。
    const control = controlValue(root);
    if (control !== null) return control;
    const lines = [];
    for (const block of readBlocks(root)) {
      if (block.b === 'p') {
        lines.push(...textInline(block.inlines));
      } else if (block.b === 'h') {
        const text = clean(block.text);
        if (text) lines.push(text);
      } else if (block.b === 'pre') {
        const text = clean(block.text);
        if (text) lines.push(text);
      } else if (block.b === 'table') {
        for (const row of block.rows) {
          for (const cell of row) {
            const text = clean(cell);
            if (text) lines.push(text);
          }
        }
      } else if (block.b === 'list') {
        for (const item of block.items) lines.push(...textInline(item.inlines));
      }
    }
    return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim();
  };

  /** visible：浏览器渲染后的文本；不做正文清洗，默认仍受 read 的 main/selector 选区约束。 */
  const visibleText = (root) => {
    const control = controlValue(root);
    if (control !== null) return control;
    if (!root) return '';
    try {
      return String(root.innerText == null ? '' : root.innerText).trim();
    } catch (_) {
      return '';
    }
  };

  /** iframe 内元素的 rect 是相对 iframe viewport 的；标注要换算到顶层截图坐标。 */
  const frameOffset = (frameEl) => {
    if (!frameEl) return { x: 0, y: 0 };
    const rect = frameEl.getBoundingClientRect();
    const parentWin = frameEl.ownerDocument && frameEl.ownerDocument.defaultView;
    const parentFrame = parentWin && parentWin.frameElement ? parentWin.frameElement : null;
    const base = frameOffset(parentFrame);
    return {
      x: base.x + rect.left + (Number(frameEl.clientLeft) || 0),
      y: base.y + rect.top + (Number(frameEl.clientTop) || 0),
    };
  };

  /** 当前视口内的 ref 标记（用于截图标注，set-of-marks）。 */
  const marks = () => {
    // 同一元素在旧 document epoch 下可能还留着一个 ref；只保留当前文档的标记，
    // 并按元素去重（后分配的 ref 胜出），否则截图会出现同一元素两个编号。
    const byElement = new Map();
    for (const { ref, el } of refEntries(0)) {
      if (!refIsCurrent(ref)) continue;
      const rect = el.getBoundingClientRect();
      if (rect.width < 2 || rect.height < 2) continue;
      const win = (el.ownerDocument && el.ownerDocument.defaultView) || window;
      const offset = frameOffset(win.frameElement || null);
      // 标注坐标必须落在截图范围内；负坐标/视口外元素会画到图外或误导 AI。
      const x = Math.max(0, Math.round(rect.left + offset.x));
      const y = Math.max(0, Math.round(rect.top + offset.y));
      const right = Math.min(window.innerWidth, Math.round(rect.right + offset.x));
      const bottom = Math.min(window.innerHeight, Math.round(rect.bottom + offset.y));
      const w = right - x;
      const h = bottom - y;
      if (w < 2 || h < 2) continue;
      byElement.set(el, {
        ref,
        role: roleOf(el),
        name: nameOf(el).slice(0, 30),
        x,
        y,
        w,
        h,
      });
    }
    return [...byElement.values()];
  };

  /**
   * 写值。options.mode=replace（fill/select）覆盖；insert（type）按当前 caret/selection 插入。
   *
   * 所有失败路径都会恢复原值：命令返回 error 时页面不能被改坏，
   * 否则 AI 继续提交表单就会提交空值。
   */
  const setValue = (el, value, options) => {
    if (!el) return { ok: false, code: 'target_not_found', message: 'target not found' };
    if (el.disabled) return { ok: false, code: 'element_disabled', message: 'target is disabled' };
    const tag = el.tagName;
    if (tag === 'INPUT') {
      const type = String(el.type || 'text').toLowerCase();
      if (type === 'file' || type === 'checkbox' || type === 'radio') {
        return { ok: false, code: 'unsupported_control', message: `cannot type into input[type=${type}]` };
      }
    }
    const doc = el.ownerDocument || document;
    const win = doc.defaultView || window;
    const text = String(value == null ? '' : value);
    const mode = options && options.mode === 'insert' ? 'insert' : 'replace';

    if (el.isContentEditable) {
      // type "" 是合法 no-op；不要让 execCommand('insertText','') 的 false 误报为失败。
      if (mode === 'insert' && text === '') return { ok: true, selfDispatched: true };
      // X/Slack/Discord 等富文本编辑器依赖原生 input 事件；直接写 textContent
      // 不会更新它们内部状态（例如 X 的“发帖”按钮仍然 disabled）。
      const beforeText = String(el.textContent || '');
      const beforeHtml = String(el.innerHTML || '');
      const restore = () => {
        try { el.innerHTML = beforeHtml; } catch (_) { el.textContent = beforeText; }
      };
      const insert = () => {
        el.focus();
        try {
          const selection = win.getSelection();
          if (selection) {
            if (mode === 'replace') {
              const range = doc.createRange();
              range.selectNodeContents(el);
              selection.removeAllRanges();
              selection.addRange(range);
            } else {
              const anchor = selection.anchorNode;
              const inside = !!anchor && (el === anchor || el.contains(anchor));
              if (!selection.rangeCount || !inside) {
                // 没有 caret 时插到末尾；有选区时必须尊重它，不能清空重写。
                const range = doc.createRange();
                range.selectNodeContents(el);
                range.collapse(false);
                selection.removeAllRanges();
                selection.addRange(range);
              }
            }
          }
          return doc.execCommand('insertText', false, text);
        } catch (_) {
          return false;
        }
      };
      let inserted = insert();
      if (mode === 'replace' && String(el.textContent || '') !== text) {
        // 极少数编辑器在首次聚焦/重载后会把同一次 insertText 应用两次；复核并修一次。
        inserted = insert();
      }
      if (mode === 'replace' && (!inserted || String(el.textContent || '') !== text)) {
        el.textContent = text;
        try {
          el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
        } catch (_) { /* ignore */ }
      } else if (!inserted) {
        restore();
        return { ok: false, code: 'input_prevented', message: 'the editor rejected insertText' };
      }
      // execCommand 已经触发原生 input；外层不要再补发，避免某些编辑器重复应用。
      return { ok: true, selfDispatched: true };
    }

    let proto = null;
    if (tag === 'INPUT' && win.HTMLInputElement) proto = win.HTMLInputElement.prototype;
    else if (tag === 'TEXTAREA' && win.HTMLTextAreaElement) proto = win.HTMLTextAreaElement.prototype;
    else if (tag === 'SELECT' && win.HTMLSelectElement) proto = win.HTMLSelectElement.prototype;
    if (!proto) return { ok: false, code: 'not_writable', message: `cannot write to ${String(tag).toLowerCase()}` };
    if (el.readOnly && tag !== 'SELECT') return { ok: false, code: 'element_readonly', message: 'target is read-only' };
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (!descriptor || typeof descriptor.set !== 'function') {
      return { ok: false, code: 'not_writable', message: 'value setter is unavailable' };
    }

    if (tag === 'SELECT') {
      const previous = [...el.options].map(option => option.selected);
      const restore = () => {
        [...el.options].forEach((option, index) => { option.selected = !!previous[index]; });
      };
      const requested = el.multiple
        ? text.split(',').map(part => part.trim()).filter(Boolean)
        : [text];
      const unique = [...new Set(requested)];
      const missing = unique.filter(want => ![...el.options].some(option => option.value === want));
      if (missing.length) {
        restore();
        return {
          ok: false, code: 'value_not_available',
          message: `select has no option with value: ${missing.join(',')}`,
        };
      }
      try {
        if (el.multiple) {
          // <select multiple> 支持逗号分隔的多个 value；真实用户是 Ctrl+click 多选。
          for (const option of el.options) option.selected = unique.includes(option.value);
          const done = unique.every(want => [...el.options].some(option => option.selected && option.value === want));
          if (!done) { restore(); return { ok: false, code: 'value_not_available', message: 'select did not accept all values' }; }
        } else {
          descriptor.set.call(el, text);
          if (String(el.value) !== text) {
            restore();
            return { ok: false, code: 'value_not_available', message: 'select has no matching option' };
          }
        }
      } catch (error) {
        restore();
        return { ok: false, code: 'value_rejected', message: clean(error && error.message || error) };
      }
      return { ok: true };
    }

    const previousValue = String(el.value == null ? '' : el.value);
    const previousStart = el.selectionStart;
    const previousEnd = el.selectionEnd;
    const restore = () => {
      try {
        descriptor.set.call(el, previousValue);
        if (typeof previousStart === 'number' && typeof previousEnd === 'number') {
          el.setSelectionRange(previousStart, previousEnd);
        }
      } catch (_) { /* 控件可能不支持 selection */ }
    };

    let nextValue;
    let caret = null;
    if (mode === 'insert') {
      const current = previousValue;
      const start = typeof previousStart === 'number'
        ? Math.min(previousStart, current.length) : current.length;
      const end = typeof previousEnd === 'number'
        ? Math.min(Math.max(previousEnd, start), current.length) : start;
      nextValue = current.slice(0, start) + text + current.slice(end);
      caret = start + text.length;
    } else {
      nextValue = text;
      caret = nextValue.length;
    }

    // 输入掩码（IMask / 金额 / 手机号格式化）依赖 beforeinput；
    // 页面主动 cancel 时说明它自己接管了输入，这时不强写，避免和页面状态打架。
    try {
      const before = new InputEvent('beforeinput', {
        bubbles: true, cancelable: true, inputType: 'insertText', data: text,
      });
      if (!el.dispatchEvent(before)) {
        return {
          ok: false,
          code: 'input_prevented',
          message: 'the page cancelled beforeinput for this control',
        };
      }
    } catch (_) { /* 老浏览器/特殊控件忽略 */ }

    try {
      descriptor.set.call(el, nextValue);
    } catch (error) {
      restore();
      return { ok: false, code: 'value_rejected', message: clean(error && error.message || error) };
    }
    // number/date/range 等控件会清洗值（例如 "abc" -> ""）；不能把 setter 没抛错当成写入成功。
    if (String(el.value) !== nextValue) {
      restore();
      return {
        ok: false,
        code: 'value_rejected',
        message: 'the control normalized the value (type/maxlength/range?); the previous value was restored',
      };
    }
    if (caret != null) {
      try { el.setSelectionRange(caret, caret); } catch (_) { /* number/date 等不支持 selection */ }
    }
    return { ok: true };
  };

  // 升级 helper 时把旧版本遗留的 DOM 属性清掉（新 ref 只在内存表里）。
  try {
    for (const { root } of collectRoots()) {
      for (const el of root.querySelectorAll('[data-ema2-ref]')) el.removeAttribute('data-ema2-ref');
    }
  } catch (_) { /* ignore */ }

  window.__ema2_helpers = {
    snapshot, markdown, plainText, visibleText, mainRoot, find, refOf, refEntries, clean, candidates,
    collectRoots, roleOf, nameOf, marks, docEpoch, setDocHandle, parseRef, refIsCurrent,
    setValue, isActionableRole, absoluteHref,
  };
  window.__ema2_helpers_version = HELPERS_VERSION;
  return { installed: true };
}
