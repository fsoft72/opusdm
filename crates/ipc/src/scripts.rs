//! JavaScript scripts: one `<name>.js` file each in `~/.config/opusdm/scripts`.
//! The Lister's Scripts window lists, edits and deletes them; the hub runs one
//! through the `Script` internal command (`InternalCmd::Script`), whose engine
//! is Boa. The files are plain text, so they can also be edited by hand.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Extension of a script file.
pub const EXTENSION: &str = "js";

/// Longest script name accepted, in characters.
pub const MAX_NAME_CHARS: usize = 100;

/// The directory holding the scripts (`~/.config/opusdm/scripts`).
pub fn scripts_dir() -> PathBuf {
    crate::settings::config_dir().join("scripts")
}

/// One script file, as the Scripts window lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInfo {
    /// The file name without `.js`.
    pub name: String,
    /// The file.
    pub path: PathBuf,
    /// The leading `//` comment lines of the script, joined by spaces.
    pub description: String,
    /// When the file was last written.
    pub modified: Option<SystemTime>,
}

/// Why `name` cannot name a script, or `Ok` when it can: it must not be
/// empty, start with a dot, contain `/` or a control character, or be longer
/// than [`MAX_NAME_CHARS`].
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("the script name is empty".to_string());
    }
    if name != name.trim() {
        return Err("the script name starts or ends with a space".to_string());
    }
    if name.starts_with('.') {
        return Err("the script name cannot start with a dot".to_string());
    }
    if name.contains('/') || name.chars().any(char::is_control) {
        return Err("the script name cannot contain '/' or control characters".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("the script name is longer than {MAX_NAME_CHARS} characters"));
    }
    Ok(())
}

/// The file of script `name` in `dir`, after checking the name.
pub fn path_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    Ok(dir.join(format!("{name}.{EXTENSION}")))
}

/// The leading `//` comment lines of `code` (a `#!` line is skipped), joined
/// by spaces: what the Scripts window shows next to the name.
pub fn description(code: &str) -> String {
    let mut parts = Vec::new();
    for (index, line) in code.lines().enumerate() {
        let line = line.trim();
        if index == 0 && line.starts_with("#!") {
            continue;
        }
        let Some(text) = line.strip_prefix("//") else { break };
        let text = text.trim_start_matches('/').trim();
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join(" ")
}

/// Every script in `dir`, sorted by name (case ignored). A missing directory
/// has none.
pub fn list_in(dir: &Path) -> Vec<ScriptInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<ScriptInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            validate_name(&name).ok()?;
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            // Only the head is read: the description is at the top.
            let head = read_head(&path).unwrap_or_default();
            Some(ScriptInfo { name, description: description(&head), modified: meta.modified().ok(), path })
        })
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

