//! Link recognition: the PCRE2 patterns VTE highlights under the pointer and
//! the conversion of a matched text into a URI that can be opened.

/// `PCRE2_CASELESS`: the patterns ignore case.
pub const PCRE2_CASELESS: u32 = 0x0000_0008;

/// `PCRE2_MULTILINE`: VTE refuses match regexes compiled without it.
pub const PCRE2_MULTILINE: u32 = 0x0000_0400;

/// Web, FTP and file URLs, plus bare `www.` hosts. The last character may not
/// be punctuation that usually ends a sentence or closes a bracket.
pub const URL_PATTERN: &str =
    r#"\b(?:(?:https?|ftps?|sftp|file)://|www\.)[^\s<>"'`]*[^\s<>"'`.,;:!?)\]}]"#;

/// `mailto:` links and bare e-mail addresses.
pub const EMAIL_PATTERN: &str = r"\b(?:mailto:)?[\w.+-]+@[\w-]+(?:\.[\w-]+)+\b";

/// Every pattern registered on a terminal.
pub const PATTERNS: [&str; 2] = [URL_PATTERN, EMAIL_PATTERN];

/// The URI to open for the matched `text`: `www.` hosts get `http://`, bare
/// e-mail addresses get `mailto:`, anything else is already a URI.
pub fn link_to_uri(text: &str) -> String {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("www.") { return format!("http://{text}"); }
    if !lower.contains("://") && !lower.starts_with("mailto:") && text.contains('@') {
        return format!("mailto:{text}");
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_hosts_and_addresses_become_uris() {
        assert_eq!(link_to_uri("www.example.com"), "http://www.example.com");
        assert_eq!(link_to_uri("me@example.com"), "mailto:me@example.com");
    }

    #[test]
    fn full_uris_are_kept() {
        assert_eq!(link_to_uri("https://example.com/a?b=c"), "https://example.com/a?b=c");
        assert_eq!(link_to_uri("mailto:me@example.com"), "mailto:me@example.com");
        assert_eq!(link_to_uri("ftp://user@host/file"), "ftp://user@host/file");
    }
}
