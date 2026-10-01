//! Shared GTK building blocks for the OpusDM processes: the ReqTools-style
//! requesters and the skeleton the larger dialogs are built on.

mod requester;
mod screen;
mod shell;
mod theme_mode;
mod version;

pub use requester::{requester, requester_entry, requester_entry_selecting, requester_rich};
pub use screen::{list_monitors, watch_monitors, watch_screen_size};
pub use shell::DialogShell;
pub use theme_mode::{apply_theme_mode, chrome_css};
pub use version::VERSION;
