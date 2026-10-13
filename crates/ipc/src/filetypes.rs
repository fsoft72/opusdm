//! User-defined file types (F-06).
//!
//! `filetypes.json` holds an ordered list of [`FileType`]s. Each one has a
//! [`MatchRule`] and the functions to run for a double click, a drop and the
//! "Open With" context menu. The first type whose rule matches wins; a file
//! that matches nothing keeps the default behaviour (`xdg-open`). The file is
//! read directly by the Lister and by the editor, no hub round trip.

use crate::function::FunctionId;
use crate::glob::GlobSet;
use crate::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

/// How many leading bytes of a file the content rules can look at.
pub const HEAD_BYTES: usize = 64 * 1024;

/// A condition on a file. `And` and `Or` nest freely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MatchRule {
    /// The name matches one of the `;` or `,` separated patterns.
    Glob { pattern: String },
    /// The file holds `bytes` at `offset` (the "Match" requester).
    ContentBytes { offset: usize, bytes: Vec<u8> },
    /// `needle` occurs in the first `range` bytes (the "Search For" requester).
    ContentText { needle: String, range: usize },
    /// The size is within the bounds; a missing bound is open.
    SizeRange { min: Option<u64>, max: Option<u64> },
    /// The modification time (Unix seconds) is within the bounds.
    DateRange { from: Option<i64>, to: Option<i64> },
    /// Every rule matches. An empty list never matches.
    And { rules: Vec<MatchRule> },
    /// At least one rule matches. An empty list never matches.
    Or { rules: Vec<MatchRule> },
}

/// What the matcher knows about a file. Directories never match a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts<'a> {
    /// File name, no directory part.
    pub name: &'a str,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub modified: i64,
    /// Leading bytes of the file, at most [`HEAD_BYTES`].
    pub head: &'a [u8],
}

impl MatchRule {
    /// Whether `facts` satisfy this rule.
    pub fn matches(&self, facts: &FileFacts) -> bool {
        match self {
            MatchRule::Glob { pattern } => GlobSet::parse(pattern).is_ok_and(|set| set.is_match(facts.name)),
            MatchRule::ContentBytes { offset, bytes } => {
                if bytes.is_empty() {
                    return false;
                }
                let Some(end) = offset.checked_add(bytes.len()) else { return false };
                facts.head.get(*offset..end).is_some_and(|found| found == bytes.as_slice())
            }
            MatchRule::ContentText { needle, range } => {
                if needle.is_empty() {
                    return false;
                }
                let window = &facts.head[..(*range).min(facts.head.len())];
                find_subslice(window, needle.as_bytes())
            }
            MatchRule::SizeRange { min, max } => {
                min.is_none_or(|m| facts.size >= m) && max.is_none_or(|m| facts.size <= m)
            }
            MatchRule::DateRange { from, to } => {
                from.is_none_or(|f| facts.modified >= f) && to.is_none_or(|t| facts.modified <= t)
            }
            MatchRule::And { rules } => !rules.is_empty() && rules.iter().all(|r| r.matches(facts)),
            MatchRule::Or { rules } => rules.iter().any(|r| r.matches(facts)),
        }
    }

    /// The largest number of leading bytes a content rule inside this rule
    /// needs, 0 when no rule looks at the content.
    pub fn head_needed(&self) -> usize {
        match self {
            MatchRule::ContentBytes { offset, bytes } => offset.saturating_add(bytes.len()).min(HEAD_BYTES),
            MatchRule::ContentText { range, .. } => (*range).min(HEAD_BYTES),
            MatchRule::And { rules } | MatchRule::Or { rules } => rules.iter().map(|r| r.head_needed()).max().unwrap_or(0),
            _ => 0,
        }
    }

    /// The first problem that makes this rule unusable, if any.
    pub fn problem(&self) -> Option<String> {
        match self {
            MatchRule::Glob { pattern } => GlobSet::parse(pattern).err(),
            MatchRule::ContentBytes { bytes, .. } => bytes.is_empty().then(|| "the byte pattern is empty".to_string()),
            MatchRule::ContentText { needle, range } => {
                if needle.is_empty() {
                    Some("the search text is empty".to_string())
                } else if *range == 0 {
                    Some("the search range must be at least 1 byte".to_string())
                } else {
                    None
                }
            }
            MatchRule::SizeRange { min: Some(min), max: Some(max) } if min > max => {
                Some("the minimum size is above the maximum".to_string())
            }
            MatchRule::DateRange { from: Some(from), to: Some(to) } if from > to => {
                Some("the start date is after the end date".to_string())
            }
            MatchRule::And { rules } | MatchRule::Or { rules } => {
                if rules.is_empty() {
                    return Some("a rule group needs at least one rule".to_string());
                }
                rules.iter().find_map(|r| r.problem())
            }
            _ => None,
        }
    }
}

