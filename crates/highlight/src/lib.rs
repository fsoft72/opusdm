//! Source text to coloured spans with syntect. No GTK here: the views turn the
//! spans into text tags, so the colouring can be tested alone.
//!
//! The tokens are not coloured with a syntect theme: each one is put in a
//! small set of categories (`Category`), so the views have one tag per
//! category and switching dark / light only changes the tag colours.

pub use syntect;
use std::ops::Range;
use std::str::FromStr;
use std::sync::OnceLock;
use syntect::easy::ScopeRangeIterator;
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// A line longer than this (bytes) stops the colouring: the regular
/// expressions get very slow on minified files.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// What a piece of source is, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Comment,
    String,
    Escape,
    Number,
    Constant,
    Keyword,
    Operator,
    Type,
    Function,
    Macro,
    Tag,
    Attribute,
    Heading,
    Invalid,
}

impl Category {
    /// Every category, in the order of the tags.
    pub const ALL: [Category; 14] = [Category::Comment, Category::String, Category::Escape, Category::Number, Category::Constant, Category::Keyword, Category::Operator, Category::Type, Category::Function, Category::Macro, Category::Tag, Category::Attribute, Category::Heading, Category::Invalid];

    /// Name of the text tag of the category.
    pub fn tag_name(self) -> &'static str {
        match self {
            Category::Comment => "syn-comment",
            Category::String => "syn-string",
            Category::Escape => "syn-escape",
            Category::Number => "syn-number",
            Category::Constant => "syn-constant",
            Category::Keyword => "syn-keyword",
            Category::Operator => "syn-operator",
            Category::Type => "syn-type",
            Category::Function => "syn-function",
            Category::Macro => "syn-macro",
            Category::Tag => "syn-tag",
            Category::Attribute => "syn-attribute",
            Category::Heading => "syn-heading",
            Category::Invalid => "syn-invalid",
        }
    }
}

/// Scope selectors of each category; the best match wins, like in a theme.
const SELECTORS: [(&str, Category); 14] = [
    ("comment", Category::Comment),
    ("string, constant.other.symbol", Category::String),
    ("constant.character.escape", Category::Escape),
    ("constant.numeric", Category::Number),
    ("constant.language, constant.character, constant.other, support.constant, variable.other.constant", Category::Constant),
    ("keyword, storage, variable.language", Category::Keyword),
    ("keyword.operator, punctuation.accessor", Category::Operator),
    ("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.union, entity.other.inherited-class, support.type, support.class", Category::Type),
    ("entity.name.function, support.function, variable.function", Category::Function),
    ("meta.preprocessor, keyword.other.preprocessor, support.macro, entity.name.macro, meta.annotation, meta.attribute", Category::Macro),
    ("entity.name.tag", Category::Tag),
    ("entity.other.attribute-name", Category::Attribute),
    ("markup.heading, entity.name.section", Category::Heading),
    ("invalid", Category::Invalid),
];

/// Colour of each `Category` on a dark page (One Dark like).
pub const DARK_COLORS: [(Category, &str); 14] = [
    (Category::Comment, "#7f848e"),
    (Category::String, "#98c379"),
    (Category::Escape, "#56b6c2"),
    (Category::Number, "#d19a66"),
    (Category::Constant, "#d19a66"),
    (Category::Keyword, "#c678dd"),
    (Category::Operator, "#56b6c2"),
    (Category::Type, "#e5c07b"),
    (Category::Function, "#61afef"),
    (Category::Macro, "#56b6c2"),
    (Category::Tag, "#e06c75"),
    (Category::Attribute, "#d19a66"),
    (Category::Heading, "#e06c75"),
    (Category::Invalid, "#ff5555"),
];

/// Colour of each `Category` on a light page (GitHub like).
pub const LIGHT_COLORS: [(Category, &str); 14] = [
    (Category::Comment, "#6a737d"),
    (Category::String, "#032f62"),
    (Category::Escape, "#005cc5"),
    (Category::Number, "#005cc5"),
    (Category::Constant, "#005cc5"),
    (Category::Keyword, "#d73a49"),
    (Category::Operator, "#d73a49"),
    (Category::Type, "#6f42c1"),
    (Category::Function, "#6f42c1"),
    (Category::Macro, "#e36209"),
    (Category::Tag, "#22863a"),
    (Category::Attribute, "#6f42c1"),
    (Category::Heading, "#005cc5"),
    (Category::Invalid, "#b31d28"),
];

/// A coloured piece of a text, in characters from the start of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSpan {
    pub start: usize,
    pub end: usize,
    pub category: Category,
}

