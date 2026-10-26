//! Library of `opusdm-applet-browser`: the texts of the window (`model`,
//! tested without a display) and the GTK window (`window`) that lists the
//! applets and drives the `opusdm-applets` process over its socket.

pub mod model;
pub mod window;

/// Errors and broken applets in red.
pub const APP_CSS: &str = "
label.error { color: #ff7b63; }
";
