//! Shared GTK building blocks for the OpusDM processes: the ReqTools-style
//! requesters, the skeleton the larger dialogs are built on and the small
//! form helpers (headings, rows, file picking, unsaved-changes guards).

mod about;
mod app_icon;
mod dirty_guard;
mod file_pick;
mod fonts;
mod form_widgets;
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

pub use about::show_about;
pub use app_icon::{set_app_icon, APP_ICON_NAME};
pub use dirty_guard::{confirm_discard, DirtyGuard};
pub use file_pick::{pick_file, pick_file_or_error, FileFilterSpec, FilePick};
pub use form_widgets::{clear_children, labeled_row, section_heading, LabelWidth, RemoveChildren};
pub use fonts::{font_files, register_theme_fonts};
pub use look_css::{font_rule, surface_rule, ui_look_css, REQUESTER_CLASS, ROW_ALPHA_WITH_PICTURE};
pub use reference_picker::{browsed_line, function_dropdown, selected_function, RefKind, RefPicker};
pub use requester::{requester, requester_entry, requester_entry_selecting, requester_rich};
pub use screen::{list_monitors, MonitorRect, watch_layout, watch_monitors, watch_screen_size};
pub use shell::DialogShell;
pub use theme_mode::{app_css, apply_theme_mode, chrome_css, install_app_css, ACCENT_COLOR_NAME};
pub use version::VERSION;
pub use x11::{retry_after_map, with_x11, x11_window_id, wrapped_desktop, x11_sync, X11Error, PLACEMENT_RETRY_MS, X11};
