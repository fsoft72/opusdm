//! Matching and ranking of the Run window's suggestions. Pure logic: no GTK,
//! so it is unit tested.

/// Most suggestions shown at once.
pub const MAX_SUGGESTIONS: usize = 8;

/// What a suggestion starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A desktop application (listed before commands on a tie).
    App,
    /// An executable found on `PATH`.
    Command,
}

/// One thing the Run window can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Application or command.
    pub kind: Kind,
    /// Desktop file id for an application, program name for a command.
    pub id: String,
    /// Text shown and matched first.
    pub name: String,
    /// Dimmed text after the name (empty for a command).
    pub comment: String,
    /// Further text matched after the name: generic name, keywords, program.
    pub extra: Vec<String>,
    /// Serialized icon (`gio::Icon::to_string`), if the entry has one.
    pub icon: Option<String>,
}

/// One row of the suggestion list: an index into the entries, plus the
/// arguments typed after a command name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Index of the entry in the slice given to [`suggest`].
    pub entry: usize,
    /// Text typed after the command name (empty for applications).
    pub args: String,
}

/// The lower-cased texts of one entry that a query is matched against,
/// computed once instead of on every keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Keys {
    name: String,
    extra: Vec<String>,
}

impl Keys {
    /// The keys of `entry`.
    fn of(entry: &Entry) -> Keys {
        Keys { name: entry.name.to_lowercase(), extra: entry.extra.iter().map(|t| t.to_lowercase()).collect() }
    }
}

/// Every entry the Run window can start, with its search keys.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: Vec<Entry>,
    keys: Vec<Keys>,
}

impl Catalog {
    /// A catalog of `entries`, in that order.
    pub fn new(entries: Vec<Entry>) -> Catalog {
        let mut catalog = Catalog::default();
        catalog.extend(entries);
        catalog
    }

    /// Append `more` entries after the current ones.
    pub fn extend(&mut self, more: Vec<Entry>) {
        self.keys.extend(more.iter().map(Keys::of));
        self.entries.extend(more);
    }

