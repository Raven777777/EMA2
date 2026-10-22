//! 截图与 HAR 的落盘：daemon 侧唯一的“文件产物”逻辑。
//!
//! 扩展返回 base64 图片（shot 非 inline）或 HAR JSON 文本（observe har=true），
//! daemon 写入磁盘后把 content 换成 `path=` 行——避免把几十 MB 的 base64
//! 推给 CLI/AI 上下文。CLI 可以用 `out` 指定落盘路径。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

/// 截图 base64 上限（与扩展侧 WS 消息上限一致）。
const MAX_SCREENSHOT_BASE64: usize = 48 * 1024 * 1024;

/// `shot --out` 支持的后缀；目录形式则使用默认文件名。
const SCREENSHOT_SUFFIXES: &str = ".png, .jpg or .jpeg";

static NEXT_OUTPUT_ID: AtomicU64 = AtomicU64::new(0);

fn next_output_path(state: &crate::state::RelayState, extension: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
    state
        .output_dir
        .join(format!("ema2-{now}-{counter}.{extension}"))
}

/// 目录型 `--out` 的默认文件名，例如 `shot-1750000000000-3.png`。
fn default_file_name(prefix: &str, extension: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let counter = NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{millis}-{counter}.{extension}")
}

/// `--out` 是否指向目录。
///
/// `./shots/` 经过 CLI 的 `absolute()` 归一化后会丢掉尾部分隔符，因此没有
/// 后缀的路径也按目录处理；`.gif` 这类明确后缀仍然走文件校验并报错。
fn out_is_directory(path: &Path) -> bool {
    path.is_dir() || path.extension().is_none()
}

/// 同目录临时文件 + `sync_all` + rename，避免并发/崩溃留下半截文件。
async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing file name")
        })?;
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut last_error = None;
    for _ in 0..8 {
        let counter = NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(
            ".{file_name}.ema2-{}-{counter}.tmp",
            std::process::id()
        ));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&temp, path).await
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "cannot create a unique temporary file next to the output",
        )
    }))
}

fn output_hint(dir: &Path) -> String {
    format!(
        "hint=set EMA2_OUTPUT_DIR to a writable path (currently {})",
        dir.display()
    )
}

fn extract_image(content: &Value) -> Option<(String, String)> {
    if content.get("type").and_then(Value::as_str) == Some("image") {
        let data = content.get("data").and_then(Value::as_str)?;
        let mime = content
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("image/png");
        return Some((data.to_owned(), mime.to_owned()));
    }
    let text = content.get("text").and_then(Value::as_str)?;
    for mime in ["image/png", "image/jpeg", "image/webp"] {
        if let Some(data) = text.strip_prefix(&format!("data:{mime};base64,")) {
            return Some((data.to_owned(), mime.to_owned()));
        }
    }
    None
}

fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// 截图默认落盘，只把路径交给 CLI（避免 base64 塞爆上下文）。
///
/// `out` 为 CLI 传来的显式路径（`shot --out` / `observe --har`）；扩展名跟随
/// 实际 mime 而不是用户写的后缀，避免拿 `.png` 名字装 JPEG 内容。
pub async fn save_screenshot(
    state: &crate::state::RelayState,
    mut result: Value,
    out: Option<&Path>,
) -> Value {
    let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) else {
        return result;
    };
    let Some(index) = content
        .iter()
        .position(|item| extract_image(item).is_some())
    else {
        return result;
    };
    let Some((encoded, mime)) = extract_image(&content[index]) else {
        return result;
    };
    if encoded.len() > MAX_SCREENSHOT_BASE64 {
        content[index] = json!({
            "type": "text",
            "text": format!(
                "error=screenshot_too_large msg={} bytes of base64 (limit {MAX_SCREENSHOT_BASE64}) hint=use shot without inline, or capture a smaller window",
                encoded.len()
            ),
        });
        mark_error(&mut result);
        return result;
    }
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&encoded) {
        Ok(bytes) => bytes,
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!("error=screenshot_decode_failed msg={error} hint=retry; if it persists reload the extension"),
            });
            mark_error(&mut result);
            return result;
        }
    };
    if bytes.is_empty() {
        content[index] = json!({
            "type": "text",
            "text": "error=screenshot_empty msg=capture returned 0 bytes hint=make the browser window visible and retry",
        });
        mark_error(&mut result);
        return result;
    }
    let extension = image_extension(&mime);
    let path = match out {
        // 目录形式：拼默认文件名，绝不 with_extension 生成相邻路径。
        Some(path) if out_is_directory(path) => path.join(default_file_name("shot", extension)),
        Some(path) => {
            let suffix = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !matches!(suffix.as_str(), "png" | "jpg" | "jpeg") {
                let given = if suffix.is_empty() {
                    "no suffix".to_owned()
                } else {
                    format!(".{suffix}")
                };
                content[index] = json!({
                    "type": "text",
                    "text": format!(
                        "error=invalid_argument msg=shot --out must end with {SCREENSHOT_SUFFIXES}, got {given} hint=pass a .png/.jpg path, or a directory such as --out ./shots/ to get shot-<timestamp>.png"
                    ),
                });
                mark_error(&mut result);
                return result;
            }
            // 内容格式跟随实际 mime，避免 .jpg 名字装 PNG 字节。
            path.with_extension(extension)
        }
        None => next_output_path(state, extension),
    };
    if write_output(
        state,
        &path,
        &bytes,
        content,
        index,
        "screenshot_not_saved",
        &mime,
    )
    .await
    {
        return result;
    }
    mark_error(&mut result);
    result
}

