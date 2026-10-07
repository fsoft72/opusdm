//! Criteria of the "Advanced Select..." requester (F-21): name patterns,
//! type, size range and modification time range.

use crate::link_dialogs::parse_size;
use crate::model::Row;
use opusdm_ipc::timefmt::{parse_local, TIME_FORMAT_HINT};
use opusdm_ipc::NameFilter;

/// Which kinds of entry the criteria may match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindFilter {
    /// Files and directories.
    Any,
    /// Regular entries only.
    FilesOnly,
    /// Directories only.
    DirsOnly,
}

/// The texts typed in the requester; an empty text means "no limit".
#[derive(Clone, Copy, Debug)]
pub struct Fields<'a> {
    /// Name patterns (`*.rs,!test*`).
    pub name: &'a str,
    /// Smallest size, such as `1M`.
    pub min_size: &'a str,
    /// Largest size, such as `2G`.
    pub max_size: &'a str,
    /// Oldest modification time.
    pub after: &'a str,
    /// Newest modification time.
    pub before: &'a str,
}

/// Parsed criteria; every one that is set must hold for a row to match.
#[derive(Clone, Debug)]
pub struct Criteria {
    name: NameFilter,
    kind: KindFilter,
    min_size: Option<u64>,
    max_size: Option<u64>,
    after: Option<i64>,
    before: Option<i64>,
}

/// Seconds in the units a relative time accepts.
const RELATIVE_UNITS: [(char, i64); 4] = [('m', 60), ('h', 3600), ('d', 86_400), ('w', 604_800)];

/// Parse a time limit: empty for none, a relative `30m` / `12h` / `7d` / `2w`
/// (that long before `now`), or a local `YYYY-MM-DD [HH:MM[:SS]]`.
pub fn parse_when(text: &str, now: i64) -> Result<Option<i64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if let Some((unit, secs)) = text.chars().last().and_then(|c| RELATIVE_UNITS.iter().find(|(u, _)| *u == c)) {
        let digits = &text[..text.len() - unit.len_utf8()];
        if let Ok(n) = digits.parse::<i64>() {
            return Ok(Some(now - n * secs));
        }
    }
    let absolute = if text.contains([' ', 'T']) { text.to_string() } else { format!("{text} 00:00") };
    parse_local(&absolute)
        .map(Some)
        .map_err(|_| format!("use 30m, 12h, 7d, 2w or {TIME_FORMAT_HINT}, got {text:?}"))
}

/// Parse an optional size; empty for none.
fn parse_optional_size(text: &str) -> Result<Option<u64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_size(text).map(Some)
}

impl Criteria {
    /// Build criteria from the requester's texts. `now` anchors relative
    /// times. The error names the offending field.
    pub fn parse(fields: &Fields, kind: KindFilter, now: i64) -> Result<Self, String> {
        let labelled = |label: &str, e: String| format!("{label}: {e}");
        let min_size = parse_optional_size(fields.min_size).map_err(|e| labelled("Minimum size", e))?;
        let max_size = parse_optional_size(fields.max_size).map_err(|e| labelled("Maximum size", e))?;
        let after = parse_when(fields.after, now).map_err(|e| labelled("Modified after", e))?;
        let before = parse_when(fields.before, now).map_err(|e| labelled("Modified before", e))?;
        if matches!((min_size, max_size), (Some(lo), Some(hi)) if lo > hi) {
            return Err("Minimum size is larger than the maximum size".to_string());
        }
        if matches!((after, before), (Some(a), Some(b)) if a > b) {
            return Err("Modified after is later than Modified before".to_string());
        }
        Ok(Self { name: NameFilter::new(fields.name, ""), kind, min_size, max_size, after, before })
    }

