//! 最小 JSONC 文本编辑器：只在对象里增删一个成员，其余字节原样保留。
//!
//! 为什么不用 `serde_json` 反序列化再序列化：那样会丢掉注释、键顺序和用户排版，
//! 而 opencode 的配置正是带注释的 `.jsonc`。这里做一个只认识“对象成员边界”的扫描器，
//! 找到目标成员后在文本上做插入/替换/删除，再用 `serde_json` 校验结果确实可解析。

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// 根值必须是对象（配置文件都是 `{...}`）。
pub fn root_object(text: &str) -> Result<Span, String> {
    let bytes = text.as_bytes();
    let start = skip_trivia(bytes, 0);
    if start >= bytes.len() || bytes[start] != b'{' {
        return Err("config root is not a JSON object".into());
    }
    scan_container(bytes, start)
}

/// 查找对象里某个键的值范围；键名按 JSON 字符串解码后比较。
pub fn member_value(text: &str, object: Span, key: &str) -> Option<Span> {
    members(text, object)
        .ok()?
        .into_iter()
        .find(|member| member.key == key)
        .map(|member| member.value)
}

/// 插入或替换成员；返回新文本。
pub fn upsert_member(
    text: &str,
    object: Span,
    key: &str,
    value_json: &str,
) -> Result<String, String> {
    if let Some(current) = member_value(text, object, key) {
        let mut out = String::with_capacity(text.len() + value_json.len());
        out.push_str(&text[..current.start]);
        out.push_str(value_json);
        out.push_str(&text[current.end..]);
        return Ok(out);
    }

    let list = members(text, object)?;
    let close = object.end - 1;
    let close_indent = line_indent(text, close);

    let indent = match list.first() {
        Some(first) if starts_line(text, first.key_span.start) => {
            line_indent(text, first.key_span.start)
        }
        Some(_) => format!("{close_indent}  "),
        None => format!("{close_indent}  "),
    };
    let entry = format!("{indent}\"{}\": {value_json}", escape_key(key));

    let mut out = String::with_capacity(text.len() + entry.len() + 3);
    match list.last() {
        None => {
            if starts_line(text, close) {
                let line_start = text[..close].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
                out.push_str(&text[..line_start]);
                out.push_str(&entry);
                out.push('\n');
                out.push_str(&close_indent);
                out.push_str(&text[close..]);
            } else {
                // 行内空对象：保持单行，避免把 `"mcp": {}` 拆成多行。
                out.push_str(&text[..close]);
                out.push_str(&format!("\"{}\": {value_json}", escape_key(key)));
                out.push_str(&text[close..]);
            }
        }
        Some(last) => {
            let after = skip_trivia(text.as_bytes(), last.value.end);
            let bytes = text.as_bytes();
            if after < bytes.len() && bytes[after] == b',' {
                // 已有尾逗号：把新成员接在逗号后面，保持“最后一项无逗号”的写法。
                out.push_str(&text[..after + 1]);
                out.push('\n');
                out.push_str(&entry);
                out.push_str(&text[after + 1..]);
            } else {
                out.push_str(&text[..last.value.end]);
                out.push_str(",\n");
                out.push_str(&entry);
                out.push_str(&text[last.value.end..]);
            }
        }
    }
    Ok(out)
}

/// 删除成员；连带删掉一个相邻逗号，保证结果仍是合法 JSON/JSONC。
pub fn remove_member(text: &str, object: Span, key: &str) -> Result<Option<String>, String> {
    let list = members(text, object)?;
    let Some(index) = list.iter().position(|member| member.key == key) else {
        return Ok(None);
    };
    let target = &list[index];
    let bytes = text.as_bytes();

    let mut start = target.key_span.start;
    let after = skip_trivia(bytes, target.value.end);
    let mut end = target.value.end;
    if after < bytes.len() && bytes[after] == b',' {
        end = after + 1;
        while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
            end += 1;
        }
    } else {
        // 最后一项且无尾逗号：删掉前面的逗号，否则会留下悬空分隔符。
        let before = skip_trivia_back(bytes, start);
        if before > 0 && bytes[before - 1] == b',' {
            start = before - 1;
        }
    }

    // 若该成员独占一行，把整行的缩进也去掉，避免留下空白行。
    let line_start = text[..start].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    if line_start < start && text[line_start..start].trim().is_empty() {
        let rest_end = text[end..].find('\n').map(|pos| end + pos);
        if rest_end.is_some_and(|pos| text[end..pos].trim().is_empty()) {
            start = line_start;
            if let Some(pos) = rest_end {
                end = pos + 1;
            }
        }
    }

    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    Ok(Some(out))
}

