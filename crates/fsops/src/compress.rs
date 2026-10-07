//! Compress: pack files and folders into an archive with the system tools
//! (`zip`, `tar`, `7z`, `gzip`, `bzip2`, ...).

use crate::cancel::CancelToken;
use crate::types::Progress;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Tag of the debug lines written to the console.
const LOG_TAG: &str = "compress";

/// How often the running tool is polled for exit, cancel and progress.
const POLL_GAP: Duration = Duration::from_millis(50);

/// Names under which the 7-Zip command line is installed, in order of preference.
const SEVEN_Z_TOOLS: &[&str] = &["7z", "7zz", "7za"];

/// An archive format `Compress` can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressFormat {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZ,
    /// A single file through `gzip`.
    Gz,
    /// A single file through `bzip2`.
    Bz2,
}

impl CompressFormat {
    /// Every format, in the order a requester should list them.
    pub const ALL: [CompressFormat; 8] = [
        Self::Zip,
        Self::TarBz2,
        Self::TarGz,
        Self::SevenZ,
        Self::TarXz,
        Self::TarZst,
        Self::Gz,
        Self::Bz2,
    ];

    /// The file name suffix, dot included.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::TarGz => ".tar.gz",
            Self::TarBz2 => ".tar.bz2",
            Self::TarXz => ".tar.xz",
            Self::TarZst => ".tar.zst",
            Self::SevenZ => ".7z",
            Self::Gz => ".gz",
            Self::Bz2 => ".bz2",
        }
    }

    /// The format a file name asks for, by its suffix (case-insensitive).
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        Self::ALL.into_iter().find(|f| lower.ends_with(f.suffix()))
    }

    /// Whether the format holds one file only (no folders, no several files).
    pub fn is_single_file(self) -> bool {
        matches!(self, Self::Gz | Self::Bz2)
    }

    /// Executable names this format needs, each one required.
    fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Zip => &["zip"],
            Self::TarGz => &["tar", "gzip"],
            Self::TarBz2 => &["tar", "bzip2"],
            Self::TarXz => &["tar", "xz"],
            Self::TarZst => &["tar", "zstd"],
            Self::SevenZ => &[],
            Self::Gz => &["gzip"],
            Self::Bz2 => &["bzip2"],
        }
    }

    /// Whether the tools this format needs are installed.
    pub fn is_available(self) -> bool {
        if self == Self::SevenZ {
            return seven_z_tool().is_some();
        }
        self.tools().iter().all(|t| find_in_path(t).is_some())
    }

    /// The installed formats that can pack the selection: the single-file
    /// ones only when `single_file` (one regular file selected).
    pub fn available(single_file: bool) -> Vec<Self> {
        Self::ALL.into_iter().filter(|f| (single_file || !f.is_single_file()) && f.is_available()).collect()
    }
}

/// The first 7-Zip executable found in `PATH`.
fn seven_z_tool() -> Option<&'static str> {
    SEVEN_Z_TOOLS.iter().copied().find(|t| find_in_path(t).is_some())
}

/// Whether an executable file called `name` exists in a `PATH` directory.
fn find_in_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// A `Done` for a job that failed as a whole.
fn failed(path: &Path, msg: String) -> Progress {
    Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: vec![(path.to_path_buf(), msg)] }
}

/// Bytes of `path`, folders summed recursively.
fn tree_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// The name of `src` as handed to a tool: never starts with `-`.
fn tool_arg(name: &str) -> String {
    if name.starts_with('-') { format!("./{name}") } else { name.to_string() }
}

/// Pack `srcs` (all in one folder) into the new archive `dest_dir/archive`.
/// The format comes from the archive name suffix. The archive must not
/// exist; a partial archive is removed on error or cancel.
pub fn run_compress(
    srcs: &[PathBuf],
    dest_dir: &Path,
    archive: &str,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Progress {
    let out_path = dest_dir.join(archive);
    if let Err(msg) = opusdm_ipc::glob::validate_new_name(archive) {
        return failed(&out_path, msg);
    }
    let Some(format) = CompressFormat::from_name(archive) else {
        return failed(&out_path, format!("unknown archive type: {archive:?}"));
    };
    if !format.is_available() {
        return failed(&out_path, format!("the tools for {} are not installed", format.suffix()));
    }
    let plan = match plan_sources(srcs, dest_dir, format) {
        Ok(p) => p,
        Err(msg) => return failed(&out_path, msg),
    };
    let out = match OpenOptions::new().write(true).create_new(true).open(&out_path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return failed(&out_path, format!("{} already exists", out_path.display()))
        }
        Err(e) => return failed(&out_path, e.to_string()),
    };
    let _ = tx.send(Progress::Scanning { files: srcs.len() as u64, bytes: plan.total_bytes });

    let child = match spawn_tool(format, &plan, &out_path, out) {
        Ok(c) => c,
        Err(e) => {
            crate::cleanup::remove_partial(&out_path);
            return failed(&out_path, format!("cannot start the compressor: {e}"));
        }
    };
    let outcome = wait_for(child, &out_path, plan.total_bytes, srcs.len() as u64, tx, cancel);
    match outcome {
        Ok(()) => Progress::Done { copied: srcs.len() as u64, skipped: 0, cancelled: false, errors: Vec::new() },
        Err(WaitError::Cancelled) => {
            crate::cleanup::remove_partial(&out_path);
            Progress::Done { copied: 0, skipped: 0, cancelled: true, errors: Vec::new() }
        }
        Err(WaitError::Failed(msg)) => {
            crate::cleanup::remove_partial(&out_path);
            failed(&out_path, msg)
        }
    }
}

