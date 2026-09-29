//! Join (concatenate files) and Split (cut a file into numbered parts).

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Streaming buffer size.
const BUF: usize = 256 * 1024;

/// Fewest digits of a part number (`.001`); grows for more than 999 parts.
const MIN_PART_DIGITS: usize = 3;

/// Shortest gap between two progress events, so a big file does not flood the channel.
const PROGRESS_GAP: Duration = Duration::from_millis(50);

/// A `Done` for a job that failed as a whole.
fn failed(path: &Path, msg: String, copied: u64, cancelled: bool) -> Progress {
    Progress::Done { copied, skipped: 0, cancelled, errors: vec![(path.to_path_buf(), msg)] }
}

/// Emits throttled `Progress::File` events for one running job.
struct Reporter<'a> {
    tx: &'a Sender<Progress>,
    total_bytes: u64,
    done_bytes: u64,
    files_total: u64,
    files_done: u64,
    last: Instant,
}

impl<'a> Reporter<'a> {
    fn new(tx: &'a Sender<Progress>, total_bytes: u64, files_total: u64) -> Self {
        let _ = tx.send(Progress::Scanning { files: files_total, bytes: total_bytes });
        Self { tx, total_bytes, done_bytes: 0, files_total, files_done: 0, last: Instant::now() }
    }

    /// Account for `n` written bytes of `path`; emit when the gap has passed or `force`.
    fn advance(&mut self, path: &Path, n: u64, force: bool) {
        self.done_bytes += n;
        if !force && self.last.elapsed() < PROGRESS_GAP {
            return;
        }
        self.last = Instant::now();
        let _ = self.tx.send(Progress::File {
            path: path.to_path_buf(),
            done_bytes: self.done_bytes,
            total_bytes: self.total_bytes,
            files_done: self.files_done,
            files_total: self.files_total,
            file_bytes_done: 0,
            file_bytes_total: 0,
        });
    }
}

/// Copy exactly `len` bytes (or to EOF when `None`) from `reader` to `writer`.
/// Returns `Interrupted` on cancel.
fn pump(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    len: Option<u64>,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<u64> {
    let mut buf = vec![0u8; BUF];
    let mut left = len.unwrap_or(u64::MAX);
    let mut total = 0u64;
    while left > 0 {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let want = buf.len().min(left.min(usize::MAX as u64) as usize);
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        on_bytes(n as u64);
        left -= n as u64;
        total += n as u64;
    }
    Ok(total)
}

/// Concatenate `srcs`, in the given order, into the new file `dest_dir/target`.
///
/// Every source must be a regular file, and `target` must not exist; both are
/// checked before anything is written. A partial target is removed on error
/// or cancel.
pub fn run_join(
    srcs: &[PathBuf],
    dest_dir: &Path,
    target: &str,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let out_path = dest_dir.join(target);
    let mut total = 0u64;
    for src in srcs {
        match std::fs::metadata(src) {
            Ok(m) if m.is_file() => total += m.len(),
            Ok(_) => return failed(src, "not a regular file".into(), 0, false),
            Err(e) => return failed(src, e.to_string(), 0, false),
        }
    }
    let mut out = match OpenOptions::new().write(true).create_new(true).open(&out_path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return failed(&out_path, format!("{} already exists", out_path.display()), 0, false)
        }
        Err(e) => return failed(&out_path, e.to_string(), 0, false),
    };

    let mut rep = Reporter::new(tx, total, srcs.len() as u64);
    let mut joined = 0u64;
    for src in srcs {
        let result = File::open(src).and_then(|mut f| {
            pump(&mut f, &mut out, None, cancel, &mut |n| rep.advance(src, n, false))
        });
        match result {
            Ok(_) => {
                joined += 1;
                rep.files_done = joined;
            }
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&out_path);
                let cancelled = e.kind() == io::ErrorKind::Interrupted;
                return failed(src, e.to_string(), 0, cancelled).with_no_error_if(cancelled);
            }
        }
    }
    if let Err(e) = out.flush() {
        return failed(&out_path, e.to_string(), joined, false);
    }
    rep.advance(&out_path, 0, true);
    Progress::Done { copied: joined, skipped: 0, cancelled: false, errors: Vec::new() }
}

impl Progress {
    /// A cancelled job reports no error entry, only the flag.
    fn with_no_error_if(self, cancelled: bool) -> Progress {
        match self {
            Progress::Done { copied, skipped, cancelled: c, errors } => Progress::Done {
                copied,
                skipped,
                cancelled: c,
                errors: if cancelled { Vec::new() } else { errors },
            },
            other => other,
        }
    }
}

/// The names of the parts of `name` for `count` parts: `name.001`, ...
pub fn part_names(name: &str, count: u64) -> Vec<String> {
    let digits = MIN_PART_DIGITS.max(count.to_string().len());
    (1..=count).map(|i| format!("{name}.{i:0digits$}")).collect()
}

