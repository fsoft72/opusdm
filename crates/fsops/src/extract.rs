//! Extract: unpack archives into a folder with the system tools
//! (`unzip`, `tar`, `7z`). Existing files are never overwritten.

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Tag of the debug lines written to the console.
const LOG_TAG: &str = "extract";

/// How often the running tool is polled for exit and cancel.
const POLL_GAP: Duration = Duration::from_millis(50);

/// Suffixes read by `tar`.
const TAR_SUFFIXES: &[&str] = &[".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst"];

/// Archive kinds `Extract` can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Zip,
    Tar,
    SevenZ,
}

/// The kind of archive a file name asks for, by its suffix (case-insensitive).
fn _kind_of(name: &str) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        Some(Kind::Zip)
    } else if lower.ends_with(".7z") {
        Some(Kind::SevenZ)
    } else if TAR_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        Some(Kind::Tar)
    } else {
        None
    }
}

/// Whether `name` is an archive `Extract` can unpack.
pub fn is_extractable(name: &str) -> bool {
    _kind_of(name).is_some()
}

/// A `Done` for a job that failed as a whole.
fn _failed(path: &Path, msg: String) -> Progress {
    Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: vec![(path.to_path_buf(), msg)] }
}

/// The command that unpacks `src` into `dest_dir` without overwriting.
fn _command(kind: Kind, src: &Path, dest_dir: &Path) -> Command {
    let mut cmd;
    match kind {
        Kind::Zip => {
            cmd = Command::new("unzip");
            cmd.args(["-n", "-q"]).arg("--").arg(src).arg("-d").arg(dest_dir);
        }
        Kind::Tar => {
            cmd = Command::new("tar");
            cmd.args(["--keep-old-files", "--no-same-owner", "-xf"]).arg(src).arg("-C").arg(dest_dir);
        }
        Kind::SevenZ => {
            cmd = Command::new(crate::tools::seven_z_program());
            let mut out = std::ffi::OsString::from("-o");
            out.push(dest_dir);
            cmd.args(["x", "-bd", "-aos", "-spd"]).arg(out).arg("--").arg(src);
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

/// Unpack every archive of `srcs` into `dest_dir`. A source that is not a
/// supported archive, or whose tool fails, is reported and the next one runs.
pub fn run_extract(srcs: &[PathBuf], dest_dir: &Path, tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    if srcs.is_empty() {
        return _failed(dest_dir, "no source files selected".into());
    }
    if !dest_dir.is_dir() {
        return _failed(dest_dir, format!("{} is not a folder", dest_dir.display()));
    }
    let _ = tx.send(Progress::Scanning { files: srcs.len() as u64, bytes: 0 });
    let mut done = 0;
    let mut errors = Vec::new();
    for src in srcs {
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(kind) = _kind_of(&name) else {
            errors.push((src.clone(), format!("{name}: not a supported archive")));
            continue;
        };
        match _extract_one(kind, src, dest_dir, cancel) {
            Ok(true) => done += 1,
            Ok(false) => return Progress::Done { copied: done, skipped: 0, cancelled: true, errors },
            Err(msg) => errors.push((src.clone(), msg)),
        }
    }
    Progress::Done { copied: done, skipped: 0, cancelled: false, errors }
}

/// Run the tool for one archive. `Ok(false)` when cancelled.
fn _extract_one(kind: Kind, src: &Path, dest_dir: &Path, cancel: &CancelToken) -> Result<bool, String> {
    let mut cmd = _command(kind, src, dest_dir);
    opusdm_ipc::err_log!(LOG_TAG, "run {:?} {:?}", cmd.get_program(), cmd.get_args().collect::<Vec<_>>());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the extractor: {e}"))?;
    let mut lost_since = None;
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        match opusdm_ipc::reaped::poll_exit(&mut child, &mut lost_since) {
            Ok(Some(status)) if status.success() => return Ok(true),
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut err, &mut text);
                }
                let text = text.trim();
                opusdm_ipc::err_log!(LOG_TAG, "failed: {status}: {text:?}");
                return Err(if text.is_empty() { format!("extractor failed: {status}") } else { text.to_string() });
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        std::thread::sleep(POLL_GAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn run(srcs: &[PathBuf], dest: &Path) -> Progress {
        let (tx, _rx) = channel();
        run_extract(srcs, dest, &tx, &CancelToken::new())
    }

    #[test]
    fn suffix_selects_the_kind() {
        assert_eq!(_kind_of("a.ZIP"), Some(Kind::Zip));
        assert_eq!(_kind_of("a.tar.gz"), Some(Kind::Tar));
        assert_eq!(_kind_of("a.7z"), Some(Kind::SevenZ));
        assert!(!is_extractable("a.txt"));
    }

    #[test]
    fn a_tar_is_unpacked_and_existing_files_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), "new").unwrap();
        let tar = dir.path().join("a.tar");
        let ok = Command::new("tar").arg("-cf").arg(&tar).arg("-C").arg(&src).arg("a.txt").status();
        if !ok.is_ok_and(|s| s.success()) {
            return;
        }
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("a.txt"), "keep").unwrap();
        let _ = run(std::slice::from_ref(&tar), &out);
        assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "keep");
        let fresh = dir.path().join("fresh");
        std::fs::create_dir(&fresh).unwrap();
        match run(&[tar], &fresh) {
            Progress::Done { copied, errors, .. } => assert_eq!((copied, errors.len()), (1, 0)),
            _ => panic!("not a Done"),
        }
        assert_eq!(std::fs::read_to_string(fresh.join("a.txt")).unwrap(), "new");
    }

    #[test]
    fn a_non_archive_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        match run(&[f], dir.path()) {
            Progress::Done { errors, .. } => assert!(errors[0].1.contains("not a supported archive")),
            _ => panic!("not a Done"),
        }
    }
}
