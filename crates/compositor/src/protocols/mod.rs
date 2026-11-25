//! Extra Wayland protocols served by the compositor (beyond the Smithay
//! handlers of `shell`): wlr-foreign-toplevel-management for the panel and
//! ext-workspace for the pager, wlr-screencopy for `opusdm-shot`.

pub mod foreign_toplevel;
pub mod screencopy;
pub mod workspace;
