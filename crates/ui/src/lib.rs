//! Shared GTK building blocks for the OpusDM processes: the ReqTools-style
//! requesters and the skeleton the larger dialogs are built on.

mod requester;
mod shell;
mod version;

pub use requester::{requester, requester_entry, requester_rich};
pub use shell::DialogShell;
pub use version::VERSION;
