//! opusdm-snap: KDE-style edge snapping for X11 windows.
//!
//! The geometry (`zones`) is pure and unit tested. The drag detection and the
//! overlay are built on top of it and talk to X11 directly, so this crate
//! does not depend on GTK.

pub mod restore;
pub mod zones;

pub use restore::restore_rect;
pub use zones::{zone_at, zone_rect, Rect, Zone, EDGE_THRESHOLD};
