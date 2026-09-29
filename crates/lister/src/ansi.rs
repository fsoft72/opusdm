//! ANSI art support for the Viewer: CP437 decoding and the colour escape
//! sequences of `.ans` files.

/// The 16 VGA colours: 0..7 normal, 8..15 bright.
pub const PALETTE: [&str; 16] = [
    "#000000", "#aa0000", "#00aa00", "#aa5500", "#0000aa", "#aa00aa", "#00aaaa", "#aaaaaa",
    "#555555", "#ff5555", "#55ff55", "#ffff55", "#5555ff", "#ff55ff", "#55ffff", "#ffffff",
];

/// CP437 glyphs of the control range 0x00..0x1F (0x00 shows as a space).
const CP437_LOW: &str = " \u{263A}\u{263B}\u{2665}\u{2666}\u{2663}\u{2660}\u{2022}\u{25D8}\u{25CB}\u{25D9}\u{2642}\u{2640}\u{266A}\u{266B}\u{263C}\u{25BA}\u{25C4}\u{2195}\u{203C}\u{B6}\u{A7}\u{25AC}\u{21A8}\u{2191}\u{2193}\u{2192}\u{2190}\u{221F}\u{2194}\u{25B2}\u{25BC}";

/// CP437 glyphs of the upper half 0x80..0xFF.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{A0}";

/// The character CP437 shows for byte `b`.
pub fn cp437_char(b: u8) -> char {
    match b {
        0x00..=0x1F => CP437_LOW.chars().nth(b as usize).unwrap_or(' '),
        0x20..=0x7E => b as char,
        0x7F => '\u{2302}',
        _ => CP437_HIGH.chars().nth((b - 0x80) as usize).unwrap_or(' '),
    }
}

/// A run of text with one style. Offsets are characters of [`AnsiDoc::text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    /// Palette index, already brightened for bold.
    pub fg: Option<u8>,
    /// Palette index.
    pub bg: Option<u8>,
    pub bold: bool,
}

/// Decoded text plus the styled runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnsiDoc {
    pub text: String,
    pub spans: Vec<Span>,
}

/// The style being applied while parsing.
#[derive(Clone, Copy, Default)]
struct Style {
    fg: Option<u8>,
    bg: Option<u8>,
    bold: bool,
}

impl Style {
    /// Foreground as shown: bold brightens a normal colour.
    fn effective_fg(&self) -> Option<u8> {
        match self.fg {
            Some(c) if self.bold && c < 8 => Some(c + 8),
            other => other,
        }
    }

    /// Apply the parameters of one `ESC [ ... m` sequence.
    fn apply_sgr(&mut self, params: &str) {
        for p in params.split(';') {
            match p.parse::<u32>().unwrap_or(0) {
                0 => *self = Style::default(),
                1 => self.bold = true,
                22 => self.bold = false,
                n @ 30..=37 => self.fg = Some((n - 30) as u8),
                n @ 90..=97 => self.fg = Some((n - 90 + 8) as u8),
                39 => self.fg = None,
                n @ 40..=47 => self.bg = Some((n - 40) as u8),
                n @ 100..=107 => self.bg = Some((n - 100 + 8) as u8),
                49 => self.bg = None,
                _ => {}
            }
        }
    }
}

/// Close the run `start..end` in `style`, if it has any styling.
fn flush(spans: &mut Vec<Span>, start: usize, end: usize, style: &Style) {
    if end <= start {
        return;
    }
    let fg = style.effective_fg();
    if fg.is_none() && style.bg.is_none() && !style.bold {
        return;
    }
    spans.push(Span { start, end, fg, bg: style.bg, bold: style.bold });
}

