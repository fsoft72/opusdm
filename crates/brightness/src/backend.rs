//! The boundary to the system. `Backend` is what the worker uses; `Sysfs`,
//! `Ddc` and `Xrandr` are the real ones, tried in this order by `detect`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::parse::{
    parse_ddc_brief, parse_xrandr_brightness, parse_xrandr_connected, percent_to_raw, raw_to_percent,
};
use crate::{clamp_percent, BrightnessError};

/// Where the kernel lists the backlights of the laptop panels.
pub const SYSFS_ROOT: &str = "/sys/class/backlight";

/// One way to read and change the brightness.
pub trait Backend {
    /// Short name shown in the settings window.
    fn name(&self) -> &'static str;

    /// The current brightness, in percent.
    fn get(&self) -> Result<u8, BrightnessError>;

    /// Set the brightness, in percent (limited to the allowed range).
    fn set(&self, percent: u8) -> Result<(), BrightnessError>;
}

/// A backend that works, with the brightness it had when it was probed.
pub struct Detected {
    pub backend: Box<dyn Backend>,
    pub percent: u8,
}

/// A way to build a backend; building it may already fail (tool missing).
pub type Candidate = Box<dyn FnOnce() -> Result<Box<dyn Backend>, BrightnessError>>;

/// Try the real backends in order: backlight, DDC/CI, software gamma.
pub fn detect() -> Result<Detected, BrightnessError> {
    detect_from(vec![
        ("backlight", Box::new(|| Sysfs::find(Path::new(SYSFS_ROOT)).map(|b| Box::new(b) as Box<dyn Backend>))),
        ("ddcutil", Box::new(|| Ok(Box::new(Ddc::new()) as Box<dyn Backend>))),
        ("xrandr", Box::new(|| Xrandr::find().map(|b| Box::new(b) as Box<dyn Backend>))),
    ])
}