#[derive(Debug)]
struct Member {
    key: String,
    key_span: Span,
    value: Span,
}

fn members(text: &str, object: Span) -> Result<Vec<Member>, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = object.start + 1;
    loop {
        i = skip_trivia(bytes, i);
        if i >= bytes.len() {
            return Err("unterminated object".into());
        }
        if bytes[i] == b'}' {
            return Ok(out);
        }
        if bytes[i] != b'"' {
            return Err(format!("expected a key at byte {i}"));
        }
        let key_span = string_span(bytes, i)?;
        let key: String = serde_json::from_str(&text[key_span.start..key_span.end])
            .map_err(|error| format!("invalid key: {error}"))?;
        i = skip_trivia(bytes, key_span.end);
        if i >= bytes.len() || bytes[i] != b':' {
            return Err(format!("expected ':' after key at byte {i}"));
        }
        i = skip_trivia(bytes, i + 1);
        let value = scan_value(bytes, i)?;
        out.push(Member {
            key,
            key_span,
            value,
        });
        i = skip_trivia(bytes, value.end);
        if i < bytes.len() && bytes[i] == b',' {
            i += 1;
            continue;
        }
        if i < bytes.len() && bytes[i] == b'}' {
            return Ok(out);
        }
        return Err(format!("expected ',' or '}}' at byte {i}"));
    }
}

fn scan_value(bytes: &[u8], start: usize) -> Result<Span, String> {
    if start >= bytes.len() {
        return Err("missing value".into());
    }
    match bytes[start] {
        b'{' | b'[' => scan_container(bytes, start),
        b'"' => string_span(bytes, start),
        _ => {
            let mut end = start;
            while end < bytes.len() {
                let byte = bytes[end];
                if byte == b','
                    || byte == b'}'
                    || byte == b']'
                    || byte == b'/'
                    || byte.is_ascii_whitespace()
                {
                    break;
                }
                end += 1;
            }
            if end == start {
                return Err(format!("empty value at byte {start}"));
            }
            Ok(Span { start, end })
        }
    }
}

fn scan_container(bytes: &[u8], start: usize) -> Result<Span, String> {
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = string_span(bytes, i)?.end;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            byte if byte == open => {
                depth += 1;
                i += 1;
            }
            byte if byte == close => {
                if depth == 0 {
                    return Err("unbalanced brackets".into());
                }
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Ok(Span { start, end: i });
                }
            }
            _ => i += 1,
        }
    }
    Err("unterminated container".into())
}

fn string_span(bytes: &[u8], start: usize) -> Result<Span, String> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok(Span { start, end: i + 1 }),
            _ => i += 1,
        }
    }
    Err("unterminated string".into())
}

fn skip_trivia(bytes: &[u8], mut i: usize) -> usize {
    // Windows 编辑器可能写 UTF-8 BOM；按空白处理，保持字节偏移不变。
    if i == 0 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
    }
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        return i;
    }
}

/// 从 `i` 向前跳过空白与块注释，返回“前一个有效字符之后”的位置。
fn skip_trivia_back(bytes: &[u8], mut i: usize) -> usize {
    loop {
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i >= 2 && bytes[i - 2] == b'*' && bytes[i - 1] == b'/' {
            let mut j = i - 2;
            while j >= 2 {
                if bytes[j - 2] == b'/' && bytes[j - 1] == b'*' {
                    j -= 2;
                    break;
                }
                j -= 1;
            }
            i = j;
            continue;
        }
        return i;
    }
}

fn line_indent(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    text[start..at]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

fn starts_line(text: &str, at: usize) -> bool {
    let start = text[..at].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    text[start..at].trim().is_empty()
}

fn escape_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// 校验任意 JSON/JSONC 文本（允许注释与尾逗号），返回解析后的值。
pub fn validate(text: &str) -> Result<Value, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let stripped = strip_trailing_commas(&strip_comments(text));
    serde_json::from_str(&stripped).map_err(|error| error.to_string())
}

