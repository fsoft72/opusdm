//! Framing of the JSON IPC sockets of Wayfire and of opusdm-compositor:
//! every message is a 4 byte little-endian length followed by that many
//! bytes of JSON. A request is `{"method": ..., "data": {...}}`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::{json, Value};

/// `body` framed for the socket: its length (4 bytes, little endian), then it.
pub(crate) fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// Send request `method` with `data` on `stream`.
pub(crate) fn send(stream: &mut UnixStream, method: &str, data: Value) -> Result<(), String> {
    let body = json!({ "method": method, "data": data }).to_string();
    stream.write_all(&frame(body.as_bytes())).map_err(|e| format!("{method}: write failed: {e}"))
}

/// Read one message of at most `max` bytes from `stream`.
pub(crate) fn receive(stream: &mut UnixStream, max: usize) -> Result<Value, String> {
    receive_from(stream, max, "the compositor")
}

/// Read one message of at most `max` bytes from `stream`; `peer` names the
/// other end in the error of a body that is not JSON.
pub(crate) fn receive_from(stream: &mut UnixStream, max: usize, peer: &str) -> Result<Value, String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).map_err(|e| format!("read failed: {e}"))?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max { return Err(format!("message of {len} bytes refused")); }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).map_err(|e| format!("read failed: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("bad JSON from {peer}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, "m", json!({ "x": 1 })).unwrap();
        assert_eq!(receive(&mut b, 1024).unwrap(), json!({ "method": "m", "data": { "x": 1 } }));
    }

    #[test]
    fn a_message_over_max_is_refused() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(&frame(b"[1,2,3]")).unwrap();
        assert_eq!(receive(&mut b, 6).unwrap_err(), "message of 7 bytes refused");
    }

    #[test]
    fn the_json_error_names_the_peer() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(&frame(b"nope")).unwrap();
        assert!(receive_from(&mut b, 1024, "Wayfire").unwrap_err().starts_with("bad JSON from Wayfire: "));
    }
}
