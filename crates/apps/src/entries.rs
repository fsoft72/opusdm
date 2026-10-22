//! Desktop applications as Run-style search entries, shared by the Run
//! window and the Lister's "Open with..." dialog.

use crate::search::{Entry, Kind};
use gtk4::gio;
use gtk4::prelude::*;

/// The file name of the program `info` runs; `None` when the entry has no
/// `Exec=`: GIO returns NULL there and the binding would panic.
fn program_name(info: &gio::DesktopAppInfo) -> Option<String> {
    if !info.has_key("Exec") {
        return None;
    }
    info.executable().file_name().map(|name| name.to_string_lossy().to_string())
}

/// The entry for one desktop application; `None` when it has no id.
pub fn app_entry(info: &gio::DesktopAppInfo) -> Option<Entry> {
    let id = info.id()?.to_string();
    let generic = info.generic_name().map(|s| s.to_string()).unwrap_or_default();
    let description = info.description().map(|s| s.to_string()).unwrap_or_default();
    let mut extra = vec![generic.clone()];
    extra.extend(info.keywords().iter().map(|k| k.to_string()));
    match program_name(info) {
        Some(program) => extra.push(program),
        None => opusdm_ipc::err_log!("apps", "{id}: desktop entry without Exec="),
    }
    extra.retain(|text| !text.is_empty());
    Some(Entry {
        kind: Kind::App,
        id,
        name: info.name().to_string(),
        comment: if generic.is_empty() { description } else { generic },
        extra,
        icon: info.icon().and_then(|icon| IconExt::to_string(&icon)).map(|s| s.to_string()),
    })
}

/// Every installed application a menu would show, as search entries.
pub fn installed_entries() -> Vec<Entry> {
    gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(|info| info.downcast::<gio::DesktopAppInfo>().ok())
        .filter_map(|info| app_entry(&info))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop entry without `Exec=` (like a hand-made `IsaacSim.desktop`)
    /// must not panic the entry builder: it used to kill the Lister when
    /// "Open with..." listed the installed applications.
    #[test]
    fn an_entry_without_exec_has_no_program_and_does_not_panic() {
        let keyfile = gtk4::glib::KeyFile::new();
        keyfile.set_string("Desktop Entry", "Type", "Application");
        keyfile.set_string("Desktop Entry", "Name", "No Exec");
        let info = gio::DesktopAppInfo::from_keyfile(&keyfile).expect("keyfile app info");
        assert_eq!(program_name(&info), None);
    }

    #[test]
    fn an_entry_with_exec_reports_its_program() {
        let keyfile = gtk4::glib::KeyFile::new();
        keyfile.set_string("Desktop Entry", "Type", "Application");
        keyfile.set_string("Desktop Entry", "Name", "With Exec");
        keyfile.set_string("Desktop Entry", "Exec", "/usr/bin/true %U");
        let info = gio::DesktopAppInfo::from_keyfile(&keyfile).expect("keyfile app info");
        assert_eq!(program_name(&info).as_deref(), Some("true"));
    }
}
