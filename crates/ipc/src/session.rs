//! Which window system this process talks to: X11 or Wayland.
//!
//! OpusDM ships one binary for both sessions and picks the backend at run
//! time. The choice follows the order GDK uses, so the GTK processes and the
//! plain ones (snap, shot, the hub threads) always agree:
//!
//! 1. `GDK_BACKEND` naming `x11` or `wayland` first wins (a Wayland session
//!    can force a program onto XWayland this way);
//! 2. a non-empty `WAYLAND_DISPLAY` means Wayland;
//! 3. a non-empty `DISPLAY` means X11.
//!
//! Under a Wayland session `DISPLAY` is usually set as well (XWayland), so
//! "an X connection opens" is no proof of an X11 session: the X11 code paths
//! must ask [`is_x11`] instead.

use std::sync::OnceLock;

/// The window system of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSystem {
    /// An X server (Xorg, Xephyr, or XWayland when forced by `GDK_BACKEND`).
    X11,
    /// A Wayland compositor.
    Wayland,
    /// Neither display variable is set (tests, a TTY).
    None,
}

/// The window system named by the given environment values (`GDK_BACKEND`,
/// `WAYLAND_DISPLAY`, `DISPLAY`).
pub fn detect_from(gdk_backend: Option<&str>, wayland_display: Option<&str>, display: Option<&str>) -> WindowSystem {
    let set = |v: Option<&str>| v.is_some_and(|v| !v.trim().is_empty());
    let first_backend = gdk_backend.and_then(|b| b.split(',').next()).map(str::trim);
    if first_backend == Some("x11") && set(display) { return WindowSystem::X11; }
    if first_backend == Some("wayland") && set(wayland_display) { return WindowSystem::Wayland; }
    if set(wayland_display) { return WindowSystem::Wayland; }
    if set(display) { return WindowSystem::X11; }
    WindowSystem::None
}

/// The window system of this process, read from the environment once.
pub fn current() -> WindowSystem {
    static CURRENT: OnceLock<WindowSystem> = OnceLock::new();
    *CURRENT.get_or_init(|| {
        let var = |name| std::env::var(name).ok();
        detect_from(var("GDK_BACKEND").as_deref(), var("WAYLAND_DISPLAY").as_deref(), var("DISPLAY").as_deref())
    })
}

/// Whether this process runs on X11. Every X11-only code path checks this.
pub fn is_x11() -> bool {
    current() == WindowSystem::X11
}

/// Whether this process runs on Wayland.
pub fn is_wayland() -> bool {
    current() == WindowSystem::Wayland
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_display_wins_over_xwayland() {
        assert_eq!(detect_from(None, Some("wayland-0"), Some(":0")), WindowSystem::Wayland);
    }

    #[test]
    fn a_plain_x_session_is_x11() {
        assert_eq!(detect_from(None, None, Some(":0")), WindowSystem::X11);
        assert_eq!(detect_from(None, Some(""), Some(":1")), WindowSystem::X11);
    }

    #[test]
    fn gdk_backend_can_force_xwayland() {
        assert_eq!(detect_from(Some("x11"), Some("wayland-0"), Some(":0")), WindowSystem::X11);
        assert_eq!(detect_from(Some("x11,wayland"), Some("wayland-0"), Some(":0")), WindowSystem::X11);
    }

    #[test]
    fn a_forced_backend_without_its_display_is_ignored() {
        assert_eq!(detect_from(Some("x11"), Some("wayland-0"), None), WindowSystem::Wayland);
        assert_eq!(detect_from(Some("wayland"), None, Some(":0")), WindowSystem::X11);
    }

    #[test]
    fn no_display_at_all_is_none() {
        assert_eq!(detect_from(None, None, None), WindowSystem::None);
        assert_eq!(detect_from(Some("x11"), None, Some(" ")), WindowSystem::None);
    }
}
