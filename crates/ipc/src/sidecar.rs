//! Wire format of the Sidecar viewer: the Lister writes the paths to show to
//! the standard input of `opusdm-viewer --sidecar`. Every path ends with a NUL
//! byte and an empty path ends the update, so any file name fits.

use std::ffi::OsString;
use std::io::{self, BufRead};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

/// Command line flag that starts `opusdm-viewer` as a Sidecar.
pub const SIDECAR_FLAG: &str = "--sidecar";

/// One update with `paths`, ready to write.
pub fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_os_str().as_bytes());
        out.push(0);
    }
    out.push(0);
    out
}

/// Read the next update; `None` once the writer is gone (a half written update
/// at the end is dropped).
pub fn read_update(reader: &mut impl BufRead) -> io::Result<Option<Vec<PathBuf>>> {
    let mut paths = Vec::new();
    loop {
        let mut buf = Vec::new();
        if reader.read_until(0, &mut buf)? == 0 { return Ok(None); }
        if buf.pop() != Some(0) { return Ok(None); }
        if buf.is_empty() { return Ok(Some(paths)); }
        paths.push(PathBuf::from(OsString::from_vec(buf)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn updates_round_trip_in_order() {
        let mut bytes = encode(&paths(&["/a/b.png", "/a/line\nbreak.jpg"]));
        bytes.extend(encode(&[]));
        bytes.extend(encode(&paths(&["/c"])));
        let mut reader = io::Cursor::new(bytes);
        assert_eq!(read_update(&mut reader).unwrap(), Some(paths(&["/a/b.png", "/a/line\nbreak.jpg"])));
        assert_eq!(read_update(&mut reader).unwrap(), Some(Vec::new()));
        assert_eq!(read_update(&mut reader).unwrap(), Some(paths(&["/c"])));
        assert_eq!(read_update(&mut reader).unwrap(), None);
    }

    #[test]
    fn a_truncated_update_is_dropped() {
        let mut reader = io::Cursor::new(b"/a.png\0/b.p".to_vec());
        assert_eq!(read_update(&mut reader).unwrap(), None);
        let mut reader = io::Cursor::new(b"/a.png\0".to_vec());
        assert_eq!(read_update(&mut reader).unwrap(), None);
    }
}
