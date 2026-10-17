//! EPUB books, shown as one reflowed text: the chapters of the spine are
//! turned into Markdown and drawn by the Markdown view, so the search, the
//! dark switch and the font size work as for a Markdown file. The pictures
//! stay in memory, keyed by their name in the archive.

use gtk4::glib;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Biggest book read; a larger file is not read into memory.
const MAX_BOOK_BYTES: u64 = 256 * 1024 * 1024;
/// The file every EPUB has that names its package document.
const CONTAINER: &str = "META-INF/container.xml";
/// Media types of the spine documents turned into text.
const TEXT_TYPES: [&str; 2] = ["application/xhtml+xml", "text/html"];
/// Separator between two chapters.
const CHAPTER_BREAK: &str = "\n\n---\n\n";
/// Elements whose content is never shown.
const HIDDEN: [&str; 4] = ["head", "script", "style", "title"];
/// Elements that start a new paragraph.
const BLOCKS: [&str; 20] = ["p", "div", "section", "article", "body", "header", "footer", "aside", "nav", "figure", "figcaption", "dl", "dt", "dd", "table", "tr", "center", "address", "main", "hgroup"];

/// A book ready for the Markdown view.
#[derive(Debug, Default)]
pub struct Book {
    /// The whole text, as Markdown.
    pub markdown: String,
    /// The bytes of every picture the text shows, by its name in the archive
    /// (the image address of the Markdown, percent-escaped).
    pub pictures: HashMap<String, glib::Bytes>,
}

/// Read the book at `path`. Blocking: call it off the GTK thread.
pub fn open(path: &Path) -> Result<Book, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BOOK_BYTES { return Err(format!("the book is too big ({} MiB, the limit is {} MiB)", size / (1024 * 1024), MAX_BOOK_BYTES / (1024 * 1024))); }
    let files: HashMap<String, Vec<u8>> = opusdm_archive::read_files(path, &|_| true).map_err(|e| e.to_string())?.into_iter().collect();
    let fallback = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    build(&files, &fallback)
}

/// The book made of the archive `files` (name to bytes); `fallback` is the
/// title when the book has none.
fn build(files: &HashMap<String, Vec<u8>>, fallback: &str) -> Result<Book, String> {
    let container = files.get(CONTAINER).ok_or("not an EPUB book (no META-INF/container.xml)")?;
    let opf_name = _rootfile(&String::from_utf8_lossy(container)).ok_or("the book does not name its package document")?;
    let opf = files.get(&opf_name).ok_or_else(|| format!("the package document {opf_name} is missing"))?;
    let package = _package(&String::from_utf8_lossy(opf), _dir_of(&opf_name));
    let mut out = String::new();
    let title = if package.title.trim().is_empty() { fallback } else { package.title.trim() };
    out.push_str("# ");
    out.push_str(&escape_text(title));
    if !package.creator.trim().is_empty() {
        out.push_str("\n\n*");
        out.push_str(&escape_text(package.creator.trim()));
        out.push('*');
    }
    let mut used = HashSet::new();
    let mut chapters = 0;
    for name in &package.spine {
        let Some(bytes) = files.get(name) else {
            opusdm_ipc::err_log!("viewer", "EPUB chapter {name} is missing");
            continue;
        };
        let chapter = chapter_markdown(&String::from_utf8_lossy(bytes), _dir_of(name), files, &mut used);
        if chapter.trim().is_empty() { continue; }
        out.push_str(CHAPTER_BREAK);
        out.push_str(chapter.trim());
        chapters += 1;
    }
    if chapters == 0 { return Err("the book has no text".to_string()); }
    out.push('\n');
    let pictures = used.into_iter().filter_map(|name| files.get(&name).map(|data| (name, glib::Bytes::from(&data[..])))).collect();
    Ok(Book { markdown: out, pictures })
}

/// What the package document says: title, author and the spine documents
/// (archive names, in reading order).
#[derive(Debug, Default)]
struct Package {
    title: String,
    creator: String,
    spine: Vec<String>,
}

