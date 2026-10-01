//! The text form behind the File Type editor (F-06): a flat list of
//! conditions combined with "all" or "any", converted to and from a
//! [`FileType`]. Pure, no GTK. A type whose rule nests groups (only possible
//! by editing `filetypes.json`) is kept as it is and shown read-only.

use crate::link_dialogs::parse_size;
use crate::select_criteria::parse_when;
use opusdm_ipc::filetypes::{format_hex, parse_hex, Actions, FileType, MatchRule};
use opusdm_ipc::function::FunctionId;
use opusdm_ipc::timefmt::format_local;

/// The kinds of condition a row can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondKind {
    /// Name pattern (`*.png;*.jpg`).
    Name,
    /// Bytes at an offset (the "Match" requester).
    Bytes,
    /// Text in the first bytes (the "Search For" requester).
    Text,
    /// Size range.
    Size,
    /// Modification time range.
    Modified,
}

impl CondKind {
    /// Every kind, in drop-down order.
    pub const ALL: [CondKind; 5] = [CondKind::Name, CondKind::Bytes, CondKind::Text, CondKind::Size, CondKind::Modified];

    /// Drop-down label.
    pub fn label(self) -> &'static str {
        match self {
            CondKind::Name => "Name matches",
            CondKind::Bytes => "Bytes at offset",
            CondKind::Text => "Contains text",
            CondKind::Size => "Size between",
            CondKind::Modified => "Modified between",
        }
    }

    /// Placeholders of the two entries, in order.
    pub fn hints(self) -> (&'static str, &'static str) {
        match self {
            CondKind::Name => ("*.png;*.jpg", ""),
            CondKind::Bytes => ("offset, e.g. 0", "hex, e.g. 89 50 4E 47"),
            CondKind::Text => ("text to find", "search first N bytes"),
            CondKind::Size => ("min, e.g. 10K", "max, e.g. 4M"),
            CondKind::Modified => ("after, e.g. 7d", "before, e.g. 2026-01-01"),
        }
    }
}

/// Bytes a "Contains text" condition searches by default.
pub const DEFAULT_TEXT_RANGE: &str = "4096";

/// One condition row: the kind and its two entry texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cond {
    /// What the row tests.
    pub kind: CondKind,
    /// First entry.
    pub a: String,
    /// Second entry.
    pub b: String,
}

impl Cond {
    /// An empty row of `kind`, with the defaults that make sense.
    pub fn new(kind: CondKind) -> Self {
        let (a, b) = match kind {
            CondKind::Bytes => ("0", ""),
            CondKind::Text => ("", DEFAULT_TEXT_RANGE),
            _ => ("", ""),
        };
        let (a, b) = (a.to_string(), b.to_string());
        Cond { kind, a, b }
    }

    /// The rule this row stands for. `now` anchors relative times.
    pub fn to_rule(&self, now: i64) -> Result<MatchRule, String> {
        match self.kind {
            CondKind::Name => Ok(MatchRule::Glob { pattern: self.a.trim().to_string() }),
            CondKind::Bytes => {
                let offset = self.a.trim().parse::<usize>().map_err(|_| format!("offset must be a number, got {:?}", self.a))?;
                Ok(MatchRule::ContentBytes { offset, bytes: parse_hex(&self.b)? })
            }
            CondKind::Text => {
                let range = self.b.trim().parse::<usize>().map_err(|_| format!("range must be a number, got {:?}", self.b))?;
                Ok(MatchRule::ContentText { needle: self.a.clone(), range })
            }
            CondKind::Size => Ok(MatchRule::SizeRange { min: optional_size(&self.a)?, max: optional_size(&self.b)? }),
            CondKind::Modified => Ok(MatchRule::DateRange { from: parse_when(&self.a, now)?, to: parse_when(&self.b, now)? }),
        }
    }