/// The checked sources of a Compress job.
struct Plan {
    /// Folder the tool runs in.
    parent: PathBuf,
    /// Source names relative to `parent`.
    names: Vec<String>,
    total_bytes: u64,
}

/// Check the sources before anything is written: one common folder, no
/// source holding the archive, one regular file for the single-file formats.
fn plan_sources(srcs: &[PathBuf], dest_dir: &Path, format: CompressFormat) -> Result<Plan, String> {
    let Some(first) = srcs.first() else { return Err("no source files selected".into()) };
    let parent = first.parent().ok_or_else(|| format!("{} has no parent folder", first.display()))?.to_path_buf();
    let mut names = Vec::new();
    let mut total_bytes = 0;
    let dest_real = dest_dir.canonicalize().unwrap_or_else(|_| dest_dir.to_path_buf());
    for src in srcs {
        if src.parent() != Some(parent.as_path()) {
            return Err("the items to compress must be in the same folder".into());
        }
        let meta = std::fs::symlink_metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.ok_or_else(|| format!("{} has no file name", src.display()))?;
        if meta.is_dir() && dest_real.starts_with(src.canonicalize().unwrap_or_else(|_| src.clone())) {
            return Err(format!("the archive would be inside {}", src.display()));
        }
        total_bytes += tree_bytes(src);
        names.push(name);
    }
    if format.is_single_file() {
        let single_file = srcs.len() == 1 && std::fs::symlink_metadata(&srcs[0]).is_ok_and(|m| m.is_file());
        if !single_file {
            return Err(format!("{} holds one regular file only", format.suffix()));
        }
    }
    Ok(Plan { parent, names, total_bytes })
}

/// Start the tool writing to `out_path` (`out` is the file already created).
fn spawn_tool(format: CompressFormat, plan: &Plan, out_path: &Path, out: File) -> io::Result<Child> {
    let names: Vec<String> = plan.names.iter().map(|n| tool_arg(n)).collect();
    let mut cmd;
    match format {
        CompressFormat::Zip => {
            // zip refuses an existing empty file as an archive: drop the placeholder.
            drop(out);
            std::fs::remove_file(out_path)?;
            cmd = Command::new("zip");
            cmd.args(["-r", "-y", "-q"]).arg(out_path).args(&names).stdout(Stdio::null());
        }
        CompressFormat::SevenZ => {
            drop(out);
            std::fs::remove_file(out_path)?;
            cmd = Command::new(seven_z_tool().unwrap_or("7z"));
            cmd.args(["a", "-bd", "-y"]).arg(out_path).args(&names).stdout(Stdio::null());
        }
        CompressFormat::TarGz | CompressFormat::TarBz2 | CompressFormat::TarXz | CompressFormat::TarZst => {
            cmd = Command::new("tar");
            let flag = match format {
                CompressFormat::TarGz => "-z",
                CompressFormat::TarBz2 => "-j",
                CompressFormat::TarXz => "-J",
                _ => "--zstd",
            };
            cmd.arg(flag).arg("-cf").arg("-").args(&names).stdout(out);
        }
        CompressFormat::Gz | CompressFormat::Bz2 => {
            cmd = Command::new(if format == CompressFormat::Gz { "gzip" } else { "bzip2" });
            cmd.arg("-c").args(&names).stdout(out);
        }
    }
    cmd.current_dir(&plan.parent).stdin(Stdio::null()).stderr(Stdio::piped());
    opusdm_ipc::err_log!(LOG_TAG, "run {:?} {:?} in {}", cmd.get_program(), cmd.get_args().collect::<Vec<_>>(), plan.parent.display());
    let child = cmd.spawn()?;
    opusdm_ipc::err_log!(LOG_TAG, "started pid {} -> {}", child.id(), out_path.display());
    Ok(child)
}

/// Why waiting for the tool did not end well.
enum WaitError {
    Cancelled,
    Failed(String),
}