/// Archive name of the package document, from `container.xml`.
fn _rootfile(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"rootfile" => return _attr(&e, b"full-path").map(|p| resolve("", &p)),
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Read the package document (`.opf`); `dir` is its folder in the archive.
fn _package(xml: &str, dir: &str) -> Package {
    let mut reader = Reader::from_str(xml);
    let mut package = Package::default();
    // id -> (archive name, media type)
    let mut manifest: HashMap<String, (String, String)> = HashMap::new();
    let mut spine_ids = Vec::new();
    let mut field: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.local_name().as_ref() {
                b"title" if package.title.is_empty() => field = Some("title"),
                b"creator" if package.creator.is_empty() => field = Some("creator"),
                b"item" => {
                    if let (Some(id), Some(href)) = (_attr(&e, b"id"), _attr(&e, b"href")) {
                        manifest.insert(id, (resolve(dir, &href), _attr(&e, b"media-type").unwrap_or_default()));
                    }
                }
                b"itemref" => {
                    if let Some(id) = _attr(&e, b"idref") { spine_ids.push(id); }
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                let text = t.decode().map(|s| s.to_string()).unwrap_or_default();
                match field {
                    Some("title") => package.title.push_str(&text),
                    Some("creator") => package.creator.push_str(&text),
                    _ => {}
                }
            }
            Ok(Event::GeneralRef(r)) => {
                let text = _entity_text(&r);
                match field {
                    Some("title") => package.title.push_str(&text),
                    Some("creator") => package.creator.push_str(&text),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => field = None,
            Ok(Event::Eof) => break,
            Err(e) => {
                opusdm_ipc::err_log!("viewer", "EPUB package document: {e}");
                break;
            }
            _ => {}
        }
    }
    package.spine = spine_ids
        .iter()
        .filter_map(|id| manifest.get(id))
        .filter(|(_, media)| media.is_empty() || TEXT_TYPES.contains(&media.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    package
}

/// The value of attribute `name` (any prefix), entities resolved.
fn _attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == name).and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.to_string()))
}

/// The text of an entity reference: a character, an HTML entity, or the
/// reference itself when unknown.
fn _entity_text(r: &quick_xml::events::BytesRef) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() { return c.to_string(); }
    let name = r.decode().map(|s| s.to_string()).unwrap_or_default();
    resolve_predefined_entity(&name).map(str::to_string).unwrap_or_else(|| format!("&{name};"))
}

/// Folder part of an archive name (`OEBPS/text/c1.xhtml` -> `OEBPS/text`).
fn _dir_of(name: &str) -> &str {
    name.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The archive name `href` points to from folder `dir`: fragment dropped,
/// percent escapes decoded, `.` and `..` applied.
pub fn resolve(dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let decoded = glib::Uri::unescape_string(href, None).map(|s| s.to_string()).unwrap_or_else(|| href.to_string());
    let mut parts: Vec<&str> = if decoded.starts_with('/') { Vec::new() } else { dir.split('/').filter(|p| !p.is_empty()).collect() };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `text` with every ASCII punctuation character backslash escaped, so
/// Markdown shows it as it is.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for c in text.chars() {
        if c.is_ascii_punctuation() { out.push('\\'); }
        out.push(c);
    }
    out
}

/// An archive name as a Markdown image address: everything but letters,
/// digits and `/._-` percent-escaped.
pub fn picture_address(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"/._-".contains(&b) { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// Turn one XHTML chapter into Markdown. `dir` is its folder in the archive;
/// the pictures it shows that exist in `files` are added to `used`.
pub fn chapter_markdown(xhtml: &str, dir: &str, files: &HashMap<String, Vec<u8>>, used: &mut HashSet<String>) -> String {
    let mut writer = MdWriter::default();
    let mut reader = Reader::from_str(xhtml);
    let config = reader.config_mut();
    config.expand_empty_elements = true;
    config.check_end_names = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                writer.start(&name, &e, dir, files, used);
            }
            Ok(Event::End(e)) => writer.end(&String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase()),
            Ok(Event::Text(t)) => writer.text(&t.decode().map(|s| s.to_string()).unwrap_or_default()),
            Ok(Event::CData(t)) => writer.text(&String::from_utf8_lossy(&t)),
            Ok(Event::GeneralRef(r)) => writer.text(&_entity_text(&r)),
            Ok(Event::Eof) => break,
            Err(e) => {
                opusdm_ipc::err_log!("viewer", "EPUB chapter in {dir}: {e}");
                break;
            }
            _ => {}
        }
    }
    writer.out.trim_end().to_string()
}

