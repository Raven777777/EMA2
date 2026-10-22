/**
 * debugger 队列、期限、js 执行与降级。
 */

function withDeadline(promise, ms, code, message, hint) {
  if (!Number.isFinite(ms) || ms <= 0) {
    return Promise.reject(new ToolError(code, message, hint || 'retry when the queue is free'));
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new ToolError(code, message, hint || 'terminate the long-running JS or retry later')),
      ms,
    );
    Promise.resolve(promise).then(
      value => { clearTimeout(timer); resolve(value); },
      error => { clearTimeout(timer); reject(error); },
    );
  });
}

/**
 * 扩展侧 RPC watchdog。必须大于扩展自己正常的等待上限，否则会在
 * `wait time 300s` / 20 步 act / batch 还没结束时先被这里掐断。
 * 数值与 rust/src/tools.rs 的 timeout_for 保持同一数量级即可；
 * daemon 侧仍然是最终兜底。
 */
function toolWatchdogMs(name, args) {
  const value = args && typeof args === 'object' ? args : {};
  const numberFrom = (source, key, fallback) => {
    const number = Number(source && source[key]);
    return Number.isFinite(number) ? Math.max(0, number) : fallback;
  };
  const seconds = (key, fallback) => numberFrom(value, key, fallback);
  switch (name) {
    case 'wait':
      return Math.round((value.await ? seconds('timeout', 3) : seconds('time', seconds('timeout', CONTRACT.timeouts.waitDefaultSecs))) * 1000)
        + CONTRACT.timeouts.waitExtensionGraceSecs * 1000;
    case 'open':
      return Math.round(seconds('waitTimeout', CONTRACT.timeouts.openDefaultSecs) * 1000)
        + CONTRACT.timeouts.openExtensionGraceSecs * 1000;
    case 'js':
      return Math.round(seconds('timeout', CONTRACT.timeouts.jsDefaultSecs) * 1000)
        + CONTRACT.timeouts.jsExtensionGraceSecs * 1000;
    case 'act': {
      const steps = Array.isArray(value.steps) ? value.steps : [value];
      let extra = 0;
      for (const step of steps) {
        if (!step || typeof step !== 'object') continue;
        if (step.action === 'await') extra += numberFrom(step, 'timeout', 3) * 1000;
        if (step.action === 'waitFor') extra += numberFrom(step, 'timeout', 10) * 1000;
      }
      if (value.waitFor && typeof value.waitFor === 'object') {
        const waitTimeout = Number(value.waitFor.timeout);
        extra += (Number.isFinite(waitTimeout) ? Math.max(0, waitTimeout) : 10) * 1000;
      }
      return CONTRACT.timeouts.toolWatchdogMs
        + steps.length * CONTRACT.timeouts.actStepsSettleSecs * 1000 + extra;
    }
    case 'batch':
      return CONTRACT.timeouts.batchExtensionWatchdogMs;
    default:
      return TOOL_WATCHDOG_MS;
  }
}

async function abortDebuggerEvaluation(tabId) {
  try { await withDeadline(chrome.debugger.sendCommand({ tabId }, 'Runtime.terminateExecution'), 2000, 'debugger_timeout', 'terminateExecution did not return'); }
  catch (_) { /* older builds may not support it */ }
}

/// attach({tabId}) 在 tab 内存在其他扩展的页面目标时会被拒；直接按 targetId 挂 page target。
async function attachByTargetId(tabId) {
  try {
    const targets = await chrome.debugger.getTargets();
    const page = targets.find(target => target.tabId === tabId && target.type === 'page' && !target.attached);
    if (!page) return null;
    const target = { targetId: page.id };
    await withDeadline(chrome.debugger.attach(target, '1.3'), 10000, 'debugger_attach_timeout', `cannot attach debugger to target ${page.id}`);
    return target;
  } catch (_) {
    return null;
  }
}

