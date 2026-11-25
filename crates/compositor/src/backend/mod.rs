//! Backends of the compositor (nested winit window, headless test backend).

use crate::args::Args;

/// Runs the selected backend. Stub: Task 3 replaces it with the real backends.
pub fn run(_args: &Args) -> Result<(), String> {
    Err("no backend yet".into())
}