/// The syntaxes shipped with syntect, loaded once (they take some milliseconds).
pub fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The selectors of `SELECTORS`, parsed once.
fn _selectors() -> &'static [(ScopeSelectors, Category)] {
    static LIST: OnceLock<Vec<(ScopeSelectors, Category)>> = OnceLock::new();
    LIST.get_or_init(|| SELECTORS.iter().map(|(text, category)| (ScopeSelectors::from_str(text).expect("valid scope selector"), *category)).collect())
}

/// The syntax of a file named `name` whose text starts with `first_line`: a
/// `#!` shebang first, then the file name (`Makefile`), the extension and the
/// first line again (`<?xml`...). `None` for plain text.
pub fn syntax_for(name: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let ext = name.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.is_empty());
    let shebang = Some(first_line).filter(|l| l.starts_with("#!")).and_then(|l| set.find_syntax_by_first_line(l));
    let found = shebang
        .or_else(|| set.find_syntax_by_extension(name))
        .or_else(|| ext.and_then(|e| set.find_syntax_by_extension(e)))
        .or_else(|| ext.and_then(|e| set.find_syntax_by_extension(&e.to_lowercase())))
        .or_else(|| set.find_syntax_by_first_line(first_line));
    found.filter(|s| s.name != set.find_syntax_plain_text().name)
}

/// The syntax named by the language of a Markdown code block (`rust`, `py`,
/// `sh`...); `None` when it is unknown.
pub fn syntax_for_token(token: &str) -> Option<&'static SyntaxReference> {
    if token.is_empty() { return None; }
    let set = syntaxes();
    set.find_syntax_by_token(token).filter(|s| s.name != set.find_syntax_plain_text().name)
}

/// The category of the innermost scopes of `stack`; `None` for plain text.
fn _classify(stack: &ScopeStack) -> Option<Category> {
    let scopes = stack.as_slice();
    _selectors().iter()
        .filter_map(|(selector, category)| selector.does_match(scopes).map(|power| (power.0, *category)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, category)| category)
}

/// Colours a text one line at a time, keeping the parser state between lines.
pub struct LineHighlighter {
    parse: ParseState,
    stack: ScopeStack,
}

impl LineHighlighter {
    /// A highlighter at the start of a text in `syntax`.
    pub fn new(syntax: &SyntaxReference) -> LineHighlighter {
        LineHighlighter { parse: ParseState::new(syntax), stack: ScopeStack::new() }
    }

    /// The coloured byte ranges of `line` (with its `\n`), next ones of the
    /// same category merged; the error says why the parser stopped.
    pub fn line(&mut self, line: &str) -> Result<Vec<(Range<usize>, Category)>, String> {
        let ops = self.parse.parse_line(line, syntaxes()).map_err(|e| e.to_string())?;
        let mut spans: Vec<(Range<usize>, Category)> = Vec::new();
        for (range, op) in ScopeRangeIterator::new(&ops, line) {
            self.stack.apply(op).map_err(|e| format!("{e:?}"))?;
            if range.is_empty() { continue; }
            let Some(category) = _classify(&self.stack) else { continue };
            match spans.last_mut() {
                Some((last, c)) if *c == category && last.end == range.start => last.end = range.end,
                _ => spans.push((range, category)),
            }
        }
        Ok(spans)
    }
}

/// Colour `text` in `syntax` and hand the spans out every `batch_lines` lines
/// (and once at the end); `deliver` returns false to stop early. The colouring
/// also stops at a parser error or at a line longer than `MAX_LINE_BYTES`, so
/// the rest of the text stays plain.
pub fn highlight_in_batches(text: &str, syntax: &SyntaxReference, batch_lines: usize, mut deliver: impl FnMut(Vec<CharSpan>) -> bool) {
    let mut highlighter = LineHighlighter::new(syntax);
    let mut batch = Vec::new();
    let mut chars_before = 0;
    for (index, line) in LinesWithEndings::from(text).enumerate() {
        if line.len() > MAX_LINE_BYTES { break; }
        let spans = match highlighter.line(line) {
            Ok(spans) => spans,
            Err(e) => {
                opusdm_ipc::err_log!("viewer", "syntax colouring stopped at line {}: {e}", index + 1);
                break;
            }
        };
        for (range, category) in spans {
            let start = chars_before + line[..range.start].chars().count();
            let end = start + line[range].chars().count();
            batch.push(CharSpan { start, end, category });
        }
        chars_before += line.chars().count();
        if (index + 1) % batch_lines.max(1) == 0 && !deliver(std::mem::take(&mut batch)) { return; }
    }
    if !batch.is_empty() { deliver(batch); }
}

