#!/usr/bin/env node
'use strict';

/**
 * EMA2 v1.4 示例：通过 MCP 控制真实 Edge，在 Google 查天气并读取结果。
 *
 * v1.4 工具面（9 个）：tabs / open / read / snapshot / act / wait / js / observe / shot
 * 输出是行式紧凑文本，错误是 `error=<code> msg=... hint=...`。
 */

const MCP_URL = process.env.EMA2_MCP_URL || 'http://127.0.0.1:8765/mcp';
const MCP_TOKEN = process.env.EDGE_MCP_TOKEN;
const REQUEST_TIMEOUT_MS = 60_000;

async function mcpRequest(body, sessionId) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  const headers = { 'content-type': 'application/json' };
  if (sessionId) headers['mcp-session-id'] = sessionId;
  if (MCP_TOKEN) headers.authorization = `Bearer ${MCP_TOKEN}`;
  try {
    const response = await fetch(MCP_URL, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const raw = await response.text();
    if (!response.ok) {
      throw new Error(`MCP request failed with HTTP ${response.status}${raw ? `: ${raw.slice(0, 200)}` : ''}`);
    }
    if (!raw) throw new Error('MCP returned an empty response');
    let data;
    try { data = JSON.parse(raw); } catch (_) { throw new Error(`MCP returned invalid JSON: ${raw.slice(0, 200)}`); }
    if (data.error) throw new Error(data.error.message || 'MCP request failed');
    return { data, sessionId: response.headers.get('mcp-session-id') || sessionId };
  } catch (error) {
    if (error.name === 'AbortError') throw new Error('MCP request timed out');
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

async function callTool(name, args, sessionId, id) {
  const result = await mcpRequest({
    jsonrpc: '2.0',
    id,
    method: 'tools/call',
    params: { name, arguments: args },
  }, sessionId);
  const content = result.data.result?.content || [];
  const text = content.find((item) => item.type === 'text')?.text;
  if (result.data.result?.isError) throw new Error(`Tool ${name} failed: ${text || 'unknown error'}`);
  if (typeof text !== 'string') throw new Error(`Tool ${name} returned no text content`);
  return text;
}

/** 从 `id=123\nurl=...` 这样的行式输出里取字段。 */
function field(text, key) {
  const match = new RegExp(`^${key}=(.*)$`, 'm').exec(text || '');
  return match ? match[1] : undefined;
}

async function queryWeather(location, options = {}) {
  if (typeof location !== 'string' || !location.trim()) throw new Error('A location is required');
  const place = location.trim();
  const url = `https://www.google.com/search?q=${encodeURIComponent(`${place} weather`)}`;
  const initialize = await mcpRequest({
    jsonrpc: '2.0',
    id: 1,
    method: 'initialize',
    params: {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'ema2-weather-script', version: '2.0.0' },
    },
  });
  const sessionId = initialize.sessionId;
  if (!sessionId) throw new Error('MCP server did not return a session id (Mcp-Session-Id header)');

  let nextId = 2;
  const opened = await callTool('open', { url, wait: 'load' }, sessionId, nextId++);
  const tabId = Number(field(opened, 'id'));

  let warning;
  try { await callTool('wait', { text: place, timeout: 30 }, sessionId, nextId++); }
  catch (error) { warning = error.message; }

  try {
    const text = await callTool('read', { mode: 'md', budget: 12000 }, sessionId, nextId++);
    return {
      location: place,
      url,
      tab: opened,
      text,
      warning,
      sessionId: options.includeSession ? sessionId : undefined,
    };
  } finally {
    if (!options.keepTab && Number.isFinite(tabId)) {
      await callTool('tabs', { action: 'close', tabId }, sessionId, nextId++).catch(() => {});
    }
  }
}

function printUsage() {
  console.log('Usage: node examples/weather.js <city or place> [--json] [--keep-tab]');
  console.log('Example: node examples/weather.js "Tokyo"');
  console.log('Requires the EMA2 relay (v1.4) and the Edge extension to be connected.');
}

async function main(argv) {
  const jsonOutput = argv.includes('--json');
  const keepTab = argv.includes('--keep-tab');
  const location = argv.filter((arg) => !arg.startsWith('--')).join(' ').trim();
  if (argv.includes('--help') || argv.includes('-h')) { printUsage(); return 0; }
  if (!location) { printUsage(); return 1; }
  const result = await queryWeather(location, { keepTab });
  console.log(jsonOutput ? JSON.stringify(result, null, 2) : result.text);
  return 0;
}

if (require.main === module) {
  main(process.argv.slice(2)).then((code) => {
    process.exitCode = code;
  }).catch((error) => {
    console.error(`Error: ${error.message}`);
    process.exitCode = 1;
  });
}

module.exports = { queryWeather };
