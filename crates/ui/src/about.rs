//! The About OpusDM window, shared by the hub desktop menu and the panel
//! start menu.

use gtk4 as gtk;
use gtk::prelude::*;

/// The OpusDM logo shown at the top of the About window.
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/opusdm-full.png");

/// Width of the logo in the window, in pixels (the file is 640x210).
const LOGO_WIDTH: i32 = 320;

/// Height of the logo in the window, keeping the 640x210 proportions.
const LOGO_HEIGHT: i32 = 105;

/// The logo as a picture, or `None` (logged) when it cannot be decoded.
fn _logo() -> Option<gtk::Widget> {
    let texture = match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_static(LOGO_PNG)) {
        Ok(texture) => texture,
        Err(e) => {
            opusdm_ipc::err_log!("ui", "cannot decode the About logo: {e}");
            return None;
        }
    };
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_size_request(LOGO_WIDTH, LOGO_HEIGHT);
    Some(picture.upcast())
}

/// Show the About OpusDM window over `parent`: the logo, then the tagline,
/// version and credits.
pub fn show_about(parent: &impl IsA<gtk::Window>) {
    crate::requester::requester_rich_header(
        parent,
        "About OpusDM",
        _logo(),
        &format!(
            "<i>An humble Linux tribute to Amiga Directory Opus 5</i>\n\n\
             Version {version}\n\n\
             Written with respect and dedication by Fabio Rotondo\n\
             <a href='https://github.com/fsof72'>https://github.com/fsof72</a>\n\n\
             Amiga will never die\n\n\
             <small>This software is Open Source</small>",
            version = crate::VERSION,
        ),
        &[("OK", ())],
        |_| {},
    );
}
