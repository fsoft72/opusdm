//! The body of a notification may carry a little markup (`body-markup`).
//! Only `b`, `i`, `u` and `a href` are kept; everything else is shown as
//! text, so a notification can never inject Pango markup (colours, sizes) or
//! break the label with an unbalanced tag. `<br>` becomes a line break and
//! `<img>` is dropped.

/// Tags kept as they are.
const SIMPLE_TAGS: [&str; 3] = ["b", "i", "u"];
/// Entities Pango understands, kept as they are.
const NAMED_ENTITIES: [&str; 5] = ["amp", "lt", "gt", "quot", "apos"];

/// `text` escaped for Pango markup.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Safe Pango markup for the notification body `body`: whitelisted tags
/// kept and balanced, anything else escaped.
pub fn sanitize(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut open: Vec<&'static str> = Vec::new();
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                if let Some((tag, len)) = parse_tag(rest) {
                    apply_tag(&tag, &mut out, &mut open);
                    rest = &rest[len..];
                } else {
                    out.push_str("&lt;");
                    rest = &rest[1..];
                }
            }
            '&' => {
                let len = entity_len(rest);
                if len > 0 {
                    out.push_str(&rest[..len]);
                    rest = &rest[len..];
                } else {
                    out.push_str("&amp;");
                    rest = &rest[1..];
                }
            }
            '>' => {
                out.push_str("&gt;");
                rest = &rest[1..];
            }
            c => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    while let Some(name) = open.pop() {
        push_close(&mut out, name);
    }
    out
}

/// `markup` (from [`sanitize`]) with its links turned into underlines:
/// what Pango alone must accept (`<a>` is GtkLabel's own markup).
pub fn links_as_underline(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(at) = rest.find("<a href=\"") {
        out.push_str(&rest[..at]);
        // The href is escaped by `sanitize`: its first `">` ends the tag.
        let Some(end) = rest[at..].find("\">") else { break };
        out.push_str("<u>");
        rest = &rest[at + end + 2..];
    }
    out.push_str(rest);
    out.replace("</a>", "</u>")
}

/// The body without any markup and with the entities decoded, for places
/// that show plain text (the panel history).
pub fn plain_text(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some((tag, len)) = parse_tag(rest) {
                if tag.name == "br" { out.push('\n'); }
                rest = &rest[len..];
                continue;
            }
        }
        if c == '&' {
            let len = entity_len(rest);
            if len > 0 {
                out.push_str(&decode_entity(&rest[1..len - 1]));
                rest = &rest[len..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// One parsed tag.
#[derive(Debug, PartialEq, Eq)]
struct Tag {
    /// Lower case name.
    name: String,
    closing: bool,
    /// The `href` of an `a` tag.
    href: Option<String>,
}

/// Parse the tag at the start of `s` (which starts with `<`): the tag and its
/// length in bytes. `None` when it is not a well-formed tag.
fn parse_tag(s: &str) -> Option<(Tag, usize)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    if inner.contains('<') { return None; }
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    // A tag name starts right after `<` with a letter: `1 < 2 > 0` is text.
    if !inner.starts_with(|c: char| c.is_ascii_alphabetic()) { return None; }
    let inner = inner.trim_end_matches('/').trim();
    let name_len = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
    let name = inner[..name_len].to_ascii_lowercase();
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) { return None; }
    let href = (name == "a" && !closing).then(|| attribute(&inner[name_len..], "href")).flatten();
    Some((Tag { name, closing, href }, end + 1))
}

/// The value of attribute `key` in `attrs` (`key="v"` or `key='v'`), with
/// its entities decoded.
fn attribute(attrs: &str, key: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let at = lower.find(&format!("{key}="))?;
    let value = &attrs[at + key.len() + 1..];
    let quote = value.chars().next().filter(|q| *q == '"' || *q == '\'')?;
    let close = value[1..].find(quote)?;
    Some(plain_text(&value[1..1 + close]))
}

/// Write `tag` to `out` if it is allowed, tracking the open tags in `open`.
/// A closing tag that does not match the innermost open one is dropped.
fn apply_tag(tag: &Tag, out: &mut String, open: &mut Vec<&'static str>) {
    if tag.name == "br" {
        out.push('\n');
        return;
    }
    let Some(name) = allowed(&tag.name) else { return };
    if tag.closing {
        if open.last() == Some(&name) {
            open.pop();
            push_close(out, name);
        }
        return;
    }
    if name == "a" {
        let href = tag.href.as_deref().unwrap_or("");
        out.push_str(&format!("<a href=\"{}\">", escape(href)));
    } else {
        out.push_str(&format!("<{name}>"));
    }
    open.push(name);
}

/// The static name of an allowed tag.
fn allowed(name: &str) -> Option<&'static str> {
    if name == "a" { return Some("a"); }
    SIMPLE_TAGS.iter().copied().find(|t| *t == name)
}