/// Whether `needle` occurs in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The functions a file type runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Actions {
    /// Run on a double click.
    pub dblclick: Option<FunctionId>,
    /// Run when files are dropped on a file of this type.
    pub drop: Option<FunctionId>,
    /// Entries of the "Open With" context menu, labelled by function name.
    pub menu: Vec<FunctionId>,
}

/// One user-defined file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileType {
    /// Display name, unique in the list.
    pub name: String,
    /// Which files belong to the type.
    #[serde(rename = "match")]
    pub rule: MatchRule,
    /// What to run for them.
    #[serde(default)]
    pub actions: Actions,
}

/// The ordered list of file types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileTypes {
    /// Types in matching order: the first match wins.
    pub types: Vec<FileType>,
}

impl FileTypes {
    /// The first type matching `facts`, or `None`.
    pub fn find(&self, facts: &FileFacts) -> Option<&FileType> {
        self.types.iter().find(|t| t.rule.matches(facts))
    }

    /// The largest head any rule needs, so the caller reads the file once.
    pub fn head_needed(&self) -> usize {
        self.types.iter().map(|t| t.rule.head_needed()).max().unwrap_or(0)
    }

    /// Every problem that should block saving: an empty or repeated name, or
    /// an unusable rule. Each message names the type.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for t in &self.types {
            let label = if t.name.trim().is_empty() { "(unnamed)" } else { t.name.as_str() };
            if t.name.trim().is_empty() {
                out.push("a file type has no name".to_string());
            } else if !seen.insert(t.name.to_lowercase()) {
                out.push(format!("{label}: the name is used twice"));
            }
            if let Some(p) = t.rule.problem() {
                out.push(format!("{label}: {p}"));
            }
        }
        out
    }
}

/// Parse `A1 b2 0xFF` style text (hex bytes, optional `0x`, any spacing, an
/// optional `,` between bytes) into bytes.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let compact: String = text.chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
    let compact = compact.replace("0x", "").replace("0X", "");
    if !compact.len().is_multiple_of(2) {
        return Err(format!("odd number of hex digits in {text:?}"));
    }
    for pair in compact.as_bytes().chunks(2) {
        let digits = std::str::from_utf8(pair).map_err(|_| format!("not hex: {text:?}"))?;
        out.push(u8::from_str_radix(digits, 16).map_err(|_| format!("not hex: {digits:?}"))?);
    }
    Ok(out)
}

/// Bytes as upper-case hex pairs separated by spaces (inverse of [`parse_hex`]).
pub fn format_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Number of leading bytes the Filetype Creator keeps as a signature.
const SIGNATURE_BYTES: usize = 8;

/// The Filetype Creator: derive a rule from a sample file. A name with an
/// extension gives a `*.ext` glob; otherwise a binary file gives its first
/// bytes as a signature, and anything else gives the exact name.
pub fn derive_rule(name: &str, head: &[u8]) -> MatchRule {
    let extension = Path::new(name).extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty());
    if let Some(ext) = extension {
        return MatchRule::Glob { pattern: format!("*.{}", ext.to_lowercase()) };
    }
    let looks_binary = head.iter().take(SIGNATURE_BYTES).any(|b| *b == 0 || (*b < 0x20 && !matches!(b, b'\n' | b'\r' | b'\t')));
    if looks_binary && !head.is_empty() {
        let len = head.len().min(SIGNATURE_BYTES);
        return MatchRule::ContentBytes { offset: 0, bytes: head[..len].to_vec() };
    }
    MatchRule::Glob { pattern: name.to_string() }
}

/// A suggested type name for a sample file: the upper-cased extension plus
/// " file", or the file name.
pub fn derive_name(name: &str) -> String {
    match Path::new(name).extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty()) {
        Some(ext) => format!("{} file", ext.to_uppercase()),
        None => name.to_string(),
    }
}

/// Path to `filetypes.json` under [`config_dir`].
pub fn filetypes_path() -> PathBuf {
    config_dir().join("filetypes.json")
}

