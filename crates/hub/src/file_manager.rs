//! The `org.freedesktop.FileManager1` D-Bus service: programs such as web
//! browsers ("Show in folder") and chat apps ask the file manager to show a
//! file or a folder; here that opens a Lister.

use crate::server::HubCmd;
use gtk4::gio::prelude::FileExt;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Well known bus name of the service.
pub const BUS_NAME: &str = "org.freedesktop.FileManager1";
/// Object path the interface is served at.
const OBJECT_PATH: &str = "/org/freedesktop/FileManager1";

/// Local paths of the `file://` URIs in `uris`, without duplicates and in
/// the order given. Other schemes and malformed URIs are skipped.
pub fn paths_from_uris(uris: &[String]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for uri in uris {
        let Some(path) = gtk4::gio::File::for_uri(uri).path() else { continue };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// The service object: every request becomes one Lister per path.
struct FileManager {
    cmd_tx: Mutex<Sender<HubCmd>>,
}

impl FileManager {
    /// Open a Lister on each of `uris`; a file opens its folder with the file selected.
    fn open(&self, uris: &[String]) {
        let Ok(tx) = self.cmd_tx.lock() else { return };
        for path in paths_from_uris(uris) {
            let _ = tx.send(HubCmd::SpawnRequested { start_path: Some(path) });
        }
    }
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager {
    /// Show each folder in `uris`.
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        self.open(&uris);
    }

    /// Show each item in `uris` in its folder, selected.
    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        self.open(&uris);
    }

    /// There is no properties window: show the items instead.
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        self.open(&uris);
    }
}

/// Take the bus name and serve the interface on a background thread for the
/// life of the process. Fails (logged) when another file manager owns the
/// name: it is never taken over from a running one.
pub fn start(cmd_tx: Sender<HubCmd>) {
    let spawned = std::thread::Builder::new().name("opusdm-filemanager1".into()).spawn(move || {
        let service = FileManager { cmd_tx: Mutex::new(cmd_tx) };
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(BUS_NAME))
            .and_then(|b| b.serve_at(OBJECT_PATH, service))
            .and_then(|b| b.build());
        match conn {
            // Dropping the connection would release the name: park with it.
            Ok(_conn) => loop {
                std::thread::park();
            },
            Err(e) => opusdm_ipc::err_log!("opusdm-hub", "cannot serve {BUS_NAME} (another file manager owns it?): {e}"),
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the {BUS_NAME} thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn file_uris_become_paths_with_percent_decoding() {
        let got = paths_from_uris(&uris(&["file:///home/u/My%20Docs/a%26b.pdf"]));
        assert_eq!(got, vec![PathBuf::from("/home/u/My Docs/a&b.pdf")]);
    }

    #[test]
    fn duplicates_and_non_local_uris_are_dropped() {
        let got = paths_from_uris(&uris(&["file:///tmp/a", "https://example.com/x", "file:///tmp/a", "file:///tmp/b"]));
        assert_eq!(got, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
    }

    #[test]
    fn an_empty_request_opens_nothing() {
        assert!(paths_from_uris(&[]).is_empty());
    }
}
