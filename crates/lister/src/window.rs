//! Lister window. Populated in Task 13 (needs GTK).

use crate::client::{Client, Roles};
use opusdm_ipc::ListerId;
use std::path::PathBuf;

/// Placeholder window runner replaced in Task 13. Returns a process exit code.
pub fn run(_client: Client, _id: ListerId, _roles: Roles, _start: PathBuf) -> u8 {
    eprintln!("opusdm-lister: window not implemented yet (plan Task 13)");
    1
}