    /// The row for a simple rule, `None` for a group.
    pub fn from_rule(rule: &MatchRule) -> Option<Cond> {
        Some(match rule {
            MatchRule::Glob { pattern } => Cond { kind: CondKind::Name, a: pattern.clone(), b: String::new() },
            MatchRule::ContentBytes { offset, bytes } => {
                Cond { kind: CondKind::Bytes, a: offset.to_string(), b: format_hex(bytes) }
            }
            MatchRule::ContentText { needle, range } => Cond { kind: CondKind::Text, a: needle.clone(), b: range.to_string() },
            MatchRule::SizeRange { min, max } => Cond {
                kind: CondKind::Size,
                a: min.map(|v| v.to_string()).unwrap_or_default(),
                b: max.map(|v| v.to_string()).unwrap_or_default(),
            },
            MatchRule::DateRange { from, to } => Cond {
                kind: CondKind::Modified,
                a: from.map(format_local).unwrap_or_default(),
                b: to.map(format_local).unwrap_or_default(),
            },
            MatchRule::And { .. } | MatchRule::Or { .. } => return None,
        })
    }
}

/// An optional size: empty for no bound.
fn optional_size(text: &str) -> Result<Option<u64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_size(text).map(Some)
}

/// The editable state of one file type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Type name.
    pub name: String,
    /// Match any condition (`Or`) instead of all of them (`And`).
    pub any: bool,
    /// The condition rows.
    pub conds: Vec<Cond>,
    /// A nested rule the form cannot show; when set it replaces `conds`.
    pub locked_rule: Option<MatchRule>,
    /// Double click function.
    pub dblclick: Option<FunctionId>,
    /// Drop function.
    pub drop: Option<FunctionId>,
    /// "Open With" functions.
    pub menu: Vec<FunctionId>,
}

impl Draft {
    /// A blank type with one empty name condition.
    pub fn blank(name: &str) -> Self {
        Draft {
            name: name.to_string(),
            any: false,
            conds: vec![Cond::new(CondKind::Name)],
            locked_rule: None,
            dblclick: None,
            drop: None,
            menu: Vec::new(),
        }
    }

    /// The form for an existing type.
    pub fn from_type(t: &FileType) -> Self {
        let (any, rules): (bool, Vec<&MatchRule>) = match &t.rule {
            MatchRule::And { rules } => (false, rules.iter().collect()),
            MatchRule::Or { rules } => (true, rules.iter().collect()),
            single => (false, vec![single]),
        };
        let conds: Option<Vec<Cond>> = rules.iter().map(|r| Cond::from_rule(r)).collect();
        let (conds, locked_rule) = match conds {
            Some(conds) => (conds, None),
            None => (Vec::new(), Some(t.rule.clone())),
        };
        Draft {
            name: t.name.clone(),
            any,
            conds,
            locked_rule,
            dblclick: t.actions.dblclick.clone(),
            drop: t.actions.drop.clone(),
            menu: t.actions.menu.clone(),
        }
    }

