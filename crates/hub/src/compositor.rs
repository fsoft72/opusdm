//! Window transparency: renders the user's picom config from the base one and
//! asks the running picom to reload it.
//!
//! picom only reloads (on `SIGUSR1`) the file it was started with, and the
//! base config lives in the install prefix, which the user cannot write. The
//! session therefore starts picom with `~/.config/opusdm/picom.conf`, and the
//! hub regenerates that file from the base one (`$OPUSDM_PICOM_BASE`) every
//! time the settings change.

use opusdm_ipc::DesktopSettings;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Environment variable holding the path of the base picom config.
pub const BASE_ENV: &str = "OPUSDM_PICOM_BASE";

/// File name of the generated config under the OpusDM config directory.
pub const USER_CONFIG_NAME: &str = "picom.conf";

/// Path of the generated picom config: `~/.config/opusdm/picom.conf`.
pub fn user_config_path() -> PathBuf {
    opusdm_ipc::settings::config_dir().join(USER_CONFIG_NAME)
}

/// The base config with the opacity keys set from `ds`. Every other line is
/// kept as is; a key missing from the base is appended. With transparency off
/// both opacities are 1.0 and picom's per-window override is left alone.
pub fn render_config(base: &str, ds: &DesktopSettings) -> String {
    let (inactive, active) = if ds.transparency_enabled {
        (ds.inactive_opacity, ds.active_opacity)
    } else {
        (100, 100)
    };
    let keys = [
        ("inactive-opacity", format!("{:.2}", inactive as f64 / 100.0)),
        ("active-opacity", format!("{:.2}", active as f64 / 100.0)),
    ];
    let mut seen = [false; 2];
    let mut out = String::with_capacity(base.len() + 64);
    for line in base.lines() {
        let key = line.split('=').next().map(str::trim).unwrap_or("");
        match keys.iter().position(|(k, _)| *k == key) {
            Some(i) if line.contains('=') => {
                seen[i] = true;
                out.push_str(&format!("{key} = {};\n", keys[i].1));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (i, (key, value)) in keys.iter().enumerate() {
        if !seen[i] {
            out.push_str(&format!("{key} = {value};\n"));
        }
    }
    out
}

/// Write the user config for `ds` from the base config at `base_path` into
/// `target` (through a temporary file, so picom never reads a half-written one).
pub fn write_config(base_path: &Path, target: &Path, ds: &DesktopSettings) -> Result<(), String> {
    let base = std::fs::read_to_string(base_path)
        .map_err(|e| format!("cannot read the base picom config {}: {e}", base_path.display()))?;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = target.with_extension("conf.tmp");
    std::fs::write(&tmp, render_config(&base, ds)).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, target).map_err(|e| format!("cannot replace {}: {e}", target.display()))
}

/// Ask this user's picom to reload its config. Returns false when no picom is running.
fn reload_picom() -> Result<bool, String> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut child = Command::new("pkill")
        .args(["-USR1", "-u", &uid.to_string(), "-x", "picom"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run pkill: {e}"))?;
    let status = opusdm_ipc::reaped::wait(&mut child).map_err(|e| format!("pkill wait failed: {e}"))?;
    Ok(status.success())
}

/// Apply the transparency settings to the running compositor. A failure is
/// logged and never stops the hub; without `$OPUSDM_PICOM_BASE` (a session not
/// started by OpusDM) nothing is done.
pub fn apply(ds: &DesktopSettings) {
    let Some(base) = std::env::var_os(BASE_ENV) else { return };
    if let Err(e) = write_config(Path::new(&base), &user_config_path(), ds) {
        opusdm_ipc::err_log!("opusdm-hub", "cannot update the picom config: {e}");
        return;
    }
    match reload_picom() {
        Ok(true) => {}
        Ok(false) => opusdm_ipc::err_log!("opusdm-hub", "picom is not running, transparency applies at its next start"),
        Err(e) => opusdm_ipc::err_log!("opusdm-hub", "cannot reload picom: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "shadow = true;\ninactive-opacity = 0.50;\nactive-opacity = 1.0;\ninactive-opacity-override = false;\n";

    #[test]
    fn render_sets_the_two_opacities_and_keeps_the_rest() {
        let ds = DesktopSettings { inactive_opacity: 70, active_opacity: 95, ..Default::default() };
        assert_eq!(
            render_config(BASE, &ds),
            "shadow = true;\ninactive-opacity = 0.70;\nactive-opacity = 0.95;\ninactive-opacity-override = false;\n"
        );
    }

    #[test]
    fn render_with_transparency_off_is_fully_opaque() {
        let ds = DesktopSettings { transparency_enabled: false, inactive_opacity: 40, ..Default::default() };
        let out = render_config(BASE, &ds);
        assert!(out.contains("inactive-opacity = 1.00;"));
        assert!(out.contains("active-opacity = 1.00;"));
    }

    #[test]
    fn render_appends_keys_missing_from_the_base() {
        let out = render_config("shadow = true;\n", &DesktopSettings::default());
        assert_eq!(out, "shadow = true;\ninactive-opacity = 0.88;\nactive-opacity = 1.00;\n");
    }

    #[test]
    fn write_config_creates_the_target_from_the_base() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().join("base.conf");
        std::fs::write(&base, BASE).unwrap();
        let target = td.path().join("nested").join("picom.conf");
        write_config(&base, &target, &DesktopSettings::default()).unwrap();
        assert!(std::fs::read_to_string(&target).unwrap().contains("inactive-opacity = 0.88;"));
    }

    #[test]
    fn write_config_reports_a_missing_base() {
        let td = tempfile::tempdir().unwrap();
        let err = write_config(&td.path().join("none"), &td.path().join("out.conf"), &DesktopSettings::default());
        assert!(err.unwrap_err().contains("cannot read the base picom config"));
    }
}