/// Load the file types from the standard path. A missing or malformed file
/// yields an empty list; a malformed one is first copied to `.bak` so the
/// next save cannot destroy it.
pub fn load_filetypes() -> FileTypes {
    load_filetypes_from(&filetypes_path())
}

/// Testable core of [`load_filetypes`].
pub fn load_filetypes_from(path: &Path) -> FileTypes {
    load_json(path, "opusdm", "file types")
}

/// Save the file types to the standard path.
pub fn save_filetypes(types: &FileTypes) -> io::Result<()> {
    save_filetypes_to(&filetypes_path(), types)
}

/// Testable core of [`save_filetypes`].
pub fn save_filetypes_to(path: &Path, types: &FileTypes) -> io::Result<()> {
    save_json(path, types)
}

/// Extensions of the audio files the default types play with `opusdm-player`.
const AUDIO_PATTERN: &str = "*.mp3;*.flac;*.ogg;*.oga;*.opus;*.wav;*.m4a;*.aac;*.wma;*.mka";
/// Extensions of the video files the default types play with `opusdm-player`.
const VIDEO_PATTERN: &str = "*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.mpg;*.mpeg;*.wmv;*.flv;*.ogv";
/// Extensions of the picture files the default type opens with `opusdm-viewer`.
const IMAGE_PATTERN: &str = "*.png;*.jpg;*.jpeg;*.gif;*.webp;*.svg;*.bmp;*.ico;*.tiff";
/// Extension of the documents the default type opens with `opusdm-viewer`.
const PDF_PATTERN: &str = "*.pdf";
/// Extensions of the Markdown texts the default type opens with `opusdm-viewer`.
const MARKDOWN_PATTERN: &str = "*.md;*.markdown";
/// Extensions and names of the source and plain texts the default type opens with `opusdm-viewer`.
const SOURCE_PATTERN: &str = "*.txt;*.log;*.rs;*.c;*.h;*.cc;*.cpp;*.cxx;*.hh;*.hpp;*.hxx;*.m;*.mm;*.cs;*.java;*.kt;*.kts;*.scala;*.groovy;*.gradle;*.go;*.swift;*.dart;*.zig;*.nim;*.d;*.py;*.pyw;*.rb;*.php;*.pl;*.pm;*.lua;*.tcl;*.r;*.jl;*.hs;*.ml;*.mli;*.clj;*.el;*.lisp;*.erl;*.ex;*.exs;*.js;*.mjs;*.cjs;*.ts;*.jsx;*.tsx;*.vue;*.svelte;*.html;*.htm;*.xml;*.xsl;*.css;*.scss;*.sass;*.less;*.json;*.jsonc;*.toml;*.yaml;*.yml;*.ini;*.conf;*.cfg;*.properties;*.desktop;*.service;*.sh;*.bash;*.zsh;*.fish;*.ps1;*.bat;*.cmd;*.sql;*.diff;*.patch;*.cmake;*.mk;*.tex;Makefile;GNUmakefile;Dockerfile;Containerfile;justfile;PKGBUILD";
/// Double click: view the picture.
const VIEWER_OPEN: &str = "exec:opusdm-viewer {f}";
/// Double click: play the file.
const PLAYER_OPEN: &str = "exec:opusdm-player {f}";
/// Open With: play the file again and again.
const PLAYER_LOOP: &str = "exec:opusdm-player --loop {f}";

/// The types a new installation starts with: audio and video files open with
/// `opusdm-player`, pictures, PDF documents, Markdown and source texts with `opusdm-viewer`. They use inline `exec:` references, so no saved Function is
/// needed.
pub fn default_filetypes() -> FileTypes {
    let player_type = |name: &str, pattern: &str| FileType {
        name: name.to_string(),
        rule: MatchRule::Glob { pattern: pattern.to_string() },
        actions: Actions { dblclick: Some(PLAYER_OPEN.to_string()), drop: None, menu: vec![PLAYER_LOOP.to_string()] },
    };
    let viewer_type = |name: &str, pattern: &str| FileType {
        name: name.to_string(),
        rule: MatchRule::Glob { pattern: pattern.to_string() },
        actions: Actions { dblclick: Some(VIEWER_OPEN.to_string()), drop: None, menu: vec![] },
    };
    FileTypes { types: vec![player_type("Audio file", AUDIO_PATTERN), player_type("Video file", VIDEO_PATTERN), viewer_type("Image file", IMAGE_PATTERN), viewer_type("PDF file", PDF_PATTERN), viewer_type("Markdown file", MARKDOWN_PATTERN), viewer_type("Source file", SOURCE_PATTERN)] }
}

