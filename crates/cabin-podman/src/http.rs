//! Just enough HTTP/1.1 to talk to Podman over a UNIX socket.
//!
//! Podman is a well-behaved local server, so this handles the cases that
//! actually occur: a normal response with `Content-Length`, a chunked
//! response, and a streaming response that never ends (the event feed).

use crate::error::{Error, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends one request and reads the whole response.
pub async fn request(
    socket: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Response> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| Error::SocketUnavailable {
            path: socket.to_string(),
            source,
        })?;
    let mut reader = BufReader::new(stream);

    write_request(reader.get_mut(), method, path, body).await?;

    let status = read_status_line(&mut reader).await?;
    let headers = read_headers(&mut reader).await?;
    let body = read_body(&mut reader, &headers).await?;

    Ok(Response { status, body })
}

/// Sends a request and hands each line of the response body to `on_line`.
/// Used for the event feed, which never completes.
pub async fn stream_lines<F>(socket: &str, path: &str, mut on_line: F) -> Result<()>
where
    F: FnMut(&str),
{
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| Error::SocketUnavailable {
            path: socket.to_string(),
            source,
        })?;
    let mut reader = BufReader::new(stream);

    write_request(reader.get_mut(), "GET", path, None).await?;

    let status = read_status_line(&mut reader).await?;
    let headers = read_headers(&mut reader).await?;
    if status >= 400 {
        let body = read_body(&mut reader, &headers).await?;
        return Err(Error::Api {
            status,
            message: String::from_utf8_lossy(&body).into_owned(),
        });
    }

    let chunked = is_chunked(&headers);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        // In a chunked stream the size lines are interleaved with the payload.
        // Podman sends one JSON object per chunk, so skipping anything that is
        // not JSON is enough here.
        if trimmed.is_empty() || (chunked && !trimmed.starts_with('{')) {
            continue;
        }
        on_line(trimmed);
    }
}

async fn write_request(
    stream: &mut UnixStream,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<()> {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: d\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some(b) = body {
        stream.write_all(b).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn read_status_line(reader: &mut BufReader<UnixStream>) -> Result<u16> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Protocol(format!("bad status line: {line:?}")))
}

async fn read_headers(reader: &mut BufReader<UnixStream>) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_lowercase(), value.trim().to_string()));
        }
    }
    Ok(headers)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn is_chunked(headers: &[(String, String)]) -> bool {
    header(headers, "transfer-encoding").is_some_and(|v| v.contains("chunked"))
}

async fn read_body(
    reader: &mut BufReader<UnixStream>,
    headers: &[(String, String)],
) -> Result<Vec<u8>> {
    if is_chunked(headers) {
        return read_chunked(reader).await;
    }
    if let Some(len) = header(headers, "content-length").and_then(|v| v.parse::<usize>().ok()) {
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        return Ok(buf);
    }
    // "Connection: close" with no length: read to the end.
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    Ok(buf)
}

async fn read_chunked(reader: &mut BufReader<UnixStream>) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line).await? == 0 {
            break;
        }
        let size = usize::from_str_radix(size_line.trim(), 16)
            .map_err(|_| Error::Protocol(format!("bad chunk size: {size_line:?}")))?;
        if size == 0 {
            break;
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk).await?;
        body.extend_from_slice(&chunk);
        // Each chunk is followed by CRLF.
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).await?;
    }
    Ok(body)
}
