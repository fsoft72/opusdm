//! Reading and writing the few fields of a freedesktop `.desktop` file that
//! the Apps editor exposes (name, comment, command, icon, working directory,
//! terminal). Pure text handling: every other key and group of an existing
//! file is kept as it is.

/// The group that holds an application's fields.
const GROUP: &str = "[Desktop Entry]";

/// The editable fields of an application entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopFields {
    pub name: String,
    pub comment: String,
    /// The `Exec=` line, field codes (`%U`) included.
    pub exec: String,
    /// An icon theme name or an absolute image path.
    pub icon: String,
    /// The `Path=` working directory.
    pub path: String,
    /// `Terminal=true`: run inside a terminal emulator.
    pub terminal: bool,
}

/// The plain (non localized) `key=value` pair of `line`, `None` for anything else.
fn _pair(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

/// The lines of the `[Desktop Entry]` group of `text`, as an index range.
fn _group_range(lines: &[String]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|l| l.trim() == GROUP)? + 1;
    let end = lines[start..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |p| start + p);
    Some((start, end))
}

/// Parse the editable fields out of the text of a `.desktop` file; missing
/// keys are empty.
pub fn parse(text: &str) -> DesktopFields {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut f = DesktopFields::default();
    let Some((start, end)) = _group_range(&lines) else { return f };
    for line in &lines[start..end] {
        let Some((key, value)) = _pair(line) else { continue };
        match key {
            "Name" => f.name = value.to_string(),
            "Comment" => f.comment = value.to_string(),
            "Exec" => f.exec = value.to_string(),
            "Icon" => f.icon = value.to_string(),
            "Path" => f.path = value.to_string(),
            "Terminal" => f.terminal = value == "true",
            _ => {}
        }
    }
    f
}

/// The `(key, value)` lines `f` stands for. Optional keys with an empty
/// value are `None`, so they are dropped from the file.
fn _wanted(f: &DesktopFields) -> [(&'static str, Option<String>); 6] {
    let opt = |v: &str| (!v.is_empty()).then(|| v.to_string());
    [
        ("Name", Some(f.name.clone())),
        ("Comment", opt(&f.comment)),
        ("Exec", Some(f.exec.clone())),
        ("Icon", opt(&f.icon)),
        ("Path", opt(&f.path)),
        ("Terminal", Some(if f.terminal { "true" } else { "false" }.to_string())),
    ]
}

/// The text of a `.desktop` file with `f` applied to `original` (`None` for a
/// new file). Existing keys are replaced in place, new ones are added at the
/// end of the group, emptied optional ones are removed.
pub fn render(original: Option<&str>, f: &DesktopFields) -> String {
    let mut lines: Vec<String> = match original {
        Some(t) => t.lines().map(str::to_string).collect(),
        None => vec![GROUP.to_string(), "Type=Application".to_string()],
    };
    if _group_range(&lines).is_none() {
        lines.insert(0, GROUP.to_string());
    }
    for (key, value) in _wanted(f) {
        let (start, end) = _group_range(&lines).expect("group exists");
        let at = (start..end).find(|&i| _pair(&lines[i]).is_some_and(|(k, _)| k == key));
        match (at, value) {
            (Some(i), Some(v)) => lines[i] = format!("{key}={v}"),
            (Some(i), None) => {
                lines.remove(i);
            }
            (None, Some(v)) => {
                // After the last non blank line of the group.
                let mut pos = end;
                while pos > start && lines[pos - 1].trim().is_empty() {
                    pos -= 1;
                }
                lines.insert(pos, format!("{key}={v}"));
            }
            (None, None) => {}
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A file name friendly slug of `name`: lowercase letters and digits, runs of
/// anything else collapsed to one `-`. `app` when nothing is left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "app".to_string()
    } else {
        out
    }
}

/// The id `opusdm-<slug>.desktop` for a new entry named `name`, with a
/// numeric suffix while `taken` says the id is in use.
pub fn new_id(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = format!("opusdm-{}", slug(name));
    let first = format!("{base}.desktop");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{base}-{n}.desktop"))
        .find(|id| !taken(id))
        .expect("an unbounded range yields a free id")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Desktop Entry]\nType=Application\nName=Firefox\nName[it]=Volpe\nExec=firefox %u\nIcon=firefox\nTerminal=false\n\n[Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n";

    #[test]
    fn parse_reads_the_main_group_only() {
        let f = parse(SAMPLE);
        assert_eq!(f.name, "Firefox");
        assert_eq!(f.exec, "firefox %u");
        assert_eq!(f.icon, "firefox");
        assert!(!f.terminal);
        assert_eq!(f.comment, "");
    }

    #[test]
    fn render_replaces_in_place_and_keeps_everything_else() {
        let mut f = parse(SAMPLE);
        f.name = "Fox".into();
        f.comment = "Browse".into();
        f.terminal = true;
        let out = render(Some(SAMPLE), &f);
        assert!(out.contains("Name=Fox\n"));
        assert!(out.contains("Name[it]=Volpe\n"));
        assert!(out.contains("Comment=Browse\n"));
        assert!(out.contains("Terminal=true\n"));
        assert!(out.contains("[Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n"));
        // The action group is untouched and the new key stayed in the main group.
        assert!(out.find("Comment=Browse").unwrap() < out.find("[Desktop Action").unwrap());
        assert_eq!(parse(&out), f);
    }

    #[test]
    fn render_drops_emptied_optional_keys() {
        let mut f = parse(SAMPLE);
        f.icon.clear();
        let out = render(Some(SAMPLE), &f);
        assert!(!out.contains("Icon="));
    }

    #[test]
    fn render_a_new_file() {
        let f = DesktopFields { name: "My App".into(), exec: "/bin/true".into(), path: "/tmp".into(), ..Default::default() };
        let out = render(None, &f);
        assert!(out.starts_with("[Desktop Entry]\nType=Application\n"));
        assert_eq!(parse(&out), f);
    }

    #[test]
    fn slug_and_new_id() {
        assert_eq!(slug("  My  Cool App!! "), "my-cool-app");
        assert_eq!(slug("???"), "app");
        assert_eq!(new_id("Foo", |_| false), "opusdm-foo.desktop");
        let taken = |id: &str| id == "opusdm-foo.desktop" || id == "opusdm-foo-2.desktop";
        assert_eq!(new_id("Foo", taken), "opusdm-foo-3.desktop");
    }
}
