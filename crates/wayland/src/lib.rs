//! Wayland plumbing shared by the OpusDM processes, with no GTK: the
//! counterpart of `opusdm-x11` for a Wayland session. It watches the windows
//! of every client and the workspaces of the compositor (wlroots protocols,
//! offered by labwc, sway and others) and sends the requests about them;
//! [`screencopy`] takes screenshots.
//!
//! Everything here does nothing outside a Wayland session: [`shared`] is
//! `None` there.

mod client;
mod model;
pub mod screencopy;

pub use client::{shared, Session};
pub use model::{apply_states, pager_order, parse_coordinates, wrapped_index, Snapshot, Toplevel, ToplevelId, Workspace};
