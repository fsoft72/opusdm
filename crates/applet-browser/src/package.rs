//! Export and import of one applet as a `.tar.gz`. Pure: no GTK, no socket.
//!
//! The archive holds the applet folder only (manifest, scripts, assets), as
//! a single top-level folder named after the applet id: `clock/applet.json`,
//! `clock/main.js`. Its settings and position are not part of it.
//!
//! Import is two steps so the window can ask the user before anything is
//! installed: `stage_archive` unpacks and validates into a hidden folder
//! inside the applets folder and returns a `Staged`; `Staged::install` moves
//! it into place (replacing an applet with the same id). Dropping a `Staged`
//! removes the staging folder.

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use opusdm_applets::discovery::MANIFEST_FILE;
use opusdm_applets::manifest::{self, Manifest};
use opusdm_ipc::err_log;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// Tag used in log lines.
const LOG_TAG: &str = "applet-browser";
/// Extension of the archives, with the dot.
pub const ARCHIVE_EXT: &str = ".tar.gz";
/// Another spelling of the same archive kind, accepted on import.
const SHORT_EXT: &str = ".tgz";
/// Most bytes an archive may unpack to: an applet is a few scripts, so a
/// bigger one is a mistake or a decompression bomb.
pub const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;
/// Prefix of the staging folder; names starting with `.` are not applets.
const STAGING_PREFIX: &str = ".import-";
/// Where the replaced applet waits inside the staging folder.
const PREVIOUS_DIR: &str = ".previous";

/// An unpacked and validated archive, waiting for the user's confirmation.
#[derive(Debug)]
pub struct Staged {
    /// The applet id: the top-level folder of the archive.
    pub id: String,
    /// The manifest read from the archive (never from an installed applet).
    pub manifest: Manifest,
    /// The folder the applet is installed into.
    applets_dir: PathBuf,
    /// The staging folder; removed when this value is dropped.
    staging: tempfile::TempDir,
}

/// The suggested file name of the archive of applet `id`.
pub fn archive_name(id: &str) -> String {
    format!("{id}{ARCHIVE_EXT}")
}

/// `path` with `.tar.gz` appended unless it already ends with `.tar.gz` or `.tgz`.
pub fn with_archive_extension(path: PathBuf) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(ARCHIVE_EXT) || name.ends_with(SHORT_EXT) { return path; }
    let mut os = path.into_os_string();
    os.push(ARCHIVE_EXT);
    PathBuf::from(os)
}

/// Write the folder `applet_dir` of applet `id` to the archive `dest`.
///
/// Only regular files and folders are packed; a symlink or other entry is
/// skipped with a log line, so the archive never points outside the applet.
/// A failure removes the partial archive.
pub fn export_applet(applet_dir: &Path, id: &str, dest: &Path) -> Result<(), String> {
    if !applet_dir.is_dir() { return Err(format!("{} is not a folder", applet_dir.display())); }
    let result = _write_archive(applet_dir, id, dest);
    if let Err(e) = &result {
        if let Err(rm) = fs::remove_file(dest) {
            if rm.kind() != io::ErrorKind::NotFound { err_log!(LOG_TAG, "export: cannot remove the partial {}: {rm}", dest.display()); }
        }
        return Err(format!("cannot write {}: {e}", dest.display()));
    }
    Ok(())
}

/// Create `dest` and pack `applet_dir` into it as `<id>/...`.
fn _write_archive(applet_dir: &Path, id: &str, dest: &Path) -> io::Result<()> {
    let file = File::create(dest)?;
    let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    _add_dir(&mut builder, applet_dir, Path::new(id))?;
    builder.into_inner()?.finish()?.flush()
}

/// Add folder `dir` as `name` and, sorted by name, everything inside it.
fn _add_dir<W: Write>(builder: &mut tar::Builder<W>, dir: &Path, name: &Path) -> io::Result<()> {
    builder.append_dir(name, dir)?;
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let inner = name.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            _add_dir(builder, &path, &inner)?;
        } else if kind.is_file() {
            builder.append_path_with_name(&path, &inner)?;
        } else {
            err_log!(LOG_TAG, "export: skipping {} (not a regular file or folder)", path.display());
        }
    }
    Ok(())
}