/// `text` cut into pieces with their category (`None` for plain text), for a
/// Markdown code block. A text that cannot be coloured is one plain piece.
pub fn pieces(text: &str, syntax: &SyntaxReference) -> Vec<(String, Option<Category>)> {
    let mut highlighter = LineHighlighter::new(syntax);
    let mut pieces = Vec::new();
    for line in LinesWithEndings::from(text) {
        let spans = if line.len() > MAX_LINE_BYTES { Ok(Vec::new()) } else { highlighter.line(line) };
        let Ok(spans) = spans else { return vec![(text.to_string(), None)] };
        let mut at = 0;
        for (range, category) in spans {
            if range.start > at { pieces.push((line[at..range.start].to_string(), None)); }
            pieces.push((line[range.clone()].to_string(), Some(category)));
            at = range.end;
        }
        if at < line.len() { pieces.push((line[at..].to_string(), None)); }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(name: &str, first_line: &str) -> Option<String> {
        syntax_for(name, first_line).map(|s| s.name.clone())
    }

    /// The text of every span of `category`.
    fn texts(text: &str, syntax: &SyntaxReference, category: Category) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        highlight_in_batches(text, syntax, 1000, |spans| {
            out.extend(spans.iter().filter(|s| s.category == category).map(|s| chars[s.start..s.end].iter().collect::<String>()));
            true
        });
        out
    }

    #[test]
    fn syntax_by_extension_and_name() {
        assert_eq!(name_of("main.rs", "").as_deref(), Some("Rust"));
        assert_eq!(name_of("A.PY", "").as_deref(), Some("Python"));
        assert_eq!(name_of("Makefile", "").as_deref(), Some("Makefile"));
        assert_eq!(name_of("notes.txt", "hello"), None);
        assert_eq!(name_of("noext", "just words"), None);
    }

    #[test]
    fn syntax_by_shebang() {
        assert_eq!(name_of("run", "#!/usr/bin/env python3\n").as_deref(), Some("Python"));
        assert_eq!(name_of("build", "#!/bin/bash\n").as_deref(), Some("Bourne Again Shell (bash)"));
        assert_eq!(name_of("tool.weird", "#!/bin/sh\n").as_deref(), Some("Bourne Again Shell (bash)"));
    }

    #[test]
    fn syntax_by_code_block_token() {
        assert_eq!(syntax_for_token("rust").map(|s| s.name.as_str()), Some("Rust"));
        assert_eq!(syntax_for_token("py").map(|s| s.name.as_str()), Some("Python"));
        assert!(syntax_for_token("").is_none());
        assert!(syntax_for_token("no-such-language").is_none());
    }

    #[test]
    fn rust_tokens_get_their_categories() {
        let rust = syntax_for("a.rs", "").unwrap();
        let text = "// note\nfn main() { let s = \"hi\\n\"; let n = 42; }\n";
        assert_eq!(texts(text, rust, Category::Comment), ["// note\n"]);
        assert!(texts(text, rust, Category::Keyword).contains(&"let".to_string()));
        assert!(texts(text, rust, Category::Number).contains(&"42".to_string()));
        assert!(texts(text, rust, Category::Function).contains(&"main".to_string()));
        assert!(texts(text, rust, Category::Escape).contains(&"\\n".to_string()));
        assert!(texts(text, rust, Category::String).iter().any(|s| s.contains("hi")));
    }

    #[test]
    fn spans_are_counted_in_characters() {
        let python = syntax_for("a.py", "").unwrap();
        let text = "s = 'è'\n# ü\n";
        assert_eq!(texts(text, python, Category::Comment), ["# ü\n"]);
    }

    #[test]
    fn batches_follow_the_line_count_and_can_stop() {
        let python = syntax_for("a.py", "").unwrap();
        let text = "# a\n# b\n# c\n# d\n# e\n";
        let mut batches = 0;
        highlight_in_batches(text, python, 2, |_| { batches += 1; true });
        assert_eq!(batches, 3, "2 + 2 + 1 lines");
        let mut batches = 0;
        highlight_in_batches(text, python, 2, |_| { batches += 1; false });
        assert_eq!(batches, 1, "stopped after the first batch");
    }

    #[test]
    fn a_huge_line_stops_the_colouring() {
        let python = syntax_for("a.py", "").unwrap();
        let text = format!("# first\n{}\n# after\n", "x".repeat(MAX_LINE_BYTES + 1));
        assert_eq!(texts(&text, python, Category::Comment), ["# first\n"]);
    }

    #[test]
    fn pieces_cover_the_whole_text() {
        let rust = syntax_for("a.rs", "").unwrap();
        let text = "let x = 1; // one\n";
        let pieces = pieces(text, rust);
        assert_eq!(pieces.iter().map(|(t, _)| t.as_str()).collect::<String>(), text);
        assert!(pieces.contains(&("// one\n".to_string(), Some(Category::Comment))));
        assert!(pieces.contains(&("let".to_string(), Some(Category::Keyword))));
    }
}