/// Close tag `name`.
fn push_close(out: &mut String, name: &str) {
    out.push_str(&format!("</{name}>"));
}

/// Length in bytes of the entity at the start of `s` (which starts with
/// `&`), or 0 when it is not one Pango accepts.
fn entity_len(s: &str) -> usize {
    let Some(end) = s[1..].find(';').map(|i| i + 1) else { return 0 };
    let body = &s[1..end];
    if body.is_empty() || end > 12 { return 0; }
    let ok = NAMED_ENTITIES.contains(&body) || numeric_entity(body).is_some();
    if ok { end + 1 } else { 0 }
}

/// The character of a numeric entity body (`#65`, `#x41`).
fn numeric_entity(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code).filter(|c| *c != '\0')
}

/// The text of an entity body known to be valid.
fn decode_entity(body: &str) -> String {
    match body {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        other => numeric_entity(other).map(String::from).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_tags_are_kept() {
        assert_eq!(sanitize("<b>Hi</b> <i>there</i> <u>you</u>"), "<b>Hi</b> <i>there</i> <u>you</u>");
        assert_eq!(sanitize("<a href=\"https://x.org/?a=1&b=2\">link</a>"), "<a href=\"https://x.org/?a=1&amp;b=2\">link</a>");
    }

    #[test]
    fn other_tags_and_attributes_disappear() {
        assert_eq!(sanitize("<span foreground=\"red\">x</span>"), "x");
        assert_eq!(sanitize("<script>alert(1)</script>"), "alert(1)");
        assert_eq!(sanitize("<img src=\"a.png\" alt=\"pic\"/>ok"), "ok");
        assert_eq!(sanitize("<B onclick=\"x\">bold</B>"), "<b>bold</b>");
    }

    #[test]
    fn stray_characters_are_escaped() {
        assert_eq!(sanitize("1 < 2 & 3 > 2"), "1 &lt; 2 &amp; 3 &gt; 2");
        assert_eq!(sanitize("a &amp; b &#65; &nope; &"), "a &amp; b &#65; &amp;nope; &amp;");
        assert_eq!(sanitize("<<b>x</b>"), "&lt;<b>x</b>");
    }

    #[test]
    fn tags_are_balanced() {
        assert_eq!(sanitize("<b>open"), "<b>open</b>");
        assert_eq!(sanitize("close</b>"), "close");
        assert_eq!(sanitize("<b><i>x</b></i>"), "<b><i>x</i></b>");
    }

    #[test]
    fn links_become_underlines_for_pango() {
        let markup = sanitize("see <a href='https://x.org/?a=\"1\"&b=2'>this</a> now");
        assert_eq!(markup, "see <a href=\"https://x.org/?a=&quot;1&quot;&amp;b=2\">this</a> now");
        assert_eq!(links_as_underline(&markup), "see <u>this</u> now");
    }

    #[test]
    fn line_breaks_become_newlines() {
        assert_eq!(sanitize("a<br>b<br/>c"), "a\nb\nc");
    }

    #[test]
    fn plain_text_drops_tags_and_decodes() {
        assert_eq!(plain_text("<b>Anna</b>: 1 &lt; 2 &amp; &#x41;<br>ok"), "Anna: 1 < 2 & A\nok");
        assert_eq!(plain_text("3 < 4"), "3 < 4");
    }
}
