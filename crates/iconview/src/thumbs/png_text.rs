//! Minimal PNG `tEXt` chunk support: read a keyword's value, or splice new
//! `tEXt` chunks in right after `IHDR`. Enough for the freedesktop thumbnail
//! `Thumb::MTime` / `Thumb::URI` tags. No image decoding.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The value of the first `tEXt` chunk whose keyword is `keyword`, or `None`.
pub fn read_text(png: &[u8], keyword: &str) -> Option<String> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return None;
    }
    let mut off = 8;
    while off + 12 <= png.len() {
        let len = u32::from_be_bytes(png[off..off + 4].try_into().ok()?) as usize;
        let ctype = &png[off + 4..off + 8];
        let data_start = off + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end + 4 > png.len() {
            return None;
        }
        if ctype == b"tEXt" {
            let data = &png[data_start..data_end];
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                if &data[..nul] == keyword.as_bytes() {
                    return Some(String::from_utf8_lossy(&data[nul + 1..]).into_owned());
                }
            }
        }
        if ctype == b"IEND" {
            return None;
        }
        off = data_end + 4; // skip CRC
    }
    None
}

/// A copy of `png` with a `tEXt` chunk per entry inserted right after `IHDR`.
/// `None` if `png` is not a PNG or has no `IHDR` where expected.
pub fn insert_text(png: &[u8], entries: &[(&str, &str)]) -> Option<Vec<u8>> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return None;
    }
    // IHDR is always the first chunk: 4 len + 4 type + 13 data + 4 crc = 25.
    let ihdr_end = 8 + 25;
    if png.len() < ihdr_end || &png[12..16] != b"IHDR" {
        return None;
    }
    let mut out = Vec::with_capacity(png.len() + entries.len() * 32);
    out.extend_from_slice(&png[..ihdr_end]);
    for (keyword, text) in entries {
        let mut data = Vec::with_capacity(keyword.len() + 1 + text.len());
        data.extend_from_slice(keyword.as_bytes());
        data.push(0);
        data.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(b"tEXt");
        out.extend_from_slice(&data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(b"tEXt");
        crc_input.extend_from_slice(&data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    out.extend_from_slice(&png[ihdr_end..]);
    Some(out)
}

/// PNG CRC-32 (polynomial 0xEDB88320, reflected, init/xor 0xFFFFFFFF).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A 1x1 greyscale PNG (signature + IHDR + IDAT + IEND), no text chunks.
/// Shared by the thumbnail tests in sibling modules.
#[cfg(test)]
pub(crate) fn tests_tiny_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x3a,
        0x7e, 0x9b, 0x55, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x62,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x03, 0x36, 0x37, 0x7c, 0xa8, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vector() {
        // CRC-32 of b"IEND" is 0xAE426082 (the standard PNG IEND CRC).
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn insert_then_read_round_trips() {
        let png = tests_tiny_png();
        assert_eq!(read_text(&png, "Thumb::MTime"), None);
        let with = insert_text(&png, &[("Thumb::MTime", "1700000000"), ("Thumb::URI", "file:///x")])
            .expect("is a png");
        assert_eq!(read_text(&with, "Thumb::MTime").as_deref(), Some("1700000000"));
        assert_eq!(read_text(&with, "Thumb::URI").as_deref(), Some("file:///x"));
        assert!(with.windows(4).any(|w| w == b"IDAT"));
        assert!(with.ends_with(b"IEND\xae\x42\x60\x82"));
    }

    #[test]
    fn non_png_is_rejected() {
        assert_eq!(read_text(b"not a png", "x"), None);
        assert_eq!(insert_text(b"not a png", &[("a", "b")]), None);
    }
}
