//! Markdown to styled text runs. No GTK here: the view turns the runs into a
//! text buffer, so the parser can be tested alone.

use crate::highlight::{pieces, syntax_for_token};
use syntect::parsing::SyntaxReference;
use unicode_width::UnicodeWidthStr;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Deepest indent level that has its own tag name.
const MAX_INDENT: usize = 6;
/// Line drawn for a horizontal rule.
const RULE_TEXT: &str = "────────────────────────────────";

/// A piece of text with the names of the text tags to apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    /// Tag names: `h1`..`h6`, `bold`, `italic`, `strike`, `code`, `codeblock`,
    /// `quote`, `link`, `rule`, `table`, `indent1`..`indent6`, and the
    /// `syn-*` tags of `highlight::Category` in a coloured code block.
    pub tags: Vec<String>,
    /// Target of a link; `None` for plain text.
    pub link: Option<String>,
    /// A picture to show instead of the text; the text is its alt text.
    pub image: Option<String>,
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
    /// Syntax of the code block being read, when its language is known: its
    /// text collects in `code` and is coloured at the end of the block.
    code_syntax: Option<&'static SyntaxReference>,
    code: String,
    quote: usize,
    /// One entry per open list: the next number of an ordered list.
    lists: Vec<Option<u64>>,
    link: Option<String>,
    /// Address of the picture being read; its alt text collects in `alt`.
    image: Option<String>,
    alt: String,
    table: Option<Table>,
}

/// A table being read: the cells are kept until its end, to know the widths.
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Run>>>,
    /// Rows that are the header.
    head_rows: usize,
    row: Vec<Vec<Run>>,
    cell: Vec<Run>,
}

/// Width of a cell in terminal columns (a wide character takes two).
fn _cell_width(cell: &[Run]) -> usize {
    cell.iter().map(|r| r.text.width()).sum()
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
        self._emit(Run { text: text.to_string(), tags, link, image: None });
    }

    /// Add a run: in a table it goes to the cell being read (on one line).
    fn _emit(&mut self, mut run: Run) {
        let Some(table) = self.table.as_mut() else { return self.runs.push(run) };
        run.text = run.text.replace('\n', " ");
        table.cell.push(run);
    }

    /// Add text with explicit tags (inline code, rules, markers).
    fn _text_with(&mut self, text: &str, extra: &[&str]) {
        let mut tags = self._tags();
        tags.extend(extra.iter().map(|t| t.to_string()));
        let link = self.link.clone();
        self._emit(Run { text: text.to_string(), tags, link, image: None });
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
            Event::Text(t) if self.image.is_some() => self.alt.push_str(&t),
            Event::Text(t) if self.code_syntax.is_some() => self.code.push_str(&t),
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
                let CodeBlockKind::Fenced(info) = kind else { return };
                self._ensure_newline();
                self.code_syntax = syntax_for_token(_language(&info));
                self.code.clear();
            }
            Tag::List(first) => self.lists.push(first),
            Tag::Item => self._item_start(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { dest_url, .. } => {
                self.image = Some(dest_url.to_string());
                self.alt.clear();
            }
            Tag::Table(aligns) => self.table = Some(Table { aligns, rows: Vec::new(), head_rows: 0, row: Vec::new(), cell: Vec::new() }),
            Tag::TableHead => self.bold += 1,
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
                self._end_code();
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
            TagEnd::Image => self._end_image(),
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() { t.row.push(std::mem::take(&mut t.cell)); }
            }
            TagEnd::TableHead => {
                self.bold = self.bold.saturating_sub(1);
                if let Some(t) = self.table.as_mut() {
                    t.rows.push(std::mem::take(&mut t.row));
                    t.head_rows = t.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() { t.rows.push(std::mem::take(&mut t.row)); }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() { self._write_table(table); }
                self._end_block();
            }
            _ => {}
        }
    }
}

impl Builder {
    /// A coloured code block closes: write its text cut by category.
    fn _end_code(&mut self) {
        let Some(syntax) = self.code_syntax.take() else { return };
        let code = std::mem::take(&mut self.code);
        for (text, category) in pieces(&code, syntax) {
            match category {
                Some(c) => self._text_with(&text, &[c.tag_name()]),
                None => self._text(&text),
            }
        }
    }

    /// A picture closes: a run with its alt text, or the text `[image: alt]`
    /// inside a table (a picture cannot sit in a cell).
    fn _end_image(&mut self) {
        let Some(url) = self.image.take() else { return };
        let alt = std::mem::take(&mut self.alt);
        if self.table.is_some() { return self._text(&format!("[image: {alt}]")); }
        let (tags, link) = (self._tags(), self.link.clone());
        self.runs.push(Run { text: alt, tags, link, image: Some(url) });
    }

