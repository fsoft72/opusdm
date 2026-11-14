//! Extract: unpack archives into a folder with libarchive (`opusdm-archive`).
//! Existing files are never overwritten. An archive whose entries are not all
//! inside one top folder is unpacked into a new folder named after it.

use crate::cancel::CancelToken;
use crate::types::Progress;
use opusdm_archive::ArchiveError;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Tag of the debug lines written to the console.
const LOG_TAG: &str = "extract";

/// How many `-N` names are tried for the container folder when a file holds
/// the plain name.
const MAX_NAME_TRIES: u32 = 1000;

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
    let is_cancelled = || cancel.is_cancelled();
    let (target, created) = match _target_dir(src, dest_dir, &is_cancelled) {
        Ok(t) => t,
        Err(ArchiveError::Cancelled) => return Ok(false),
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "{}: cannot prepare the folder: {e}", src.display());
            return Err(e.to_string());
        }
    };
    opusdm_ipc::err_log!(LOG_TAG, "extract {} into {}", src.display(), target.display());
    let result = opusdm_archive::extract(src, &target, None, &is_cancelled);
    if result.is_err() && created {
        _remove_if_empty(&target);
    }
    match result {
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

/// The folder `src` is unpacked into, and whether it was created here:
/// `dest_dir` when the archive keeps everything in one top folder, otherwise
/// `dest_dir/<archive name without suffix>`. A folder already there is
/// reused (existing files are kept anyway); when a file holds that name,
/// `<name>-1`, `<name>-2` ... is used instead.
fn _target_dir(src: &Path, dest_dir: &Path, cancel: &dyn Fn() -> bool) -> Result<(PathBuf, bool), ArchiveError> {
    if opusdm_archive::has_root_folder(src, cancel)? {
        return Ok((dest_dir.to_path_buf(), false));
    }
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = opusdm_archive::archive_stem(&name);
    for i in 0..=MAX_NAME_TRIES {
        let candidate = if i == 0 { dest_dir.join(stem) } else { dest_dir.join(format!("{stem}-{i}")) };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok((candidate, true)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A real folder (not a link to one) is reused.
                if candidate.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    return Ok((candidate, false));
                }
            }
            Err(e) => return Err(ArchiveError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", candidate.display())))),
        }
    }
    Err(ArchiveError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("no free folder name for {stem} in {}", dest_dir.display()),
    )))
}

/// Remove the folder this job created for a failed or cancelled extraction,
/// when nothing was written in it. Other failures are logged.
fn _remove_if_empty(dir: &Path) {
    let Err(e) = std::fs::remove_dir(dir) else { return };
    if e.kind() == std::io::ErrorKind::DirectoryNotEmpty || e.kind() == std::io::ErrorKind::NotFound { return; }
    opusdm_ipc::err_log!(LOG_TAG, "cannot remove the empty folder {}: {e}", dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn run(srcs: &[PathBuf], dest: &Path) -> Progress {
        let (tx, _rx) = channel();
        run_extract(srcs, dest, &tx, &CancelToken::new())
    }

    /// A tar holding `a.txt` with `body`, at the top level.
    fn make_tar(path: &Path, body: &str) {
        make_tar_with(path, &[("a.txt", body)]);
    }

    /// A tar holding the files `entries` (`(name, body)`).
    fn make_tar_with(path: &Path, entries: &[(&str, &str)]) {
        let mut builder = tar::Builder::new(std::fs::File::create(path).unwrap());
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            builder.append_data(&mut header, name, body.as_bytes()).unwrap();
        }
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
        std::fs::create_dir_all(out.join("a")).unwrap();
        std::fs::write(out.join("a").join("a.txt"), "keep").unwrap();
        let _ = run(std::slice::from_ref(&tar), &out);
        assert_eq!(std::fs::read_to_string(out.join("a").join("a.txt")).unwrap(), "keep");
        let fresh = dir.path().join("fresh");
        std::fs::create_dir(&fresh).unwrap();
        match run(&[tar], &fresh) {
            Progress::Done { copied, errors, .. } => assert_eq!((copied, errors.len()), (1, 0)),
            _ => panic!("not a Done"),
        }
        assert_eq!(std::fs::read_to_string(fresh.join("a").join("a.txt")).unwrap(), "new");
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
        assert_eq!(std::fs::read_to_string(out.join("good").join("a.txt")).unwrap(), "ok");
        assert!(!out.join("bad").exists(), "a failed archive leaves no empty folder");
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
        assert!(!out.join("a").exists());
    }

    #[test]
    fn an_archive_with_one_top_folder_is_unpacked_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("pack.tar");
        make_tar_with(&tar, &[("proj/a.txt", "a"), ("proj/sub/b.txt", "b")]);
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let _ = run(&[tar], &out);
        assert_eq!(std::fs::read_to_string(out.join("proj/a.txt")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(out.join("proj/sub/b.txt")).unwrap(), "b");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "nothing but the archive folder");
    }

    #[test]
    fn an_archive_without_a_top_folder_gets_one_named_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("Photos.TAR");
        make_tar_with(&tar, &[("one.txt", "1"), ("sub/two.txt", "2")]);
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let _ = run(&[tar], &out);
        assert_eq!(std::fs::read_to_string(out.join("Photos/one.txt")).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(out.join("Photos/sub/two.txt")).unwrap(), "2");
        assert!(!out.join("one.txt").exists());
    }

    #[test]
    fn a_file_with_the_folder_name_moves_the_folder_to_a_numbered_name() {
        let dir = tempfile::tempdir().unwrap();
        let tar = dir.path().join("a.tar");
        make_tar(&tar, "x");
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("a"), "a plain file").unwrap();
        let _ = run(&[tar], &out);
        assert_eq!(std::fs::read_to_string(out.join("a")).unwrap(), "a plain file");
        assert_eq!(std::fs::read_to_string(out.join("a-1/a.txt")).unwrap(), "x");
    }
}