/// Unpack the archive at `archive` into a staging folder inside
/// `applets_dir` and check it: one top-level folder (the id) holding a valid
/// `applet.json` and the entry script it names. Nothing is installed.
pub fn stage_archive(archive: &Path, applets_dir: &Path) -> Result<Staged, String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !name.ends_with(ARCHIVE_EXT) && !name.ends_with(SHORT_EXT) {
        return Err("unsupported file type (expected .tar.gz)".into());
    }
    fs::create_dir_all(applets_dir).map_err(|e| format!("cannot create {}: {e}", applets_dir.display()))?;
    let staging = tempfile::Builder::new().prefix(STAGING_PREFIX).tempdir_in(applets_dir)
        .map_err(|e| format!("cannot create a staging folder in {}: {e}", applets_dir.display()))?;
    let id = _unpack(archive, staging.path())?;
    let folder = staging.path().join(&id);
    let text = fs::read_to_string(folder.join(MANIFEST_FILE)).map_err(|e| format!("{id}/{MANIFEST_FILE}: {e}"))?;
    let manifest = manifest::parse(&text).map_err(|e| format!("{id}/{MANIFEST_FILE}: {e}"))?;
    if !folder.join(&manifest.entry).is_file() {
        return Err(format!("the entry script '{}' is missing from the archive", manifest.entry));
    }
    Ok(Staged { id, manifest, applets_dir: applets_dir.to_path_buf(), staging })
}

