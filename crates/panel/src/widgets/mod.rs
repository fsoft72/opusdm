//! The individual panel widgets. Each `build()` (or `new()`) returns a ready
//! GTK widget that owns its own refresh timer or is fed by the EWMH observer.

pub mod audio;
pub mod clock;
pub mod devices;
pub mod network;
pub mod pager;
pub mod quickapps;
pub mod sni;
pub mod sysmon;
pub mod tasklist;
pub mod tray;
