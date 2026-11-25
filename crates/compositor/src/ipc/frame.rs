//! Framing of the private IPC: a 4-byte little-endian length, then that
//! many bytes of JSON (the Wayfire IPC framing).

/// Size of the length header.
pub const HEADER_LEN: usize = 4;

/// One framed message: the length header of `json`, then `json`. The
/// callers bound what they send far below 4 GiB (`IPC_MAX_PENDING`); a
/// longer `json` would get a saturated header and be refused by the peer.
pub fn encode(json: &[u8]) -> Vec<u8> {
    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER_LEN + json.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(json);
    out
}

/// Takes one whole message out of the front of `buf`: `Ok(None)` while it
/// is incomplete, `Err` when its length is over `max` (checked as soon as
/// the header is in, so a peer cannot make the buffer grow past it).
pub fn decode(buf: &mut Vec<u8>, max: usize) -> Result<Option<Vec<u8>>, String> {
    let Some(header) = buf.first_chunk::<HEADER_LEN>() else { return Ok(None) };
    let len = usize::try_from(u32::from_le_bytes(*header)).unwrap_or(usize::MAX);
    if len > max {
        return Err(format!("message of {len} bytes is over the limit of {max}"));
    }
    let end = HEADER_LEN.saturating_add(len);
    if buf.len() < end { return Ok(None); }
    let message = buf.get(HEADER_LEN..end).map(<[u8]>::to_vec).unwrap_or_default();
    buf.drain(..end);
    Ok(Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips() {
        let mut buf = encode(br#"{"method":"windows/list"}"#);
        assert_eq!(&buf[..HEADER_LEN], &25u32.to_le_bytes());
        assert_eq!(decode(&mut buf, 1024).unwrap().as_deref(), Some(&br#"{"method":"windows/list"}"#[..]));
        assert!(buf.is_empty());
        assert_eq!(decode(&mut buf, 1024).unwrap(), None);
    }

    #[test]
    fn a_message_split_in_two_reads_comes_out_whole() {
        let whole = encode(b"[1,2,3]");
        let mut buf = whole[..2].to_vec();
        assert_eq!(decode(&mut buf, 1024).unwrap(), None);
        buf.extend_from_slice(&whole[2..6]);
        assert_eq!(decode(&mut buf, 1024).unwrap(), None);
        buf.extend_from_slice(&whole[6..]);
        assert_eq!(decode(&mut buf, 1024).unwrap().as_deref(), Some(&b"[1,2,3]"[..]));
    }

    #[test]
    fn two_messages_in_one_read_come_out_in_order() {
        let mut buf = encode(b"1");
        buf.extend(encode(b"22"));
        assert_eq!(decode(&mut buf, 1024).unwrap().as_deref(), Some(&b"1"[..]));
        assert_eq!(decode(&mut buf, 1024).unwrap().as_deref(), Some(&b"22"[..]));
        assert_eq!(decode(&mut buf, 1024).unwrap(), None);
    }

    #[test]
    fn a_length_over_max_is_an_error() {
        let mut buf = u32::MAX.to_le_bytes().to_vec();
        assert!(decode(&mut buf, 1024).is_err());
        let mut buf = encode(&[b'x'; 11]);
        assert!(decode(&mut buf, 10).is_err());
        let mut buf = encode(&[b'x'; 10]);
        assert!(decode(&mut buf, 10).unwrap().is_some());
    }
}