/// Unpack `archive` under `dest` and return the id (its one top-level folder).
///
/// Refuses absolute paths and `..`, more than one top-level name, a file at
/// the top level and an archive bigger than `MAX_UNPACKED_BYTES`; symlinks,
/// hard links and device entries are skipped with a log line.
fn _unpack(archive: &Path, dest: &Path) -> Result<String, String> {
    let file = File::open(archive).map_err(|e| format!("cannot open {}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let mut id: Option<String> = None;
    let mut total: u64 = 0;
    for entry in tar.entries().map_err(|e| format!("not a valid .tar.gz: {e}"))? {
        let mut entry = entry.map_err(|e| format!("not a valid .tar.gz: {e}"))?;
        let path = entry.path().map_err(|e| format!("bad entry name: {e}"))?.into_owned();
        let shown = path.to_string_lossy().into_owned();
        let parts = _parts(&path).ok_or_else(|| format!("unsafe path in the archive: {shown}"))?;
        let Some(top) = parts.first() else { continue };
        match &id {
            None => id = Some(_check_id(top)?),
            Some(first) if first != top => return Err(format!("the archive must hold one applet folder, found '{first}' and '{top}'")),
            Some(_) => {}
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            err_log!(LOG_TAG, "import: skipping non-regular archive entry {shown}");
            continue;
        }
        if kind.is_file() && parts.len() == 1 { return Err(format!("'{shown}' is a file at the top level: the archive must hold one applet folder")); }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES { return Err(format!("the archive unpacks to more than {} MB", MAX_UNPACKED_BYTES / 1024 / 1024)); }
        let out = parts.iter().fold(dest.to_path_buf(), |p, part| p.join(part));
        if let Some(parent) = out.parent() { fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?; }
        entry.unpack(&out).map_err(|e| format!("cannot unpack {shown}: {e}"))?;
    }
    id.ok_or_else(|| "the archive is empty".to_string())
}

/// The normal components of `path`, or `None` if it has an absolute prefix or `..`.
fn _parts(path: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts)
}

/// `top` as an applet id: not hidden (hidden names are not applets).
fn _check_id(top: &str) -> Result<String, String> {
    if top.starts_with('.') { return Err(format!("'{top}' cannot be an applet folder: names starting with '.' are skipped")); }
    Ok(top.to_string())
}

/// Remove the applet folder `id` from `applets_dir`.
///
/// `id` must be one plain folder name, not hidden: nothing outside the
/// applets folder can be reached. A symlink to a folder (the applets folder
/// follows them) is removed itself, never what it points to. Settings and
/// position are in the state folder and are not touched.
pub fn delete_applet(applets_dir: &Path, id: &str) -> Result<(), String> {
    let single = _parts(Path::new(id)).filter(|p| p.len() == 1 && p[0] == id);
    if single.is_none() || id.starts_with('.') { return Err(format!("'{id}' is not an applet folder name")); }
    let path = applets_dir.join(id);
    let meta = path.symlink_metadata().map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let removed = if meta.is_dir() { fs::remove_dir_all(&path) } else if meta.file_type().is_symlink() { fs::remove_file(&path) } else {
        return Err(format!("{} is not a folder", path.display()));
    };
    removed.map_err(|e| format!("cannot remove {}: {e}", path.display()))
}

impl Staged {
    /// True if an applet with this id is installed and would be replaced.
    pub fn replaces(&self) -> bool {
        self.applets_dir.join(&self.id).symlink_metadata().is_ok()
    }

    /// Move the staged applet into the applets folder. An applet with the
    /// same id is kept aside first and put back if the move fails.
    pub fn install(self) -> Result<(), String> {
        let dest = self.applets_dir.join(&self.id);
        let staged = self.staging.path().join(&self.id);
        let previous = self.staging.path().join(PREVIOUS_DIR);
        let had_previous = self.replaces();
        if had_previous {
            fs::rename(&dest, &previous).map_err(|e| format!("cannot replace {}: {e}", dest.display()))?;
        }
        if let Err(e) = fs::rename(&staged, &dest) {
            if had_previous {
                if let Err(back) = fs::rename(&previous, &dest) { err_log!(LOG_TAG, "import: cannot put back {}: {back}", dest.display()); }
            }
            return Err(format!("cannot install {}: {e}", dest.display()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid `applet.json` for tests.
    const MANIFEST: &str = r#"{"name":"Clock","version":"1.0","permissions":["net","exec"]}"#;

    /// An applet folder `id` under `root` with a manifest, `main.js` and an asset.
    fn make_applet(root: &Path, id: &str) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(dir.join("img")).unwrap();
        fs::write(dir.join(MANIFEST_FILE), MANIFEST).unwrap();
        fs::write(dir.join("main.js"), "// code").unwrap();
        fs::write(dir.join("img/a.txt"), "asset").unwrap();
        dir
    }

    /// Set the raw name of a tar header (`set_path` refuses `..` and `/`).
    fn raw_name(header: &mut tar::Header, name: &str) {
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    }

    /// A `.tar.gz` with the given `(name, kind, content)` entries, names written as is.
    fn archive_of(dir: &Path, entries: &[(&str, tar::EntryType, &str)]) -> PathBuf {
        let path = dir.join("in.tar.gz");
        let mut builder = tar::Builder::new(GzEncoder::new(File::create(&path).unwrap(), Compression::default()));
        for (name, kind, content) in entries {
            let mut h = tar::Header::new_gnu();
            raw_name(&mut h, name);
            h.set_entry_type(*kind);
            h.set_mode(0o644);
            h.set_size(if *kind == tar::EntryType::Regular { content.len() as u64 } else { 0 });
            if *kind == tar::EntryType::Symlink { h.set_link_name("/etc/passwd").unwrap(); }
            h.set_cksum();
            builder.append(&h, content.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    const FILE: tar::EntryType = tar::EntryType::Regular;

    #[test]
    fn the_archive_name_and_extension() {
        assert_eq!(archive_name("clock"), "clock.tar.gz");
        assert_eq!(with_archive_extension(PathBuf::from("/x/clock")), PathBuf::from("/x/clock.tar.gz"));
        assert_eq!(with_archive_extension(PathBuf::from("/x/clock.tar.gz")), PathBuf::from("/x/clock.tar.gz"));
        assert_eq!(with_archive_extension(PathBuf::from("/x/Clock.TGZ")), PathBuf::from("/x/Clock.TGZ"));
    }

    #[test]
    fn export_then_stage_then_install_round_trips() {
        let src = tempfile::tempdir().unwrap();
        let dir = make_applet(src.path(), "clock");
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("clock.tar.gz");
        export_applet(&dir, "clock", &archive).unwrap();

        let target = tempfile::tempdir().unwrap();
        let applets = target.path().join("applets");
        let staged = stage_archive(&archive, &applets).unwrap();
        assert_eq!(staged.id, "clock");
        assert_eq!(staged.manifest.name, "Clock");
        assert_eq!(staged.manifest.permissions, vec!["net", "exec"]);
        assert!(!staged.replaces());
        staged.install().unwrap();
        assert_eq!(fs::read_to_string(applets.join("clock/main.js")).unwrap(), "// code");
        assert_eq!(fs::read_to_string(applets.join("clock/img/a.txt")).unwrap(), "asset");
        let left: Vec<_> = fs::read_dir(&applets).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("clock")], "the staging folder is gone");
    }

    #[test]
    fn export_packs_files_and_folders_and_skips_symlinks() {
        let src = tempfile::tempdir().unwrap();
        let dir = make_applet(src.path(), "clock");
        std::os::unix::fs::symlink("/etc/passwd", dir.join("link")).unwrap();
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("clock.tar.gz");
        export_applet(&dir, "clock", &archive).unwrap();
        let mut names: Vec<String> = tar::Archive::new(GzDecoder::new(File::open(&archive).unwrap()))
            .entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["clock", "clock/applet.json", "clock/img", "clock/img/a.txt", "clock/main.js"]);
    }

    #[test]
    fn a_failed_export_leaves_no_partial_archive() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("x.tar.gz");
        let err = export_applet(&src.path().join("missing"), "missing", &archive).unwrap_err();
        assert!(err.contains("not a folder"), "{err}");
        assert!(!archive.exists());
        let err = export_applet(src.path(), "x", &out.path().join("no-such-dir/x.tar.gz")).unwrap_err();
        assert!(err.contains("cannot write"), "{err}");
    }

    #[test]
    fn stage_rejects_a_wrong_file_type_and_garbage() {
        let d = tempfile::tempdir().unwrap();
        let zip = d.path().join("a.zip");
        fs::write(&zip, "x").unwrap();
        assert!(stage_archive(&zip, &d.path().join("apps")).unwrap_err().contains("unsupported file type"));
        let bad = d.path().join("bad.tar.gz");
        fs::write(&bad, "not gzip").unwrap();
        assert!(stage_archive(&bad, &d.path().join("apps")).unwrap_err().contains("not a valid .tar.gz"));
        assert!(stage_archive(&d.path().join("gone.tar.gz"), &d.path().join("apps")).unwrap_err().contains("cannot open"));
    }

    #[test]
    fn stage_rejects_unsafe_paths() {
        let d = tempfile::tempdir().unwrap();
        for name in ["../evil/applet.json", "/abs/applet.json", "clock/../../evil"] {
            let a = archive_of(d.path(), &[(name, FILE, "x")]);
            let err = stage_archive(&a, &d.path().join("apps")).unwrap_err();
            assert!(err.contains("unsafe path"), "{name}: {err}");
        }
        assert!(!d.path().join("evil").exists());
    }

    #[test]
    fn stage_needs_exactly_one_top_level_folder() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        let err = stage_archive(&archive_of(d.path(), &[("applet.json", FILE, MANIFEST)]), &apps).unwrap_err();
        assert!(err.contains("top level"), "{err}");
        let two = archive_of(d.path(), &[("a/applet.json", FILE, MANIFEST), ("b/applet.json", FILE, MANIFEST)]);
        assert!(stage_archive(&two, &apps).unwrap_err().contains("one applet folder"));
        let hidden = archive_of(d.path(), &[(".h/applet.json", FILE, MANIFEST)]);
        assert!(stage_archive(&hidden, &apps).unwrap_err().contains("cannot be an applet folder"));
        let empty = archive_of(d.path(), &[]);
        assert!(stage_archive(&empty, &apps).unwrap_err().contains("empty"));
    }

    #[test]
    fn stage_validates_the_manifest_and_the_entry() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        let no_manifest = archive_of(d.path(), &[("c/main.js", FILE, "x")]);
        assert!(stage_archive(&no_manifest, &apps).unwrap_err().contains("c/applet.json"));
        let bad = archive_of(d.path(), &[("c/applet.json", FILE, r#"{"name":"  "}"#), ("c/main.js", FILE, "x")]);
        assert!(stage_archive(&bad, &apps).unwrap_err().contains("'name' must not be empty"));
        let no_entry = archive_of(d.path(), &[("c/applet.json", FILE, MANIFEST)]);
        assert!(stage_archive(&no_entry, &apps).unwrap_err().contains("entry script 'main.js' is missing"));
        let leftover: Vec<_> = fs::read_dir(&apps).unwrap().collect();
        assert!(leftover.is_empty(), "a failed stage leaves no staging folder");
    }

    #[test]
    fn stage_skips_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let a = archive_of(d.path(), &[("c/applet.json", FILE, MANIFEST), ("c/main.js", FILE, "x"), ("c/link", tar::EntryType::Symlink, "")]);
        let staged = stage_archive(&a, &d.path().join("apps")).unwrap();
        assert!(staged.staging.path().join("c/link").symlink_metadata().is_err());
    }

    #[test]
    fn stage_refuses_an_archive_that_unpacks_too_big() {
        let d = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_UNPACKED_BYTES as usize / 2 + 1);
        let a = archive_of(d.path(), &[("c/applet.json", FILE, MANIFEST), ("c/main.js", FILE, &big), ("c/more.js", FILE, &big)]);
        assert!(stage_archive(&a, &d.path().join("apps")).unwrap_err().contains("more than 64 MB"));
    }

    #[test]
    fn install_replaces_an_existing_applet_and_keeps_its_state_files() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        let old = make_applet(&apps, "clock");
        fs::write(old.join("old-only.txt"), "gone").unwrap();
        let a = archive_of(d.path(), &[("clock/applet.json", FILE, MANIFEST), ("clock/main.js", FILE, "// new")]);
        let staged = stage_archive(&a, &apps).unwrap();
        assert!(staged.replaces());
        staged.install().unwrap();
        assert_eq!(fs::read_to_string(apps.join("clock/main.js")).unwrap(), "// new");
        assert!(!apps.join("clock/old-only.txt").exists());
        assert_eq!(fs::read_dir(&apps).unwrap().count(), 1);
    }

    #[test]
    fn dropping_a_staged_archive_installs_nothing() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        let a = archive_of(d.path(), &[("clock/applet.json", FILE, MANIFEST), ("clock/main.js", FILE, "x")]);
        drop(stage_archive(&a, &apps).unwrap());
        assert_eq!(fs::read_dir(&apps).unwrap().count(), 0);
    }

    #[test]
    fn delete_removes_the_applet_folder_only() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        make_applet(&apps, "clock");
        make_applet(&apps, "other");
        delete_applet(&apps, "clock").unwrap();
        assert!(!apps.join("clock").exists());
        assert!(apps.join("other/main.js").exists());
    }

    #[test]
    fn delete_of_a_symlink_keeps_what_it_points_to() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        fs::create_dir_all(&apps).unwrap();
        let real = make_applet(d.path(), "real");
        std::os::unix::fs::symlink(&real, apps.join("clock")).unwrap();
        delete_applet(&apps, "clock").unwrap();
        assert!(apps.join("clock").symlink_metadata().is_err());
        assert!(real.join("main.js").exists(), "the target is untouched");
    }

    #[test]
    fn delete_refuses_names_that_leave_the_applets_folder() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("apps");
        make_applet(&apps, "clock");
        let outside = make_applet(d.path(), "outside");
        for id in ["", ".", "..", "../outside", "clock/img", "/abs", ".hidden", "clock/"] {
            assert!(delete_applet(&apps, id).is_err(), "{id:?} must be refused");
        }
        assert!(outside.join("main.js").exists());
        assert!(apps.join("clock/main.js").exists());
        assert!(delete_applet(&apps, "missing").unwrap_err().contains("cannot read"));
    }
}
