//! The history of the clipboard manager: the text entries, newest first.
//! Pure: no GTK, so the rules are unit tested.

/// Entries kept in the history.
pub const MAX_ENTRIES: usize = 20;
/// Longest text kept, in bytes. A bigger copy (a whole log, a file dump) is
/// left on the clipboard but not remembered.
pub const MAX_ENTRY_BYTES: usize = 64 * 1024;
/// Characters of an entry shown in the list.
pub const PREVIEW_CHARS: usize = 60;

/// What `History::push` did with a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// A new entry on top.
    Added,
    /// The text was already there: it went back to the top.
    Moved,
    /// The text is the top entry already.
    Unchanged,
    /// Blank or too big: not remembered.
    Ignored,
}

/// The remembered texts, newest first.
#[derive(Debug)]
pub struct History {
    entries: Vec<String>,
    limit: usize,
}

impl History {
    /// An empty history that keeps at most `limit` entries.
    pub fn new(limit: usize) -> Self {
        Self { entries: Vec::new(), limit }
    }

    /// The entries, newest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Remember `text`: on top, once, and the oldest entry goes when full.
    pub fn push(&mut self, text: &str) -> Push {
        if text.trim().is_empty() || text.len() > MAX_ENTRY_BYTES { return Push::Ignored; }
        if self.entries.first().is_some_and(|top| top == text) { return Push::Unchanged; }
        let moved = match self.entries.iter().position(|e| e == text) {
            Some(i) => { self.entries.remove(i); true }
            None => false,
        };
        self.entries.insert(0, text.to_string());
        self.entries.truncate(self.limit);
        if moved { Push::Moved } else { Push::Added }
    }

    /// Forget the entry at `index`; false when there is none.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.entries.len() { return false; }
        self.entries.remove(index);
        true
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The entries holding `query` (case insensitive; blank matches all), with
    /// their index in the whole history.
    pub fn filtered(&self, query: &str) -> Vec<(usize, &str)> {
        let query = query.trim().to_lowercase();
        self.entries.iter().enumerate().filter(|(_, e)| query.is_empty() || e.to_lowercase().contains(&query)).map(|(i, e)| (i, e.as_str())).collect()
    }
}

/// One line of `text` for the list: the first line with text, white space
/// squeezed, cut to [`PREVIEW_CHARS`] with `...`.
pub fn preview(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if squeezed.chars().count() <= PREVIEW_CHARS { return squeezed; }
    let cut: String = squeezed.chars().take(PREVIEW_CHARS).collect();
    format!("{cut}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_text_goes_on_top() {
        let mut h = History::new(5);
        assert_eq!(h.push("a"), Push::Added);
        assert_eq!(h.push("b"), Push::Added);
        assert_eq!(h.entries(), ["b", "a"]);
    }

    #[test]
    fn the_top_text_again_changes_nothing() {
        let mut h = History::new(5);
        h.push("a");
        assert_eq!(h.push("a"), Push::Unchanged);
        assert_eq!(h.entries(), ["a"]);
    }

    #[test]
    fn an_old_text_copied_again_moves_to_the_top() {
        let mut h = History::new(5);
        h.push("a");
        h.push("b");
        assert_eq!(h.push("a"), Push::Moved);
        assert_eq!(h.entries(), ["a", "b"]);
    }

    #[test]
    fn the_oldest_entry_goes_when_full() {
        let mut h = History::new(2);
        h.push("a");
        h.push("b");
        h.push("c");
        assert_eq!(h.entries(), ["c", "b"]);
    }

    #[test]
    fn blank_and_huge_texts_are_not_remembered() {
        let mut h = History::new(5);
        assert_eq!(h.push(""), Push::Ignored);
        assert_eq!(h.push(" \n\t "), Push::Ignored);
        assert_eq!(h.push(&"x".repeat(MAX_ENTRY_BYTES + 1)), Push::Ignored);
        assert!(h.entries().is_empty());
        assert_eq!(h.push(&"x".repeat(MAX_ENTRY_BYTES)), Push::Added);
    }

    #[test]
    fn remove_and_clear() {
        let mut h = History::new(5);
        h.push("a");
        h.push("b");
        assert!(h.remove(0));
        assert_eq!(h.entries(), ["a"]);
        assert!(!h.remove(3));
        h.clear();
        assert!(h.entries().is_empty());
    }

    #[test]
    fn the_filter_ignores_case_and_keeps_the_real_index() {
        let mut h = History::new(5);
        h.push("Hello World");
        h.push("other");
        h.push("say HELLO");
        assert_eq!(h.filtered("hello"), vec![(0, "say HELLO"), (2, "Hello World")]);
        assert_eq!(h.filtered("  ").len(), 3);
        assert!(h.filtered("zzz").is_empty());
    }

    #[test]
    fn the_preview_is_one_short_line() {
        assert_eq!(preview("\n  first   line \nsecond"), "first line");
        assert_eq!(preview("short"), "short");
        let long = "a".repeat(PREVIEW_CHARS + 10);
        assert_eq!(preview(&long), format!("{}...", "a".repeat(PREVIEW_CHARS)));
        // Cuts on characters, not bytes.
        let accents = "è".repeat(PREVIEW_CHARS + 1);
        assert_eq!(preview(&accents).chars().count(), PREVIEW_CHARS + 3);
    }
}