/// attach 失败时列出该 tab 的全部 debugger target，避免把"另一个扩展的页面目标"误报成 DevTools。
async function describeAttachFailure(tabId) {
  const parts = [];
  try {
    const tab = await chrome.tabs.get(tabId);
    parts.push(`tab=${shortText(tab.url || '', 80)}`);
  } catch (_) {
    parts.push('tab=missing');
  }
  try {
    const targets = await chrome.debugger.getTargets();
    const mine = targets.filter(target => target.tabId === tabId);
    if (!mine.length) parts.push('targets=0');
    else parts.push('targets=' + mine.map(target => `${target.type}:${shortText(target.url || '', 70)}`).join(' '));
  } catch (error) {
    parts.push(`targets=? ${oneLine(error && error.message || error).slice(0, 120)}`);
  }
  return parts.join(' ');
}

function withDebugger(tabId, run, options = {}) {
  const taskTimeout = options.timeoutMs || DEBUGGER_TASK_TIMEOUT_MS;
  const queueWait = options.queueWaitMs || DEBUGGER_QUEUE_TIMEOUT_MS;
  const queuedAt = Date.now();
  const task = debuggerQueue.then(async () => {
    const waited = Date.now() - queuedAt;
    if (waited >= queueWait) {
      throw new ToolError('debugger_busy', `debugger queue wait exceeded ${Math.round(queueWait / 1000)}s`, 'close other long-running js/files calls and retry');
    }
    const remaining = Math.min(taskTimeout, queueWait - waited);
    let attachError = null;
    let target = { tabId };
    // 扩展重载/DevTools 关闭的瞬间会偶发 attach 失败；非超时错误重试一次。
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        await withDeadline(chrome.debugger.attach(target, '1.3'), Math.min(10000, remaining), 'debugger_attach_timeout', `cannot attach debugger to tab ${tabId}`);
        attachError = null;
        break;
      } catch (error) {
        attachError = error;
        if (error instanceof ToolError && error.code === 'debugger_attach_timeout') break;
        if (attempt === 0) {
          await sleep(200);
          const byTarget = await attachByTargetId(tabId);
          if (byTarget) {
            target = byTarget;
            attachError = null;
            break;
          }
        }
      }
    }
    if (attachError) {
      if (attachError instanceof ToolError && attachError.code === 'debugger_attach_timeout') throw attachError;
      const detail = await describeAttachFailure(tabId);
      throw new ToolError('debugger_attach_failed',
        `cannot attach debugger to tab ${tabId}: ${attachError.message || attachError} | ${detail}`,
        'the tab has no attachable page target (DevTools open, or another extension page owns the tab); reload the tab and retry');
    }
    let work;
    try {
      work = Promise.resolve().then(run);
      return await withDeadline(work, remaining, 'debugger_timeout', `debugger task exceeded ${Math.round(remaining / 1000)}s`);
    } catch (error) {
      if (error instanceof ToolError && error.code === 'debugger_timeout') {
        // 先终止本次 Runtime.evaluate，再 detach；绝不能只给调用者报错却把任务留在全局队列里。
        await abortDebuggerEvaluation(tabId);
        if (work) work.catch(() => {});
      }
      throw error;
    } finally {
      try { await withDeadline(chrome.debugger.detach(target), 2000, 'debugger_detach_timeout', 'detach timed out'); }
      catch (_) { /* ignore */ }
    }
  });
  debuggerQueue = task.then(() => {}, () => {});
  return task;
}

/** 只编译、不执行；用来区分“编译期语法错误”和“运行时 SyntaxError”。 */
async function compileProbe(tabId, expression) {
  try {
    const response = await chrome.debugger.sendCommand({ tabId }, 'Runtime.compileScript', {
      expression,
      sourceURL: 'ema2://probe',
      persistScript: false,
    });
    if (response && response.exceptionDetails) {
      const details = response.exceptionDetails;
      // description 才是带 `SyntaxError: Unexpected token ...` 的完整文本；
      // text 往往只有 "Uncaught"。
      return {
        ok: false,
        message: (details.exception && details.exception.description)
          || details.text || 'syntax error',
      };
    }
    return { ok: true };
  } catch (error) {
    return { ok: false, message: String((error && error.message) || error) };
  }
}

function jsErrorHint(syntax) {
  if (syntax) {
    return 'syntax error: send an expression (`document.title`) or a statement body with optional top-level return/await; unbalanced brackets or a stray `{ }` block are the usual causes';
  }
  return 'runtime error: the code parsed but threw; read the page first (snapshot/read) or guard with try/catch';
}

