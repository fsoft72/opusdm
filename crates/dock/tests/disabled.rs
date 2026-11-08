//! The dock process with the dock turned off: it must exit at once with
//! `EXIT_DISABLED`, before it opens a display or builds a window.

use std::process::Command;

#[test]
fn a_dock_turned_off_exits_with_exit_disabled_and_no_display() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("opusdm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dock.json"), br#"{"visible":false,"layout_migrated":true}"#).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_opusdm-dock"))
        .env("XDG_CONFIG_HOME", td.path())
        // No display at all: reaching GTK would fail with another code.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("OPUSDM_DOCK_SOCK", td.path().join("dock.sock"))
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(opusdm_ipc::instance::EXIT_DISABLED));
    assert!(!td.path().join("dock.sock").exists(), "the socket is not even bound");
}
