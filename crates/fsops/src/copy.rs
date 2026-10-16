//! Copy pass: recreate directories, copy files with mode + mtime, resolve
//! overwrite conflicts, honor cancellation. Shared by Copy and the
//! cross-filesystem Move path.
//!
//! Small files are copied by a [`WorkerPool`]; directories, symlinks and
//! conflict prompts stay on the orchestrating thread so the user is never asked
//! two questions at once.

use crate::cancel::CancelToken;
use crate::file_meta;
use crate::pool::{self, WorkerPool, SMALL_FILE_MAX};
use crate::scan::{scan_with, Item, Plan};
use crate::types::{Progress, TransferOpts};
use opusdm_ipc::Resolution;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// Copy buffer size. 256 KiB balances syscall count against memory.
const COPY_BUF: usize = 256 * 1024;

use std::os::unix::io::AsRawFd;

/// Kernel chunk size for `copy_file_range` (1 GiB max per syscall recommended).
const KERNEL_COPY_CHUNK: usize = 1024 * 1024 * 1024;

/// Shortest gap between two `Progress::File` events. Emitting one per chunk
/// saturates the IPC channel and stalls the GTK main thread.
const PROGRESS_INTERVAL_US: u64 = 50_000;

/// Lock ignoring poisoning: one panicking worker must not wedge the whole job.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Copy one regular file, preserving unix permission bits and mtime.
///
/// Tries Linux `copy_file_range(2)` first for zero-copy in-kernel transfer and
/// filesystem-level reflink/clone (CoW) support. Falls back to userspace read/write
/// if the kernel returns EXDEV, ENOSYS, EINVAL, or EOPNOTSUPP.
///
/// If `overwrite` is false, atomically attempts to create the file without
/// replacing an existing destination (`ErrorKind::AlreadyExists`).
///
/// With `overwrite` the data goes to a temp file that replaces the destination
/// only when the copy succeeds; an existing destination survives any failure.
///
/// Calls `on_bytes` with the size of each chunk written. On cancel it removes
/// the partial output and returns `ErrorKind::Interrupted`.
pub fn copy_file(
    src: &Path,
    dst: &Path,
    overwrite: bool,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> io::Result<()> {
    let mut reader = std::fs::File::open(src)?;
    // A symlink, bind mount or alternate spelling can make DEST the very
    // inode we are reading; truncating it would destroy the source.
    if overwrite && is_same_file(&reader, dst) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source and destination are the same file: {}", dst.display()),
        ));
    }
    // Overwriting writes a temp file next to the destination and renames it
    // over the target only once the copy succeeded, so a cancel, a full disk
    // or a read error leaves the old file untouched.
    let out = if overwrite { temp_sibling(dst) } else { dst.to_path_buf() };
    let mut writer = std::fs::OpenOptions::new().write(true).create_new(true).open(&out)?;

    let copy_res = copy_file_kernel(&mut reader, &mut writer, cancel, on_bytes).or_else(|e| {
        if !should_fallback_userspace(&e) {
            return Err(e);
        }
        // Seek or reopen reader/writer at current positions for userspace fallback
        copy_file_userspace(&mut reader, &mut writer, cancel, on_bytes)
    });
    let finish = copy_res.and_then(|()| {
        writer.flush()?;
        let meta = std::fs::metadata(src)?;
        std::fs::set_permissions(&out, meta.permissions())?;
        let mtime = filetime::FileTime::from_last_modification_time(&meta);
        filetime::set_file_mtime(&out, mtime)?;
        if overwrite {
            std::fs::rename(&out, dst)?;
        }
        Ok(())
    });
    if finish.is_err() {
        drop(writer);
        crate::cleanup::remove_partial(&out);
    }
    finish
}

/// A hidden, unused path in the same folder as `dst` (same filesystem, so the
/// final `rename` is atomic).
fn temp_sibling(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    dst.with_file_name(format!(".{name}.opusdm-tmp-{}-{seq}", std::process::id()))
}

/// Whether `path` exists and is the same inode (device + inode number) as the
/// open file `f`.
fn is_same_file(f: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(a), Ok(b)) = (f.metadata(), std::fs::metadata(path)) else { return false };
    a.dev() == b.dev() && a.ino() == b.ino()
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

