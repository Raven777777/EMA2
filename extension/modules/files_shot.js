/**
 * files upload 与 shot 截图/annotate。
 */

// ---------------------------------------------------------------- files / js / observe / shot

let platformOs = null;
async function currentOs() {
  if (platformOs) return platformOs;
  try {
    const info = await chrome.runtime.getPlatformInfo();
    platformOs = info.os;
  } catch (_) {
    platformOs = 'unknown';
  }
  return platformOs;
}

async function filesTool(args) {
  const tab = await resolveTab(args);
  const action = args.action;
  if (action === 'upload') {
    const payload = Array.isArray(args.files) ? args.files : [];
    if (!payload.length) {
      fail('missing_files', 'upload requires file bytes',
        'use the ema2 CLI: `ema2 files upload C:/dir/file --ref ...`');
    }
    if (!args.ref && !args.selector && !args.target) {
      fail('missing_target', 'upload requires ref, selector or target', 'call snapshot and pass the file input ref');
    }
    await ensureHelpers(tab.id);
    if (args.selector) await requireValidSelector(tab.id, args.selector);
    let targetRef = args.ref || null;
    if (targetRef) {
      // 短 ref（e12）先补成 e12@dX；完整 ref 必须通过当前文档校验。
      const current = await runInTab(tab.id, (ref) => {
        const helpers = window.__ema2_helpers;
        if (!helpers || !helpers.refIsCurrent(ref)) return null;
        const parsed = helpers.parseRef(ref);
        return {
          ref: parsed && !parsed.epoch ? `e${parsed.seq}@${helpers.docEpoch()}` : ref,
        };
      }, [targetRef]);
      if (!current || !current.ref) {
        fail('stale_ref', `ref ${targetRef} is stale or belongs to another document`, 'call snapshot again');
      }
      targetRef = current.ref;
    }
    if (payload.length) {
      if (payload.length > 20) fail('too_many_files', 'at most 20 files per upload', 'split into multiple calls');
      // 字节由 CLI 读好后随请求下发；页面侧用 DataTransfer 赋给 input.files，
      // 不依赖 debugger（别的扩展占住 tab 时 upload 仍然可用）。
      const result = await runInTab(tab.id, (ref, selector, target, files) => {
        const helpers = window.__ema2_helpers;
        if (!helpers) return { ok: false, code: 'upload_failed', message: 'page helpers are not installed' };
        const el = helpers.find(ref, selector, target);
        if (!el) {
          return {
            ok: false,
            code: ref && !helpers.refIsCurrent(ref) ? 'stale_ref' : 'target_not_found',
            message: 'file input not found',
          };
        }
        if (el.tagName !== 'INPUT' || String(el.type || '').toLowerCase() !== 'file') {
          return { ok: false, code: 'unsupported_control', message: 'target is not input[type=file]' };
        }
        try {
          const transfer = new DataTransfer();
          for (const item of files) {
            const binary = atob(String(item.data || ''));
            const bytes = new Uint8Array(binary.length);
            for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
            transfer.items.add(new File([bytes], String(item.name || 'file'), {
              type: String(item.type || 'application/octet-stream'),
            }));
          }
          el.files = transfer.files;
          el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true, names: [...el.files].map(file => file.name) };
        } catch (error) {
          return { ok: false, code: 'upload_failed', message: String((error && error.message) || error) };
        }
      }, [targetRef, args.selector || null, args.target || null, payload.map(item => ({
        name: String(item.name || 'file'),
        type: String(item.type || 'application/octet-stream'),
        data: String(item.data || ''),
      }))], undefined, 1);
      if (!result || !result.ok) {
        fail((result && result.code) || 'upload_failed',
          (result && result.message) || 'cannot assign files',
          'pass the file input ref from snapshot, or use --selector "input[type=file]"');
      }
      return 'uploaded=' + result.names.length + '\nfiles=' + result.names.join(',');
    }
  }
  if (action === 'download-dir') {
    // Edge 不允许扩展修改下载目录；在触碰浏览器之前就给出稳定错误。
    fail('download_dir_unsupported',
      'Edge does not allow an extension to change the download directory',
      'set it manually in edge://settings/downloads; files land in the default Downloads folder',
      ['deprecated=1 set the directory in edge://settings/downloads']);
  }
  fail('invalid_action', `unknown files action: ${action}`, 'use upload|download-dir');
}

function resultText(value) {
  if (value && Array.isArray(value.content)) {
    return value.content.filter(item => item.type === 'text').map(item => item.text).join('\n');
  }
  return String(value == null ? '' : value);
}

/** 标注结果的 base64 上限：再大就不适合经 relay 回传，应落盘。 */
const ANNOTATED_MAX_BASE64 = 12 * 1024 * 1024;

