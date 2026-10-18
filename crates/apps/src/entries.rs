//! Desktop applications as Run-style search entries, shared by the Run
//! window and the Lister's "Open with..." dialog.

use crate::search::{Entry, Kind};
use gtk4::gio;
use gtk4::prelude::*;

/// The entry for one desktop application; `None` when it has no id.
pub fn app_entry(info: &gio::DesktopAppInfo) -> Option<Entry> {
    let id = info.id()?.to_string();
    let generic = info.generic_name().map(|s| s.to_string()).unwrap_or_default();
    let description = info.description().map(|s| s.to_string()).unwrap_or_default();
    let mut extra = vec![generic.clone()];
    extra.extend(info.keywords().iter().map(|k| k.to_string()));
    if let Some(program) = info.executable().file_name() {
        extra.push(program.to_string_lossy().to_string());
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
