//! opusdm-apps: the installed applications as the OpusDM processes see
//! them. Resolving and launching pinned apps, the default application
//! menu, and the one lookup of an application or theme icon shared by the
//! panel, the dock, the notification popups and the polkit agent.

pub mod apps;
pub mod icons;
pub mod menu_defaults;

pub use icons::{app_image, desktop_icon, themed_or};
