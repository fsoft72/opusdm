//! Markdown to styled text runs. No GTK here: the view turns the runs into a
//! text buffer, so the parser can be tested alone.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Deepest indent level that has its own tag name.
const MAX_INDENT: usize = 6;
/// Line drawn for a horizontal rule.
const RULE_TEXT: &str = "────────────────────────────────";

/// A piece of text with the names of the text tags to apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    /// Tag names: `h1`..`h6`, `bold`, `italic`, `strike`, `code`, `codeblock`,
    /// `quote`, `link`, `rule`, `indent1`..`indent6`.
    pub tags: Vec<String>,
    /// Target of a link; `None` for plain text.
    pub link: Option<String>,
}

/// Name of the tag that indents a block `level` steps.
pub fn indent_tag(level: usize) -> String {
    format!("indent{}", level.min(MAX_INDENT))
}

/// Parse state: what is open while the events go by.
#[derive(Default)]
struct Builder {
    runs: Vec<Run>,
    bold: usize,
    italic: usize,
    strike: usize,
    heading: Option<u8>,
    in_code_block: bool,
    quote: usize,
    /// One entry per open list: the next number of an ordered list.
    lists: Vec<Option<u64>>,
    link: Option<String>,
}

impl Builder {
    /// Indent steps of the block being written.
    fn _depth(&self) -> usize {
        self.quote + self.lists.len()
    }

    /// Tags of the text being written.
    fn _tags(&self) -> Vec<String> {
        let mut tags = Vec::new();
        if let Some(level) = self.heading { tags.push(format!("h{level}")); }
        if self.bold > 0 { tags.push("bold".to_string()); }
        if self.italic > 0 { tags.push("italic".to_string()); }
        if self.strike > 0 { tags.push("strike".to_string()); }
        if self.in_code_block { tags.push("codeblock".to_string()); }
        if self.quote > 0 { tags.push("quote".to_string()); }
        if self.link.is_some() { tags.push("link".to_string()); }
        if self._depth() > 0 { tags.push(indent_tag(self._depth())); }
        tags
    }

    /// Add text with the current style.
    fn _text(&mut self, text: &str) {
        if text.is_empty() { return; }
        let (tags, link) = (self._tags(), self.link.clone());
        self.runs.push(Run { text: text.to_string(), tags, link });
    }

    /// Add text with explicit tags (inline code, rules, markers).
    fn _text_with(&mut self, text: &str, extra: &[&str]) {
        let mut tags = self._tags();
        tags.extend(extra.iter().map(|t| t.to_string()));
        let link = self.link.clone();
        self.runs.push(Run { text: text.to_string(), tags, link });
    }

    /// True when the text so far is empty or ends with a new line.
    fn _at_line_start(&self) -> bool {
        self.runs.iter().rev().find(|r| !r.text.is_empty()).is_none_or(|r| r.text.ends_with('\n'))
    }

    /// Start a new line unless one has just started.
    fn _ensure_newline(&mut self) {
        if !self._at_line_start() { self._text("\n"); }
    }

    /// End a block: a blank line between blocks, none inside a list.
    fn _end_block(&mut self) {
        self._ensure_newline();
        if self.lists.is_empty() { self._text("\n"); }
    }

    /// The marker at the start of a list item: a bullet or the next number.
    fn _item_start(&mut self) {
        self._ensure_newline();
        let marker = match self.lists.last_mut() {
            Some(Some(n)) => {
                let text = format!("{n}. ");
                *n += 1;
                text
            }
            _ => "• ".to_string(),
        };
        self._text(&marker);
    }