/// Builds the Markdown of a chapter from its elements. Whitespace is
/// collapsed; emphasis and link marks are written only around real text.
#[derive(Default)]
struct MdWriter {
    out: String,
    /// Depth inside elements whose content is not shown.
    hidden: usize,
    /// Depth inside `<pre>`: text is kept as it is.
    pre: usize,
    quote: usize,
    lists: Vec<bool>,
    /// A space is owed before the next text.
    space: bool,
    /// Marks opened but not written yet (no text since).
    pending: Vec<String>,
    /// Closing text of every open emphasis or link, with whether its opening was written.
    marks: Vec<(String, bool)>,
    /// Length of the text before and after the last paragraph separator, so
    /// an empty paragraph can be taken back.
    last_block: Option<(usize, usize)>,
}

impl MdWriter {
    /// An element opens.
    fn start(&mut self, name: &str, e: &BytesStart, dir: &str, files: &HashMap<String, Vec<u8>>, used: &mut HashSet<String>) {
        if self.hidden > 0 || HIDDEN.contains(&name) { self.hidden += 1; return; }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self._block();
                let level = name[1..].parse::<usize>().unwrap_or(1);
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
            }
            "pre" => {
                self._block();
                self.out.push_str("```\n");
                self.pre += 1;
            }
            "blockquote" => { self.quote += 1; self._block(); }
            "ul" | "ol" => { self.lists.push(name == "ol"); self._block(); }
            "li" => {
                self._block();
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"  ".repeat(depth));
                self.out.push_str(if self.lists.last() == Some(&true) { "1. " } else { "- " });
            }
            "br" => {
                if self.pre > 0 { self.out.push('\n'); } else if !self._at_line_start() { self.out.push_str("\\\n"); self._prefix(); self.space = false; }
            }
            "hr" => { self._block(); self.out.push_str("* * *"); self._block(); }
            "td" | "th" => { if !self._at_line_start() { self.space = false; self.out.push_str(" \\| "); } }
            "em" | "i" | "cite" | "dfn" | "var" => self._open_mark("*", "*"),
            "strong" | "b" => self._open_mark("**", "**"),
            "a" => match _attr(e, b"href").filter(|h| h.starts_with("http://") || h.starts_with("https://") || h.starts_with("mailto:")) {
                Some(href) => self._open_mark("[", &format!("]({})", href.replace('(', "%28").replace(')', "%29").replace(' ', "%20"))),
                None => self.marks.push((String::new(), false)),
            },
            "img" | "image" => {
                let src = _attr(e, b"src").or_else(|| _attr(e, b"href"));
                let Some(src) = src.filter(|s| !s.contains("://") && !s.starts_with("data:")) else { return };
                let target = resolve(dir, &src);
                if !files.contains_key(&target) { return opusdm_ipc::err_log!("viewer", "EPUB picture {target} is missing"); }
                let alt = _attr(e, b"alt").unwrap_or_default();
                self._flush();
                self.out.push_str(&format!("![{}]({})", escape_text(&alt), picture_address(&target)));
                used.insert(target);
            }
            n if BLOCKS.contains(&n) => self._block(),
            _ => {}
        }
    }

    /// An element closes.
    fn end(&mut self, name: &str) {
        if self.hidden > 0 { self.hidden -= 1; return; }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self._block(),
            "pre" => {
                self.pre = self.pre.saturating_sub(1);
                if !self.out.ends_with('\n') { self.out.push('\n'); }
                self.out.push_str("```");
                self._block();
            }
            "blockquote" => { self.quote = self.quote.saturating_sub(1); self._block(); }
            "ul" | "ol" => { self.lists.pop(); self._block(); }
            "em" | "i" | "cite" | "dfn" | "var" | "strong" | "b" | "a" => self._close_mark(),
            n if BLOCKS.contains(&n) || n == "li" => self._block(),
            _ => {}
        }
    }

    /// Text of the chapter.
    fn text(&mut self, text: &str) {
        if self.hidden > 0 { return; }
        if self.pre > 0 { self.out.push_str(text); return; }
        for (i, word) in text.split(|c: char| c.is_ascii_whitespace()).enumerate() {
            if i > 0 { self.space = true; }
            if word.is_empty() { continue; }
            self._flush();
            self.out.push_str(&escape_text(word));
        }
    }

    /// Write the owed space and the marks waiting for text.
    fn _flush(&mut self) {
        if self.space && !self._at_line_start() { self.out.push(' '); }
        self.space = false;
        for mark in self.pending.drain(..) { self.out.push_str(&mark); }
        for m in self.marks.iter_mut() { if !m.0.is_empty() { m.1 = true; } }
    }

    /// Open an emphasis or a link; written with the first text inside it.
    fn _open_mark(&mut self, open: &str, close: &str) {
        self.pending.push(open.to_string());
        self.marks.push((close.to_string(), false));
    }

    /// Close the innermost mark: written only when its opening was.
    fn _close_mark(&mut self) {
        let Some((close, written)) = self.marks.pop() else { return };
        if close.is_empty() { return; }
        if written { self.out.push_str(&close); } else { self.pending.pop(); }
    }

    /// True at the start of a line (or of the text).
    fn _at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n') || self.out.ends_with("> ")
    }

    /// End the paragraph: a blank line, then the quote marks. Marks still
    /// open are closed and opened again, so they never span paragraphs.
    fn _block(&mut self) {
        if self.pre > 0 { return; }
        match self.last_block {
            Some((before, after)) if self.out.len() == after => self.out.truncate(before),
            _ => {
                let open: Vec<String> = self.marks.iter().filter(|m| m.1).map(|m| m.0.clone()).collect();
                for close in open.iter().rev() { self.out.push_str(close); }
            }
        }
        for m in self.marks.iter_mut() { m.1 = false; }
        self.pending = self.marks.iter().filter(|m| !m.0.is_empty()).map(|m| _opening_of(&m.0)).collect();
        let before = self.out.len();
        if !self.out.is_empty() { self.out.push_str("\n\n"); }
        self._prefix();
        self.last_block = Some((before, self.out.len()));
        self.space = false;
    }

    /// Write the quote marks of the current depth.
    fn _prefix(&mut self) {
        for _ in 0..self.quote { self.out.push_str("> "); }
    }
}

