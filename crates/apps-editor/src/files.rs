//! Where the editor reads and writes `.desktop` files.

use gtk4 as gtk;
use gtk::gio;
use opusdm_ipc::desktop_entry::{self, DesktopFields};
use std::io;
use std::path::PathBuf;

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

/// The text of the file currently providing `id`, if there is one.
fn _source_text(id: &str) -> Option<String> {
    let info = if id.starts_with('/') { gio::DesktopAppInfo::from_filename(id) } else { gio::DesktopAppInfo::new(id) }?;
    std::fs::read_to_string(info.filename()?).ok()
}

/// The editable fields of the installed entry `id`, `None` when it is not installed.
pub fn load_fields(id: &str) -> Option<DesktopFields> {
    _source_text(id).map(|t| desktop_entry::parse(&t))
}

/// Whether an entry with this `id` exists on the system or in the user directory.
pub fn id_taken(id: &str) -> bool {
    gio::DesktopAppInfo::new(id).is_some() || target_path(id).is_some_and(|p| p.exists())
}

/// Write `fields` for `id`, based on the file that currently provides it (so
/// unrelated keys survive), through a temporary file and a rename.
pub fn write_entry(id: &str, fields: &DesktopFields) -> io::Result<()> {
    let target = target_path(id).ok_or_else(|| io::Error::other("no user data directory"))?;
    let text = desktop_entry::render(_source_text(id).as_deref(), fields);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension(format!("desktop.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &target)
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

    #[test]
    fn deletes_that_work_leave_nothing_pending() {
        let (pending, errors) = delete_entries(vec!["opusdm-a.desktop".to_string()], |_| Ok(()));
        assert!(pending.is_empty() && errors.is_empty());
    }
}