/// Parse ANSI art: CP437 text with SGR colours and cursor-forward. Every
/// other escape sequence is skipped; a truncated one at the end is dropped.
pub fn parse_ansi(bytes: &[u8]) -> AnsiDoc {
    let mut text = String::new();
    let mut chars = 0usize;
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut span_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1A {
            break;
        }
        if b == b'\r' {
            i += 1;
            continue;
        }
        if b == 0x1B {
            if bytes.get(i + 1) != Some(&b'[') {
                i += 1;
                continue;
            }
            // CSI: parameter and intermediate bytes, then a final byte.
            let Some(rel) = bytes[i + 2..].iter().position(|c| (0x40..=0x7E).contains(c)) else { break };
            let end = i + 2 + rel;
            let params = String::from_utf8_lossy(&bytes[i + 2..end]).into_owned();
            match bytes[end] {
                b'm' => {
                    flush(&mut spans, span_start, chars, &style);
                    style.apply_sgr(&params);
                    span_start = chars;
                }
                b'C' => {
                    let n = params.parse::<usize>().unwrap_or(1).max(1);
                    text.extend(std::iter::repeat_n(' ', n));
                    chars += n;
                }
                _ => {}
            }
            i = end + 1;
            continue;
        }
        text.push(if b == b'\n' { '\n' } else { cp437_char(b) });
        chars += 1;
        i += 1;
    }
    flush(&mut spans, span_start, chars, &style);
    AnsiDoc { text, spans }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize, fg: Option<u8>, bg: Option<u8>, bold: bool) -> Span {
        Span { start, end, fg, bg, bold }
    }

    #[test]
    fn plain_text_keeps_lines_and_drops_carriage_returns() {
        let doc = parse_ansi(b"hi\r\nthere");
        assert_eq!(doc.text, "hi\nthere");
        assert!(doc.spans.is_empty());
    }

    #[test]
    fn a_foreground_colour_styles_only_the_text_it_covers() {
        let doc = parse_ansi(b"\x1b[31mred\x1b[0m ok");
        assert_eq!(doc.text, "red ok");
        assert_eq!(doc.spans, vec![span(0, 3, Some(1), None, false)]);
    }

    #[test]
    fn bold_turns_a_normal_foreground_bright() {
        let doc = parse_ansi(b"\x1b[1;31mX");
        assert_eq!(doc.spans, vec![span(0, 1, Some(9), None, true)]);
    }

    #[test]
    fn bright_and_background_codes_map_to_the_palette() {
        assert_eq!(parse_ansi(b"\x1b[91mA").spans[0].fg, Some(9));
        assert_eq!(parse_ansi(b"\x1b[44mB").spans[0].bg, Some(4));
        assert_eq!(parse_ansi(b"\x1b[100mB").spans[0].bg, Some(8));
    }

    #[test]
    fn default_colour_codes_end_the_style() {
        let doc = parse_ansi(b"\x1b[31mA\x1b[39mB");
        assert_eq!(doc.text, "AB");
        assert_eq!(doc.spans, vec![span(0, 1, Some(1), None, false)]);
    }

    #[test]
    fn a_style_carries_across_a_newline() {
        let doc = parse_ansi(b"\x1b[32mA\nB");
        assert_eq!(doc.text, "A\nB");
        assert_eq!(doc.spans, vec![span(0, 3, Some(2), None, false)]);
    }

    #[test]
    fn cursor_forward_inserts_spaces() {
        assert_eq!(parse_ansi(b"a\x1b[3Cb").text, "a   b");
        assert_eq!(parse_ansi(b"a\x1b[Cb").text, "a b");
        assert_eq!(parse_ansi(b"a\x1b[0Cb").text, "a b", "0 counts as 1");
    }

    #[test]
    fn unknown_sequences_are_skipped_without_leaving_bytes_behind() {
        assert_eq!(parse_ansi(b"a\x1b[2Jb\x1b[?25hc\x1b[10;20Hd").text, "abcd");
    }

    #[test]
    fn a_truncated_or_lone_escape_never_panics_or_leaks() {
        assert_eq!(parse_ansi(b"ab\x1b[31").text, "ab");
        assert_eq!(parse_ansi(b"ab\x1b[").text, "ab");
        assert_eq!(parse_ansi(b"ab\x1b").text, "ab");
        assert_eq!(parse_ansi(b"a\x1bb").text, "ab");
    }

    #[test]
    fn the_sub_byte_ends_the_content_before_a_sauce_record() {
        assert_eq!(parse_ansi(b"abc\x1adef SAUCE00").text, "abc");
    }

    #[test]
    fn span_offsets_count_characters_not_bytes() {
        let doc = parse_ansi(b"\xc9\x1b[31m\xcd");
        assert_eq!(doc.text, "\u{2554}\u{2550}");
        assert_eq!(doc.spans, vec![span(1, 2, Some(1), None, false)]);
    }

    #[test]
    fn cp437_maps_the_block_and_box_characters() {
        let shown: String = [0xB0u8, 0xB1, 0xB2, 0xC9, 0xE1, 0xDB].iter().map(|&b| cp437_char(b)).collect();
        assert_eq!(shown, "\u{2591}\u{2592}\u{2593}\u{2554}\u{DF}\u{2588}");
        assert_eq!(cp437_char(0x01), '\u{263A}');
        assert_eq!(cp437_char(0x7F), '\u{2302}');
        assert_eq!(cp437_char(b'A'), 'A');
        assert_eq!(cp437_char(0xFF), '\u{A0}');
    }

    #[test]
    fn the_palette_has_sixteen_colours() {
        assert_eq!(PALETTE.len(), 16);
        assert!(PALETTE.iter().all(|c| c.starts_with('#') && c.len() == 7));
    }
}