    /// Write a finished table as a box of lines with the cells aligned.
    fn _write_table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 { return; }
        let widths: Vec<usize> = (0..columns).map(|c| table.rows.iter().filter_map(|r| r.get(c)).map(|cell| _cell_width(cell)).max().unwrap_or(0).max(1)).collect();
        self._ensure_newline();
        self._border('┌', '┬', '┐', &widths);
        for (i, row) in table.rows.iter().enumerate() {
            self._row_line(row, &widths, &table.aligns);
            if i + 1 == table.head_rows && i + 1 < table.rows.len() { self._border('├', '┼', '┤', &widths); }
        }
        self._border('└', '┴', '┘', &widths);
    }

    /// One horizontal line of the box.
    fn _border(&mut self, left: char, middle: char, right: char, widths: &[usize]) {
        let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        self._text_with(&format!("{left}{}{right}\n", parts.join(&middle.to_string())), &["table"]);
    }

    /// One row of cells between vertical lines, each padded to its column.
    fn _row_line(&mut self, row: &[Vec<Run>], widths: &[usize], aligns: &[Alignment]) {
        self._text_with("│", &["table"]);
        for (c, width) in widths.iter().enumerate() {
            let cell = row.get(c).map(Vec::as_slice).unwrap_or(&[]);
            let free = width - _cell_width(cell);
            let (before, after) = match aligns.get(c) {
                Some(Alignment::Right) => (free, 0),
                Some(Alignment::Center) => (free / 2, free - free / 2),
                _ => (0, free),
            };
            self._text_with(&format!(" {}", " ".repeat(before)), &["table"]);
            for run in cell {
                let mut run = run.clone();
                run.tags.push("table".to_string());
                self.runs.push(run);
            }
            self._text_with(&format!("{} │", " ".repeat(after)), &["table"]);
        }
        self._text_with("\n", &["table"]);
    }
}

/// Language of a fenced code block: the first word of its info string
/// (`python {.numbered}` and `rust,ignore` give `python` and `rust`).
fn _language(info: &str) -> &str {
    info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("")
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
        let runs = parse("```\nfn a() {}\nfn b() {}\n```\n");
        assert_eq!(plain(&runs), "fn a() {}\nfn b() {}\n\n");
        assert_eq!(tags_of(&runs, "fn a() {}\nfn b() {}\n"), ["codeblock"]);
    }

    #[test]
    fn code_block_with_a_known_language_is_coloured() {
        let runs = parse("```rust\nlet n = 42; // x\n```\n");
        assert_eq!(plain(&runs), "let n = 42; // x\n\n");
        assert_eq!(tags_of(&runs, "let"), ["codeblock", "syn-keyword"]);
        assert_eq!(tags_of(&runs, "42"), ["codeblock", "syn-number"]);
        assert_eq!(tags_of(&runs, "// x\n"), ["codeblock", "syn-comment"]);
    }

    #[test]
    fn code_block_with_an_unknown_language_is_plain() {
        let runs = parse("```nosuchlang\nlet n = 42;\n```\n");
        assert_eq!(tags_of(&runs, "let n = 42;\n"), ["codeblock"]);
    }

    #[test]
    fn code_block_language_ignores_attributes() {
        let runs = parse("```python {.numbered}\npass\n```\n");
        assert_eq!(tags_of(&runs, "pass"), ["codeblock", "syn-keyword"]);
    }

    #[test]
    fn quote_is_tagged_and_indented() {
        let runs = parse("> quoted\n");
        assert_eq!(tags_of(&runs, "quoted"), ["quote", "indent1"]);
    }

    #[test]
    fn rule_and_html() {
        let runs = parse("---\n\n<div>raw</div>\n");
        let text = plain(&runs);
        assert!(text.contains(RULE_TEXT));
        assert!(!text.contains("<div>"));
    }

    #[test]
    fn table_is_a_box_with_aligned_cells() {
        let runs = parse("| name | n |\n|:--|--:|\n| **bob** | 7 |\n| al | 123 |\n");
        let expected = "┌──────┬─────┐\n│ name │   n │\n├──────┼─────┤\n│ bob  │   7 │\n│ al   │ 123 │\n└──────┴─────┘\n\n";
        assert_eq!(plain(&runs), expected);
        assert_eq!(tags_of(&runs, "name"), ["bold", "table"]);
        assert_eq!(tags_of(&runs, "bob"), ["bold", "table"]);
        assert_eq!(tags_of(&runs, "al"), ["table"]);
    }

    #[test]
    fn wide_characters_count_two_columns() {
        let runs = parse("| a |\n|---|\n| 日本 |\n");
        assert_eq!(plain(&runs), "┌──────┐\n│ a    │\n├──────┤\n│ 日本 │\n└──────┘\n\n");
    }

    #[test]
    fn table_centers_and_keeps_links() {
        let runs = parse("| a |\n|:-:|\n| [xxxx](u) |\n");
        assert_eq!(plain(&runs), "┌──────┐\n│  a   │\n├──────┤\n│ xxxx │\n└──────┘\n\n");
        assert_eq!(runs.iter().find(|r| r.text == "xxxx").and_then(|r| r.link.as_deref()), Some("u"));
    }

    #[test]
    fn image_is_one_run_with_its_address() {
        let runs = parse("text ![the alt](pics/a%20b.png) end");
        let image = runs.iter().find(|r| r.image.is_some()).expect("image run");
        assert_eq!(image.text, "the alt");
        assert_eq!(image.image.as_deref(), Some("pics/a%20b.png"));
        assert_eq!(plain(&runs), "text the alt end\n\n");
    }

    #[test]
    fn image_in_a_table_cell_is_text() {
        let runs = parse("| a |\n|---|\n| ![pic](p.png) |\n");
        assert!(runs.iter().all(|r| r.image.is_none()));
        assert!(plain(&runs).contains("[image: pic]"));
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
