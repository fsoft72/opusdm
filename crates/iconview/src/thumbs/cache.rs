//! The freedesktop thumbnail cache: `~/.cache/thumbnails/<bucket>/<md5>.png`,
//! where `<md5>` is the MD5 of the canonical `file://` URI. Validity is the
//! `Thumb::MTime` tEXt tag matching the source file's mtime.

use crate::dnd::file_uri;
use crate::thumbs::png_text;
use crate::types::ThumbSize;
use md5::{Digest, Md5};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

fn cache_root() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
}

/// The absolute cache file path for `path` at `size`.
pub fn cache_path(path: &Path, size: ThumbSize) -> Option<PathBuf> {
    let digest = Md5::digest(file_uri(path).as_bytes());
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Some(cache_root()?.join("thumbnails").join(size.dir_name()).join(format!("{hex}.png")))
}

fn source_mtime(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// The cached PNG for `path` at `size`, iff it exists and its `Thumb::MTime`
/// matches the source's current mtime.
pub fn load_valid(path: &Path, size: ThumbSize) -> Option<Vec<u8>> {
    let cp = cache_path(path, size)?;
    let bytes = std::fs::read(&cp).ok()?;
    let tagged: i64 = png_text::read_text(&bytes, "Thumb::MTime")?.trim().parse().ok()?;
    if tagged == source_mtime(path)? {
        Some(bytes)
    } else {
        None
    }
}

/// Write `png` into the cache for `path` at `size`, stamping `Thumb::MTime`
/// and `Thumb::URI`. Atomic: a uniquely named temp file in the same folder is
/// renamed into place, so workers storing the same entry never share one.
pub fn store(path: &Path, size: ThumbSize, png: &[u8]) -> io::Result<()> {
    let cp = cache_path(path, size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no cache directory"))?;
    let dir = cp.parent().expect("cache_path always has a parent");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let mtime = source_mtime(path).unwrap_or(0).to_string();
    let uri = file_uri(path);
    let stamped = png_text::insert_text(png, &[("Thumb::MTime", &mtime), ("Thumb::URI", &uri)])
        .unwrap_or_else(|| png.to_vec());
    // `NamedTempFile` is created 0600 and removed again if anything fails.
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    io::Write::write_all(&mut tmp, &stamped)?;
    tmp.persist(&cp).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::png_text::tests_tiny_png;

    #[test]
    fn uri_matches_the_freedesktop_example() {
        let p = Path::new("/home/jens/photos/me.png");
        assert_eq!(file_uri(p), "file:///home/jens/photos/me.png");
        let digest = Md5::digest(file_uri(p).as_bytes());
        let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex, "c6ee772d9e49320e97ec29a7eb5b1697");
    }

    #[test]
    fn uri_percent_encodes_spaces() {
        assert_eq!(file_uri(Path::new("/tmp/a b.png")), "file:///tmp/a%20b.png");
    }

    #[test]
    fn store_then_load_valid_round_trips_and_detects_staleness() {
        let _env_guard = crate::test_env_lock();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("pic.png");
        std::fs::write(&src, b"source bytes").unwrap();
        filetime::set_file_mtime(&src, filetime::FileTime::from_unix_time(1_700_000_000, 0)).unwrap();

        store(&src, ThumbSize::Normal, &tests_tiny_png()).unwrap();
        let got = load_valid(&src, ThumbSize::Normal).expect("fresh cache is valid");
        assert_eq!(png_text::read_text(&got, "Thumb::URI").as_deref(), Some(file_uri(&src).as_str()));
        assert_eq!(png_text::read_text(&got, "Thumb::MTime").as_deref(), Some("1700000000"));

        // Move the source's mtime forward: the cached tag no longer matches.
        filetime::set_file_mtime(&src, filetime::FileTime::from_unix_time(1_700_000_999, 0)).unwrap();
        assert!(load_valid(&src, ThumbSize::Normal).is_none());

        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn concurrent_stores_of_one_entry_all_succeed_and_leave_no_temp_files() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 50;
        let _env_guard = crate::test_env_lock();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", td.path().join("cache"));
        let src = td.path().join("pic.png");
        std::fs::write(&src, b"source bytes").unwrap();
        let png = tests_tiny_png();

        let failures: usize = std::thread::scope(|s| {
            let workers: Vec<_> = (0..THREADS)
                .map(|_| s.spawn(|| (0..ROUNDS).filter(|_| store(&src, ThumbSize::Normal, &png).is_err()).count()))
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).sum()
        });
        assert_eq!(failures, 0, "a worker's rename lost its temp file to another worker");
        assert!(load_valid(&src, ThumbSize::Normal).is_some());
        let dir = cache_path(&src, ThumbSize::Normal).unwrap().parent().unwrap().to_path_buf();
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1, "only the entry itself is left");

        std::env::remove_var("XDG_CACHE_HOME");
    }
}
