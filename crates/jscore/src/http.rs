//! Blocking HTTP GET and POST shared by the Lister scripts and the applets.
//!
//! `http://` and `https://` are accepted, every other scheme is refused.
//! TLS goes through the system `native-tls` (OpenSSL), already linked by the
//! FTP crate, with the platform root certificates.

use std::io::Read;
use std::time::Duration;

use serde_json::{json, Map, Value};
use ureq::config::Config;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// Largest response body that is read, in bytes.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Redirects followed before the request fails.
pub const MAX_REDIRECTS: u32 = 5;
/// Timeout used when the request does not give one, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;
/// Longest timeout a request may ask, in milliseconds: a larger `timeoutMs`
/// is clamped to it (not refused), so a script cannot keep a worker thread
/// busy for hours.
pub const MAX_TIMEOUT_MS: u64 = 120_000;

/// A GET or POST request.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    /// Absolute `http://` or `https://` URL.
    pub url: String,
    /// Extra request headers, name and value.
    pub headers: Vec<(String, String)>,
    /// Timeout for the whole request, in milliseconds.
    pub timeout_ms: u64,
    /// Request body, sent only by `post` (`get` ignores it).
    pub body: Option<String>,
}

/// The answer to a GET request; an HTTP error status is still a response.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers, name and value, in the order received.
    pub headers: Vec<(String, String)>,
    /// Body decoded as UTF-8, invalid bytes replaced by U+FFFD.
    pub body: String,
}

/// Builds a request from the JSON given by a script: `url` (required),
/// `headers` (object of strings), `body` (string, used by `post`) and
/// `timeoutMs` (positive number, clamped to [`MAX_TIMEOUT_MS`]).
pub fn request_from_json(payload: &Value) -> Result<HttpRequest, String> {
    let Some(url) = payload.get("url").and_then(Value::as_str) else {
        return Err("http request: missing string field `url`".to_string());
    };
    let mut headers = Vec::new();
    match payload.get("headers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                let Some(value) = value.as_str() else {
                    return Err(format!("http request: header `{name}` must be a string"));
                };
                headers.push((name.clone(), value.to_string()));
            }
        }
        Some(_) => return Err("http request: `headers` must be an object".to_string()),
    }
    let timeout_ms = match payload.get("timeoutMs") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_MS,
        Some(v) => match _timeout_ms(v) {
            Some(ms) if ms > 0 => ms.min(MAX_TIMEOUT_MS),
            _ => return Err("http request: `timeoutMs` must be a positive integer".to_string()),
        },
    };
    let body = match payload.get("body") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => return Err("http request: `body` must be a string".to_string()),
    };
    Ok(HttpRequest { url: url.to_string(), headers, timeout_ms, body })
}

/// Reads a timeout as whole milliseconds: an unsigned integer, or an integral
/// finite non-negative float (a JS number may reach serde_json as `1500.0`).
fn _timeout_ms(value: &Value) -> Option<u64> {
    if let Some(ms) = value.as_u64() { return Some(ms) }
    let f = value.as_f64()?;
    // 2^53 keeps the float to integer conversion exact.
    (f.is_finite() && f >= 0.0 && f.fract() == 0.0 && f <= 9_007_199_254_740_992.0).then_some(f as u64)
}

/// Converts a response to the JSON seen by scripts:
/// `{ status, ok, headers: { lowercase-name: value }, body }`.
pub fn response_to_json(resp: &HttpResponse) -> Value {
    let mut headers = Map::new();
    for (name, value) in &resp.headers {
        headers.insert(name.to_lowercase(), Value::String(value.clone()));
    }
    json!({
        "status": resp.status,
        "ok": (200..300).contains(&resp.status),
        "headers": headers,
        "body": resp.body,
    })
}

/// Performs the GET, following up to `MAX_REDIRECTS` redirects and reading at
/// most `MAX_BODY_BYTES` of body.
pub fn get(req: &HttpRequest) -> Result<HttpResponse, String> {
    send_with_limit(Method::Get, req, MAX_BODY_BYTES)
}

/// Performs the POST of `req.body` (empty when none), with the same limits as
/// `get`. The caller sets the `Content-Type` header.
pub fn post(req: &HttpRequest) -> Result<HttpResponse, String> {
    send_with_limit(Method::Post, req, MAX_BODY_BYTES)
}

/// The HTTP methods this module sends.
#[derive(Clone, Copy)]
enum Method {
    Get,
    Post,
}

impl Method {
    /// Lowercase name used in error messages.
    fn name(self) -> &'static str {
        match self {
            Method::Get => "get",
            Method::Post => "post",
        }
    }
}

