#!/usr/bin/env node
'use strict';

const MCP_URL = process.env.EMA2_MCP_URL || 'http://127.0.0.1:8765/mcp';
const MCP_TOKEN = process.env.EDGE_MCP_TOKEN;
const REQUEST_TIMEOUT_MS = 30_000;

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
    const data = await response.json();
    if (!response.ok) throw new Error(`MCP request failed with HTTP ${response.status}`);
    if (data.error) throw new Error(data.error.message || 'MCP request failed');
    return { data, sessionId: response.headers.get('mcp-session-id') || sessionId };
  } catch (error) {
    if (error.name === 'AbortError') throw new Error('MCP request timed out');
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

async function callTool(name, arguments_, sessionId, id) {
  const result = await mcpRequest({
    jsonrpc: '2.0',
    id,
    method: 'tools/call',
    params: { name, arguments: arguments_ },
  }, sessionId);
  const text = result.data.result?.content?.find((item) => item.type === 'text')?.text;
  if (typeof text !== 'string') throw new Error(`Tool ${name} returned no text content`);
  return text;
}

async function queryWeather(location, options = {}) {
  if (typeof location !== 'string' || !location.trim()) throw new Error('A location is required');
  const place = location.trim();
  const query = `${place} weather`;
  const url = `https://www.google.com/search?q=${encodeURIComponent(query)}`;
  const initialize = await mcpRequest({
    jsonrpc: '2.0',
    id: 1,
    method: 'initialize',
    params: {
      protocolVersion: '2025-03-26',
      capabilities: {},
      clientInfo: { name: 'ema2-weather-script', version: '1.0.0' },
    },
  });
  const sessionId = initialize.sessionId || initialize.data.result?._meta?.sessionId;
  if (!sessionId) throw new Error('MCP server did not return a session ID');

  const opened = await callTool('edge_new_tab', { url }, sessionId, 2);
  let waitError;
  try {
    await callTool('edge_wait_for', { text: place }, sessionId, 3);
  } catch (error) {
    waitError = error.message;
  }
  const text = await callTool('edge_read_active_tab', {}, sessionId, 4);
  return {
    location: place,
    url,
    tab: opened,
    text,
    warning: waitError,
    sessionId: options.includeSession ? sessionId : undefined,
  };
}

function printUsage() {
  console.log('Usage: node scripts/weather.js <city or place> [--json]');
  console.log('Example: node scripts/weather.js "Tokyo"');
  console.log('Requires the EMA2 relay and Edge extension to be connected.');
}

async function main(argv) {
  const jsonOutput = argv.includes('--json');
  const location = argv.filter((arg) => !arg.startsWith('--')).join(' ').trim();
  if (argv.includes('--help') || argv.includes('-h')) {
    printUsage();
    return 0;
  }
  if (!location) {
    printUsage();
    return 1;
  }
  const result = await queryWeather(location);
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
