//! The user-facing OpusDM version string.
//!
//! It is the workspace version: to cut a release, edit `version` under
//! `[workspace.package]` in the root `Cargo.toml` and nothing else.

/// Human-readable OpusDM version, shown in the About requester.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