function evaluateInTab(tabId, expression) {
  return chrome.debugger.sendCommand({ tabId }, 'Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
}

function jsTimeoutMs(args) {
  const seconds = Number(args && args.timeout);
  const value = Number.isFinite(seconds) ? seconds * 1000 : JS_DEFAULT_TIMEOUT_MS;
  return Math.max(100, Math.min(JS_MAX_TIMEOUT_MS, Math.round(value)));
}

/** js 的统一输出：截断时给出明确标记；结构化值用可解析的 JSON 分片信封。 */
function formatJsOutput(value, budget, cursor, extraLine) {
  const isString = typeof value === 'string';
  const raw = value === undefined ? 'undefined' : stringifyValue(value);
  const slice = sliceText(raw, budget, cursor);
  const lines = [];
  if (slice.next != null && !isString && value !== undefined) {
    // 直接把 JSON 从中间切断会得到非法 JSON；把切片包进合法信封，
    // 并保留 next 让调用方用 cursor 继续。
    lines.push(JSON.stringify({
      ema2_truncated: true,
      total: slice.total,
      next: slice.next,
      chunk: slice.text,
    }));
    lines.push(footer(slice.total, slice.text.length, slice.next));
    lines.push('truncated=1');
    lines.push('note=structured value is a chunk envelope; pass cursor=<next> to continue');
  } else {
    lines.push(slice.text);
    if (slice.next != null) {
      lines.push(footer(slice.total, slice.text.length, slice.next));
      lines.push('truncated=1');
    }
  }
  if (extraLine) lines.push(extraLine);
  return lines.join('\n');
}

async function jsTool(args) {
  const tab = await resolveTab(args);
  if (isInternalUrl(tab.url)) fail('internal_page', 'browser internal pages cannot run JS', 'open an http(s) page');
  // js 里也可能调用 alert/print；page_guard 已由 withCommandGuard/guardTargetForTool
  // 在动作前安装，这里不能重复注入。
  // js 里的 console.* 也要能被后续 observe console 看到；open 超时跳过 hook 时尤其重要。
  try { await ensureConsoleHook(tab.id); } catch (_) { /* non-http page */ }
  try {
    return await jsViaDebugger(tab.id, args);
  } catch (error) {
    if (!(error instanceof ToolError)) throw error;
    // 无限循环/永不 resolve 的 Promise：withDebugger 已经 terminateExecution，
    // 把内部的 debugger_timeout 映射成对 AI 更有意义的 script_timeout。
    if (error.code === 'debugger_timeout') {
      fail('script_timeout',
        `JS did not finish within ${Math.round(jsTimeoutMs(args) / 1000)}s; execution was terminated`,
        'simplify the expression, add --timeout, or use read/snapshot/wait instead of a long-running loop');
    }
    // 别的扩展占住 tab 的页面目标时 attach 会持续失败；js 是原语，不能跟着一起消失。
    if (error.code !== 'debugger_attach_failed' && error.code !== 'debugger_attach_timeout') throw error;
    return jsViaScripting(tab.id, args, error);
  }
}

async function jsViaDebugger(tabId, args) {
  return withDebugger(tabId, async () => {
    // Runtime.compileScript 需要先启用 Runtime domain（evaluate 本身不需要）。
    await chrome.debugger.sendCommand({ tabId }, 'Runtime.enable');
    // 先用 Runtime.compileScript 只做编译探测：表达式能编译就只执行一次；
    // 运行时抛出的 SyntaxError 不是编译错误，绝不能重跑（否则副作用执行两次）。
    // compileScript 对标的是“脚本”，顶层 return 可能被接受；套一层括号才能确认是表达式。
    const expressionSource = String(args.code).trim().replace(/;+\s*$/, '');
    const asExpression = await compileProbe(tabId, `(${expressionSource}
)`);
    let result;
    let ranAsBody = false;
    if (asExpression.ok) {
      result = await evaluateInTab(tabId, args.code);
    } else {
      const wrapped = `(async () => {\n${args.code}\n})()`;
      const asBody = await compileProbe(tabId, wrapped);
      if (!asBody.ok) {
        fail('js_exception', oneLine(asBody.message || 'syntax error').slice(0, 300), jsErrorHint(true));
      }
      ranAsBody = true;
      result = await evaluateInTab(tabId, wrapped);
    }
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      const message = details.exception && details.exception.description || details.text || 'JS error';
      fail('js_exception', oneLine(message).slice(0, 300), jsErrorHint(false));
    }
    let value = result.result ? result.result.value : undefined;
    // returnByValue 会把 DOM 节点变成 {}；至少回一个可读的节点描述。
    if (value === undefined && result.result && result.result.objectId) {
      value = result.result.description || String(result.result.className || 'object');
    }
    const budget = normalizeBudget(args.budget === undefined ? 8000 : args.budget);
    const cursor = normalizeCursor(args.cursor);
    const extra = ranAsBody && value === undefined
      ? 'note=ran as an async function body; add `return <value>` to get a value back'
      : '';
    return formatJsOutput(value, budget, cursor, extra);
  }, { timeoutMs: jsTimeoutMs(args), queueWaitMs: jsTimeoutMs(args) + 5000 });
}