    /// Handle one parser event.
    fn _event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self._start(tag),
            Event::End(tag) => self._end(tag),
            Event::Text(t) => self._text(&t),
            Event::Code(t) => self._text_with(&t, &["code"]),
            Event::SoftBreak => self._text(" "),
            Event::HardBreak => self._text("\n"),
            Event::Rule => {
                self._ensure_newline();
                self._text_with(RULE_TEXT, &["rule"]);
                self._text("\n\n");
            }
            Event::TaskListMarker(done) => self._text(if done { "☑ " } else { "☐ " }),
            _ => {}
        }
    }

    /// An element opens.
    fn _start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => self.heading = Some(_level_number(level)),
            Tag::BlockQuote(_) => self.quote += 1,
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                if let CodeBlockKind::Fenced(_) = kind { self._ensure_newline(); }
            }
            Tag::List(first) => self.lists.push(first),
            Tag::Item => self._item_start(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { .. } => self._text("[image: "),
            _ => {}
        }
    }

    /// An element closes.
    fn _end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self._end_block(),
            TagEnd::Heading(_) => {
                self._end_block();
                self.heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.quote = self.quote.saturating_sub(1);
                if self.quote == 0 { self._end_block(); }
            }
            TagEnd::CodeBlock => {
                self._end_block();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() { self._end_block(); }
            }
            TagEnd::Item => self._ensure_newline(),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = None,
            TagEnd::Image => self._text("]"),
            TagEnd::TableCell => self._text("  |  "),
            TagEnd::TableHead | TagEnd::TableRow => {
                self._ensure_newline();
            }
            TagEnd::Table => self._end_block(),
            _ => {}
        }
    }
}

/// Number of a heading level, 1 to 6.
fn _level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Turn Markdown text into styled runs. Raw HTML is dropped; a table is shown
/// as rows of cells separated by `|`; an image shows as `[image: alt]`.
pub fn parse(source: &str) -> Vec<Run> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder::default();
    for event in Parser::new_ext(source, options) {
        builder._event(event);
    }
    builder.runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(runs: &[Run]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn tags_of<'a>(runs: &'a [Run], text: &str) -> &'a [String] {
        &runs.iter().find(|r| r.text == text).unwrap_or_else(|| panic!("no run {text:?}")).tags
    }

    #[test]
    fn heading_and_paragraph() {
        let runs = parse("# Title\n\nSome text.\n");
        assert_eq!(plain(&runs), "Title\n\nSome text.\n\n");
        assert_eq!(tags_of(&runs, "Title"), ["h1"]);
        assert!(tags_of(&runs, "Some text.").is_empty());
    }

    #[test]
    fn inline_styles_nest() {
        let runs = parse("a **b *c*** `d` ~~e~~");
        assert_eq!(tags_of(&runs, "b "), ["bold"]);
        assert_eq!(tags_of(&runs, "c"), ["bold", "italic"]);
        assert_eq!(tags_of(&runs, "d"), ["code"]);
        assert_eq!(tags_of(&runs, "e"), ["strike"]);
    }

    #[test]
    fn link_keeps_its_target() {
        let runs = parse("see [the site](https://example.org/x)");
        let link = runs.iter().find(|r| r.text == "the site").expect("link run");
        assert_eq!(link.link.as_deref(), Some("https://example.org/x"));
        assert_eq!(link.tags, ["link"]);
        assert!(runs.iter().filter(|r| r.link.is_none()).count() >= 1);
    }

    #[test]
    fn lists_have_markers_and_indent() {
        let runs = parse("- one\n- two\n  - deep\n\n1. first\n2. second\n");
        assert_eq!(plain(&runs), "• one\n• two\n• deep\n\n1. first\n2. second\n\n");
        assert_eq!(tags_of(&runs, "one"), ["indent1"]);
        assert_eq!(tags_of(&runs, "deep"), ["indent2"]);
    }

    #[test]
    fn task_list_markers() {
        let runs = parse("- [x] done\n- [ ] todo\n");
        assert_eq!(plain(&runs), "• ☑ done\n• ☐ todo\n\n");
    }

    #[test]
    fn code_block_keeps_its_lines() {
        let runs = parse("```rust\nfn a() {}\nfn b() {}\n```\n");
        assert_eq!(plain(&runs), "fn a() {}\nfn b() {}\n\n");
        assert_eq!(tags_of(&runs, "fn a() {}\nfn b() {}\n"), ["codeblock"]);
    }

    #[test]
    fn quote_is_tagged_and_indented() {
        let runs = parse("> quoted\n");
        assert_eq!(tags_of(&runs, "quoted"), ["quote", "indent1"]);
    }

    #[test]
    fn rule_table_image_and_html() {
        let runs = parse("---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n![alt](x.png)\n\n<div>raw</div>\n");
        let text = plain(&runs);
        assert!(text.contains(RULE_TEXT));
        assert!(text.contains("a  |  b  |  \n1  |  2  |  \n"));
        assert!(text.contains("[image: alt]"));
        assert!(!text.contains("<div>"));
    }

    #[test]
    fn empty_source_gives_no_runs() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn deep_indent_is_capped() {
        assert_eq!(indent_tag(40), "indent6");
    }
}
