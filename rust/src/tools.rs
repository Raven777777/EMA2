//! 浏览器控制工具定义（与 EMA2 JS 扩展协议一致）。

use serde_json::{Value, json};

/// MCP tools/list 返回的浏览器工具。
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
            "description": "Return console messages captured after console capture is initialized on the page",
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

pub fn contains(name: &str) -> bool {
    matches!(
        name,
        "edge_get_tabs"
            | "edge_get_active_tab"
            | "edge_read_active_tab"
            | "edge_read_tab"
            | "edge_execute_js"
            | "edge_click_element"
            | "edge_new_tab"
            | "edge_navigate"
            | "edge_close_tab"
            | "edge_wait_for"
            | "edge_take_screenshot"
            | "edge_console_messages"
            | "edge_network_requests"
            | "edge_type"
            | "edge_press_key"
            | "edge_fill_form"
            | "edge_hover"
            | "edge_scroll"
    )
}

pub fn validate_arguments(name: &str, args: &Value) -> Result<(), &'static str> {
    let object = args.as_object().ok_or("Tool arguments must be an object")?;
    let string = |key: &str, max: usize, required: bool| -> Result<(), &'static str> {
        match object.get(key) {
            Some(Value::String(value))
                if value.len() <= max && (!required || !value.trim().is_empty()) =>
            {
                Ok(())
            }
            None if !required => Ok(()),
            _ => Err("Invalid tool argument"),
        }
    };
    let number = |key: &str, required: bool| -> Result<Option<f64>, &'static str> {
        match object.get(key) {
            Some(value) => match value.as_f64() {
                Some(value) if value.is_finite() => Ok(Some(value)),
                _ => Err("Invalid numeric tool argument"),
            },
            None if !required => Ok(None),
            None => Err("Missing numeric tool argument"),
        }
    };

    match name {
        "edge_read_tab" => {
            let value = number("tabId", true)?.ok_or("tabId is required")?;
            if value < 0.0 || value.fract() != 0.0 {
                return Err("Invalid tabId");
            }
        }
        "edge_execute_js" => string("code", 100_000, true)?,
        "edge_click_element" | "edge_hover" => string("selector", 2_048, true)?,
        "edge_new_tab" | "edge_navigate" => {
            string("url", 8_192, true)?;
            let url = object.get("url").and_then(Value::as_str).unwrap_or("");
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("URL must use http or https");
            }
        }
        "edge_close_tab" => {
            if let Some(value) = number("tabId", false)?
                && (value < 0.0 || value.fract() != 0.0)
            {
                return Err("Invalid tabId");
            }
        }
        "edge_wait_for" => {
            string("text", 100_000, false)?;
            string("textGone", 100_000, false)?;
            if object.contains_key("text") && object.contains_key("textGone") {
                return Err("Use text or textGone, not both");
            }
            let time = number("time", false)?;
            if let Some(value) = time {
                if !(0.0..=300.0).contains(&value)
                    || object.contains_key("text")
                    || object.contains_key("textGone")
                {
                    return Err("Invalid wait condition");
                }
            } else if !object.contains_key("text") && !object.contains_key("textGone") {
                return Err("A wait condition is required");
            }
        }
        "edge_console_messages" => {
            if let Some(level) = object.get("level")
                && !matches!(level.as_str(), Some("log" | "warn" | "error"))
            {
                return Err("Invalid level");
            }
        }
        "edge_network_requests" => string("filter", 2_048, false)?,
        "edge_type" => {
            string("selector", 2_048, true)?;
            string("text", 100_000, true)?;
            if let Some(value) = object.get("submit")
                && !value.is_boolean()
            {
                return Err("Invalid submit");
            }
        }
        "edge_press_key" => string("key", 100, true)?,
        "edge_fill_form" => {
            let fields = object
                .get("fields")
                .and_then(Value::as_array)
                .ok_or("fields is required")?;
            if fields.len() > 100 {
                return Err("Too many fields");
            }
            for field in fields {
                let field = field.as_object().ok_or("Invalid field")?;
                let selector = field
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or("Invalid selector")?;
                let value = field
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or("Invalid value")?;
                if selector.is_empty() || selector.len() > 2_048 || value.len() > 100_000 {
                    return Err("Invalid field");
                }
                if let Some(name) = field.get("name")
                    && (!name.is_string() || name.as_str().unwrap_or("").len() > 200)
                {
                    return Err("Invalid field name");
                }
            }
        }
        "edge_scroll" => {
            if let Some(value) = number("deltaY", false)?
                && value.abs() > 100_000.0
            {
                return Err("Invalid deltaY");
            }
        }
        "edge_get_tabs"
        | "edge_get_active_tab"
        | "edge_read_active_tab"
        | "edge_take_screenshot" => {}
        _ => return Err("Unknown tool"),
    }
    Ok(())
}
