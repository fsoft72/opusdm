//! Pictures open in the stand-alone `opusdm-viewer`, the one image viewer of
//! OpusDM. The Lister's own Viewer window shows text, ANSI and hex only.

use crate::viewer_mode::image_files;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Name of the image viewer program.
pub const VIEWER_BIN: &str = "opusdm-viewer";

/// Split `files` into (pictures, everything else), keeping their order.
pub fn split_images(files: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let images = image_files(&files);
    let others = files.into_iter().filter(|f| !images.contains(f)).collect();
    (images, others)
}

/// The command that shows `files` in the image viewer: the binary next to
/// `exe_dir` (the Lister's own folder) when known, else a `PATH` lookup.
pub fn viewer_command(exe_dir: Option<&Path>, files: &[PathBuf]) -> Command {
    let bin = exe_dir.map(|d| d.join(VIEWER_BIN)).filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from(VIEWER_BIN));
    let mut cmd = Command::new(bin);
    cmd.args(files);
    cmd
}

/// Start the image viewer on `files`. A thread reaps it when it exits.
pub fn launch(files: &[PathBuf]) -> io::Result<()> {
    let exe = std::env::current_exe().ok();
    let mut child = viewer_command(exe.as_deref().and_then(Path::parent), files).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_images_keeps_the_order_of_both_halves() {
        let files: Vec<PathBuf> = ["/a/1.png", "/a/n.txt", "/a/2.JPG", "/a/m.rs"].iter().map(PathBuf::from).collect();
        let (images, others) = split_images(files);
        assert_eq!(images, vec![PathBuf::from("/a/1.png"), PathBuf::from("/a/2.JPG")]);
        assert_eq!(others, vec![PathBuf::from("/a/n.txt"), PathBuf::from("/a/m.rs")]);
    }

    #[test]
    fn the_viewer_is_taken_next_to_the_lister_then_from_path() {
        let files = vec![PathBuf::from("/p/a.png"), PathBuf::from("/p/b.png")];
        let td = tempfile::tempdir().unwrap();
        let cmd = viewer_command(Some(td.path()), &files);
        assert_eq!(cmd.get_program(), VIEWER_BIN, "no sibling binary: PATH lookup");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["/p/a.png", "/p/b.png"]);
        std::fs::write(td.path().join(VIEWER_BIN), "").unwrap();
        let cmd = viewer_command(Some(td.path()), &files);
        assert_eq!(Path::new(cmd.get_program()), td.path().join(VIEWER_BIN));
    }
}