/// Build each candidate in turn and keep the first whose `get` works. When
/// none does, the error lists why each one was skipped.
pub fn detect_from(candidates: Vec<(&'static str, Candidate)>) -> Result<Detected, BrightnessError> {
    let mut reasons = Vec::new();
    for (name, build) in candidates {
        match build().and_then(|backend| backend.get().map(|percent| Detected { backend, percent })) {
            Ok(found) => return Ok(found),
            Err(e) => reasons.push(format!("{name}: {e}")),
        }
    }
    Err(BrightnessError::Unavailable(format!("no way to change the brightness ({})", reasons.join("; "))))
}

/// Run `tool` and return its standard output.
fn run(tool: &str, package: &str, args: &[&str]) -> Result<String, BrightnessError> {
    let command = format!("{tool} {}", args.join(" "));
    let output = Command::new(tool).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            BrightnessError::MissingTool { tool: tool.to_string(), package: package.to_string() }
        } else {
            BrightnessError::CommandFailed { command: command.clone(), stderr: e.to_string() }
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(BrightnessError::CommandFailed { command, stderr: if stderr.is_empty() { stdout } else { stderr } });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The backlight of a laptop panel, through `/sys/class/backlight`.
pub struct Sysfs {
    dir: PathBuf,
}

impl Sysfs {
    /// The first backlight under `root` that has a readable, non zero
    /// `max_brightness`.
    pub fn find(root: &Path) -> Result<Self, BrightnessError> {
        let io = |path: &Path, e: std::io::Error| BrightnessError::Io { path: path.display().to_string(), reason: e.to_string() };
        let mut dirs: Vec<PathBuf> = fs::read_dir(root).map_err(|e| io(root, e))?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        dirs.sort();
        let found = dirs.into_iter().find(|d| read_number(&d.join("max_brightness")).is_ok_and(|max| max > 0));
        found.map(|dir| Self { dir }).ok_or_else(|| BrightnessError::Unavailable(format!("no backlight in {}", root.display())))
    }

    /// The raw maximum of this backlight.
    fn max(&self) -> Result<u32, BrightnessError> {
        read_number(&self.dir.join("max_brightness"))
    }
}

/// Read a file that holds one number.
fn read_number(path: &Path) -> Result<u32, BrightnessError> {
    let text = fs::read_to_string(path).map_err(|e| BrightnessError::Io { path: path.display().to_string(), reason: e.to_string() })?;
    text.trim().parse().map_err(|_| BrightnessError::Parse { tool: path.display().to_string(), detail: format!("not a number: {}", text.trim()) })
}

impl Backend for Sysfs {
    /// "backlight".
    fn name(&self) -> &'static str {
        "backlight"
    }

    /// `brightness` over `max_brightness`.
    fn get(&self) -> Result<u8, BrightnessError> {
        Ok(raw_to_percent(read_number(&self.dir.join("brightness"))?, self.max()?))
    }

    /// Write the raw value; needs write access to the file (a udev rule or
    /// the `video` group on most systems).
    fn set(&self, percent: u8) -> Result<(), BrightnessError> {
        let raw = percent_to_raw(clamp_percent(percent), self.max()?);
        let path = self.dir.join("brightness");
        fs::write(&path, raw.to_string()).map_err(|e| BrightnessError::Io { path: path.display().to_string(), reason: e.to_string() })
    }
}

/// An external monitor, through DDC/CI (VCP feature 10) with `ddcutil`.
pub struct Ddc {
    /// The maximum the monitor reported; 100 until the first read.
    max: AtomicU32,
}

impl Ddc {
    /// A DDC backend; nothing is run until `get` or `set`.
    pub fn new() -> Self {
        Self { max: AtomicU32::new(100) }
    }
}

impl Default for Ddc {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Ddc {
    /// "DDC/CI".
    fn name(&self) -> &'static str {
        "DDC/CI"
    }

    /// Read VCP 10 and remember the maximum of the monitor.
    fn get(&self) -> Result<u8, BrightnessError> {
        let out = run("ddcutil", "ddcutil", &["getvcp", "10", "--brief"])?;
        let (current, max) = parse_ddc_brief(&out).ok_or_else(|| BrightnessError::Parse { tool: "ddcutil".to_string(), detail: out.trim().to_string() })?;
        self.max.store(max, Ordering::Relaxed);
        Ok(raw_to_percent(current, max))
    }

    /// Write VCP 10 scaled to the maximum of the monitor.
    fn set(&self, percent: u8) -> Result<(), BrightnessError> {
        let raw = percent_to_raw(clamp_percent(percent), self.max.load(Ordering::Relaxed));
        run("ddcutil", "ddcutil", &["setvcp", "10", &raw.to_string(), "--noverify"]).map(|_| ())
    }
}

/// Software brightness of one output, through `xrandr --brightness`. It only
/// darkens the picture, the backlight stays as it is.
pub struct Xrandr {
    output: String,
}

impl Xrandr {
    /// Use the first connected output.
    pub fn find() -> Result<Self, BrightnessError> {
        let out = run("xrandr", "x11-xserver-utils", &["--query"])?;
        let output = parse_xrandr_connected(&out).ok_or_else(|| BrightnessError::Unavailable("no connected output".to_string()))?;
        Ok(Self { output })
    }
}

impl Backend for Xrandr {
    /// "xrandr (software)".
    fn name(&self) -> &'static str {
        "xrandr (software)"
    }

    /// The `Brightness:` line of the output.
    fn get(&self) -> Result<u8, BrightnessError> {
        let out = run("xrandr", "x11-xserver-utils", &["--verbose"])?;
        parse_xrandr_brightness(&out, &self.output).ok_or_else(|| BrightnessError::Parse { tool: "xrandr".to_string(), detail: format!("no brightness for {}", self.output) })
    }

    /// Set the factor (percent over 100).
    fn set(&self, percent: u8) -> Result<(), BrightnessError> {
        let factor = format!("{:.2}", clamp_percent(percent) as f64 / 100.0);
        run("xrandr", "x11-xserver-utils", &["--output", &self.output, "--brightness", &factor]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeBackend;

    fn backlight(root: &Path, name: &str, current: &str, max: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("brightness"), current).unwrap();
        fs::write(dir.join("max_brightness"), max).unwrap();
        dir
    }

    #[test]
    fn sysfs_reads_a_percent() {
        let td = tempfile::tempdir().unwrap();
        backlight(td.path(), "intel_backlight", "480\n", "960\n");
        let b = Sysfs::find(td.path()).unwrap();
        assert_eq!(b.get().unwrap(), 50);
    }

    #[test]
    fn sysfs_writes_the_scaled_value_and_clamps() {
        let td = tempfile::tempdir().unwrap();
        let dir = backlight(td.path(), "intel_backlight", "960", "960");
        let b = Sysfs::find(td.path()).unwrap();
        b.set(25).unwrap();
        assert_eq!(fs::read_to_string(dir.join("brightness")).unwrap(), "240");
        b.set(0).unwrap();
        assert_eq!(fs::read_to_string(dir.join("brightness")).unwrap(), "96");
    }

    #[test]
    fn sysfs_skips_a_device_without_max_and_fails_when_empty() {
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("broken")).unwrap();
        assert!(matches!(Sysfs::find(td.path()), Err(BrightnessError::Unavailable(_))));
        backlight(td.path(), "ok", "5", "10");
        assert!(Sysfs::find(td.path()).unwrap().dir.ends_with("ok"));
    }

    #[test]
    fn sysfs_missing_root_is_an_io_error() {
        assert!(matches!(Sysfs::find(Path::new("/nonexistent/backlight")), Err(BrightnessError::Io { .. })));
    }

    #[test]
    fn detect_takes_the_first_working_backend() {
        let found = detect_from(vec![
            ("one", Box::new(|| Err(BrightnessError::Unavailable("nope".to_string())))),
            ("two", Box::new(|| Ok(Box::new(FakeBackend::new(70)) as Box<dyn Backend>))),
            ("three", Box::new(|| Ok(Box::new(FakeBackend::new(10)) as Box<dyn Backend>))),
        ])
        .unwrap();
        assert_eq!(found.percent, 70);
    }

    #[test]
    fn detect_skips_a_backend_whose_probe_fails() {
        let found = detect_from(vec![
            ("broken", Box::new(|| Ok(Box::new(FakeBackend::failing()) as Box<dyn Backend>))),
            ("good", Box::new(|| Ok(Box::new(FakeBackend::new(40)) as Box<dyn Backend>))),
        ])
        .unwrap();
        assert_eq!(found.percent, 40);
    }

    #[test]
    fn detect_lists_every_reason_when_nothing_works() {
        let err = detect_from(vec![
            ("one", Box::new(|| Err(BrightnessError::Unavailable("first".to_string())))),
            ("two", Box::new(|| Err(BrightnessError::Unavailable("second".to_string())))),
        ])
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("one: first") && err.contains("two: second"), "{err}");
    }
}