/// Write [`default_filetypes`] to the standard path when `filetypes.json` does
/// not exist yet. An existing file (even an empty or malformed one) is never
/// touched, so what the user edited or deleted stays that way. Returns whether
/// the file was written.
pub fn seed_default_filetypes() -> bool {
    seed_default_filetypes_at(&filetypes_path())
}

/// Testable core of [`seed_default_filetypes`]. A failure is logged, never fatal.
pub fn seed_default_filetypes_at(path: &Path) -> bool {
    if path.exists() { return false; }
    match save_filetypes_to(path, &default_filetypes()) {
        Ok(()) => true,
        Err(e) => {
            crate::err_log!("opusdm", "cannot write the default file types to {}: {e}", path.display());
            false
        }
    }
}

/// Read the leading bytes of the file at `path`, at most `wanted` (capped at
/// [`HEAD_BYTES`]). An unreadable file gives an empty head.
pub fn read_head(path: &Path, wanted: usize) -> Vec<u8> {
    use std::io::Read;
    let wanted = wanted.min(HEAD_BYTES);
    if wanted == 0 {
        return Vec::new();
    }
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    let mut buf = Vec::with_capacity(wanted);
    let _ = file.take(wanted as u64).read_to_end(&mut buf);
    buf
}

/// The double click reference of the user file type of the file at `path`,
/// or `None` when the path is not a regular file, no type matches or the type
/// has no double click action. Reads `filetypes.json` on every call, like the
/// Lister does, so an edit applies at once.
pub fn dblclick_for(path: &Path) -> Option<FunctionId> {
    dblclick_in(&load_filetypes(), path)
}

