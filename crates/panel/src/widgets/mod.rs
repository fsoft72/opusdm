//! The individual panel widgets. Each `build()` (or `new()`) returns a ready
//! GTK widget that owns its own refresh timer or is fed by the EWMH observer.

pub mod clock;
pub mod pager;
pub mod sni;
pub mod sysmon;
pub mod tasklist;
pub mod tray;