/// One planned file, ready to copy.
struct CopyTask {
    abs: PathBuf,
    dst: PathBuf,
    overwrite: bool,
}

/// What to do about a destination that already exists.
enum Decision {
    /// Copy it, replacing the destination when `overwrite`.
    Copy { overwrite: bool },
    /// Leave the destination alone and count a skip.
    Skip,
    /// Abandon the whole job.
    Cancel,
}

/// State shared between the orchestrating thread and the copy workers.
struct CopyShared {
    tx: Sender<Progress>,
    cancel: CancelToken,
    total_files: u64,
    total_bytes: u64,
    copied: AtomicU64,
    skipped: AtomicU64,
    done_bytes: AtomicU64,
    /// Bytes written so far for whichever file last called [`Self::add_bytes`].
    current_file_bytes_done: AtomicU64,
    /// Size of whichever file last called [`Self::add_bytes`].
    current_file_bytes_total: AtomicU64,
    /// Microseconds since `started` at the last emitted `Progress::File`.
    last_emit_us: AtomicU64,
    started: Instant,
    errors: Mutex<Vec<(PathBuf, String)>>,
    overwrite_all: AtomicBool,
    skip_all: AtomicBool,
    cancelled: AtomicBool,
    /// Serialises interactive conflict prompts to one at a time.
    conflict_lock: Mutex<()>,
    /// Copy only files whose name already exists at the destination.
    update: bool,
    /// Copy only files missing at the destination or newer than it.
    newer: bool,
    /// Source paths copied so far; `Some` only for a Move, which deletes
    /// exactly those when some sources were skipped.
    copied_srcs: Option<Mutex<Vec<PathBuf>>>,
}

impl CopyShared {
    fn new(
        tx: Sender<Progress>,
        cancel: CancelToken,
        total_files: u64,
        total_bytes: u64,
        opts: &TransferOpts,
        track_copied: bool,
    ) -> Self {
        Self {
            tx,
            cancel,
            total_files,
            total_bytes,
            copied: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            current_file_bytes_done: AtomicU64::new(0),
            current_file_bytes_total: AtomicU64::new(0),
            last_emit_us: AtomicU64::new(0),
            started: Instant::now(),
            errors: Mutex::new(Vec::new()),
            overwrite_all: AtomicBool::new(false),
            skip_all: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            conflict_lock: Mutex::new(()),
            update: opts.update,
            newer: opts.newer,
            copied_srcs: track_copied.then(|| Mutex::new(Vec::new())),
        }
    }

    /// Remember that `src` was copied, when the job tracks that.
    fn record_copied(&self, src: &Path) {
        if let Some(list) = &self.copied_srcs {
            lock(list).push(src.to_path_buf());
        }
    }

    /// Take the recorded copied source paths.
    fn take_copied(&self) -> Vec<PathBuf> {
        self.copied_srcs.as_ref().map(|l| std::mem::take(&mut *lock(l))).unwrap_or_default()
    }

    /// Whether the `update`/`newer` flags allow copying `src` over `dst`.
    fn passes_flags(&self, src: &Path, dst: &Path) -> bool {
        if !self.update && !self.newer {
            return true;
        }
        let dst_meta = std::fs::symlink_metadata(dst).ok();
        if self.update && dst_meta.is_none() {
            return false;
        }
        if self.newer {
            let dst_time = dst_meta.and_then(|m| m.modified().ok());
            let src_time = std::fs::metadata(src).and_then(|m| m.modified()).ok();
            if let (Some(d), Some(s)) = (dst_time, src_time) {
                return s > d;
            }
        }
        true
    }

    /// Account for `n` freshly written bytes of `path` (`file_done`/`file_total`
    /// bytes into that file alone), then emit a throttled snapshot.
    fn add_bytes(&self, path: &Path, n: u64, file_done: u64, file_total: u64) {
        self.done_bytes.fetch_add(n, Ordering::Relaxed);
        self.current_file_bytes_done.store(file_done, Ordering::Relaxed);
        self.current_file_bytes_total.store(file_total, Ordering::Relaxed);
        self.emit(path, false);
    }