/// The first 4 KiB of `path`, as text.
fn read_head(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut buf = vec![0; 4096];
    let n = std::fs::File::open(path)?.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The code of script `name` in `dir`.
pub fn load_in(dir: &Path, name: &str) -> io::Result<String> {
    let path = path_in(dir, name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    std::fs::read_to_string(path)
}

/// Write `code` as script `name` in `dir` (created when missing). The file is
/// replaced at once (written aside, then renamed), so a hub reading it never
/// sees half a script.
pub fn save_in(dir: &Path, name: &str, code: &str) -> io::Result<()> {
    let path = path_in(dir, name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.{EXTENSION}.tmp"));
    std::fs::write(&tmp, code)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Delete script `name` from `dir`.
pub fn delete_in(dir: &Path, name: &str) -> io::Result<()> {
    let path = path_in(dir, name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    std::fs::remove_file(path)
}

/// Rename script `old` to `new` in `dir`; fails when `new` already exists.
pub fn rename_in(dir: &Path, old: &str, new: &str) -> io::Result<()> {
    let from = path_in(dir, old).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let to = path_in(dir, new).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("a script called \"{new}\" already exists")));
    }
    std::fs::rename(from, to)
}

/// [`list_in`] on [`scripts_dir`].
pub fn list() -> Vec<ScriptInfo> {
    list_in(&scripts_dir())
}

/// [`load_in`] on [`scripts_dir`].
pub fn load(name: &str) -> io::Result<String> {
    load_in(&scripts_dir(), name)
}

/// [`save_in`] on [`scripts_dir`].
pub fn save(name: &str, code: &str) -> io::Result<()> {
    save_in(&scripts_dir(), name, code)
}

/// [`delete_in`] on [`scripts_dir`].
pub fn delete(name: &str) -> io::Result<()> {
    delete_in(&scripts_dir(), name)
}

/// [`rename_in`] on [`scripts_dir`].
pub fn rename(old: &str, new: &str) -> io::Result<()> {
    rename_in(&scripts_dir(), old, new)
}

/// One entry of the scripting API, as the editor's reference panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiEntry {
    /// The group it is listed under.
    pub group: &'static str,
    /// How it is called.
    pub signature: &'static str,
    /// The text the editor inserts for it.
    pub snippet: &'static str,
    /// What it does.
    pub doc: &'static str,
}

const fn api(group: &'static str, signature: &'static str, snippet: &'static str, doc: &'static str) -> ApiEntry {
    ApiEntry { group, signature, snippet, doc }
}

/// The `opus` API, in the order the reference panel lists it. Kept next to
/// the storage so the editor and the documentation share one list; the
/// implementation is the hub's `scripting/prelude.js`.
pub const API: &[ApiEntry] = &[
    api("Listers", "opus.source", "opus.source", "The SOURCE Lister (or null): path, selected, selectedNames, selectedFiles, selectedDirs."),
    api("Listers", "opus.dest", "opus.dest", "The DEST Lister (or null)."),
    api("Listers", "opus.listers", "opus.listers", "Every open Lister: id, role (source, dest, none), path."),
    api("Listers", "lister.go(dir, {mode})", "opus.source.go(\"~\")", "Show a folder in the Lister. On the SOURCE it is also the folder of the next commands. mode: details or icons."),
    api("Listers", "lister.files(pattern)", "opus.source.files(\"*.txt\")", "The entries of the Lister's folder, optionally only those matching a pattern."),
    api("Listers", "lister.refresh()", "opus.source.refresh()", "Read the Lister's folder again."),
    api("Commands", "opus.command(name, params)", "opus.command(\"Copy\", { })", "Run an internal command (Copy, Move, Delete, MakeDir, CopyAs, ChangeDir, ...). params.files replaces the selection, params.to the DEST folder; true turns a checkbox on."),
    api("Commands", "opus.commands", "opus.commands", "Every internal command with its parameters: name, label, params [{key, label, required, kind}]."),
    api("Commands", "opus.runFunction(name)", "opus.runFunction(\"\")", "Run a saved Function, by name or id."),
    api("Commands", "opus.functions", "opus.functions", "The saved Functions: [{id, name}]."),
    api("Commands", "opus.runScript(name, args)", "opus.runScript(\"\", \"\")", "Run another script; args becomes its opus.args."),
    api("Commands", "opus.scripts", "opus.scripts", "The saved scripts: [{name, description}]."),
    api("Programs", "opus.run(commandLine, {wait, window})", "opus.run(\"\")", "Start a program like the Run command; {f}, {o}, ... are expanded."),
    api("Programs", "opus.exec(program, args, {cwd, input})", "opus.exec(\"ls\", [\"-l\"])", "Run a program, wait and capture its output: {code, stdout, stderr}."),
    api("Programs", "opus.shell(script, {cwd, input})", "opus.shell(\"\")", "bash -c script, captured like opus.exec."),
    api("User", "opus.log(...values)", "opus.log()", "Write a line to the output (console.log does the same)."),
    api("User", "opus.confirm(message)", "opus.confirm(\"\")", "Ask a yes / no question; true for yes."),
    api("User", "opus.sleep(ms)", "opus.sleep(1000)", "Wait some milliseconds."),
    api("Desktop", "opus.newLister(dir)", "opus.newLister()", "Open a new Lister, on its default folder without dir."),
    api("Desktop", "opus.setBackground(file, fit)", "opus.setBackground(\"\")", "Make a picture or video the desktop background; fit: fill, fit, center or tile."),
    api("Desktop", "opus.leaveOut(files) / opus.putAway(files)", "opus.leaveOut()", "Leave files out on the desktop, or put them away (default: the selection)."),
    api("Desktop", "opus.hide() / reveal() / toggleHide()", "opus.toggleHide()", "Hide or bring back the Listers, panel and dock."),
    api("Desktop", "opus.desktop.next() / prev()", "opus.desktop.next()", "Switch the virtual desktop."),
    api("Desktop", "opus.bank.open|close|toggle|hide(name)", "opus.bank.toggle(\"\")", "Act on a button bank."),
    api("Files", "opus.fs.read(path) / write(path, text) / append(path, text)", "opus.fs.read(\"\")", "Read or write a text file. Relative paths are relative to the SOURCE folder, ~ is home."),
    api("Files", "opus.fs.list(dir, pattern)", "opus.fs.list(\".\")", "The entries of a folder: [{name, path, isFile, isDir, isLink, size, modified, mode}]."),
    api("Files", "opus.fs.stat(path)", "opus.fs.stat(\"\")", "What a path is (as in list), or null when it does not exist."),
    api("Files", "opus.fs.exists / isFile / isDir(path)", "opus.fs.exists(\"\")", "Whether a path exists, is a file, is a folder."),
    api("Files", "opus.fs.mkdir(path)", "opus.fs.mkdir(\"\")", "Create a folder and its parents."),
    api("Files", "opus.path.join / dirname / basename / extname / normalize", "opus.path.join()", "Path helpers working on text."),
    api("Script", "opus.args", "opus.args", "The Arguments of the Run Script step, placeholders ({f}, {o}, ...) expanded."),
    api("Script", "opus.scriptName", "opus.scriptName", "The name of the running script."),
    api("Script", "opus.env(name)", "opus.env(\"HOME\")", "An environment variable, or null."),
    api("Script", "opus.refresh(dir)", "opus.refresh(\"\")", "Read a folder again in every Lister showing it."),
    api("Script", "opus.version / opus.home", "opus.version", "The OpusDM version; the home folder."),
];

/// The code a new script starts with.
pub fn template(name: &str) -> String {
    format!(
        "// {name}: describe what this script does here.\n\
         \n\
         // opus.source / opus.dest are the SOURCE and DEST Listers,\n\
         // opus.command(name, params) runs an internal command.\n\
         \n\
         const src = opus.source;\n\
         if (!src) throw new Error(\"no SOURCE Lister\");\n\
         opus.log(`${{src.selected.length}} item(s) selected in ${{src.path}}`);\n\
         for (const path of src.selected) {{\n\
         \x20   opus.log(\"  \" + path);\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(validate_name("backup photos").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("  ").is_err());
        assert!(validate_name(" lead").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn description_is_the_leading_comment() {
        assert_eq!(description("// Make a backup\n// of the selection\nlet a = 1;\n// later"), "Make a backup of the selection");
        assert_eq!(description("#!/usr/bin/env opusdm\n/// Doc\ncode"), "Doc");
        assert_eq!(description("code();\n// not this"), "");
    }

    #[test]
    fn save_list_load_rename_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scripts");
        assert!(list_in(&dir).is_empty());
        save_in(&dir, "beta", "// second\nx").unwrap();
        save_in(&dir, "Alpha", "// first\ny").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a script").unwrap();
        let names: Vec<String> = list_in(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Alpha", "beta"]);
        assert_eq!(list_in(&dir)[0].description, "first");
        assert_eq!(load_in(&dir, "beta").unwrap(), "// second\nx");
        save_in(&dir, "beta", "z").unwrap();
        assert_eq!(load_in(&dir, "beta").unwrap(), "z");
        assert!(rename_in(&dir, "beta", "Alpha").is_err());
        rename_in(&dir, "beta", "gamma").unwrap();
        assert!(load_in(&dir, "beta").is_err());
        delete_in(&dir, "gamma").unwrap();
        let names: Vec<String> = list_in(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Alpha"]);
        assert!(save_in(&dir, "../escape", "x").is_err());
    }

    #[test]
    fn the_template_describes_itself() {
        assert_eq!(description(&template("demo")), "demo: describe what this script does here.");
    }
}
