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
pub async fn stream_lines<F>(socket: &str, path: &str, on_line: F) -> Result<()>
where
    F: FnMut(&str),
{
    stream_request(socket, "GET", path, None, on_line).await
}

/// The body of a request that is not JSON.
pub struct RawBody<'a> {
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

/// Sends a request and hands each line of the response body to `on_line`.
///
/// Building an image needs this: the request carries a tar archive and the
/// reply is a stream of progress lines that must be shown as they arrive
/// rather than collected and shown at the end.
pub async fn stream_request<F>(
    socket: &str,
    method: &str,
    path: &str,
    body: Option<RawBody<'_>>,
    mut on_line: F,
) -> Result<()>
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

    write_raw_request(reader.get_mut(), method, path, body).await?;

    let status = read_status_line(&mut reader).await?;
    let headers = read_headers(&mut reader).await?;
    if status >= 400 {
        let body = read_body(&mut reader, &headers).await?;
        return Err(Error::Api {
            status,
            message: crate::client::extract_message(&body),
        });
    }

    let chunked = is_chunked(&headers);
    // A chunked stream ends with a zero size line. A connection that drops
    // before it looks the same as a clean end unless this is tracked, and a
    // build cut short would then be reported as a success.
    let mut finished = false;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            if chunked && !finished {
                return Err(Error::Protocol(
                    "the stream ended before Podman finished it".to_string(),
                ));
            }
            return Ok(());
        }
        let trimmed = line.trim();
        // A payload line is a JSON object, so it can never be a bare "0".
        if chunked && trimmed == "0" {
            finished = true;
            continue;
        }
        // In a chunked stream the size lines are interleaved with the payload.
        // Podman sends JSON objects, so skipping anything that is not one is
        // enough to drop the sizes.
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
    let body = body.map(|bytes| RawBody {
        content_type: "application/json",
        bytes,
    });
    write_raw_request(stream, method, path, body).await
}

async fn write_raw_request(
    stream: &mut UnixStream,
    method: &str,
    path: &str,
    body: Option<RawBody<'_>>,
) -> Result<()> {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: d\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(b) = &body {
        head.push_str(&format!("Content-Type: {}\r\n", b.content_type));
        head.push_str(&format!("Content-Length: {}\r\n", b.bytes.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some(b) = &body {
        stream.write_all(b.bytes).await?;
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
            // No zero size chunk, so the body is incomplete.
            return Err(Error::Protocol("the response ended early".to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    /// Serves one canned reply on a socket in a fresh directory, and returns
    /// the directory and the socket path. The reply is written whole and the
    /// connection then closes, so a reply without its end marker is a
    /// truncated one.
    fn serve_once(tag: &str, reply: Vec<u8>) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("cabin-http-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("podman.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            // Read the request head first, so the client is not cut off
            // while it writes.
            let mut line = String::new();
            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                if line == "\r\n" {
                    break;
                }
                line.clear();
            }
            reader.get_mut().write_all(&reply).await.unwrap();
            reader.get_mut().shutdown().await.ok();
        });
        let path = socket.display().to_string();
        (dir, path)
    }

    const HEAD: &str = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";

    #[tokio::test]
    async fn a_complete_chunked_stream_succeeds() {
        let reply = format!("{HEAD}d\r\n{{\"stream\":1}}\n\r\n0\r\n\r\n");
        let (dir, socket) = serve_once("complete", reply.into_bytes());
        let mut lines = Vec::new();
        let result = stream_lines(&socket, "/build", |l| lines.push(l.to_string())).await;
        std::fs::remove_dir_all(&dir).ok();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(lines, vec!["{\"stream\":1}"]);
    }

    #[tokio::test]
    async fn a_chunked_stream_cut_short_is_an_error() {
        // A build whose connection drops must not look like a finished one.
        let reply = format!("{HEAD}d\r\n{{\"stream\":1}}\n\r\n");
        let (dir, socket) = serve_once("truncated", reply.into_bytes());
        let result = stream_lines(&socket, "/build", |_| {}).await;
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    }

    #[tokio::test]
    async fn a_chunked_body_cut_short_is_an_error() {
        let reply = format!("{HEAD}2\r\n{{}}\r\n");
        let (dir, socket) = serve_once("body", reply.into_bytes());
        let result = request(&socket, "GET", "/x", None).await;
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(result, Err(Error::Protocol(_))));
    }

    #[tokio::test]
    async fn an_error_reply_carries_the_bare_message() {
        // The user should read Podman's message, not its JSON.
        let body = r#"{"cause":"x","message":"image not known","response":404}"#;
        let reply = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (dir, socket) = serve_once("error", reply.into_bytes());
        let result = stream_lines(&socket, "/build", |_| {}).await;
        std::fs::remove_dir_all(&dir).ok();
        match result {
            Err(Error::Api { message, .. }) => assert_eq!(message, "image not known"),
            other => panic!("expected an API error, got {other:?}"),
        }
    }
}
