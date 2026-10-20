//! 极简本机 HTTP/1.1 客户端。
//!
//! CLI 是短命进程，只需要向 `127.0.0.1:7744` 发一个请求；不引入 reqwest，
//! 保持单 exe 体积。响应可能是普通 `Content-Length`，也可能是 axum 的
//! `Transfer-Encoding: chunked`，两种都处理。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

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
    let connect_timeout = timeout.min(Duration::from_secs(3));
    let mut stream = TcpStream::connect_timeout(&address, connect_timeout)?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let _ = stream.set_nodelay(true);

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nUser-Agent: ema2/{}\r\nConnection: close\r\n",
        env!("CARGO_PKG_VERSION")
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

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_response(&raw)
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
            content_length = value.parse().ok();
        }
    }

    let mut body = raw[split + 4..].to_vec();
    if chunked {
        body = decode_chunked(&body);
    } else if let Some(length) = content_length {
        body.truncate(length);
    }
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn decode_chunked(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(line_end) = data.windows(2).position(|window| window == b"\r\n") {
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            break;
        };
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        if data.len() < size {
            out.extend_from_slice(data);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}

pub fn get_json(port: u16, path: &str, timeout: Duration) -> Option<Value> {
    let response = request(port, "GET", path, None, timeout).ok()?;
    if response.status != 200 {
        return None;
    }
    serde_json::from_str(&response.body).ok()
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
}
