//! A small fixed pool of worker threads that turn `ThumbRequest`s into
//! `ThumbResult`s. Modelled on `opusdm_fsops::pool` but dependency-free and
//! specialised: each worker checks the freedesktop cache, generates on a miss,
//! stores the result, and forwards the PNG bytes.

use crate::thumbs::{cache, generate, ThumbKind};
use crate::types::ThumbSize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Upper bound on worker threads, whatever the core count.
const MAX_WORKERS: usize = 4;

/// A request to produce one thumbnail.
pub struct ThumbRequest {
    pub index: usize,
    pub path: PathBuf,
    pub kind: ThumbKind,
    pub size: ThumbSize,
    /// Modification time of the entry the request was made for, handed back
    /// in the result so a thumbnail of an older version is not shown.
    pub modified: i64,
    /// Generation the request was submitted under. Checked against the
    /// pool's current generation before any decoding starts, so a request
    /// made stale by a directory change is skipped instead of paying for a
    /// full decode or subprocess run that will only be discarded.
    pub generation: u64,
}

/// A finished thumbnail request, as PNG bytes (texture creation is the
/// caller's job). `png` is `None` when the thumbnail could not be made, so
/// the caller can stop asking for it.
pub struct ThumbResult {
    pub index: usize,
    pub path: PathBuf,
    pub size: ThumbSize,
    /// The `modified` of the request.
    pub modified: i64,
    pub png: Option<Vec<u8>>,
    pub generation: u64,
}

/// The worker pool handle. Dropping it closes the queue and marks every
/// queued request stale; the workers finish the job in hand, skip the rest
/// and exit on their own. The drop never waits for them, since it runs on
/// the GTK thread and a job can take seconds (a video frame, a PDF page).
pub struct ThumbPool {
    tx: Option<Sender<ThumbRequest>>,
    generation: Arc<AtomicU64>,
}

impl ThumbPool {
    /// Spawn the pool. The receiver yields results in completion order.
    pub fn new() -> (ThumbPool, async_channel::Receiver<ThumbResult>) {
        let workers =
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, MAX_WORKERS);
        let (tx, rx) = channel::<ThumbRequest>();
        let rx = Arc::new(Mutex::new(rx));
        let (out_tx, out_rx) = async_channel::unbounded::<ThumbResult>();
        let generation = Arc::new(AtomicU64::new(0));

        for i in 0..workers {
            let rx = Arc::clone(&rx);
            let out_tx = out_tx.clone();
            let generation = Arc::clone(&generation);
            std::thread::Builder::new()
                .name(format!("thumb-{i}"))
                .spawn(move || loop {
                    let req = match rx.lock().map(|g| g.recv()) {
                        Ok(Ok(req)) => req,
                        _ => break,
                    };
                    let gen_at_start = generation.load(Ordering::SeqCst);
                    if req.generation != gen_at_start {
                        // Stale before any work started: the directory moved
                        // on since this was queued. Skip the decode entirely.
                        continue;
                    }
                    let ThumbRequest { index, path, kind, size, modified, generation: _ } = req;
                    let cached = cache::load_valid(&path, size);
                    // Read before the decode: an edit during it must not
                    // cache the old picture under the new time.
                    let made_at = cache::source_mtime(&path).unwrap_or(0);
                    let png = match cached {
                        Some(bytes) => Some(bytes),
                        None => match generate::generate(&path, kind, size.px()) {
                            Ok(bytes) => {
                                // The thumbnail is still shown; only the disk cache misses out.
                                if let Err(e) = cache::store_made_at(&path, size, &bytes, made_at) {
                                    opusdm_ipc::err_log!(
                                        "iconview",
                                        "cannot cache the thumbnail of {}: {e}",
                                        path.display()
                                    );
                                }
                                Some(bytes)
                            }
                            Err(e) => {
                                opusdm_ipc::err_log!(
                                    "iconview",
                                    "thumbnail failed for {}: {e}",
                                    path.display()
                                );
                                None
                            }
                        },
                    };
                    let res = ThumbResult { index, path, size, modified, png, generation: gen_at_start };
                    // The receiver is gone only when the field was dropped.
                    if out_tx.send_blocking(res).is_err() {
                        break;
                    }
                })
                .expect("spawn thumb worker");
        }

        (ThumbPool { tx: Some(tx), generation }, out_rx)
    }

    /// Queue a request. Silently dropped after the pool is shut down.
    pub fn submit(&self, req: ThumbRequest) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    /// The current generation counter.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Advance the generation so in-flight results are recognised as stale.
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether thumbnails are switched off by `OPUSDM_NO_THUMBS`.
    pub fn disabled() -> bool {
        std::env::var_os("OPUSDM_NO_THUMBS").map(|v| !v.is_empty()).unwrap_or(false)
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.bump_generation();
        self.tx.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_serves_a_cached_image_without_a_generator() {
        let _env_guard = crate::test_env_lock();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("pic.png");
        std::fs::write(&src, b"src").unwrap();
        let png = crate::thumbs::png_text::tests_tiny_png();
        cache::store(&src, ThumbSize::Normal, &png).unwrap();

        let (pool, rx) = ThumbPool::new();
        pool.submit(ThumbRequest {
            index: 3,
            path: src.clone(),
            kind: ThumbKind::Image,
            size: ThumbSize::Normal,
            modified: 42,
            generation: pool.generation(),
        });
        let res = rx.recv_blocking().expect("a result");
        assert_eq!(res.index, 3);
        assert_eq!(res.modified, 42, "the result names the version it was made for");
        assert_eq!(res.path, src);
        assert!(res.png.expect("a png").windows(4).any(|w| w == b"IDAT"));

        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn pool_reports_a_failed_thumbnail() {
        let _env_guard = crate::test_env_lock();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("not-an-image.png");
        std::fs::write(&src, b"garbage").unwrap();

        let (pool, rx) = ThumbPool::new();
        pool.submit(ThumbRequest {
            index: 7,
            path: src.clone(),
            kind: ThumbKind::Image,
            size: ThumbSize::Normal,
            modified: 42,
            generation: pool.generation(),
        });
        let res = rx.recv_blocking().expect("a failure result, not silence");
        assert_eq!(res.index, 7);
        assert_eq!(res.path, src);
        assert!(res.png.is_none());

        std::env::remove_var("XDG_CACHE_HOME");
    }
}