    /// The entries, in catalog order; [`Suggestion::entry`] indexes them.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// How well an entry with `keys` matches `query` (lower-cased); `None` when
/// it does not. 0 exact name, 1 name prefix, 2 word prefix, 3 name
/// substring, 4 extra text.
fn score(keys: &Keys, query: &str) -> Option<u32> {
    if query.is_empty() {
        return None;
    }
    let name = keys.name.as_str();
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    if name.split(|c: char| !c.is_alphanumeric()).any(|word| !word.is_empty() && word.starts_with(query)) {
        return Some(2);
    }
    if name.contains(query) {
        return Some(3);
    }
    keys.extra.iter().any(|text| text.contains(query)).then_some(4)
}

/// The best match of `entry` (with `keys`) for `text`: the whole text
/// against anything, and for a command also its first word, the rest being
/// kept as arguments.
fn best_match(entry: &Entry, keys: &Keys, text: &str) -> Option<(u32, String)> {
    let whole = score(keys, text).map(|s| (s, String::new()));
    if entry.kind != Kind::Command {
        return whole;
    }
    let Some((first, rest)) = text.split_once(char::is_whitespace) else { return whole };
    let split = score(keys, first).map(|s| (s, rest.trim().to_string()));
    match (whole, split) {
        (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The suggestions for the typed `text`, best first, at most `limit`. Empty
/// text suggests nothing.
pub fn suggest(catalog: &Catalog, text: &str, limit: usize) -> Vec<Suggestion> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(u32, Kind, &str, Suggestion)> = catalog
        .entries
        .iter()
        .zip(&catalog.keys)
        .enumerate()
        .filter_map(|(entry, (e, keys))| {
            let (score, args) = best_match(e, keys, &text)?;
            Some((score, e.kind, keys.name.as_str(), Suggestion { entry, args }))
        })
        .collect();
    found.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    found.into_iter().take(limit).map(|(_, _, _, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, name: &str, extra: &[&str]) -> Entry {
        Entry { kind, id: name.to_string(), name: name.to_string(), comment: String::new(), extra: extra.iter().map(|s| s.to_string()).collect(), icon: None }
    }

    fn names(entries: &[Entry], text: &str) -> Vec<String> {
        suggest(&Catalog::new(entries.to_vec()), text, MAX_SUGGESTIONS).iter().map(|s| entries[s.entry].name.clone()).collect()
    }

    #[test]
    fn the_catalog_lowercases_the_keys_once() {
        let catalog = Catalog::new(vec![entry(Kind::App, "GNU Image Editor", &["GIMP", "Paint"])]);
        assert_eq!(catalog.keys, [Keys { name: "gnu image editor".into(), extra: vec!["gimp".into(), "paint".into()] }]);
        assert_eq!(catalog.entries()[0].name, "GNU Image Editor");
    }

    #[test]
    fn entries_added_later_are_searched_after_the_first_ones() {
        let mut catalog = Catalog::new(vec![entry(Kind::App, "Terminal", &[])]);
        assert!(suggest(&catalog, "xterm", 8).is_empty());
        catalog.extend(vec![entry(Kind::Command, "XTerm", &[])]);
        assert_eq!(suggest(&catalog, "xterm", 8), [Suggestion { entry: 1, args: String::new() }]);
        assert_eq!(catalog.keys.len(), catalog.entries().len());
    }

    #[test]
    fn empty_text_suggests_nothing() {
        let entries = [entry(Kind::App, "Neovim", &[])];
        assert!(suggest(&Catalog::new(entries.to_vec()), "   ", 8).is_empty());
    }

    #[test]
    fn matches_are_ranked_exact_prefix_word_substring_extra() {
        let entries = [
            entry(Kind::App, "Text Editor", &[]),
            entry(Kind::App, "Context", &[]),
            entry(Kind::App, "Notepad", &["text"]),
            entry(Kind::App, "Text", &[]),
            entry(Kind::App, "Texture Viewer", &[]),
        ];
        assert_eq!(names(&entries, "text"), ["Text", "Text Editor", "Texture Viewer", "Context", "Notepad"]);
        assert_eq!(names(&entries, "edi"), ["Text Editor"]);
    }

    #[test]
    fn matching_ignores_case() {
        let entries = [entry(Kind::App, "Neovim", &[])];
        assert_eq!(names(&entries, "NEO"), ["Neovim"]);
    }

    #[test]
    fn an_exact_command_beats_a_longer_app_name() {
        let entries = [entry(Kind::App, "Bash Reference", &[]), entry(Kind::Command, "bash", &[])];
        assert_eq!(names(&entries, "bash"), ["bash", "Bash Reference"]);
    }

    #[test]
    fn apps_come_before_commands_on_a_tie() {
        let entries = [entry(Kind::Command, "zen-helper", &[]), entry(Kind::App, "Zen", &[])];
        assert_eq!(names(&entries, "ze"), ["Zen", "zen-helper"]);
    }

    #[test]
    fn a_command_keeps_the_arguments_after_its_name() {
        let entries = [entry(Kind::Command, "bash", &[]), entry(Kind::App, "Visual Studio", &[])];
        let found = suggest(&Catalog::new(entries.to_vec()), "bash -c ls", 8);
        assert_eq!(found, [Suggestion { entry: 0, args: "-c ls".to_string() }]);
        let found = suggest(&Catalog::new(entries.to_vec()), "visual st", 8);
        assert_eq!(found, [Suggestion { entry: 1, args: String::new() }]);
    }

    #[test]
    fn the_list_is_cut_at_the_limit() {
        let entries: Vec<Entry> = (0..20).map(|i| entry(Kind::Command, &format!("tool{i:02}"), &[])).collect();
        assert_eq!(suggest(&Catalog::new(entries.to_vec()), "tool", MAX_SUGGESTIONS).len(), MAX_SUGGESTIONS);
    }
}
