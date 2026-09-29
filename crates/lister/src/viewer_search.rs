//! Search helpers of the Viewer.

/// Fold a character for case-insensitive comparison, one character to one
/// character so offsets stay aligned with the original text.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Every non-overlapping occurrence of `query` in `text`, ignoring case, as
/// half-open ranges of character offsets (the unit GTK text buffers use).
pub fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = query.chars().map(fold).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let haystack: Vec<char> = text.chars().map(fold).collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if haystack[i..i + needle.len()] == needle[..] {
            found.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

/// The match after (`forward`) or before `current`, wrapping around. With no
/// current match it starts at the first (forward) or the last (backward).
pub fn step(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (current, forward) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(i), true) => (i + 1) % count,
        (Some(i), false) => (i + count - 1) % count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ignore_case_and_do_not_overlap() {
        assert_eq!(find_matches("Foo foo FOO", "foo"), vec![(0, 3), (4, 7), (8, 11)]);
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        // "é" is two bytes but one character.
        assert_eq!(find_matches("éé ab", "ab"), vec![(3, 5)]);
        assert_eq!(find_matches("Ünï ÜNÏ", "ün"), vec![(0, 2), (4, 6)]);
    }

    #[test]
    fn an_empty_query_or_text_has_no_matches() {
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("", "a").is_empty());
        assert!(find_matches("abc", "zz").is_empty());
    }

    #[test]
    fn step_wraps_around_in_both_directions() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(None, 3, false), Some(2));
        assert_eq!(step(Some(2), 3, true), Some(0));
        assert_eq!(step(Some(0), 3, false), Some(2));
        assert_eq!(step(Some(1), 3, true), Some(2));
    }

    #[test]
    fn step_with_no_matches_is_none() {
        assert_eq!(step(None, 0, true), None);
        assert_eq!(step(Some(5), 0, false), None);
    }
}
