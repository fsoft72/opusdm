//! Length-prefixed framing: a 4-byte big-endian length, then that many bytes.
//! Bodies are JSON. Blocking I/O only.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{self, Read, Write};

/// Largest frame body accepted by [`read_frame`], in bytes.
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// Write one frame: the length prefix, the bytes, then flush.
pub fn write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame body too large"))?;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame body exceeds MAX_FRAME",
        ));
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incoming frame exceeds MAX_FRAME",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Serialize `msg` as JSON and write it as one frame.
pub fn send<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, &bytes)
}

/// Read one frame and deserialize it from JSON. `Ok(None)` on clean EOF.
pub fn recv<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    match read_frame(r)? {
        Some(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(io::Error::other)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A reader that yields at most `chunk` bytes per `read`, to exercise
    /// partial-read handling inside `read_frame`.
    struct Drip<'a> {
        data: &'a [u8],
        pos: usize,
        chunk: usize,
    }
    impl std::io::Read for Drip<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.data[self.pos..];
            let n = remaining.len().min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&remaining[..n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn roundtrip_single_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"hello"[..]));
        assert_eq!(read_frame(&mut cur).unwrap(), None);
    }

    #[test]
    fn two_frames_in_one_buffer() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"aa").unwrap();
        write_frame(&mut buf, b"bbbb").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"aa"[..]));
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(&b"bbbb"[..]));
    }

    #[test]
    fn partial_reads_are_reassembled() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"payload-across-drips").unwrap();
        let mut drip = Drip { data: &buf, pos: 0, chunk: 1 };
        assert_eq!(
            read_frame(&mut drip).unwrap().as_deref(),
            Some(&b"payload-across-drips"[..])
        );
    }

    #[test]
    fn oversized_length_is_rejected() {
        let mut cur = Cursor::new((MAX_FRAME + 1).to_be_bytes().to_vec());
        let err = read_frame(&mut cur).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn typed_send_recv_roundtrip() {
        let mut buf = Vec::new();
        send(&mut buf, &vec![1u32, 2, 3]).unwrap();
        let mut cur = Cursor::new(buf);
        let got: Option<Vec<u32>> = recv(&mut cur).unwrap();
        assert_eq!(got, Some(vec![1, 2, 3]));
    }
}