/// The opening mark of a closing one.
fn _opening_of(close: &str) -> String {
    if close.starts_with(']') { "[".to_string() } else { close.to_string() }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// Turn `body` into Markdown with no pictures available.
    fn md(body: &str) -> String {
        chapter_markdown(&format!("<html><head><title>T</title><style>p {{}}</style></head><body>{body}</body></html>"), "", &HashMap::new(), &mut HashSet::new())
    }

    /// Write an EPUB holding `entries` (name, bytes).
    pub(crate) fn write_epub(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// A two chapter book with a cover picture, stored under `OEBPS/`.
    pub(crate) fn sample_epub(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("book.epub");
        let container = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>The &amp; Book</dc:title><dc:creator>Ann Author</dc:creator></metadata>
            <manifest><item id="c2" href="text/two.xhtml" media-type="application/xhtml+xml"/><item id="c1" href="text/one%20a.xhtml" media-type="application/xhtml+xml"/><item id="pic" href="img/cover.png" media-type="image/png"/><item id="css" href="s.css" media-type="text/css"/></manifest>
            <spine><itemref idref="c1"/><itemref idref="css"/><itemref idref="c2"/></spine></package>"#;
        let one = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Chapter One</h1><p>It was a <em>dark</em> night.&nbsp;Really.</p><img src="../img/cover.png" alt="cover"/></body></html>"#;
        let two = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h2>Chapter Two</h2><p>The end.</p></body></html>"#;
        write_epub(&path, &[("mimetype", b"application/epub+zip"), (CONTAINER, container), ("OEBPS/content.opf", opf), ("OEBPS/text/one a.xhtml", one), ("OEBPS/text/two.xhtml", two), ("OEBPS/img/cover.png", b"PNGDATA"), ("OEBPS/s.css", b"p {}")]);
        path
    }

    #[test]
    fn a_book_is_its_chapters_in_spine_order_with_title_and_author() {
        let dir = tempfile::tempdir().unwrap();
        let book = open(&sample_epub(dir.path())).unwrap();
        let md = &book.markdown;
        assert!(md.starts_with("# The \\& Book\n\n*Ann Author*"), "{md}");
        let (one, two) = (md.find("# Chapter One").unwrap(), md.find("## Chapter Two").unwrap());
        assert!(one < two, "{md}");
        assert!(md.contains("It was a *dark* night\\.\u{a0}Really\\."), "{md}");
        assert!(md.contains("![cover](OEBPS/img/cover.png)"), "{md}");
        assert_eq!(book.pictures.keys().collect::<Vec<_>>(), vec!["OEBPS/img/cover.png"]);
        assert!(!md.contains("p {}"), "the CSS is not a chapter");
    }

    #[test]
    fn the_markdown_view_parses_the_book_as_meant() {
        let dir = tempfile::tempdir().unwrap();
        let book = open(&sample_epub(dir.path())).unwrap();
        let runs = crate::markdown::parse(&book.markdown);
        let has = |text: &str, tag: &str| runs.iter().filter(|r| r.tags.iter().any(|t| t == tag)).map(|r| r.text.as_str()).collect::<String>().contains(text);
        let all: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(has("The & Book", "h1"));
        assert!(has("Ann Author", "italic"));
        assert!(has("dark", "italic"));
        assert!(has("Chapter Two", "h2"));
        assert!(all.contains("night.\u{a0}Really."), "escapes are gone: {all}");
        let picture = runs.iter().find_map(|r| r.image.clone()).expect("the cover is a picture");
        assert!(book.pictures.contains_key(glib::Uri::unescape_string(&picture, None).unwrap().as_str()));
    }

    #[test]
    fn not_an_epub_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("x.epub");
        write_epub(&zip, &[("a.txt", b"hi")]);
        assert!(open(&zip).unwrap_err().contains("not an EPUB"));
        assert!(open(&dir.path().join("missing.epub")).is_err());
    }

    #[test]
    fn markdown_characters_in_the_text_are_escaped() {
        assert_eq!(md("<p># not a heading * [x](y) 1. two</p>"), "\\# not a heading \\* \\[x\\]\\(y\\) 1\\. two");
    }

    #[test]
    fn whitespace_collapses_and_emphasis_hugs_its_text() {
        assert_eq!(md("<p>a\n   <em> b </em>  c<strong></strong></p>"), "a *b* c");
        assert_eq!(md("<p><b>bold <i>both</i></b></p>"), "**bold *both***");
    }

    #[test]
    fn blocks_lists_quotes_breaks_and_rules() {
        assert_eq!(md("<p>one</p><div>two</div>"), "one\n\ntwo");
        assert_eq!(md("<ul><li>a</li><li>b</li></ul>"), "- a\n\n- b");
        assert_eq!(md("<blockquote><p>q1</p><p>q2</p></blockquote><p>after</p>"), "> q1\n\n> q2\n\nafter");
        assert_eq!(md("<p>line<br/>next</p>"), "line\\\nnext");
        assert_eq!(md("<p>a</p><hr/><p>b</p>"), "a\n\n* * *\n\nb");
        assert_eq!(md("<pre>  keep  this\n x</pre>"), "```\n  keep  this\n x\n```");
    }

    #[test]
    fn web_links_are_kept_and_inner_links_are_text() {
        assert_eq!(md(r#"<p><a href="https://x.org/a b">site</a> and <a href="c2.xhtml#n1">note</a></p>"#), "[site](https://x.org/a%20b) and note");
    }

    #[test]
    fn hrefs_resolve_from_the_chapter_folder() {
        assert_eq!(resolve("OEBPS/text", "../img/a%20b.png#x"), "OEBPS/img/a b.png");
        assert_eq!(resolve("", "./c.xhtml"), "c.xhtml");
        assert_eq!(resolve("a", "/root.png"), "root.png");
        assert_eq!(picture_address("OEBPS/img/a b(1).png"), "OEBPS/img/a%20b%281%29.png");
    }
}