/// 把注释替换成等长空格（保留换行），字符串内容不动。
pub fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                let end = (i + 2).min(bytes.len());
                for byte in out.iter_mut().take(end).skip(start) {
                    if *byte != b'\n' && *byte != b'\r' {
                        *byte = b' ';
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b',' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                    out[i] = b' ';
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(text: &str) -> Span {
        root_object(text).unwrap()
    }

    #[test]
    fn inserts_into_an_empty_object() {
        let text = "{\n}\n";
        let out = upsert_member(text, root(text), "mcp", "{}").unwrap();
        assert_eq!(out, "{\n  \"mcp\": {}\n}\n");
        assert!(validate(&out).is_ok());
    }

    #[test]
    fn preserves_comments_and_adds_member_after_trailing_comma() {
        let text = "{\n  // keep me\n  \"a\": 1,\n  /* block */\n}\n";
        let out = upsert_member(text, root(text), "mcp", "{\"x\":1}").unwrap();
        assert!(out.contains("// keep me"));
        assert!(out.contains("/* block */"));
        assert!(out.contains("\"mcp\": {\"x\":1}"));
        let value = validate(&out).unwrap();
        assert_eq!(value["a"], 1);
        assert_eq!(value["mcp"]["x"], 1);
    }

    #[test]
    fn inserts_after_last_member_without_trailing_comma() {
        let text = "{\n  \"a\": 1\n}\n";
        let out = upsert_member(text, root(text), "mcp", "{}").unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  \"mcp\": {}\n}\n");
        assert_eq!(validate(&out).unwrap()["mcp"], serde_json::json!({}));
    }

    #[test]
    fn replaces_existing_member_in_place() {
        let text =
            "{\n  \"mcp\": {\n    \"edge-browser\": {\"url\": \"old\"}\n  },\n  \"z\": 2\n}\n";
        let root_span = root(text);
        let mcp = member_value(text, root_span, "mcp").unwrap();
        let out = upsert_member(text, mcp, "edge-browser", "{\"url\":\"new\"}").unwrap();
        assert!(out.contains("\"z\": 2"), "siblings must be preserved");
        assert!(out.contains("\"url\":\"new\""));
        assert!(!out.contains("old"));
        assert_eq!(validate(&out).unwrap()["mcp"]["edge-browser"]["url"], "new");
    }

    #[test]
    fn braces_and_commas_inside_strings_do_not_confuse_the_scanner() {
        let text = r#"{ "note": "a } b, c [d]", "n": 1 }"#;
        let out = upsert_member(text, root(text), "mcp", "{}").unwrap();
        let value = validate(&out).unwrap();
        assert_eq!(value["note"], "a } b, c [d]");
        assert_eq!(value["n"], 1);
    }

    #[test]
    fn nested_objects_keep_their_shape() {
        let text = "{\n  \"provider\": {\n    \"x\": { \"models\": [{\"id\": \"y\"}] }\n  }\n}\n";
        let out = upsert_member(text, root(text), "mcp", "{}").unwrap();
        assert!(out.contains("\"models\""));
        assert_eq!(
            validate(&out).unwrap()["provider"]["x"]["models"][0]["id"],
            "y"
        );
    }

    #[test]
    fn removes_members_without_leaving_dangling_commas() {
        for text in [
            "{\"a\":1,\"b\":2}",
            "{\"b\":2,\"a\":1}",
            "{\"a\":1}",
            "{\"a\":1,}",
            "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
        ] {
            let out = remove_member(text, root(text), "b").unwrap();
            match out {
                Some(out) => {
                    let value = validate(&out).unwrap();
                    assert_eq!(value, serde_json::json!({"a": 1}), "input={text} out={out}");
                }
                None => assert!(!text.contains("\"b\""), "input={text}"),
            }
        }
    }

    #[test]
    fn removing_a_missing_member_is_a_no_op() {
        let text = "{\"a\":1}";
        assert!(remove_member(text, root(text), "nope").unwrap().is_none());
    }

    #[test]
    fn rejects_malformed_input_instead_of_guessing() {
        assert!(root_object("{ not json").is_err());
        assert!(root_object("[1,2]").is_err());
        assert!(root_object("{\"a\": 1").is_err());
        // 结构错误由完整校验兜住（写入前必须能解析）。
        assert!(validate("{\"a\":}").is_err());
        assert!(validate("{\"a\": 1}").is_ok());
    }

    #[test]
    fn handles_a_utf8_bom_without_losing_offsets() {
        let text = "\u{feff}{\n  \"a\": 1\n}\n";
        assert_eq!(validate(text).unwrap()["a"], 1);
        let out = upsert_member(text, root(text), "mcp", "{}").unwrap();
        assert_eq!(validate(&out).unwrap()["mcp"], serde_json::json!({}));
    }

    #[test]
    fn validate_accepts_comments_and_trailing_commas() {
        let text = "{\n  // c\n  \"a\": [1, 2,],\n}\n";
        assert_eq!(validate(text).unwrap()["a"][0], 1);
    }
}
