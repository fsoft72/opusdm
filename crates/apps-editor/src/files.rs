//! Where the editor reads and writes `.desktop` files.

use gtk4 as gtk;
use gtk::gio;
use opusdm_ipc::desktop_entry::{self, DesktopFields};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Prefix of the entries the editor creates, the only ones it may delete.
pub const OWN_PREFIX: &str = "opusdm-";

/// Where the entry `id` is written: the file itself for an absolute path,
/// else the user applications directory, which shadows a system entry of the
/// same id.
pub fn target_path(id: &str) -> Option<PathBuf> {
    if id.starts_with('/') {
        return Some(PathBuf::from(id));
    }
    Some(dirs::data_dir()?.join("applications").join(id))
}

/// The file currently providing `id`: the one the system resolves, else the
/// file at the target path when it exists (an entry the system does not
/// list, such as a broken one, is still the file the write replaces).
fn _source_path(id: &str) -> Option<PathBuf> {
    let info = if id.starts_with('/') { gio::DesktopAppInfo::from_filename(id) } else { gio::DesktopAppInfo::new(id) };
    info.and_then(|i| i.filename()).or_else(|| target_path(id).filter(|p| p.exists()))
}

/// The text of `source`, `None` when there is no source. A source that
/// exists but cannot be read is an error, never a missing file.
fn _read_source(source: Option<&Path>) -> io::Result<Option<String>> {
    let Some(source) = source else { return Ok(None) };
    std::fs::read_to_string(source)
        .map(Some)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot read {}: {e}", source.display())))
}

/// The editable fields of the installed entry `id`, `None` when it is not
/// installed or cannot be read (logged).
pub fn load_fields(id: &str) -> Option<DesktopFields> {
    match _read_source(_source_path(id).as_deref()) {
        Ok(text) => text.map(|t| desktop_entry::parse(&t)),
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-apps-editor", "{e}");
            None
        }
    }
}

/// Whether an entry with this `id` exists on the system or in the user directory.
pub fn id_taken(id: &str) -> bool {
    gio::DesktopAppInfo::new(id).is_some() || target_path(id).is_some_and(|p| p.exists())
}

/// Write `fields` for `id`, based on the file that currently provides it (so
/// unrelated keys survive), through a temporary file and a rename.
pub fn write_entry(id: &str, fields: &DesktopFields) -> io::Result<()> {
    let target = target_path(id).ok_or_else(|| io::Error::other("no user data directory"))?;
    _write_entry_at(&target, _source_path(id).as_deref(), fields)
}

/// Write `fields` applied to the text of `source` (`None` for a new entry)
/// to `target`. An unreadable source stops the write, so its other keys are
/// never lost to a file holding only the editable fields.
fn _write_entry_at(target: &Path, source: Option<&Path>, fields: &DesktopFields) -> io::Result<()> {
    let original = _read_source(source)?;
    let text = desktop_entry::render(original.as_deref(), fields);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    _write_atomic(target, text.as_bytes())
}

/// Write `data` to `path` through a synced sibling temporary file and a
/// rename, so a crash or a full disk leaves the old file intact. The
/// temporary file is removed when anything fails. (The same steps as
/// `opusdm_ipc::settings::save_json`, which only takes JSON.)
fn _write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let written = _write_synced(&tmp, data).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        if let Err(e) = std::fs::remove_file(&tmp) {
            if e.kind() != io::ErrorKind::NotFound {
                opusdm_ipc::err_log!("opusdm-apps-editor", "cannot remove {}: {e}", tmp.display());
            }
        }
    }
    written
}

/// Write `data` to `path` and flush it to disk before returning.
fn _write_synced(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// Delete the user file of an entry the editor created. Anything else is
/// left alone.
pub fn delete_own_entry(id: &str) -> io::Result<()> {
    if id.starts_with('/') || !id.starts_with(OWN_PREFIX) {
        return Ok(());
    }
    let Some(path) = target_path(id) else { return Ok(()) };
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Delete every entry of `ids` with `delete`. Returns the ids whose delete
/// failed, to try again at the next save, and one error line for each.
pub fn delete_entries(ids: Vec<String>, delete: impl Fn(&str) -> io::Result<()>) -> (Vec<String>, Vec<String>) {
    let mut pending = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        if let Err(e) = delete(&id) {
            errors.push(format!("{id}: {e}"));
            pending.push(id);
        }
    }
    (pending, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_delete_stays_pending() {
        let ids = vec!["opusdm-a.desktop".to_string(), "opusdm-b.desktop".to_string()];
        let (pending, errors) = delete_entries(ids, |id| if id == "opusdm-b.desktop" { Err(io::Error::other("read-only")) } else { Ok(()) });
        assert_eq!(pending, vec!["opusdm-b.desktop".to_string()]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("opusdm-b.desktop") && errors[0].contains("read-only"), "{}", errors[0]);
    }

    /// A fresh empty folder under the system temp folder, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("opusdm-apps-editor-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fields() -> DesktopFields {
        DesktopFields { name: "Mine".into(), exec: "mine %U".into(), ..DesktopFields::default() }
    }

    #[test]
    fn writing_keeps_the_unknown_keys_of_the_source() {
        let dir = TempDir::new("keeps");
        let source = dir.path().join("system.desktop");
        std::fs::write(&source, "[Desktop Entry]\nName=Old\nExec=old\nX-Custom=keep\nMimeType=text/plain;\n").unwrap();
        let target = dir.path().join("apps").join("mine.desktop");
        _write_entry_at(&target, Some(&source), &fields()).unwrap();
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("Name=Mine") && text.contains("Exec=mine %U"), "{text}");
        assert!(text.contains("X-Custom=keep") && text.contains("MimeType=text/plain;"), "{text}");
    }

    #[test]
    fn an_unreadable_source_stops_the_write() {
        let dir = TempDir::new("unreadable");
        // A folder cannot be read as text.
        let source = dir.path().join("unreadable.desktop");
        std::fs::create_dir(&source).unwrap();
        let target = dir.path().join("mine.desktop");
        std::fs::write(&target, "[Desktop Entry]\nName=Old\nX-Custom=keep\n").unwrap();
        let err = _write_entry_at(&target, Some(&source), &fields()).unwrap_err();
        assert!(err.to_string().contains("unreadable.desktop"), "{err}");
        assert!(std::fs::read_to_string(&target).unwrap().contains("X-Custom=keep"));
    }

    #[test]
    fn a_new_entry_has_no_source() {
        let dir = TempDir::new("new");
        let target = dir.path().join("mine.desktop");
        _write_entry_at(&target, None, &fields()).unwrap();
        assert!(std::fs::read_to_string(&target).unwrap().starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = TempDir::new("failed");
        // A non empty folder in the way makes the rename fail.
        let target = dir.path().join("mine.desktop");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("inside"), "x").unwrap();
        assert!(_write_entry_at(&target, None, &fields()).is_err());
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("mine.desktop")]);
    }

    #[test]
    fn deletes_that_work_leave_nothing_pending() {
        let (pending, errors) = delete_entries(vec!["opusdm-a.desktop".to_string()], |_| Ok(()));
        assert!(pending.is_empty() && errors.is_empty());
    }
}