    /// Build the type. A single condition is stored bare, several are
    /// wrapped in `And` / `Or`. Errors name the offending row.
    pub fn to_type(&self, now: i64) -> Result<FileType, String> {
        let rule = match &self.locked_rule {
            Some(rule) => rule.clone(),
            None => {
                let mut rules = Vec::new();
                for (i, cond) in self.conds.iter().enumerate() {
                    rules.push(cond.to_rule(now).map_err(|e| format!("condition {}: {e}", i + 1))?);
                }
                match rules.len() {
                    0 => return Err("add at least one condition".to_string()),
                    1 => rules.remove(0),
                    _ if self.any => MatchRule::Or { rules },
                    _ => MatchRule::And { rules },
                }
            }
        };
        if let Some(problem) = rule.problem() {
            return Err(problem);
        }
        Ok(FileType {
            name: self.name.trim().to_string(),
            rule,
            actions: Actions { dblclick: self.dblclick.clone(), drop: self.drop.clone(), menu: self.menu.clone() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn cond(kind: CondKind, a: &str, b: &str) -> Cond {
        Cond { kind, a: a.into(), b: b.into() }
    }

    #[test]
    fn each_kind_converts_to_its_rule() {
        assert_eq!(cond(CondKind::Name, " *.png ", "").to_rule(NOW).unwrap(), MatchRule::Glob { pattern: "*.png".into() });
        assert_eq!(
            cond(CondKind::Bytes, "1", "50 4e").to_rule(NOW).unwrap(),
            MatchRule::ContentBytes { offset: 1, bytes: vec![0x50, 0x4e] }
        );
        assert_eq!(
            cond(CondKind::Text, "#!", "64").to_rule(NOW).unwrap(),
            MatchRule::ContentText { needle: "#!".into(), range: 64 }
        );
        assert_eq!(
            cond(CondKind::Size, "1K", "").to_rule(NOW).unwrap(),
            MatchRule::SizeRange { min: Some(1024), max: None }
        );
        assert_eq!(
            cond(CondKind::Modified, "1d", "").to_rule(NOW).unwrap(),
            MatchRule::DateRange { from: Some(NOW - 86_400), to: None }
        );
    }

    #[test]
    fn bad_entries_are_reported() {
        assert!(cond(CondKind::Bytes, "x", "50").to_rule(NOW).is_err());
        assert!(cond(CondKind::Bytes, "0", "5").to_rule(NOW).is_err());
        assert!(cond(CondKind::Text, "a", "many").to_rule(NOW).is_err());
        assert!(cond(CondKind::Size, "lots", "").to_rule(NOW).is_err());
        assert!(cond(CondKind::Modified, "soonish", "").to_rule(NOW).is_err());
    }

    #[test]
    fn single_condition_is_stored_bare_and_several_are_grouped() {
        let mut draft = Draft::blank("Pics");
        draft.conds = vec![cond(CondKind::Name, "*.png", "")];
        assert_eq!(draft.to_type(NOW).unwrap().rule, MatchRule::Glob { pattern: "*.png".into() });

        draft.conds.push(cond(CondKind::Size, "1K", ""));
        assert!(matches!(draft.to_type(NOW).unwrap().rule, MatchRule::And { .. }));
        draft.any = true;
        assert!(matches!(draft.to_type(NOW).unwrap().rule, MatchRule::Or { .. }));
    }

    #[test]
    fn errors_name_the_condition_and_empty_forms_are_refused() {
        let mut draft = Draft::blank("X");
        draft.conds = vec![cond(CondKind::Name, "*.a", ""), cond(CondKind::Size, "lots", "")];
        assert!(draft.to_type(NOW).unwrap_err().starts_with("condition 2:"));
        draft.conds.clear();
        assert!(draft.to_type(NOW).is_err());
        // An empty pattern is caught by the rule check.
        draft.conds = vec![cond(CondKind::Name, "  ", "")];
        assert!(draft.to_type(NOW).is_err());
    }

    #[test]
    fn a_type_round_trips_through_the_form() {
        let original = FileType {
            name: "Mixed".into(),
            rule: MatchRule::Or {
                rules: vec![
                    MatchRule::Glob { pattern: "*.png".into() },
                    MatchRule::ContentBytes { offset: 1, bytes: b"PNG".to_vec() },
                    MatchRule::ContentText { needle: "x".into(), range: 10 },
                    MatchRule::SizeRange { min: Some(5), max: Some(9) },
                ],
            },
            actions: Actions { dblclick: Some("a".into()), drop: Some("b".into()), menu: vec!["c".into(), "d".into()] },
        };
        let draft = Draft::from_type(&original);
        assert!(draft.any);
        assert_eq!(draft.conds.len(), 4);
        assert_eq!(draft.to_type(NOW).unwrap(), original);
    }

    #[test]
    fn a_nested_rule_is_locked_and_kept() {
        let nested = FileType {
            name: "Nested".into(),
            rule: MatchRule::And {
                rules: vec![
                    MatchRule::Glob { pattern: "*.a".into() },
                    MatchRule::Or { rules: vec![MatchRule::Glob { pattern: "*.b".into() }] },
                ],
            },
            actions: Actions::default(),
        };
        let draft = Draft::from_type(&nested);
        assert_eq!(draft.locked_rule, Some(nested.rule.clone()));
        assert_eq!(draft.to_type(NOW).unwrap(), nested);
    }

    #[test]
    fn a_date_round_trips_through_local_text() {
        let rule = MatchRule::DateRange { from: Some(1_700_000_000), to: None };
        let c = Cond::from_rule(&rule).unwrap();
        assert_eq!(c.to_rule(NOW).unwrap(), rule);
    }
}