async function annotateScreenshot(dataUrl, marks, dpr, format) {
  // 这条路径同时持有原图 base64、解码后的 bitmap、OffscreenCanvas 和输出 blob，
  // 每一份都是整图级别的大对象：用完必须立刻释放，否则 SW 峰值能到数百 MB。
  const blob = await (await fetch(dataUrl)).blob();
  let bitmap = null;
  let canvas = null;
  try {
    bitmap = await createImageBitmap(blob);
    canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext('2d');
    ctx.drawImage(bitmap, 0, 0);
    ctx.scale(dpr || 1, dpr || 1);
    ctx.font = '600 12px sans-serif';
    ctx.textBaseline = 'top';
    for (const mark of marks) {
      ctx.strokeStyle = '#ff2d55';
      ctx.lineWidth = 2;
      ctx.strokeRect(mark.x, mark.y, mark.w, mark.h);
      const label = String(mark.ref).replace(/^e(\d+)@.*$/, '$1').replace(/^e/, '');
      const width = ctx.measureText(label).width + 8;
      const top = Math.max(0, mark.y - 16);
      ctx.fillStyle = '#ff2d55';
      ctx.fillRect(mark.x, top, width, 16);
      ctx.fillStyle = '#ffffff';
      ctx.fillText(label, mark.x + 4, top + 2);
    }
    const outBlob = await canvas.convertToBlob({ type: format === 'jpeg' ? 'image/jpeg' : 'image/png' });
    const bytes = new Uint8Array(await outBlob.arrayBuffer());
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return { base64: btoa(binary), mime: 'image/' + (format === 'jpeg' ? 'jpeg' : 'png') };
  } finally {
    // 每个 createImageBitmap 都必须显式 close（否则解码缓冲要等 GC）；
    // 中间 canvas 置 0 让引擎立刻回收后备缓冲区。
    if (bitmap) bitmap.close();
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
  }
}

async function activeTabInWindow(windowId) {
  const [active] = await chrome.tabs.query({ windowId, active: true });
  return active || null;
}

async function requireActiveShotTab(tab) {
  const active = await activeTabInWindow(tab.windowId);
  if (!active || active.id !== tab.id) {
    fail('tab_not_active',
      `tab ${tab.id} is not the active tab in window ${tab.windowId}`,
      'shot --tab auto-activates the tab; if it still fails, call tabs activate and keep Edge in the foreground');
  }
}

async function shotTool(args) {
  const tab = await resolveTab(args);
  const format = args.format === 'jpeg' ? 'jpeg' : 'png';
  // `--tab` 语义：自动把目标标签/窗口提到前台，而不是让用户猜为什么永远失败。
  let activated = false;
  const active = await activeTabInWindow(tab.windowId);
  if (!active || active.id !== tab.id) {
    try {
      await chrome.tabs.update(tab.id, { active: true });
      await chrome.windows.update(tab.windowId, { focused: true });
      activated = true;
      await sleep(120);
    } catch (_) { /* requireActiveShotTab 会给出稳定错误 */ }
  }
  await requireActiveShotTab(tab);
  let marks = [];
  let marksText = '';
  let docBefore = '';
  if (args.annotate) {
    await ensureHelpers(tab.id);
    docBefore = await runInTab(tab.id, () => window.__ema2_helpers.docEpoch(), []);
    marks = (await runInTab(tab.id, () => {
      const helpers = window.__ema2_helpers;
      helpers.snapshot('interactive', 20000, 0);
      return helpers.marks();
    }, [])) || [];
    marksText = marks.map(mark => `[${mark.ref}] ${mark.role} "${mark.name}" @${mark.x},${mark.y} ${mark.w}x${mark.h}`).join('\n');
  }
  let dataUrl;
  try {
    dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format });
  } catch (error) {
    fail('capture_failed', `captureVisibleTab failed: ${error.message}`, 'make the browser window visible/focused and retry');
  }
  // 截图过程中用户/页面可能切换活动标签页：目标变了就明确失败，不把别的页标成目标页。
  await requireActiveShotTab(tab);
  if (args.annotate) {
    const docAfter = await runInTab(tab.id, () => window.__ema2_helpers.docEpoch(), []).catch(() => null);
    if (docAfter !== docBefore) {
      fail('tab_changed', 'the target document changed while taking the screenshot', 'retry after the page stops navigating');
    }
  }
  const match = new RegExp('^data:image/(' + format + ');base64,(.+)$', 's').exec(dataUrl || '');
  if (!match) fail('capture_empty', 'screenshot payload was empty', 'retry');
  if (!args.annotate) {
    const mime = 'image/' + (format === 'png' ? 'png' : 'jpeg');
    const content = [];
    if (activated) content.push({ type: 'text', text: `activated=1 tab=${tab.id}` });
    content.push({ type: 'image', data: match[2], mimeType: mime });
    return { content };
  }
  const dpr = await runInTab(tab.id, () => window.devicePixelRatio || 1, []).catch(() => 1);
  const annotated = await annotateScreenshot(dataUrl, marks, dpr, format);
  // 标注图要整份经 relay 回传，base64 过大时会长时间占住 SW/daemon 内存；
  // 超过上限直接报错，让调用方改用 --out 落盘（path= 输出契约不变）。
  if (annotated.base64.length > ANNOTATED_MAX_BASE64) {
    fail('payload_too_large',
      `annotated screenshot is ${annotated.base64.length} bytes of base64 (limit ${ANNOTATED_MAX_BASE64})`,
      'use `ema2 shot --annotate --out <file>` to write the image to disk, or capture a smaller window');
  }
  const content = [];
  const prefix = activated ? `activated=1 tab=${tab.id}\n` : '';
  if (marks.length) content.push({ type: 'text', text: prefix + marksText + '\nmarks=' + marks.length });
  else if (prefix) content.push({ type: 'text', text: prefix.trimEnd() });
  content.push({ type: 'image', data: annotated.base64, mimeType: annotated.mime });
  return { content };
}

/** 按完整记录分页：next 只推进到已经完整交付的记录。 */
