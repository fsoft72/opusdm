//! The ufw firewall tool of the Advanced tab of `opusdm-settings`: the rules
//! (`model`), the privileged run through `pkexec` (`backend`) and the page
//! (`window`). See `docs/superpowers/specs/2026-10-30-firewall-design.md`.

pub mod backend;
#[cfg(test)]
mod gtk_test;
pub mod model;
mod rule_dialog;
pub mod window;
