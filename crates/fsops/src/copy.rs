//! Copy pass: recreate directories, copy files with mode + mtime, resolve
//! overwrite conflicts, honor cancellation. Shared by Copy and the
//! cross-filesystem Move path.

use crate::cancel::CancelToken;
use crate::file_meta;
use crate::scan::{scan, Item};
use crate::types::Progress;
use opusdm_ipc::Resolution;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Copy buffer size. 256 KiB balances syscall count against memory.
const COPY_BUF: usize = 256 * 1024;

use std::os::unix::io::AsRawFd;

/// Kernel chunk size for `copy_file_range` (1 GiB max per syscall recommended).
const KERNEL_COPY_CHUNK: usize = 1024 * 1024 * 1024;

/// Copy one regular file, preserving unix permission bits and mtime.
///
/// Tries Linux `copy_file_range(2)` first for zero-copy in-kernel transfer and
/// filesystem-level reflink/clone (CoW) support. Falls back to userspace read/write
/// if the kernel returns EXDEV, ENOSYS, EINVAL, or EOPNOTSUPP.
///
/// If `overwrite` is false, atomically attempts to create the file without
/// replacing an existing destination (`ErrorKind::AlreadyExists`).
///
/// Calls `on_bytes` with the size of each chunk written. On cancel it removes
/// the partial destination and returns `ErrorKind::Interrupted`.
pub fn copy_file(
    src: &Path,
    dst: &Path,
    overwrite: bool,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<()> {
    let mut reader = std::fs::File::open(src)?;
    let mut writer = if overwrite {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?
    };

    let copy_res = copy_file_kernel(&mut reader, &mut writer, cancel, on_bytes);
    match copy_res {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::Interrupted => {
            drop(writer);
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
        Err(e) if should_fallback_userspace(&e) => {
            // Seek or reopen reader/writer at current positions for userspace fallback
            let fallback_res = copy_file_userspace(&mut reader, &mut writer, cancel, on_bytes);
            if let Err(fe) = fallback_res {
                if fe.kind() == io::ErrorKind::Interrupted {
                    drop(writer);
                    let _ = std::fs::remove_file(dst);
                }
                return Err(fe);
            }
        }
        Err(e) => {
            drop(writer);
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
    }

    writer.flush()?;
    let meta = std::fs::metadata(src)?;
    std::fs::set_permissions(dst, meta.permissions())?;
    let mtime = filetime::FileTime::from_last_modification_time(&meta);
    filetime::set_file_mtime(dst, mtime)?;
    Ok(())
}

fn should_fallback_userspace(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EXDEV)
            | Some(libc::ENOSYS)
            | Some(libc::EINVAL)
            | Some(libc::EOPNOTSUPP)
    )
}

fn copy_file_kernel(
    reader: &mut std::fs::File,
    writer: &mut std::fs::File,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<()> {
    let fd_in = reader.as_raw_fd();
    let fd_out = writer.as_raw_fd();
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let ret = unsafe {
            libc::copy_file_range(
                fd_in,
                std::ptr::null_mut(),
                fd_out,
                std::ptr::null_mut(),
                KERNEL_COPY_CHUNK,
                0,
            )
        };
        if ret == 0 {
            // EOF reached
            break;
        } else if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        on_bytes(ret as u64);
    }
    Ok(())
}

thread_local! {
    static COPY_BUFFER: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; COPY_BUF]);
}