/// 把 `observe har=true` 的 HAR JSON 落盘，只把路径返回给 CLI。
pub async fn save_har(
    state: &crate::state::RelayState,
    mut result: Value,
    out: Option<&Path>,
) -> Value {
    let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) else {
        return result;
    };
    let Some(index) = content
        .iter()
        .position(|item| item.get("type").and_then(Value::as_str) == Some("text"))
    else {
        return result;
    };
    let Some(text) = content[index].get("text").and_then(Value::as_str) else {
        return result;
    };
    if !text.trim_start().starts_with('{') {
        return result;
    }
    let entries = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/log/entries")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    let path = match out {
        Some(path) if out_is_directory(path) => path.join(default_file_name("har", "har")),
        Some(path) => path.with_extension("har"),
        None => next_output_path(state, "har"),
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            content[index] = json!({
                "type": "text",
                "text": format!("error=har_not_saved msg=cannot create {}: {error} hint=check the --har path", parent.display()),
            });
            mark_error(&mut result);
            return result;
        }
    }
    let text = text.to_owned();
    match write_atomic(&path, text.as_bytes()).await {
        Ok(()) => {
            content[index] = json!({
                "type": "text",
                "text": format!("path={}\nbytes={} entries={entries}", path.display(), text.len()),
            });
            result
        }
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!("error=har_not_saved msg={} : {error} hint={}", path.display(), output_hint(&state.output_dir)),
            });
            mark_error(&mut result);
            result
        }
    }
}

async fn write_output(
    state: &crate::state::RelayState,
    path: &Path,
    bytes: &[u8],
    content: &mut [Value],
    index: usize,
    code: &str,
    mime: &str,
) -> bool {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            content[index] = json!({
                "type": "text",
                "text": format!("error={code} msg=cannot create {}: {error} hint=check the --out path", parent.display()),
            });
            return false;
        }
    }
    match write_atomic(path, bytes).await {
        Ok(()) => {
            content[index] = json!({
                "type": "text",
                "text": format!("path={}\nbytes={} mime={mime}", path.display(), bytes.len()),
            });
            true
        }
        Err(error) => {
            content[index] = json!({
                "type": "text",
                "text": format!("error={code} msg={} : {error} hint={}", path.display(), output_hint(&state.output_dir)),
            });
            false
        }
    }
}

