//! Extract: unpack archives into a folder with libarchive (`opusdm-archive`).
//! Existing files are never overwritten.

use crate::cancel::CancelToken;
use crate::types::Progress;
use opusdm_archive::ArchiveError;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Tag of the debug lines written to the console.
const LOG_TAG: &str = "extract";

/// Whether `name` is an archive `Extract` can unpack.
pub fn is_extractable(name: &str) -> bool {
    opusdm_archive::is_archive_name(name)
}

/// A `Done` for a job that failed as a whole.
fn _failed(path: &Path, msg: String) -> Progress {
    Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: vec![(path.to_path_buf(), msg)] }
}

/// Unpack every archive of `srcs` into `dest_dir`. A source that is not a
/// supported archive, or that cannot be read, is reported and the next one runs.
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
        if !is_extractable(&name) {
            errors.push((src.clone(), format!("{name}: not a supported archive")));
            continue;
        }
        match _extract_one(src, dest_dir, cancel) {
            Ok(true) => done += 1,
            Ok(false) => return Progress::Done { copied: done, skipped: 0, cancelled: true, errors },
            Err(msg) => errors.push((src.clone(), msg)),
        }
    }
    Progress::Done { copied: done, skipped: 0, cancelled: false, errors }
}

/// Unpack one archive. `Ok(false)` when cancelled.
fn _extract_one(src: &Path, dest_dir: &Path, cancel: &CancelToken) -> Result<bool, String> {
    opusdm_ipc::err_log!(LOG_TAG, "extract {} into {}", src.display(), dest_dir.display());
    match opusdm_archive::extract(src, dest_dir, None, &|| cancel.is_cancelled()) {
        Ok(done) => {
            opusdm_ipc::err_log!(LOG_TAG, "{} entries written, {} kept", done.written, done.skipped);
            Ok(true)
        }
        Err(ArchiveError::Cancelled) => Ok(false),
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "failed: {e}");
            Err(e.to_string())
        }
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

    /// A tar holding `a.txt` with `body`.
    fn make_tar(path: &Path, body: &str) {
        let mut builder = tar::Builder::new(std::fs::File::create(path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, "a.txt", body.as_bytes()).unwrap();
        builder.finish().unwrap();
    }

    #[test]
    fn suffix_selects_the_archives() {
        assert!(is_extractable("a.ZIP"));
        assert!(is_extractable("a.tar.gz"));
        assert!(is_extractable("a.7z"));
        assert!(!is_extractable("a.txt"));
    }

    #[test]
    fn a_tar_is_unpacked_and_existing_files_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("a.tar");
        make_tar(&tar, "new");
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

    #[test]
    fn a_broken_archive_is_reported_and_the_next_one_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.zip");
        std::fs::write(&bad, "not a zip").unwrap();
        let good = dir.path().join("good.tar");
        make_tar(&good, "ok");
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        match run(&[bad.clone(), good], &out) {
            Progress::Done { copied, errors, .. } => {
                assert_eq!(copied, 1);
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].0, bad);
            }
            _ => panic!("not a Done"),
        }
        assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "ok");
    }

    #[test]
    fn a_cancelled_job_stops_before_unpacking() {
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("a.tar");
        make_tar(&tar, "x");
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let (tx, _rx) = channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        match run_extract(&[tar], &out, &tx, &cancel) {
            Progress::Done { cancelled, copied, .. } => assert_eq!((cancelled, copied), (true, 0)),
            _ => panic!("not a Done"),
        }
        assert!(!out.join("a.txt").exists());
    }
}
