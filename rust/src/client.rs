//! 极简本机 HTTP/1.1 客户端。
//!
//! CLI 是短命进程，只需要向 `127.0.0.1:7744` 发一个请求；不引入 reqwest，
//! 保持单 exe 体积。响应可能是普通 `Content-Length`，也可能是 axum 的
//! `Transfer-Encoding: chunked`，两种都处理。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::Value;

/// 响应体的累计上限：daemon 正常响应远小于此，超过即视为异常对端。
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// 发一个请求。`body` 为 `None` 时不带请求体（GET）。
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> std::io::Result<HttpResponse> {
    let address = ("127.0.0.1", port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other("cannot resolve 127.0.0.1"))?;
    // `timeout` 是整次请求的总预算，不只是单次 read 的超时。
    let deadline = Instant::now() + timeout;
    let connect_timeout = timeout.min(Duration::from_secs(3));
    let mut stream = TcpStream::connect_timeout(&address, connect_timeout)?;
    let _ = stream.set_write_timeout(Some(timeout));
    let _ = stream.set_nodelay(true);

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nUser-Agent: ema2/{}\r\nX-EMA2-Client: ema2-cli\r\nConnection: close\r\n",
        crate::paths::PRODUCT_VERSION
    );
    if let Some(body) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(body) = body {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()?;

    let raw = read_response(&mut stream, deadline, timeout)?;
    parse_response(&raw)
}

/// 按总 deadline 读取完整响应，并限制累计字节数。
///
/// `SO_RCVTIMEO` 只约束单次 `read`：对端每 100ms 吐 1 字节即可无限拖住 CLI。
/// 这里每次 read 前按剩余预算重设超时，并在累计超过 32MB 时报错。
fn read_response(
    stream: &mut TcpStream,
    deadline: Instant,
    budget: Duration,
) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "HTTP response did not complete within {:.1}s ({} bytes read)",
                    budget.as_secs_f64(),
                    raw.len()
                ),
            ));
        }
        let remaining = (deadline - now).max(Duration::from_millis(1));
        let _ = stream.set_read_timeout(Some(remaining));
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(raw),
            Ok(read) => {
                if raw.len() + read > MAX_RESPONSE_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("HTTP response exceeded the {MAX_RESPONSE_BYTES} byte limit"),
                    ));
                }
                raw.extend_from_slice(&buffer[..read]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "HTTP response did not complete within {:.1}s ({} bytes read)",
                        budget.as_secs_f64(),
                        raw.len()
                    ),
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

fn parse_response(raw: &[u8]) -> std::io::Result<HttpResponse> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "short HTTP response")
        })?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid HTTP status line: {status_line:?}"),
            )
        })?;

    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if name == "content-length" {
            content_length = Some(value.parse().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid Content-Length: {value:?}"),
                )
            })?);
        }
    }

    let mut body = raw[split + 4..].to_vec();
    if chunked {
        body = decode_chunked(&body)?;
    } else if let Some(length) = content_length {
        if body.len() < length {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("truncated HTTP body: got {} of {length} bytes", body.len()),
            ));
        }
        body.truncate(length);
    }
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn decode_chunked(mut data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = data.windows(2).position(|window| window == b"\r\n") else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated chunked body: missing chunk header",
            ));
        };
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid chunk size: {size_text:?}"),
            )
        })?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("truncated chunk: got {} of {size} bytes", data.len()),
            ));
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if !data.starts_with(b"\r\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated chunked body: missing chunk terminator",
            ));
        }
        data = &data[2..];
    }
}

pub fn get_json(port: u16, path: &str, timeout: Duration) -> Option<Value> {
    let response = request(port, "GET", path, None, timeout).ok()?;
    if response.status != 200 {
        return None;
    }
    let value = serde_json::from_str::<Value>(&response.body).ok()?;
    // 端口可能被别的 HTTP 服务占用：有 name 的响应必须是 ema2。
    // 没有该字段的响应（例如未来新增的端点）保持兼容。
    match value.get("name") {
        Some(Value::String(name)) if name == "ema2" => Some(value),
        Some(_) => None,
        None => Some(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_and_chunked() {
        let response =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello");

        let response = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(response.body, "hello world");
    }

    #[test]
    fn rejects_truncated_content_length_and_chunked_bodies() {
        let error =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

        let error = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n wo",
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

        let error =
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    /// 起一个只应答一次的 HTTP 服务，返回端口和线程句柄。
    fn serve_once(body: String) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).unwrap();
            socket.write_all(body.as_bytes()).unwrap();
            socket.flush().unwrap();
        });
        (port, handle)
    }

    #[test]
    fn get_json_accepts_ema2_health_and_rejects_other_services() {
        let (port, server) = serve_once(r#"{"name":"ema2","port":7744}"#.to_owned());
        let value = get_json(port, "/health", Duration::from_secs(2)).unwrap();
        assert_eq!(value["port"], 7744);
        server.join().unwrap();

        let (port, server) = serve_once(r#"{"name":"not-ema2","port":7744}"#.to_owned());
        assert!(
            get_json(port, "/health", Duration::from_secs(2)).is_none(),
            "a foreign service on the same port must not be treated as ema2"
        );
        server.join().unwrap();
    }

    /// 总预算必须约束“慢速滴灌”的对端，而不只是单次 read。
    #[test]
    fn slow_drip_response_hits_the_overall_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
            // 每 40ms 一个字节：单次 read 永远不超时，总预算必须兜住。
            for _ in 0..100 {
                if socket.write_all(b"x").is_err() {
                    return;
                }
                let _ = socket.flush();
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let error = request(port, "GET", "/health", None, Duration::from_millis(250)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        server.join().unwrap();
    }
}