/// [`dblclick_for`] against the given `types` (testable).
fn dblclick_in(types: &FileTypes, path: &Path) -> Option<FunctionId> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let head = read_head(path, types.head_needed());
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    let facts = FileFacts { name: &name, size: meta.len(), modified, head: &head };
    types.find(&facts).and_then(|t| t.actions.dblclick.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(name: &'a str, size: u64, head: &'a [u8]) -> FileFacts<'a> {
        FileFacts { name, size, modified: 1000, head }
    }

    fn glob(p: &str) -> MatchRule {
        MatchRule::Glob { pattern: p.into() }
    }

    fn png_type() -> FileType {
        FileType {
            name: "PNG image".into(),
            rule: MatchRule::Or {
                rules: vec![glob("*.png"), MatchRule::ContentBytes { offset: 1, bytes: b"PNG".to_vec() }],
            },
            actions: Actions { dblclick: Some("feh".into()), drop: None, menu: vec!["gimp".into()] },
        }
    }

    #[test]
    fn glob_matches_case_insensitively_and_with_alternatives() {
        let rule = glob("*.png;*.jpg");
        assert!(rule.matches(&facts("A.PNG", 1, b"")));
        assert!(rule.matches(&facts("b.jpg", 1, b"")));
        assert!(!rule.matches(&facts("c.gif", 1, b"")));
    }

    #[test]
    fn content_bytes_checks_the_offset_and_bounds() {
        let rule = MatchRule::ContentBytes { offset: 1, bytes: b"PNG".to_vec() };
        assert!(rule.matches(&facts("x", 8, b"\x89PNG\r\n")));
        assert!(!rule.matches(&facts("x", 8, b"\x89PN")));
        assert!(!rule.matches(&facts("x", 8, b"PNG")));
        let huge = MatchRule::ContentBytes { offset: usize::MAX, bytes: vec![1] };
        assert!(!huge.matches(&facts("x", 8, b"abc")));
    }

    #[test]
    fn content_text_only_looks_inside_the_range() {
        let rule = MatchRule::ContentText { needle: "#!/bin/sh".into(), range: 16 };
        assert!(rule.matches(&facts("x", 40, b"#!/bin/sh\necho hi")));
        let late = MatchRule::ContentText { needle: "hi".into(), range: 4 };
        assert!(!late.matches(&facts("x", 40, b"#!/bin/sh\necho hi")));
    }

    #[test]
    fn size_and_date_ranges_have_open_bounds() {
        let size = MatchRule::SizeRange { min: Some(10), max: Some(20) };
        assert!(size.matches(&facts("x", 10, b"")));
        assert!(size.matches(&facts("x", 20, b"")));
        assert!(!size.matches(&facts("x", 21, b"")));
        assert!(MatchRule::SizeRange { min: None, max: None }.matches(&facts("x", 5, b"")));
        let date = MatchRule::DateRange { from: Some(500), to: None };
        assert!(date.matches(&facts("x", 1, b"")));
        assert!(!MatchRule::DateRange { from: Some(2000), to: None }.matches(&facts("x", 1, b"")));
    }

    #[test]
    fn and_or_combine_and_empty_groups_never_match() {
        let both = MatchRule::And { rules: vec![glob("*.txt"), MatchRule::SizeRange { min: Some(5), max: None }] };
        assert!(both.matches(&facts("a.txt", 9, b"")));
        assert!(!both.matches(&facts("a.txt", 1, b"")));
        assert!(!MatchRule::And { rules: vec![] }.matches(&facts("a", 1, b"")));
        assert!(!MatchRule::Or { rules: vec![] }.matches(&facts("a", 1, b"")));
    }

    #[test]
    fn find_returns_the_first_matching_type() {
        let any = FileType { name: "Anything".into(), rule: glob("*"), actions: Actions::default() };
        let list = FileTypes { types: vec![png_type(), any] };
        assert_eq!(list.find(&facts("a.png", 1, b"")).unwrap().name, "PNG image");
        assert_eq!(list.find(&facts("a.txt", 1, b"")).unwrap().name, "Anything");
        assert!(FileTypes::default().find(&facts("a.png", 1, b"")).is_none());
    }

    #[test]
    fn head_needed_is_the_largest_content_requirement() {
        assert_eq!(glob("*").head_needed(), 0);
        assert_eq!(png_type().rule.head_needed(), 4);
        let list = FileTypes {
            types: vec![png_type(), FileType {
                name: "T".into(),
                rule: MatchRule::ContentText { needle: "a".into(), range: 1_000_000 },
                actions: Actions::default(),
            }],
        };
        assert_eq!(list.head_needed(), HEAD_BYTES);
    }

    #[test]
    fn problems_flag_names_and_rules() {
        let bad = FileTypes {
            types: vec![
                png_type(),
                FileType { name: "png IMAGE".into(), rule: glob("*.x"), actions: Actions::default() },
                FileType { name: " ".into(), rule: glob("*.x"), actions: Actions::default() },
                FileType { name: "Empty".into(), rule: MatchRule::Or { rules: vec![] }, actions: Actions::default() },
                FileType { name: "Range".into(), rule: MatchRule::SizeRange { min: Some(5), max: Some(1) }, actions: Actions::default() },
            ],
        };
        let problems = bad.problems();
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("used twice")));
        assert!(problems.iter().any(|p| p.contains("no name")));
        assert!(FileTypes { types: vec![png_type()] }.problems().is_empty());
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(parse_hex("89 50 4e 47").unwrap(), vec![0x89, 0x50, 0x4e, 0x47]);
        assert_eq!(parse_hex("0x89,0x50").unwrap(), vec![0x89, 0x50]);
        assert!(parse_hex("8").is_err());
        assert!(parse_hex("zz").is_err());
        assert_eq!(format_hex(&[0x89, 0x0a]), "89 0A");
    }

    #[test]
    fn derive_rule_prefers_extension_then_signature_then_name() {
        assert_eq!(derive_rule("Photo.PNG", b""), glob("*.png"));
        assert_eq!(derive_rule("archive.tar.gz", b""), glob("*.gz"));
        assert_eq!(
            derive_rule("blob", b"\x7fELF\x02\x01\x01\x00\x00\x00"),
            MatchRule::ContentBytes { offset: 0, bytes: b"\x7fELF\x02\x01\x01\x00".to_vec() }
        );
        assert_eq!(derive_rule("Makefile", b"all:\n"), glob("Makefile"));
        // A dotfile has no extension.
        assert_eq!(derive_rule(".bashrc", b"# rc"), glob(".bashrc"));
        assert_eq!(derive_name("a.png"), "PNG file");
        assert_eq!(derive_name("Makefile"), "Makefile");
    }

    #[test]
    fn json_round_trip_uses_the_match_key() {
        let list = FileTypes { types: vec![png_type()] };
        let text = serde_json::to_string(&list).unwrap();
        assert!(text.contains("\"match\""), "{text}");
        assert_eq!(serde_json::from_str::<FileTypes>(&text).unwrap(), list);
    }

    #[test]
    fn save_and_load_round_trip_and_a_bad_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filetypes.json");
        assert_eq!(load_filetypes_from(&path), FileTypes::default());
        let list = FileTypes { types: vec![png_type()] };
        save_filetypes_to(&path, &list).unwrap();
        assert_eq!(load_filetypes_from(&path), list);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_filetypes_from(&path), FileTypes::default());
        assert_eq!(std::fs::read_to_string(dir.path().join("filetypes.json.bak")).unwrap(), "{ not json");
    }

    #[test]
    fn read_head_caps_the_length_and_tolerates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"0123456789").unwrap();
        assert_eq!(read_head(&path, 4), b"0123");
        assert_eq!(read_head(&path, 100), b"0123456789");
        assert!(read_head(&path, 0).is_empty());
        assert!(read_head(&dir.path().join("none"), 4).is_empty());
    }

    #[test]
    fn dblclick_finds_the_action_of_a_matching_regular_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let (song, bin) = (dir.path().join("a.mp3"), dir.path().join("a.bin"));
        std::fs::write(&song, "x").unwrap();
        std::fs::write(&bin, "x").unwrap();
        let types = default_filetypes();
        assert_eq!(dblclick_in(&types, &song).as_deref(), Some("exec:opusdm-player {f}"));
        assert_eq!(dblclick_in(&types, &bin), None, "no type matches");
        assert_eq!(dblclick_in(&types, dir.path()), None, "a folder never matches");
        assert_eq!(dblclick_in(&types, &dir.path().join("missing.mp3")), None);
    }

    #[test]
    fn default_types_are_valid_and_match_media_files() {
        let list = default_filetypes();
        assert!(list.problems().is_empty(), "{:?}", list.problems());
        assert_eq!(list.find(&facts("Song.MP3", 1, b"")).unwrap().name, "Audio file");
        assert_eq!(list.find(&facts("film.mkv", 1, b"")).unwrap().name, "Video file");
        assert!(list.find(&facts("data.bin", 1, b"")).is_none());
        assert_eq!(list.find(&facts("notes.txt", 1, b"")).unwrap().name, "Source file");
        assert_eq!(list.find(&facts("main.RS", 1, b"")).unwrap().name, "Source file");
        assert_eq!(list.find(&facts("Makefile", 1, b"")).unwrap().name, "Source file");
        assert_eq!(list.find(&facts("photo.PNG", 1, b"")).unwrap().name, "Image file");
        assert_eq!(list.find(&facts("manual.Pdf", 1, b"")).unwrap().name, "PDF file");
        assert_eq!(list.find(&facts("README.MD", 1, b"")).unwrap().name, "Markdown file");
    }

    #[test]
    fn default_actions_start_the_program_with_the_file() {
        use crate::funcref::{parse, FuncRef};
        for t in default_filetypes().types {
            let refs = std::iter::once(t.actions.dblclick.clone().unwrap()).chain(t.actions.menu.clone());
            for r in refs {
                let FuncRef::Exec { line } = parse(&r) else { panic!("{}: {r} is not an exec reference", t.name) };
                let words = crate::cli::tokenize(&line).unwrap();
                let program = if matches!(t.name.as_str(), "Image file" | "PDF file" | "Markdown file" | "Source file") { "opusdm-viewer" } else { "opusdm-player" };
                assert_eq!(words.first().map(String::as_str), Some(program), "{r}");
                assert_eq!(words.last().map(String::as_str), Some("{f}"), "{r}");
            }
        }
    }

    #[test]
    fn seed_writes_the_defaults_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("filetypes.json");
        assert!(seed_default_filetypes_at(&path));
        assert_eq!(load_filetypes_from(&path), default_filetypes());
        // The user deletes a type: the next start does not bring it back.
        let mut list = load_filetypes_from(&path);
        list.types.pop();
        save_filetypes_to(&path, &list).unwrap();
        assert!(!seed_default_filetypes_at(&path));
        assert_eq!(load_filetypes_from(&path).types.len(), default_filetypes().types.len() - 1);
    }

    #[test]
    fn seed_never_touches_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        for content in ["{\"types\": []}", "not json at all", ""] {
            let path = dir.path().join("filetypes.json");
            std::fs::write(&path, content).unwrap();
            assert!(!seed_default_filetypes_at(&path));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
    }
}
