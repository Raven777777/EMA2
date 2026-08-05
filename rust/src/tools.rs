//! 浏览器控制工具定义（与 EMA2 JS 扩展协议一致）。

use serde_json::{json, Value};

/// MCP tools/list 返回的 9 个工具。
pub fn tool_list() -> Value {
    json!([
        {
            "name": "edge_get_tabs",
            "description": "List all open tabs in Edge (id, title, url, active)",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "edge_get_active_tab",
            "description": "Get the active tab url and title",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "edge_read_active_tab",
            "description": "Extract the plain text content of the active tab (for AI to read the current page)",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "edge_read_tab",
            "description": "Extract plain text content of a specific tab by id",
            "inputSchema": {
                "type": "object",
                "properties": { "tabId": { "type": "number", "description": "Tab id from edge_get_tabs" } },
                "required": ["tabId"]
            }
        },
        {
            "name": "edge_execute_js",
            "description": "Execute arbitrary JavaScript in the active tab (via chrome.debugger Runtime.evaluate) and return the serialized result",
            "inputSchema": {
                "type": "object",
                "properties": { "code": { "type": "string", "description": "JavaScript expression (async supported)" } },
                "required": ["code"]
            }
        },
        {
            "name": "edge_click_element",
            "description": "Click an element in the active tab by CSS selector",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string" } },
                "required": ["selector"]
            }
        },
        {
            "name": "edge_new_tab",
            "description": "Open a new tab with the given URL",
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        },
        {
            "name": "edge_navigate",
            "description": "Navigate the active tab to a URL",
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        },
        {
            "name": "edge_close_tab",
            "description": "Close a tab by id (defaults to the active tab)",
            "inputSchema": {
                "type": "object",
                "properties": { "tabId": { "type": "number" } }
            }
        },
        {
            "name": "edge_wait_for",
            "description": "Wait for text to appear or disappear on the page, or wait a fixed time. Resolves when the condition is met (default timeout 30s)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Wait until this text appears in the page" },
                    "textGone": { "type": "string", "description": "Wait until this text disappears from the page" },
                    "time": { "type": "number", "description": "Wait this many seconds instead" }
                }
            }
        },
        {
            "name": "edge_take_screenshot",
            "description": "Take a screenshot of the active tab. Saved as PNG in the output directory; returns the file path",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "edge_console_messages",
            "description": "Return console messages (log/warn/error) captured on the page since the first call",
            "inputSchema": {
                "type": "object",
                "properties": { "level": { "type": "string", "enum": ["log", "warn", "error"], "default": "error" } }
            }
        },
        {
            "name": "edge_network_requests",
            "description": "List network requests the page has made (from performance entries)",
            "inputSchema": {
                "type": "object",
                "properties": { "filter": { "type": "string", "description": "Only return URLs matching this substring/regex" } }
            }
        },
        {
            "name": "edge_type",
            "description": "Type text into an element by CSS selector (dispatches input/change events)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "text": { "type": "string" },
                    "submit": { "type": "boolean", "description": "Press Enter after typing" }
                },
                "required": ["selector", "text"]
            }
        },
        {
            "name": "edge_press_key",
            "description": "Press a key on the currently focused element (e.g. Enter, Escape, ArrowDown)",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"]
            }
        },
        {
            "name": "edge_fill_form",
            "description": "Fill multiple form fields by CSS selector",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fields": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "selector": { "type": "string" },
                                "name": { "type": "string" },
                                "value": { "type": "string" }
                            },
                            "required": ["selector", "value"]
                        }
                    }
                },
                "required": ["fields"]
            }
        },
        {
            "name": "edge_hover",
            "description": "Hover the mouse over an element by CSS selector",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string" } },
                "required": ["selector"]
            }
        },
        {
            "name": "edge_scroll",
            "description": "Scroll the page by a delta (default one viewport height)",
            "inputSchema": {
                "type": "object",
                "properties": { "deltaY": { "type": "number", "description": "Scroll amount in px" } }
            }
        }
    ])
}