/// Tells whether the URL starts with `http://` or `https://` (case-insensitive).
fn _has_allowed_scheme(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Sends `req` with `method` and reads at most `max_body` bytes of answer
/// (a parameter so tests need not send 8 MiB).
fn send_with_limit(method: Method, req: &HttpRequest, max_body: usize) -> Result<HttpResponse, String> {
    let what = method.name();
    if !_has_allowed_scheme(&req.url) {
        return Err(format!("http {what} {}: unsupported scheme, only http and https are allowed", req.url));
    }
    let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
    let config = Config::builder()
        .http_status_as_error(false)
        .max_redirects(MAX_REDIRECTS)
        .max_redirects_will_error(true)
        .timeout_global(Some(Duration::from_millis(req.timeout_ms)))
        .tls_config(tls)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let call_error = |e: ureq::Error| format!("http {what} {}: {e}", req.url);
    let mut response = match method {
        Method::Get => {
            let mut builder = agent.get(&req.url);
            for (name, value) in &req.headers {
                builder = builder.header(name, value);
            }
            builder.call().map_err(call_error)?
        }
        Method::Post => {
            let mut builder = agent.post(&req.url);
            for (name, value) in &req.headers {
                builder = builder.header(name, value);
            }
            builder.send(req.body.as_deref().unwrap_or("")).map_err(call_error)?
        }
    };
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    let mut bytes = Vec::new();
    let mut reader = response.body_mut().with_config().limit(max_body as u64 + 1).reader();
    reader
        .by_ref()
        .take(max_body as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("http {what} {}: reading the body: {e}", req.url))?;
    if bytes.len() > max_body {
        return Err(format!("http {what} {}: body larger than {max_body} bytes", req.url));
    }
    Ok(HttpResponse { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    /// Reads the request head so the client does not see a reset.
    fn _read_head(stream: &mut std::net::TcpStream) {
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
    }

    /// Serves `response` to one connection on 127.0.0.1 and returns the port.
    fn serve_once(response: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            _read_head(&mut stream);
            let _ = stream.write_all(&response);
        });
        port
    }

    /// Builds a plain HTTP/1.1 response with the given status line and body.
    fn _reply(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut out =
            format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// A request for the local server with a short timeout.
    fn _req(port: u16) -> HttpRequest {
        HttpRequest { url: format!("http://127.0.0.1:{port}/"), headers: vec![], timeout_ms: 2000, body: None }
    }

    #[test]
    fn get_returns_status_headers_and_body() {
        let port = serve_once(_reply("200 OK", "Content-Type: text/plain\r\n", b"hello"));
        let resp = get(&_req(port)).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "hello");
        let json = response_to_json(&resp);
        assert_eq!(json["ok"], true);
        assert_eq!(json["headers"]["content-type"], "text/plain");
        assert_eq!(json["body"], "hello");
    }

    #[test]
    fn a_404_is_a_response_not_an_error() {
        let port = serve_once(_reply("404 Not Found", "", b"nope"));
        let resp = get(&_req(port)).unwrap();
        assert_eq!(resp.status, 404);
        assert_eq!(response_to_json(&resp)["ok"], false);
    }

    #[test]
    fn refuses_schemes_other_than_http_and_https() {
        for url in ["file:///etc/passwd", "ftp://x/"] {
            let req = HttpRequest { url: url.to_string(), headers: vec![], timeout_ms: 1000, body: None };
            let err = get(&req).unwrap_err();
            assert!(err.contains("scheme"), "{err}");
        }
    }

    #[test]
    fn times_out_on_a_silent_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut req = _req(port);
        req.timeout_ms = 200;
        let start = Instant::now();
        assert!(get(&req).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(listener);
    }

    #[test]
    fn a_body_over_the_cap_is_an_error() {
        let port = serve_once(_reply("200 OK", "", &vec![b'a'; 4096]));
        let err = send_with_limit(Method::Get, &_req(port), 1024).unwrap_err();
        assert!(err.contains("1024"), "{err}");
    }

    #[test]
    fn a_redirect_loop_stops_at_the_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                _read_head(&mut stream);
                let location = format!("Location: http://127.0.0.1:{port}/\r\n");
                let _ = stream.write_all(&_reply("302 Found", &location, b""));
            }
        });
        let err = get(&_req(port)).unwrap_err();
        assert!(err.contains("redirects"), "{err}");
    }

    #[test]
    fn a_non_utf8_body_is_decoded_lossily() {
        let port = serve_once(_reply("200 OK", "", &[0xFF, 0xFE]));
        let resp = get(&_req(port)).unwrap();
        assert!(resp.body.contains('\u{FFFD}'));
    }

    #[test]
    fn request_from_json_rejects_a_missing_url() {
        let err = request_from_json(&json!({ "headers": {} })).unwrap_err();
        assert!(err.contains("url"), "{err}");
    }

    #[test]
    fn request_from_json_reads_headers_and_timeout() {
        let req = request_from_json(&json!({ "url": "http://x/", "headers": { "A": "b" }, "timeoutMs": 50 })).unwrap();
        assert_eq!(req.headers, vec![("A".to_string(), "b".to_string())]);
        assert_eq!(req.timeout_ms, 50);
    }

    #[test]
    fn request_from_json_accepts_an_integral_float_timeout() {
        let req = request_from_json(&json!({ "url": "http://x/", "timeoutMs": 1500.0 })).unwrap();
        assert_eq!(req.timeout_ms, 1500);
        for bad in [json!(-5.0), json!(0.0), json!(1.5), json!("9"), json!(1e30)] {
            assert!(request_from_json(&json!({ "url": "http://x/", "timeoutMs": bad })).is_err(), "{bad}");
        }
    }

    #[test]
    fn request_from_json_clamps_a_huge_timeout() {
        let req = request_from_json(&json!({ "url": "http://x/", "timeoutMs": MAX_TIMEOUT_MS })).unwrap();
        assert_eq!(req.timeout_ms, MAX_TIMEOUT_MS, "the cap itself is kept");
        for huge in [json!(MAX_TIMEOUT_MS + 1), json!(86_400_000), json!(u64::MAX), json!(9_007_199_254_740_992.0)] {
            let req = request_from_json(&json!({ "url": "http://x/", "timeoutMs": huge })).unwrap();
            assert_eq!(req.timeout_ms, MAX_TIMEOUT_MS, "{huge} is clamped");
        }
    }

    /// Serves one connection: replies 200 with the received request (head and
    /// body) echoed back as the body. Returns the port.
    fn serve_echo() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let Ok(n) = stream.read(&mut buf) else { return };
                if n == 0 { return }
                seen.extend_from_slice(&buf[..n]);
                let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&seen[..end]).to_lowercase();
                let want = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                if seen.len() >= end + 4 + want { break }
            }
            let _ = stream.write_all(&_reply("200 OK", "", &seen));
        });
        port
    }

    #[test]
    fn post_sends_method_headers_and_body() {
        let port = serve_echo();
        let mut req = _req(port);
        req.headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        req.body = Some("{\"a\":1}".to_string());
        let resp = post(&req).unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.body.starts_with("POST / HTTP/1.1"), "{}", resp.body);
        assert!(resp.body.to_lowercase().contains("content-type: application/json"), "{}", resp.body);
        assert!(resp.body.ends_with("{\"a\":1}"), "{}", resp.body);
    }

    #[test]
    fn post_without_a_body_sends_an_empty_one() {
        let port = serve_echo();
        let resp = post(&_req(port)).unwrap();
        assert!(resp.body.starts_with("POST / HTTP/1.1"), "{}", resp.body);
    }

    #[test]
    fn get_does_not_send_the_body() {
        let port = serve_echo();
        let mut req = _req(port);
        req.body = Some("secret".to_string());
        let resp = get(&req).unwrap();
        assert!(resp.body.starts_with("GET / HTTP/1.1"), "{}", resp.body);
        assert!(!resp.body.contains("secret"), "{}", resp.body);
    }

    #[test]
    fn post_401_is_a_response_not_an_error() {
        let port = serve_once(_reply("401 Unauthorized", "", b"{}"));
        let resp = post(&_req(port)).unwrap();
        assert_eq!(resp.status, 401);
        assert_eq!(response_to_json(&resp)["ok"], false);
    }

    #[test]
    fn post_refuses_schemes_other_than_http_and_https() {
        let req = HttpRequest { url: "ftp://x/".to_string(), headers: vec![], timeout_ms: 1000, body: None };
        let err = post(&req).unwrap_err();
        assert!(err.contains("scheme") && err.contains("post"), "{err}");
    }

    #[test]
    fn post_error_does_not_leak_the_authorization_header() {
        let req = HttpRequest {
            url: "http://127.0.0.1:1/".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer fatto_SECRET".to_string())],
            timeout_ms: 500,
            body: None,
        };
        let err = post(&req).unwrap_err();
        assert!(!err.contains("fatto_SECRET"), "{err}");
    }

    #[test]
    fn request_from_json_reads_the_body() {
        let req = request_from_json(&json!({ "url": "http://x/", "body": "hi" })).unwrap();
        assert_eq!(req.body.as_deref(), Some("hi"));
        let none = request_from_json(&json!({ "url": "http://x/" })).unwrap();
        assert_eq!(none.body, None);
        assert!(request_from_json(&json!({ "url": "http://x/", "body": 5 })).is_err());
    }
}
