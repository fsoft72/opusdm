//! Save the frame on screen as a PNG, at the full resolution of the video.

use gtk4 as gtk;
use gtk::{gdk, gio, graphene};
use gtk::prelude::*;
use opusdm_media::timefmt::format_time;
use opusdm_media::Engine;
use std::path::{Path, PathBuf};

/// Extension of the saved file.
const PNG_EXT: &str = "png";

/// The frame currently shown by `paintable`, as a texture of the size of the
/// video (not of the widget). `Err` says why there is none: an audio file, a
/// video that has no frame yet, or a window without a renderer.
pub fn frame_texture(paintable: &gdk::Paintable, window: &gtk::ApplicationWindow) -> Result<gdk::Texture, String> {
    let current = paintable.current_image();
    let (w, h) = (current.intrinsic_width(), current.intrinsic_height());
    if w <= 0 || h <= 0 { return Err("there is no video frame to save yet".into()); }
    let snapshot = gtk::Snapshot::new();
    current.snapshot(&snapshot, w as f64, h as f64);
    let node = snapshot.to_node().ok_or("the frame is empty")?;
    let renderer = window.native().and_then(|n| n.renderer()).ok_or("the window has no renderer")?;
    Ok(renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32))))
}

/// The button and key action: grab the frame now, then ask where to save it.
/// The frame is taken before the dialog opens, so it is the one the user saw
/// when pressing, even if the video keeps playing. `video` is the opened file.
pub fn save_frame(window: &gtk::ApplicationWindow, engine: &Engine, video: &str) {
    let Some(paintable) = engine.paintable() else { return _fail(window, "this player has no video output") };
    match frame_texture(&paintable, window) {
        Ok(texture) => ask_and_save(window, texture, video, engine.position().unwrap_or(0)),
        Err(e) => _fail(window, &e),
    }
}

/// Ask where to save `texture` and write it as PNG. `video` is the opened file
/// or URL and `position_ns` the time of the frame: they only propose a name and
/// a folder. Every failure is logged and shown in a requester; a dismissed
/// dialog does nothing.
pub fn ask_and_save(window: &gtk::ApplicationWindow, texture: gdk::Texture, video: &str, position_ns: u64) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("PNG image"));
    filter.add_suffix(PNG_EXT);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title("Save frame").modal(true).build();
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));
    dialog.set_initial_name(Some(&proposed_name(video, position_ns)));
    if let Some(dir) = Path::new(video).parent().filter(|d| d.is_dir()) {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let w = window.clone();
    dialog.save(Some(window), gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(f) => f,
            Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
            Err(e) => return _fail(&w, &format!("the save dialog failed: {e}")),
        };
        let Some(chosen) = file.path() else { return _fail(&w, "the chosen location is not a local file") };
        let target = with_png_extension(chosen.clone());
        // The dialog already asked about `chosen`; only a name changed by us needs asking.
        if target == chosen || !target.exists() { return _write(&w, &texture, &target); }
        let message = format!("{} already exists. Replace it?", target.display());
        let w2 = w.clone();
        opusdm_ui::requester(&w, "Replace file?", &message, None, &[("Replace", true), ("Cancel", false)], move |replace| {
            if replace { _write(&w2, &texture, &target); }
        });
    });
}

/// Write `texture` to `target`, reporting a failure.
fn _write(window: &gtk::ApplicationWindow, texture: &gdk::Texture, target: &Path) {
    if let Err(e) = texture.save_to_png(target) {
        _fail(window, &format!("cannot save {}: {e}", target.display()));
    }
}

/// Log `message` and show it.
fn _fail(window: &gtk::ApplicationWindow, message: &str) {
    opusdm_ipc::err_log!("player", "{message}");
    opusdm_ui::requester(window, "Save frame", message, None, &[("Close", ())], |_| {});
}

/// `<video name>_<mm-ss>.png`, e.g. `holiday_01-23.png`.
pub fn proposed_name(video: &str, position_ns: u64) -> String {
    let stem = Path::new(video).file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "frame".into());
    let time = format_time(position_ns).replace(':', "-");
    format!("{stem}_{time}.{PNG_EXT}")
}

/// `path` when it already ends in `.png` (any case), else `.png` is appended.
pub fn with_png_extension(path: PathBuf) -> PathBuf {
    let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(PNG_EXT));
    if is_png { return path; }
    let mut name = path.into_os_string();
    name.push(format!(".{PNG_EXT}"));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_has_the_video_stem_and_the_time() {
        assert_eq!(proposed_name("/home/a/holiday.mp4", 83_000_000_000), "holiday_01-23.png");
        assert_eq!(proposed_name("https://x.org/a/clip.webm", 0), "clip_00-00.png");
        assert_eq!(proposed_name("/", 0), "frame_00-00.png");
    }

    #[test]
    fn png_extension_is_kept_or_appended() {
        assert_eq!(with_png_extension("/t/a.png".into()), PathBuf::from("/t/a.png"));
        assert_eq!(with_png_extension("/t/a.PNG".into()), PathBuf::from("/t/a.PNG"));
        assert_eq!(with_png_extension("/t/a".into()), PathBuf::from("/t/a.png"));
        assert_eq!(with_png_extension("/t/a.mp4".into()), PathBuf::from("/t/a.mp4.png"));
    }
}