fn mark_error(result: &mut Value) {
    if let Some(object) = result.as_object_mut() {
        object.insert("isError".to_owned(), Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RelayState;
    use std::path::PathBuf;

    fn temp_state() -> (std::sync::Arc<RelayState>, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ema2-save-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        (RelayState::new(dir.clone(), 7744), dir)
    }

    fn image_response(data: &str) -> Value {
        json!({
            "content": [{ "type": "image", "data": data, "mimeType": "image/png" }]
        })
    }

    #[tokio::test]
    async fn screenshot_is_saved_as_a_compact_path_line() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
        let response = save_screenshot(&state, image_response(&data), None).await;
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("path="), "{text}");
        assert!(text.contains("bytes=14"), "{text}");
        assert!(response.get("isError").is_none());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn explicit_out_path_wins_and_keeps_the_real_extension() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"jpeg-bytes");
        let out = dir.join("custom-name.png"); // 名字写 png，实际是 png mime
        let response = save_screenshot(&state, image_response(&data), Some(&out)).await;
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("custom-name.png"), "{text}");
        assert!(out.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn annotated_screenshot_keeps_text_and_saves_the_image() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"annotated-png");
        let response = json!({
            "content": [
                { "type": "text", "text": "[e1] button \"Submit\" @10,20 30x8\nmarks=1" },
                { "type": "image", "data": data, "mimeType": "image/png" }
            ]
        });
        let saved = save_screenshot(&state, response, None).await;
        let content = saved["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert!(content[0]["text"].as_str().unwrap().starts_with("[e1]"));
        assert!(content[1]["text"].as_str().unwrap().starts_with("path="));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn screenshot_decode_failure_is_compact_error() {
        let (state, dir) = temp_state();
        let response = save_screenshot(&state, image_response("not base64!!!"), None).await;
        assert_eq!(response["isError"], true);
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("error=screenshot_decode_failed"), "{text}");
        assert!(text.contains("hint="), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn har_is_saved_with_entry_count() {
        let (state, dir) = temp_state();
        let har = json!({ "log": { "entries": [{}, {}] } }).to_string();
        let response = json!({ "content": [{ "type": "text", "text": har }] });
        let saved = save_har(&state, response, Some(&dir.join("net.har"))).await;
        let text = saved["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("path="), "{text}");
        assert!(text.contains("entries=2"), "{text}");
        assert!(dir.join("net.har").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn non_screenshot_results_are_left_untouched() {
        let (state, dir) = temp_state();
        let response = json!({ "content": [{ "type": "text", "text": "hello" }] });
        let saved = save_screenshot(&state, response.clone(), None).await;
        assert_eq!(saved, response);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--out` 是目录时拼接默认文件名，而不是写一个叫 `shots` 的文件。
    #[tokio::test]
    async fn out_directory_gets_the_default_file_name() {
        let (state, dir) = temp_state();
        let out = dir.join("shots");
        std::fs::create_dir_all(&out).unwrap();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");

        let response = save_screenshot(&state, image_response(&data), Some(&out)).await;
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("shot-"), "{text}");
        let files: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name().to_string_lossy().starts_with("shot-"));
        assert!(files[0].file_name().to_string_lossy().ends_with(".png"));

        // 不存在的无后缀路径也按目录处理（CLI 会去掉 `./shots/` 的尾部分隔符）。
        let fresh = dir.join("fresh");
        let response = save_screenshot(&state, image_response(&data), Some(&fresh)).await;
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("path="), "{text}");
        assert!(fresh.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unsupported_screenshot_suffix_is_invalid_argument() {
        let (state, dir) = temp_state();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");
        let out = dir.join("shot.gif");
        let response = save_screenshot(&state, image_response(&data), Some(&out)).await;
        assert_eq!(response["isError"], true);
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("error=invalid_argument"), "{text}");
        assert!(text.contains(".png"), "{text}");
        assert!(!out.exists(), "invalid suffix must not write anything");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 并发写同一路径：结果必须是两份完整内容之一，且不留下临时文件。
    #[tokio::test]
    async fn concurrent_writes_to_one_path_never_leave_a_partial_file() {
        let (state, dir) = temp_state();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("net.har");
        let first = json!({ "log": { "entries": [{}] } }).to_string();
        let second = json!({ "log": { "entries": [{}, {}] } }).to_string();
        let (left, right) = tokio::join!(
            save_har(
                &state,
                json!({ "content": [{ "type": "text", "text": first.clone() }] }),
                Some(&path),
            ),
            save_har(
                &state,
                json!({ "content": [{ "type": "text", "text": second.clone() }] }),
                Some(&path),
            ),
        );
        assert!(
            left["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("path=")
        );
        assert!(
            right["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("path=")
        );

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written == first || written == second,
            "concurrent writes produced a mixed file: {written}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