    /// Send a `Progress::File` snapshot. Without `force`, at most one event per
    /// `PROGRESS_INTERVAL_US` is emitted across all workers.
    fn emit(&self, path: &Path, force: bool) {
        let now_us = self.started.elapsed().as_micros() as u64;
        if force {
            self.last_emit_us.store(now_us, Ordering::Relaxed);
        } else {
            let last = self.last_emit_us.load(Ordering::Relaxed);
            if now_us.saturating_sub(last) < PROGRESS_INTERVAL_US {
                return;
            }
            // Only the thread that claims the slot emits; the rest skip.
            let claimed = self
                .last_emit_us
                .compare_exchange(last, now_us, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok();
            if !claimed {
                return;
            }
        }
        let _ = self.tx.send(Progress::File {
            path: path.to_path_buf(),
            done_bytes: self.done_bytes.load(Ordering::Relaxed),
            total_bytes: self.total_bytes,
            files_done: self.copied.load(Ordering::Relaxed),
            files_total: self.total_files,
            file_bytes_done: self.current_file_bytes_done.load(Ordering::Relaxed),
            file_bytes_total: self.current_file_bytes_total.load(Ordering::Relaxed),
        });
    }

    fn push_error(&self, path: PathBuf, msg: String) {
        lock(&self.errors).push((path, msg));
    }

    fn take_errors(&self) -> Vec<(PathBuf, String)> {
        std::mem::take(&mut *lock(&self.errors))
    }

    /// Stop the whole job. Workers observe the token and unwind.
    fn abort(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    /// Resolve an existing destination, honouring and updating the sticky "all"
    /// answers. At most one thread prompts at a time.
    fn decide(&self, dst: &Path, src: &Path) -> Decision {
        if let Some(d) = self.sticky_decision() {
            return d;
        }
        let _guard = lock(&self.conflict_lock);
        // Another thread may have answered "all" while we waited for the lock.
        if let Some(d) = self.sticky_decision() {
            return d;
        }
        match ask_conflict(&self.tx, dst, src) {
            Some(Resolution::Overwrite) => Decision::Copy { overwrite: true },
            Some(Resolution::OverwriteAll) => {
                self.overwrite_all.store(true, Ordering::SeqCst);
                Decision::Copy { overwrite: true }
            }
            Some(Resolution::Skip) => Decision::Skip,
            Some(Resolution::SkipAll) => {
                self.skip_all.store(true, Ordering::SeqCst);
                Decision::Skip
            }
            Some(Resolution::Cancel) | None => Decision::Cancel,
        }
    }

    /// The answer implied by a previous "apply to all", if one was given.
    fn sticky_decision(&self) -> Option<Decision> {
        if self.overwrite_all.load(Ordering::SeqCst) {
            return Some(Decision::Copy { overwrite: true });
        }
        if self.skip_all.load(Ordering::SeqCst) {
            return Some(Decision::Skip);
        }
        None
    }
}

/// `copy_file` plus byte accounting and the success counter.
fn copy_file_counted(shared: &CopyShared, task: &CopyTask, overwrite: bool) -> io::Result<()> {
    let file_total = std::fs::metadata(&task.abs).map(|m| m.len()).unwrap_or(0);
    let file_done = AtomicU64::new(0);
    let mut on_bytes = |n: u64| {
        let done = file_done.fetch_add(n, Ordering::Relaxed) + n;
        shared.add_bytes(&task.abs, n, done, file_total);
    };
    copy_file(&task.abs, &task.dst, overwrite, &shared.cancel, &mut on_bytes)?;
    shared.copied.fetch_add(1, Ordering::Relaxed);
    shared.record_copied(&task.abs);
    shared.emit(&task.abs, false);
    Ok(())
}

/// Copy one planned file and record the outcome. Runs on the orchestrating
/// thread for big files and on a pool worker for small ones.
fn run_copy_task(shared: &CopyShared, task: CopyTask) {
    if shared.cancel.is_cancelled() {
        return;
    }
    let mut overwrite = task.overwrite;
    // Two attempts at most: the retry only happens when the destination
    // appeared between the plan and the copy (TOCTOU) and the user said so.
    for attempt in 0..2 {
        match copy_file_counted(shared, &task, overwrite) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                shared.cancelled.store(true, Ordering::SeqCst);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt == 0 => {
                match shared.decide(&task.dst, &task.abs) {
                    Decision::Copy { overwrite: o } => overwrite = o,
                    Decision::Skip => {
                        shared.skipped.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Decision::Cancel => {
                        shared.abort();
                        return;
                    }
                }
            }
            Err(e) => {
                shared.push_error(task.abs.clone(), e.to_string());
                return;
            }
        }
    }
}

/// Walk the plan: create directories and links inline, dispatch file copies.
/// Small files go to `pool`, everything else is copied on this thread.
fn run_plan(plan: &Plan, dest_dir: &Path, shared: &CopyShared, pool: Option<&WorkerPool<CopyTask>>) {
    for item in &plan.items {
        if shared.cancel.is_cancelled() {
            shared.cancelled.store(true, Ordering::SeqCst);
            return;
        }
        match item {
            Item::Dir { rel, .. } => {
                let dir = dest_dir.join(rel);
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    shared.push_error(dir, e.to_string());
                }
            }
            Item::Symlink { rel, abs, target } => {
                let link = dest_dir.join(rel);
                if !shared.passes_flags(abs, &link) {
                    shared.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let _ = std::fs::remove_file(&link);
                match std::os::unix::fs::symlink(target, &link) {
                    Ok(()) => shared.record_copied(abs),
                    Err(e) => shared.push_error(link, e.to_string()),
                }
            }
            Item::File { rel, abs, size } => {
                let dst = dest_dir.join(rel);
                if !shared.passes_flags(abs, &dst) {
                    shared.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let mut overwrite = shared.overwrite_all.load(Ordering::SeqCst);
                if !overwrite && dst.exists() {
                    match shared.decide(&dst, abs) {
                        Decision::Copy { overwrite: o } => overwrite = o,
                        Decision::Skip => {
                            shared.skipped.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        Decision::Cancel => {
                            shared.abort();
                            return;
                        }
                    }
                }
                let task = CopyTask { abs: abs.clone(), dst, overwrite };
                match pool {
                    Some(p) if *size <= SMALL_FILE_MAX => p.submit(task),
                    _ => run_copy_task(shared, task),
                }
            }
        }
    }
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
    copy_tree_opts(srcs, dest_dir, tx, cancel, delete_src_after, &TransferOpts::default())
}

/// [`copy_tree`] with a filter, `update`/`newer` flags and per-source names.
///
/// When `delete_src_after` is set and `opts` is partial or any file was
/// skipped (filter, flags or a conflict answered with Skip), only the source
/// files that were actually copied are removed (then the directories that
/// became empty), so a Move never destroys what it skipped.
pub fn copy_tree_opts(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    delete_src_after: bool,
    opts: &TransferOpts,
) -> Progress {
    copy_tree_workers(srcs, dest_dir, tx, cancel, delete_src_after, opts, pool::worker_count())
}

/// [`copy_tree`] with an explicit worker count. `workers <= 1` copies everything
/// on the calling thread and spawns no pool.
#[cfg(test)]
fn copy_tree_with_workers(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    delete_src_after: bool,
    workers: usize,
) -> Progress {
    copy_tree_workers(srcs, dest_dir, tx, cancel, delete_src_after, &TransferOpts::default(), workers)
}

/// The real copy driver behind every `copy_tree*` entry point.
fn copy_tree_workers(
    srcs: &[PathBuf],
    dest_dir: &Path,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
    delete_src_after: bool,
    opts: &TransferOpts,
    workers: usize,
) -> Progress {
    let plan = match scan_with(srcs, opts.glob.as_ref(), opts.names.as_deref()) {
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

    // A Move always records what it copied: a conflict answered with Skip
    // leaves sources behind even when no filter or flag was set.
    let shared = Arc::new(CopyShared::new(
        tx.clone(),
        cancel.clone(),
        plan.total_files,
        plan.total_bytes,
        opts,
        delete_src_after,
    ));

    // One pool for the whole job. On a single core there is nothing to overlap,
    // so everything stays on this thread.
    let pool = (workers > 1).then(|| {
        let shared = Arc::clone(&shared);
        WorkerPool::new(workers, "opusdm-copy", move |task| run_copy_task(&shared, task))
    });

    run_plan(&plan, dest_dir, &shared, pool.as_ref());
    if let Some(p) = pool {
        p.join();
    }
    shared.emit(Path::new(""), true);

    let cancelled = shared.cancelled.load(Ordering::SeqCst);
    let mut errors = shared.take_errors();

    if delete_src_after && errors.is_empty() && !cancelled {
        let partial_move = opts.is_partial() || shared.skipped.load(Ordering::SeqCst) > 0;
        if partial_move {
            errors.extend(remove_copied(&plan, &shared.take_copied()));
        } else {
            for src in srcs {
                if let Err(e) = remove_any(src) {
                    errors.push((src.clone(), e.to_string()));
                }
            }
        }
    }

    Progress::Done {
        copied: shared.copied.load(Ordering::SeqCst),
        skipped: shared.skipped.load(Ordering::SeqCst),
        cancelled,
        errors,
    }
}

/// Delete the source files in `copied`, then the planned source directories
/// that are now empty (deepest first). Directories that still hold skipped
/// files stay.
fn remove_copied(plan: &Plan, copied: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    for path in copied {
        if let Err(e) = std::fs::remove_file(path) {
            errors.push((path.clone(), e.to_string()));
        }
    }
    for item in plan.items.iter().rev() {
        if let Item::Dir { abs, .. } = item {
            // Fails when the directory is not empty, which is the point.
            let _ = std::fs::remove_dir(abs);
        }
    }
    errors
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
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::channel;
    use std::thread;

    /// Overwriting a file through a symlink that points at the source must
    /// refuse instead of truncating the source.
    #[test]
    fn copy_file_refuses_to_overwrite_the_source_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let link = dir.path().join("link.txt");
        fs::write(&src, b"precious").unwrap();
        std::os::unix::fs::symlink(&src, &link).unwrap();
        let err = copy_file(&src, &link, true, &CancelToken::new(), &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(&src).unwrap(), b"precious");
    }

    /// A cancelled overwrite keeps the old destination and leaves no temp file.
    #[test]
    fn cancelled_overwrite_keeps_the_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src.txt"), dir.path().join("dst.txt"));
        fs::write(&src, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = copy_file(&src, &dst, true, &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(fs::read(&dst).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// A successful overwrite replaces the content and leaves no temp file.
    #[test]
    fn overwrite_replaces_the_destination_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src.txt"), dir.path().join("dst.txt"));
        fs::write(&src, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        copy_file(&src, &dst, true, &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// Worker counts covering both the inline path and the pooled path, so
    /// every behavioural test runs against each.
    const WORKER_MODES: [usize; 2] = [1, 4];

    /// Drive one `copy_tree` to completion, answering every conflict with
    /// `answer`, and return the fields of the terminal `Progress::Done`.
    fn run_copy_workers(
        srcs: Vec<PathBuf>,
        dest: PathBuf,
        answer: Resolution,
        workers: usize,
    ) -> (u64, u64, bool, Vec<(PathBuf, String)>) {
        run_transfer_workers(srcs, dest, answer, workers, false)
    }

    /// [`run_copy_workers`] that can also delete the sources afterwards, as
    /// the cross-filesystem Move does.
    fn run_transfer_workers(
        srcs: Vec<PathBuf>,
        dest: PathBuf,
        answer: Resolution,
        workers: usize,
        delete_src_after: bool,
    ) -> (u64, u64, bool, Vec<(PathBuf, String)>) {
        let (tx, rx) = channel::<Progress>();
        let cancel = CancelToken::new();
        let worker = {
            let tx = tx.clone();
            thread::spawn(move || {
                let done = copy_tree_with_workers(
                    &srcs,
                    &dest,
                    &tx,
                    &cancel,
                    delete_src_after,
                    workers,
                );
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
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(src.join("sub")).unwrap();
            fs::write(src.join("f.txt"), b"data").unwrap();
            fs::set_permissions(src.join("f.txt"), fs::Permissions::from_mode(0o640)).unwrap();
            fs::write(src.join("sub/g.txt"), b"more").unwrap();
            fs::create_dir_all(&dst).unwrap();

            let (copied, skipped, cancelled, errors) =
                run_copy_workers(vec![src.clone()], dst.clone(), Resolution::Skip, workers);

            assert_eq!((copied, skipped, cancelled), (2, 0, false), "workers={workers}");
            assert!(errors.is_empty(), "workers={workers}");
            assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"data");
            assert_eq!(fs::read(dst.join("s/sub/g.txt")).unwrap(), b"more");
            let mode = fs::metadata(dst.join("s/f.txt")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640, "workers={workers}");
        }
    }

    #[test]
    fn conflict_skip_keeps_destination() {
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(dst.join("s")).unwrap();
            fs::write(src.join("f.txt"), b"NEW").unwrap();
            fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

            let (copied, skipped, _, _) =
                run_copy_workers(vec![src], dst.clone(), Resolution::Skip, workers);

            assert_eq!((copied, skipped), (0, 1), "workers={workers}");
            assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"OLD");
        }
    }

    /// A move through the copy engine must keep the source files the user
    /// chose to skip, and still delete the ones it copied.
    #[test]
    fn move_keeps_the_sources_skipped_on_conflict() {
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(dst.join("s")).unwrap();
            fs::write(src.join("kept.txt"), b"NEW").unwrap();
            fs::write(src.join("moved.txt"), b"MOVED").unwrap();
            fs::write(dst.join("s/kept.txt"), b"OLD").unwrap();

            let (copied, skipped, cancelled, errors) =
                run_transfer_workers(vec![src.clone()], dst.clone(), Resolution::Skip, workers, true);

            assert_eq!((copied, skipped, cancelled), (1, 1, false), "workers={workers}");
            assert!(errors.is_empty(), "workers={workers}: {errors:?}");
            assert_eq!(fs::read(src.join("kept.txt")).unwrap(), b"NEW", "workers={workers}");
            assert!(!src.join("moved.txt").exists(), "workers={workers}");
            assert_eq!(fs::read(dst.join("s/moved.txt")).unwrap(), b"MOVED");
            assert_eq!(fs::read(dst.join("s/kept.txt")).unwrap(), b"OLD");
        }
    }

    /// A move with no conflict still removes the whole source tree.
    #[test]
    fn move_without_conflicts_removes_the_source_tree() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("sub/f.txt"), b"x").unwrap();

        let (copied, _, _, errors) =
            run_transfer_workers(vec![src.clone()], dst.clone(), Resolution::Skip, 1, true);

        assert_eq!(copied, 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!src.exists());
        assert_eq!(fs::read(dst.join("s/sub/f.txt")).unwrap(), b"x");
    }

    #[test]
    fn conflict_overwrite_replaces_destination() {
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(dst.join("s")).unwrap();
            fs::write(src.join("f.txt"), b"NEW").unwrap();
            fs::write(dst.join("s/f.txt"), b"OLD").unwrap();

            let (copied, skipped, _, _) =
                run_copy_workers(vec![src], dst.clone(), Resolution::OverwriteAll, workers);

            assert_eq!((copied, skipped), (1, 0), "workers={workers}");
            assert_eq!(fs::read(dst.join("s/f.txt")).unwrap(), b"NEW");
        }
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
        let spec = JobSpec { op: opusdm_ipc::Op::Copy, srcs: vec![src], dest_dir: dst.clone(), options: Default::default() };
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

    #[test]
    fn many_small_files_all_land_intact() {
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(src.join("nested/deeper")).unwrap();
            fs::create_dir_all(&dst).unwrap();
            const N: usize = 500;
            for i in 0..N {
                let dir = if i % 3 == 0 { src.join("nested/deeper") } else { src.clone() };
                fs::write(dir.join(format!("f{i:04}.txt")), format!("payload-{i}")).unwrap();
            }

            let (copied, skipped, cancelled, errors) =
                run_copy_workers(vec![src.clone()], dst.clone(), Resolution::Skip, workers);

            assert_eq!(errors, Vec::new(), "workers={workers}");
            assert_eq!((copied, skipped, cancelled), (N as u64, 0, false), "workers={workers}");
            for i in 0..N {
                let rel = if i % 3 == 0 { "s/nested/deeper" } else { "s" };
                let got = fs::read_to_string(dst.join(rel).join(format!("f{i:04}.txt"))).unwrap();
                assert_eq!(got, format!("payload-{i}"), "workers={workers}");
            }
        }
    }

    #[test]
    fn progress_events_are_throttled_not_per_chunk() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        const N: usize = 400;
        for i in 0..N {
            fs::write(src.join(format!("f{i:04}.bin")), vec![7u8; 1024]).unwrap();
        }

        let (tx, rx) = channel::<Progress>();
        let s2 = vec![src.clone()];
        let d2 = dst.clone();
        let worker = thread::spawn(move || copy_tree(&s2, &d2, &tx, &CancelToken::new(), false));
        let mut file_events = 0usize;
        while let Ok(ev) = rx.recv() {
            if matches!(ev, Progress::File { .. }) {
                file_events += 1;
            }
        }
        let done = worker.join().unwrap();
        match done {
            Progress::Done { copied, .. } => assert_eq!(copied, N as u64),
            _ => panic!("expected Done"),
        }
        // The old per-file forced emit produced at least one event per file.
        assert!(file_events < N, "expected throttling, got {file_events} events for {N} files");
    }

    #[test]
    fn mixed_large_and_small_files_are_all_copied() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("s");
        let dst = td.path().join("d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        // One file above SMALL_FILE_MAX takes the inline path, the rest the pool.
        fs::write(src.join("big.bin"), vec![3u8; SMALL_FILE_MAX as usize + 1]).unwrap();
        for i in 0..20 {
            fs::write(src.join(format!("small{i}.txt")), b"x").unwrap();
        }

        let (copied, _, cancelled, errors) =
            run_copy_workers(vec![src.clone()], dst.clone(), Resolution::Skip, 4);

        assert!(errors.is_empty());
        assert_eq!((copied, cancelled), (21, false));
        assert_eq!(
            fs::metadata(dst.join("s/big.bin")).unwrap().len(),
            SMALL_FILE_MAX + 1
        );
        let names: HashSet<String> = fs::read_dir(dst.join("s"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 21);
    }

    #[test]
    fn skip_all_applies_to_every_later_conflict() {
        for workers in WORKER_MODES {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(dst.join("s")).unwrap();
            for i in 0..30 {
                fs::write(src.join(format!("f{i:02}.txt")), b"NEW").unwrap();
                fs::write(dst.join("s").join(format!("f{i:02}.txt")), b"OLD").unwrap();
            }

            let (copied, skipped, _, _) =
                run_copy_workers(vec![src], dst.clone(), Resolution::SkipAll, workers);

            assert_eq!((copied, skipped), (0, 30), "workers={workers}");
            for i in 0..30 {
                assert_eq!(fs::read(dst.join("s").join(format!("f{i:02}.txt"))).unwrap(), b"OLD");
            }
        }
    }

    #[test]
    fn worker_count_never_changes_the_result() {
        let mut results = Vec::new();
        for workers in [1usize, 2, 8] {
            let td = tempfile::tempdir().unwrap();
            let src = td.path().join("s");
            let dst = td.path().join("d");
            fs::create_dir_all(src.join("a/b")).unwrap();
            fs::create_dir_all(&dst).unwrap();
            std::os::unix::fs::symlink("../../nowhere", src.join("a/b/link")).unwrap();
            for i in 0..120 {
                fs::write(src.join("a/b").join(format!("f{i:03}")), vec![9u8; i * 32]).unwrap();
            }

            let (copied, skipped, cancelled, errors) =
                run_copy_workers(vec![src.clone()], dst.clone(), Resolution::Skip, workers);
            assert!(errors.is_empty(), "workers={workers}: {errors:?}");
            assert!(dst.join("s/a/b/link").is_symlink(), "workers={workers}");
            results.push((copied, skipped, cancelled));
        }
        assert!(
            results.windows(2).all(|w| w[0] == w[1]),
            "worker count changed the outcome: {results:?}"
        );
    }
}
