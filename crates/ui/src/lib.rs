//! Shared GTK building blocks for the OpusDM processes: the ReqTools-style
//! requesters and the skeleton the larger dialogs are built on.

mod fonts;
pub mod layer;
pub mod line_numbers;
mod look_css;
mod reference_picker;
mod requester;
mod screen;
mod shell;
mod theme_mode;
mod version;
mod x11;

pub use fonts::{font_files, register_theme_fonts};
pub use look_css::{font_rule, surface_rule, ui_look_css, REQUESTER_CLASS, ROW_ALPHA_WITH_PICTURE};
pub use reference_picker::{browsed_line, function_dropdown, selected_function, RefKind, RefPicker};
pub use requester::{requester, requester_entry, requester_entry_selecting, requester_rich};
pub use screen::{list_monitors, MonitorRect, watch_monitors, watch_screen_size};
pub use shell::DialogShell;
pub use theme_mode::{app_css, apply_theme_mode, chrome_css, install_app_css, ACCENT_COLOR_NAME};
pub use version::VERSION;
pub use x11::{retry_after_map, with_x11, x11_window_id, wrapped_desktop, x11_sync, X11Error, PLACEMENT_RETRY_MS, X11};