/// Wait for `child`, reporting progress from the archive size, and kill it
/// on cancel. A non-zero exit becomes the tool's error text.
fn wait_for(
    mut child: Child,
    out_path: &Path,
    total_bytes: u64,
    files_total: u64,
    tx: &Sender<Progress>,
    cancel: &CancelToken,
) -> Result<(), WaitError> {
    let mut last = Instant::now();
    let mut lost_since = None;
    loop {
        if cancel.is_cancelled() {
            opusdm_ipc::err_log!(LOG_TAG, "cancelled, killing pid {}", child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err(WaitError::Cancelled);
        }
        match opusdm_ipc::reaped::poll_exit(&mut child, &mut lost_since) {
            Ok(Some(status)) if status.success() => {
                opusdm_ipc::err_log!(LOG_TAG, "pid {} finished: {status}", child.id());
                return Ok(());
            }
            Ok(Some(status)) => {
                opusdm_ipc::err_log!(LOG_TAG, "pid {} failed: {status}", child.id());
                let mut text = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = io::Read::read_to_string(&mut err, &mut text);
                }
                let text = text.trim();
                opusdm_ipc::err_log!(LOG_TAG, "stderr: {text:?}");
                return Err(WaitError::Failed(if text.is_empty() { format!("compressor failed: {status}") } else { text.to_string() }));
            }
            Ok(None) => {}
            Err(e) => {
                opusdm_ipc::err_log!(LOG_TAG, "wait on pid {} failed: {e}", child.id());
                return Err(WaitError::Failed(e.to_string()));
            }
        }
        if last.elapsed() >= POLL_GAP {
            last = Instant::now();
            let written = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
            let _ = tx.send(Progress::File {
                path: out_path.to_path_buf(),
                done_bytes: written.min(total_bytes),
                total_bytes,
                files_done: 0,
                files_total,
                file_bytes_done: 0,
                file_bytes_total: 0,
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn run(srcs: &[PathBuf], dest: &Path, name: &str) -> Progress {
        let (tx, _rx) = channel();
        run_compress(srcs, dest, name, &tx, &CancelToken::new())
    }

    fn errors(p: &Progress) -> Vec<String> {
        match p {
            Progress::Done { errors, .. } => errors.iter().map(|(_, m)| m.clone()).collect(),
            _ => panic!("not a Done"),
        }
    }

    #[test]
    fn suffix_selects_the_format() {
        assert_eq!(CompressFormat::from_name("a.TAR.GZ"), Some(CompressFormat::TarGz));
        assert_eq!(CompressFormat::from_name("a.tar.bz2"), Some(CompressFormat::TarBz2));
        assert_eq!(CompressFormat::from_name("a.bz2"), Some(CompressFormat::Bz2));
        assert_eq!(CompressFormat::from_name("a.txt"), None);
    }

    #[test]
    fn single_file_formats_are_offered_for_one_file_only() {
        let many = CompressFormat::available(false);
        assert!(many.iter().all(|f| !f.is_single_file()));
        let one = CompressFormat::available(true);
        assert!(many.iter().all(|f| one.contains(f)));
    }

    #[test]
    fn every_available_format_packs_a_folder_or_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/a.txt"), "hello").unwrap();
        let file = dir.path().join("-odd.txt");
        std::fs::write(&file, "x".repeat(1000)).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        for format in CompressFormat::ALL.into_iter().filter(|f| f.is_available()) {
            let (srcs, name) = if format.is_single_file() {
                (vec![file.clone()], format!("single{}", format.suffix()))
            } else {
                (vec![src.clone(), file.clone()], format!("many{}", format.suffix()))
            };
            let done = run(&srcs, &out, &name);
            assert!(errors(&done).is_empty(), "{format:?}: {:?}", errors(&done));
            assert!(std::fs::metadata(out.join(&name)).unwrap().len() > 0, "{format:?}");
        }
    }

    #[test]
    fn an_existing_archive_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        std::fs::write(dir.path().join("a.tar.gz"), "keep").unwrap();
        if !CompressFormat::TarGz.is_available() {
            return;
        }
        let done = run(&[file], dir.path(), "a.tar.gz");
        assert!(errors(&done)[0].contains("already exists"));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.tar.gz")).unwrap(), "keep");
    }

    #[test]
    fn bad_input_is_rejected_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("d");
        std::fs::create_dir(&sub).unwrap();
        assert!(errors(&run(std::slice::from_ref(&sub), dir.path(), "x.unknown"))[0].contains("unknown"));
        if CompressFormat::Gz.is_available() {
            assert!(errors(&run(std::slice::from_ref(&sub), dir.path(), "d.gz"))[0].contains("one regular file"));
        }
        if CompressFormat::TarGz.is_available() {
            assert!(errors(&run(std::slice::from_ref(&sub), &sub, "d.tar.gz"))[0].contains("inside"));
        }
        assert!(!dir.path().join("x.unknown").exists());
    }
}