/** 在页面 MAIN world 里执行源码；函数在扩展页被序列化，eval 发生在页面侧。 */
async function injectMainWorld(tabId, source) {
  const results = await chrome.scripting.executeScript({
    target: { tabId },
    world: 'MAIN',
    args: [source],
    // executeScript 会把页面里抛出的异常退化成 null 结果，所以用信封把成功/失败都带回来。
    func: async (src) => {
      // eslint-disable-next-line no-eval
      const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      // 只编译、不执行地探测表达式；编译成功才 eval，运行时异常不再重跑。
      let expressionCompiles = false;
      try {
        // eslint-disable-next-line no-new-func
        new Function('return (' + src + '\n)');
        expressionCompiles = true;
      } catch (_) { /* statement body */ }
      if (expressionCompiles) {
        try {
          return { __ema2: 1, ok: true, value: await (0, eval)(src) };
        } catch (error) {
          return { __ema2: 1, ok: false, error: String((error && error.stack) || error) };
        }
      }
      let bodyError = null;
      try {
        new AsyncFunction(src);
      } catch (error) {
        bodyError = error;
      }
      if (bodyError) {
        return { __ema2: 1, ok: false, syntax: true, error: String((bodyError && bodyError.stack) || bodyError) };
      }
      try {
        return { __ema2: 1, ok: true, value: await new AsyncFunction(src)() };
      } catch (error) {
        return { __ema2: 1, ok: false, error: String((error && error.stack) || error) };
      }
    },
  });
  const payload = results && results[0] ? results[0].result : undefined;
  if (payload && typeof payload === 'object' && payload.__ema2 === 1) return payload;
  // 信封丢失说明注入没有真正跑（frame 切走/被 CSP 拦），不能当成 undefined 成功。
  return { ok: false, error: 'scripting fallback produced no result (frame changed?)', syntax: false };
}

/// debugger 被占时的降级：MAIN world 注入，参数/输出语义与 `js` 一致，额外给一行 via=scripting。
async function jsViaScripting(tabId, args, attachError) {
  const code = String(args.code || '');
  const budget = normalizeBudget(args.budget === undefined ? 8000 : args.budget);
  let payload;
  try {
    payload = await injectMainWorld(tabId, code);
  } catch (error) {
    // 注入本身失败（CSP 拦 eval、tab 被关）：统一报 js_exception，不伪装成 attach 问题。
    const message = oneLine(error && error.message || error).slice(0, 300);
    fail('js_exception', message,
      `scripting fallback failed after ${attachError.code}: page CSP may forbid eval; reload the tab and retry, or close what owns the debugger target`);
  }
  if (!payload.ok) {
    fail('js_exception', oneLine(payload.error || 'JS error').slice(0, 300),
      jsErrorHint(!!payload.syntax));
  }
  const value = payload.value;
  const cursor = normalizeCursor(args.cursor);
  return formatJsOutput(value, budget, cursor,
    'via=scripting (debugger unavailable; this path cannot be interrupted by --timeout)');
}

function stringifyValue(value) {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object' && value.nodeType === 1) {
    const html = value.outerHTML ? String(value.outerHTML) : String(value);
    return html.slice(0, 2000);
  }
  if (typeof value === 'function') return String(value);
  try { return JSON.stringify(value); } catch (_) { return String(value); }
}