    /// Whether `row` satisfies every criterion that is set. A size limit
    /// never matches a directory, whose size means nothing.
    pub fn matches(&self, row: &Row) -> bool {
        match self.kind {
            KindFilter::FilesOnly if row.is_dir => return false,
            KindFilter::DirsOnly if !row.is_dir => return false,
            _ => {}
        }
        if row.is_dir && (self.min_size.is_some() || self.max_size.is_some()) {
            return false;
        }
        self.name.allows(&row.name)
            && self.min_size.is_none_or(|lo| row.size >= lo)
            && self.max_size.is_none_or(|hi| row.size <= hi)
            && self.after.is_none_or(|t| row.modified_unix >= t)
            && self.before.is_none_or(|t| row.modified_unix <= t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RowAttrs;

    /// A fixed "now" for the relative dates: 2026-09-29 12:00:00 UTC.
    const NOW: i64 = 1_790_683_200;
    const DAY: i64 = 86_400;
    const MIB: u64 = 1024 * 1024;

    fn file(name: &str, size: u64, age_days: i64) -> Row {
        Row::new(name.to_string(), RowAttrs { size, modified_unix: NOW - age_days * DAY, mode: 0o644, ..Default::default() })
    }

    fn dir(name: &str, age_days: i64) -> Row {
        Row::new(name.to_string(), RowAttrs { is_dir: true, modified_unix: NOW - age_days * DAY, mode: 0o755, ..Default::default() })
    }

    fn fields() -> Fields<'static> {
        Fields { name: "", min_size: "", max_size: "", after: "", before: "" }
    }

    fn parse(fields: Fields, kind: KindFilter) -> Criteria {
        Criteria::parse(&fields, kind, NOW).unwrap()
    }

    #[test]
    fn empty_criteria_match_everything() {
        let c = parse(fields(), KindFilter::Any);
        assert!(c.matches(&file("a", 0, 0)));
        assert!(c.matches(&dir("d", 0)));
    }

    #[test]
    fn the_name_uses_glob_lists_with_exclusions() {
        let c = parse(Fields { name: "*.rs,!test*", ..fields() }, KindFilter::Any);
        assert!(c.matches(&file("lib.rs", 1, 0)));
        assert!(!c.matches(&file("test_a.rs", 1, 0)));
        assert!(!c.matches(&file("notes.txt", 1, 0)));
    }

    #[test]
    fn the_type_filter_splits_files_and_directories() {
        let files = parse(fields(), KindFilter::FilesOnly);
        let dirs = parse(fields(), KindFilter::DirsOnly);
        assert!(files.matches(&file("a", 1, 0)) && !files.matches(&dir("d", 0)));
        assert!(dirs.matches(&dir("d", 0)) && !dirs.matches(&file("a", 1, 0)));
    }

    #[test]
    fn size_limits_are_inclusive_and_never_match_directories() {
        let c = parse(Fields { min_size: "1M", max_size: "2M", ..fields() }, KindFilter::Any);
        assert!(c.matches(&file("exact_min", MIB, 0)));
        assert!(c.matches(&file("exact_max", 2 * MIB, 0)));
        assert!(!c.matches(&file("small", MIB - 1, 0)));
        assert!(!c.matches(&file("big", 2 * MIB + 1, 0)));
        assert!(!c.matches(&dir("d", 0)), "a directory has no meaningful size");
    }

    #[test]
    fn absolute_dates_are_local_times_and_a_bare_date_is_midnight() {
        let c = parse(Fields { after: "2026-09-01", before: "2026-09-30 00:00:00", ..fields() }, KindFilter::Any);
        let sep1 = parse_when("2026-09-01", NOW).unwrap().unwrap();
        let mut f = file("a", 1, 0);
        f.modified_unix = sep1;
        assert!(c.matches(&f), "midnight of the first day is included");
        f.modified_unix = sep1 - 1;
        assert!(!c.matches(&f));
    }

    #[test]
    fn relative_dates_count_back_from_now() {
        assert_eq!(parse_when("30m", NOW).unwrap(), Some(NOW - 30 * 60));
        assert_eq!(parse_when("12h", NOW).unwrap(), Some(NOW - 12 * 3600));
        assert_eq!(parse_when("7d", NOW).unwrap(), Some(NOW - 7 * DAY));
        assert_eq!(parse_when("2w", NOW).unwrap(), Some(NOW - 14 * DAY));
        assert_eq!(parse_when("  ", NOW).unwrap(), None);
    }

    #[test]
    fn acceptance_files_over_1mb_modified_in_the_last_7_days_and_no_directories() {
        let c = parse(Fields { min_size: "1M", after: "7d", ..fields() }, KindFilter::FilesOnly);
        assert!(c.matches(&file("recent_big", 5 * MIB, 3)));
        assert!(!c.matches(&file("recent_small", 100, 3)));
        assert!(!c.matches(&file("old_big", 5 * MIB, 30)));
        assert!(!c.matches(&dir("recent_dir", 1)));
    }

    #[test]
    fn a_before_limit_selects_older_files() {
        let c = parse(Fields { before: "30d", ..fields() }, KindFilter::Any);
        assert!(c.matches(&file("old", 1, 60)));
        assert!(!c.matches(&file("new", 1, 1)));
    }

    #[test]
    fn a_bad_field_is_reported_by_name() {
        let err = |f: Fields| Criteria::parse(&f, KindFilter::Any, NOW).unwrap_err();
        assert!(err(Fields { min_size: "lots", ..fields() }).starts_with("Minimum size:"));
        assert!(err(Fields { max_size: "-1", ..fields() }).starts_with("Maximum size:"));
        assert!(err(Fields { after: "yesterday", ..fields() }).starts_with("Modified after:"));
        assert!(err(Fields { before: "2026-13-45", ..fields() }).starts_with("Modified before:"));
    }

    #[test]
    fn contradictory_ranges_are_rejected() {
        let err = |f: Fields| Criteria::parse(&f, KindFilter::Any, NOW).unwrap_err();
        assert!(err(Fields { min_size: "2M", max_size: "1M", ..fields() }).contains("larger"));
        assert!(err(Fields { after: "1d", before: "7d", ..fields() }).contains("after"));
    }
}
