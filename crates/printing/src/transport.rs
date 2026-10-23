//! One HTTP POST of an IPP request over the CUPS domain socket. The socket
//! also tells CUPS who is asking (peer credentials), so no password is sent.

use crate::model::PrintError;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where CUPS listens.
pub const CUPS_SOCKET: &str = "/run/cups/cups.sock";
/// Where older systems put it.
pub const CUPS_SOCKET_OLD: &str = "/var/run/cups/cups.sock";
/// Longest wait for CUPS to read or answer, in seconds.
pub const SOCKET_TIMEOUT_SECS: u64 = 5;

/// The CUPS socket: the first of the two that exists, else the usual one.
pub fn socket_path() -> PathBuf {
    [CUPS_SOCKET, CUPS_SOCKET_OLD].iter().map(PathBuf::from).find(|p| p.exists()).unwrap_or_else(|| PathBuf::from(CUPS_SOCKET))
}

/// Send `body` to CUPS on `socket` and return the body of the answer.
pub fn post(socket: &Path, body: &[u8]) -> Result<Vec<u8>, PrintError> {
    let fail = |e: std::io::Error| PrintError::new(format!("CUPS is not available ({}): {e}", socket.display()));
    let mut stream = UnixStream::connect(socket).map_err(fail)?;
    let timeout = Some(Duration::from_secs(SOCKET_TIMEOUT_SECS));
    stream.set_read_timeout(timeout).map_err(fail)?;
    stream.set_write_timeout(timeout).map_err(fail)?;
    let head = format!("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).map_err(fail)?;
    stream.write_all(body).map_err(fail)?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).map_err(fail)?;
    _http_body(&answer)
}

/// The body of an HTTP answer, when its status is 200.
fn _http_body(answer: &[u8]) -> Result<Vec<u8>, PrintError> {
    let split = answer.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| PrintError::new("CUPS sent an answer without headers"))?;
    let head = String::from_utf8_lossy(&answer[..split]);
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") { return Err(PrintError::new(format!("CUPS refused the request: {status}"))); }
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    let rest = &answer[split + 4..];
    if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) { return _dechunk(rest); }
    let mut body = rest.to_vec();
    if let Some(n) = header("content-length").and_then(|v| v.parse::<usize>().ok()) { body.truncate(n); }
    Ok(body)
}

/// The data of a chunked body: `size in hex CRLF data CRLF`, until size 0.
fn _dechunk(mut rest: &[u8]) -> Result<Vec<u8>, PrintError> {
    let cut = || PrintError::new("CUPS sent a chunked answer that is cut");
    let mut body = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|w| w == b"\r\n").ok_or_else(cut)?;
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim().to_string();
        let size = usize::from_str_radix(&size_text, 16).map_err(|_| PrintError::new(format!("CUPS sent a bad chunk size {size_text:?}")))?;
        rest = &rest[line_end + 2..];
        if size == 0 { return Ok(body); }
        if rest.len() < size { return Err(cut()); }
        body.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).ok_or_else(cut)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    /// A socket in a temporary folder that answers one request with `reply`
    /// and hands back what it received.
    fn serve(reply: &'static [u8]) -> (tempfile::TempDir, PathBuf, std::thread::JoinHandle<Vec<u8>>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cups.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = vec![0u8; 4096];
            let n = stream.read(&mut got).unwrap();
            got.truncate(n);
            stream.write_all(reply).unwrap();
            got
        });
        (dir, path, handle)
    }

    #[test]
    fn transport_reports_a_missing_socket() {
        let e = post(Path::new("/nonexistent/cups.sock"), b"").unwrap_err();
        assert!(e.0.contains("CUPS is not available"), "{e}");
        assert!(e.0.contains("/nonexistent/cups.sock"), "{e}");
    }

    #[test]
    fn transport_reads_the_body() {
        let (_dir, path, handle) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcdef");
        assert_eq!(post(&path, b"xyz").unwrap(), b"abc");
        let got = String::from_utf8(handle.join().unwrap()).unwrap();
        assert!(got.starts_with("POST / HTTP/1.1\r\n"), "{got}");
        assert!(got.contains("Content-Type: application/ipp\r\n"));
        assert!(got.contains("Content-Length: 3\r\n"));
        assert!(got.ends_with("\r\n\r\nxyz"));
    }

    #[test]
    fn transport_decodes_chunked_bodies() {
        let (_dir, path, _handle) = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\na\r\n0123456789\r\n0\r\n\r\n");
        assert_eq!(post(&path, b"").unwrap(), b"abc0123456789");
    }

    #[test]
    fn transport_refuses_http_errors() {
        let (_dir, path, _handle) = serve(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        let e = post(&path, b"").unwrap_err();
        assert!(e.0.contains("403"), "{e}");
    }
}
