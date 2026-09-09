//! The user-facing OpusDM version string.
//!
//! This is deliberately kept in its own tiny module so the version can be
//! bumped by hand without touching any real code. Edit the `VERSION`
//! constant below whenever you cut a new release.

/// Human-readable OpusDM version, shown in the About requester.
pub const VERSION: &str = "0.1.0";