fn copy_file_userspace(
    reader: &mut std::fs::File,
    writer: &mut std::fs::File,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<()> {
    COPY_BUFFER.with(|cell| {
        let mut buf_guard = cell.borrow_mut();
        let buf: &mut [u8] = &mut buf_guard;
        loop {
            if cancel.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = reader.read(buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            on_bytes(n as u64);
        }
        Ok(())
    })
}

/// Recreate `srcs` under `dest_dir`. Returns the `Progress::Done` to send.
///
/// `delete_src_after` removes each source tree once fully copied - the
/// cross-filesystem Move path.
pub fn copy_tree(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    delete_src_after: bool,
) -> Progress {
    let plan = match scan(srcs) {
        Ok(p) => p,
        Err(e) => {
            return Progress::Done {
                copied: 0,
                skipped: 0,
                cancelled: false,
                errors: vec![(dest_dir.to_path_buf(), e.to_string())],
            }
        }
    };
    let _ = tx.send(Progress::Scanning { files: plan.total_files, bytes: plan.total_bytes });

    let mut copied = 0u64;
    let mut skipped = 0u64;
    let mut done_bytes = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    let mut overwrite_all = false;
    let mut skip_all = false;

    for item in &plan.items {
        if cancel.is_cancelled() {
            return Progress::Done { copied, skipped, cancelled: true, errors };
        }
        match item {
            Item::Dir { rel } => {
                if let Err(e) = std::fs::create_dir_all(dest_dir.join(rel)) {
                    errors.push((dest_dir.join(rel), e.to_string()));
                }
            }
            Item::Symlink { rel, target } => {
                let link = dest_dir.join(rel);
                let _ = std::fs::remove_file(&link);
                if let Err(e) = std::os::unix::fs::symlink(target, &link) {
                    errors.push((link, e.to_string()));
                }
            }
            Item::File { rel, abs, size } => {
                let dst = dest_dir.join(rel);
                let mut overwrite = overwrite_all;
                if !overwrite && dst.exists() {
                    if skip_all {
                        skipped += 1;
                        continue;
                    }
                    match ask_conflict(tx, &dst, abs) {
                        Some(Resolution::Skip) => {
                            skipped += 1;
                            continue;
                        }
                        Some(Resolution::SkipAll) => {
                            skip_all = true;
                            skipped += 1;
                            continue;
                        }
                        Some(Resolution::Overwrite) => {
                            overwrite = true;
                        }
                        Some(Resolution::OverwriteAll) => {
                            overwrite_all = true;
                            overwrite = true;
                        }
                        Some(Resolution::Cancel) | None => {
                            return Progress::Done { copied, skipped, cancelled: true, errors };
                        }
                    }
                }
                let total = plan.total_bytes;
                let files_total = plan.total_files;
                let mut last_emit = std::time::Instant::now();
                let mut emit_progress = |n: u64, force: bool, done_bytes: &mut u64, copied: u64| {
                    *done_bytes += n;
                    if force || last_emit.elapsed() >= std::time::Duration::from_millis(50) {
                        last_emit = std::time::Instant::now();
                        let _ = tx.send(Progress::File {
                            path: abs.clone(),
                            done_bytes: *done_bytes,
                            total_bytes: total,
                            files_done: copied,
                            files_total,
                        });
                    }
                };

                let res = {
                    let mut on_bytes = |n: u64| {
                        emit_progress(n, false, &mut done_bytes, copied);
                    };
                    copy_file(abs, &dst, overwrite, cancel, &mut on_bytes)
                };
                emit_progress(0, true, &mut done_bytes, copied);

                match res {
                    Ok(()) => copied += 1,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        // Concurrent creation detected (TOCTOU)
                        if skip_all {
                            skipped += 1;
                            continue;
                        }
                        match ask_conflict(tx, &dst, abs) {
                            Some(Resolution::Skip) => {
                                skipped += 1;
                                continue;
                            }
                            Some(Resolution::SkipAll) => {
                                skip_all = true;
                                skipped += 1;
                                continue;
                            }
                            Some(Resolution::Overwrite) => {
                                let mut on_bytes = |n: u64| {
                                    emit_progress(n, false, &mut done_bytes, copied);
                                };
                                match copy_file(abs, &dst, true, cancel, &mut on_bytes) {
                                    Ok(()) => {
                                        emit_progress(0, true, &mut done_bytes, copied);
                                        copied += 1;
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                                        return Progress::Done { copied, skipped, cancelled: true, errors };
                                    }
                                    Err(e) => errors.push((abs.clone(), e.to_string())),
                                }
                            }
                            Some(Resolution::OverwriteAll) => {
                                overwrite_all = true;
                                let mut on_bytes = |n: u64| {
                                    emit_progress(n, false, &mut done_bytes, copied);
                                };
                                match copy_file(abs, &dst, true, cancel, &mut on_bytes) {
                                    Ok(()) => {
                                        emit_progress(0, true, &mut done_bytes, copied);
                                        copied += 1;
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                                        return Progress::Done { copied, skipped, cancelled: true, errors };
                                    }
                                    Err(e) => errors.push((abs.clone(), e.to_string())),
                                }
                            }
                            Some(Resolution::Cancel) | None => {
                                return Progress::Done { copied, skipped, cancelled: true, errors };
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        return Progress::Done { copied, skipped, cancelled: true, errors };
                    }
                    Err(e) => errors.push((abs.clone(), e.to_string())),
                }
                let _ = size; // size is informational; progress uses byte totals
            }
        }
    }

    if delete_src_after && errors.is_empty() && !cancel.is_cancelled() {
        for src in srcs {
            if let Err(e) = remove_any(src) {
                errors.push((src.clone(), e.to_string()));
            }
        }
    }

    Progress::Done { copied, skipped, cancelled: false, errors }
}

/// Emit a `Conflict` event and block for the reply. `None` if the channel closed.
fn ask_conflict(tx: &Sender<Progress>, dst: &Path, src: &Path) -> Option<Resolution> {
    let (reply_tx, reply_rx) = channel::<Resolution>();
    let src_meta = std::fs::metadata(src).ok().map(|m| file_meta(&m))?;
    let dst_meta = std::fs::metadata(dst).ok().map(|m| file_meta(&m))?;
    tx.send(Progress::Conflict {
        path: dst.to_path_buf(),
        src: src_meta,
        dst: dst_meta,
        reply: reply_tx,
    })
    .ok()?;
    reply_rx.recv().ok()
}

/// Remove a file or a directory tree.
fn remove_any(p: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobSpec;
    use opusdm_ipc::Resolution;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::channel;
    use std::thread;

    fn run_copy(
        srcs: Vec<PathBuf>,
        dest: PathBuf,
        answer: Resolution,
    ) -> (u64, u64, bool, Vec<(PathBuf, String)>) {
        let (tx, rx) = channel::<Progress>();
        let cancel = CancelToken::new();
        let worker = {
            let tx = tx.clone();
            thread::spawn(move || {
                let done = copy_tree(&srcs, &dest, &tx, &cancel, false);
                tx.send(done).unwrap();
            })
        };
        drop(tx);
        let mut last = (0, 0, false, Vec::new());
        while let Ok(ev) = rx.recv() {
            match ev {
                Progress::Conflict { reply, .. } => {
                    let _ = reply.send(answer);
                }
                Progress::Done { copied, skipped, cancelled, errors } => {
                    last = (copied, skipped, cancelled, errors);
                }
                _ => {}
            }
        }
        worker.join().unwrap();
        last
    }

    #[test]
    fn copies_a_tree_and_preserves_mode() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("f.txt"), b"data").unwrap();
        fs::set_permissions(src.join("f.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(src.join("sub/g.txt"), b"more").unwrap();
        fs::create_dir_all(&dst).unwrap();

        let (copied, skipped, cancelled, errors) =
            run_copy(vec![src.clone()], dst.clone(), Resolution::Skip);

        assert_eq!((copied, skipped, cancelled), (2, 0, false));
        assert!(errors.is_empty());
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"data");
        assert_eq!(fs::read(dst.join("s/sub/g.txt")).unwrap(), b"more");
        let mode = fs::metadata(dst.join("s/f.txt")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn conflict_skip_keeps_destination() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.join("s")).unwrap();
        fs::write(src.join("f.txt"), b"NEW").unwrap();
        fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

        let (copied, skipped, _, _) = run_copy(vec![src], dst.clone(), Resolution::Skip);

        assert_eq!((copied, skipped), (0, 1));
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"OLD");
    }

    #[test]
    fn conflict_overwrite_replaces_destination() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.join("s")).unwrap();
        fs::write(src.join("f.txt"), b"NEW").unwrap();
        fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

        let (copied, skipped, _, _) = run_copy(vec![src], dst.clone(), Resolution::OverwriteAll);

        assert_eq!((copied, skipped), (1, 0));
        assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"NEW");
    }

    #[test]
    fn cancel_stops_and_reports_partial() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for i in 0..50 {
            fs::write(src.join(format!("f{i:02}.bin")), vec![0u8; 4096]).unwrap();
        }
        let (tx, rx) = channel::<Progress>();
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let s2 = vec![src.clone()];
        let d2 = dst.clone();
        let worker = thread::spawn(move || copy_tree(&s2, &d2, &tx, &c2, false));
        cancel.cancel();
        while rx.recv().is_ok() {}
        let done = worker.join().unwrap();
        match done {
            Progress::Done { cancelled, .. } => assert!(cancelled),
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn run_job_copy_emits_scanning_then_done() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a"), b"12345").unwrap();

        let (tx, rx) = channel::<Progress>();
        let spec = JobSpec { op: opusdm_ipc::Op::Copy, srcs: vec![src], dest_dir: dst.clone() };
        let h = thread::spawn(move || crate::run_job(spec, tx, CancelToken::new()));
        let mut saw_scanning = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.recv() {
            match ev {
                Progress::Scanning { files, bytes } => {
                    assert_eq!((files, bytes), (1, 5));
                    saw_scanning = true;
                }
                Progress::Done { copied, .. } => {
                    assert_eq!(copied, 1);
                    saw_done = true;
                }
                _ => {}
            }
        }
        h.join().unwrap();
        assert!(saw_scanning && saw_done);
    }
}