/// Split the single regular file in `srcs` into parts of `chunk_bytes`
/// inside `dest_dir`. No part may exist already (checked up front); the
/// parts written so far are removed on error or cancel.
pub fn run_split(
    srcs: &[PathBuf],
    dest_dir: &Path,
    chunk_bytes: u64,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let [src] = srcs else {
        return failed(dest_dir, "select exactly one file to split".into(), 0, false);
    };
    if chunk_bytes == 0 {
        return failed(src, "part size must be greater than zero".into(), 0, false);
    }
    let len = match std::fs::metadata(src) {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => return failed(src, "not a regular file".into(), 0, false),
        Err(e) => return failed(src, e.to_string(), 0, false),
    };
    if len == 0 {
        return failed(src, "the file is empty".into(), 0, false);
    }
    let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return failed(src, "source has no file name".into(), 0, false);
    };
    let count = len.div_ceil(chunk_bytes);
    let parts: Vec<PathBuf> = part_names(&name, count).iter().map(|p| dest_dir.join(p)).collect();
    if let Some(existing) = parts.iter().find(|p| std::fs::symlink_metadata(p).is_ok()) {
        return failed(existing, format!("{} already exists", existing.display()), 0, false);
    }

    let mut reader = match File::open(src) {
        Ok(f) => f,
        Err(e) => return failed(src, e.to_string(), 0, false),
    };
    let mut rep = Reporter::new(tx, len, count);
    let mut written: Vec<&PathBuf> = Vec::new();
    for part in &parts {
        let result = OpenOptions::new().write(true).create_new(true).open(part).and_then(|mut out| {
            written.push(part);
            pump(&mut reader, &mut out, Some(chunk_bytes), cancel, &mut |n| rep.advance(part, n, false))
        });
        if let Err(e) = result {
            for p in &written {
                let _ = std::fs::remove_file(p);
            }
            let cancelled = e.kind() == io::ErrorKind::Interrupted;
            return failed(part, e.to_string(), 0, cancelled).with_no_error_if(cancelled);
        }
        rep.files_done += 1;
    }
    rep.advance(src, 0, true);
    Progress::Done { copied: count, skipped: 0, cancelled: false, errors: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::channel;

    fn done(p: Progress) -> (u64, bool, Vec<(PathBuf, String)>) {
        match p {
            Progress::Done { copied, cancelled, errors, .. } => (copied, cancelled, errors),
            _ => panic!("not Done"),
        }
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn part_names_grow_their_width() {
        assert_eq!(part_names("f", 2), vec!["f.001", "f.002"]);
        assert_eq!(part_names("f", 1000).last().unwrap(), "f.1000");
        assert_eq!(part_names("f", 1000)[0], "f.0001");
    }

    #[test]
    fn split_then_join_gives_back_the_same_bytes() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        let data = pattern(10_000);
        fs::write(dir.join("big.bin"), &data).unwrap();
        let (tx, _rx) = channel();

        let (parts, _, errors) = done(run_split(&[dir.join("big.bin")], dir, 4096, &tx, &CancelToken::new()));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(parts, 3);
        assert_eq!(fs::metadata(dir.join("big.bin.001")).unwrap().len(), 4096);
        assert_eq!(fs::metadata(dir.join("big.bin.003")).unwrap().len(), 10_000 - 2 * 4096);

        let srcs: Vec<PathBuf> = (1..=3).map(|i| dir.join(format!("big.bin.00{i}"))).collect();
        let (joined, _, errors) = done(run_join(&srcs, dir, "back.bin", &tx, &CancelToken::new()));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(joined, 3);
        assert_eq!(fs::read(dir.join("back.bin")).unwrap(), data);
    }

    #[test]
    fn split_refuses_bad_input_without_writing() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        fs::write(dir.join("f"), "abc").unwrap();
        fs::write(dir.join("empty"), "").unwrap();
        fs::write(dir.join("f.001"), "taken").unwrap();
        fs::create_dir(dir.join("d")).unwrap();
        let (tx, _rx) = channel();
        let c = CancelToken::new();

        for srcs in [vec![dir.join("empty")], vec![dir.join("d")], vec![dir.join("f"), dir.join("empty")], vec![dir.join("f")]] {
            let (n, _, errors) = done(run_split(&srcs, dir, 2, &tx, &c));
            assert_eq!(n, 0);
            assert_eq!(errors.len(), 1, "{srcs:?}");
        }
        assert_eq!(fs::read_to_string(dir.join("f.001")).unwrap(), "taken");
        assert!(!dir.join("f.002").exists());
        let (_, _, errors) = done(run_split(&[dir.join("f")], dir, 0, &tx, &c));
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn join_refuses_existing_target_and_non_files() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        fs::write(dir.join("a"), "A").unwrap();
        fs::write(dir.join("out"), "keep").unwrap();
        fs::create_dir(dir.join("d")).unwrap();
        let (tx, _rx) = channel();
        let c = CancelToken::new();

        let (_, _, errors) = done(run_join(&[dir.join("a")], dir, "out", &tx, &c));
        assert_eq!(errors.len(), 1);
        assert_eq!(fs::read_to_string(dir.join("out")).unwrap(), "keep");

        let (_, _, errors) = done(run_join(&[dir.join("a"), dir.join("d")], dir, "new", &tx, &c));
        assert_eq!(errors.len(), 1);
        assert!(!dir.join("new").exists(), "nothing written when a source is invalid");
    }

    #[test]
    fn cancelled_join_removes_the_partial_target() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        fs::write(dir.join("a"), pattern(1000)).unwrap();
        let (tx, _rx) = channel();
        let c = CancelToken::new();
        c.cancel();

        let (_, cancelled, errors) = done(run_join(&[dir.join("a")], dir, "out", &tx, &c));

        assert!(cancelled);
        assert!(errors.is_empty());
        assert!(!dir.join("out").exists());
    }
}
