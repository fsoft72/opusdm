//! Extra Wayland protocols served by the compositor (beyond the Smithay
//! handlers of `shell`): wlr-foreign-toplevel-management for the panel and
//! ext-workspace for the pager.

pub mod foreign_toplevel;
pub mod workspace;
