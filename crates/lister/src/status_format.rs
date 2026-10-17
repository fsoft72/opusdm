//! Expansion of the status bar format string (`Behavior::status_format`).

/// How the byte totals (`%b`, `%B`) are written. A click on the status line
/// cycles through them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ByteUnit {
    /// The plain number of bytes.
    #[default]
    Bytes,
    /// Megabytes, written by `format_size_as`.
    Megabytes,
    /// Gigabytes, written by `format_size_as`.
    Gigabytes,
}

impl ByteUnit {
    /// The unit after this one: bytes, megabytes, gigabytes, bytes again.
    pub fn next(self) -> ByteUnit {
        match self {
            ByteUnit::Bytes => ByteUnit::Megabytes,
            ByteUnit::Megabytes => ByteUnit::Gigabytes,
            ByteUnit::Gigabytes => ByteUnit::Bytes,
        }
    }

    /// `bytes` written in this unit, with the same scaling the file sizes use.
    fn write(self, bytes: u64) -> String {
        match self {
            ByteUnit::Bytes => bytes.to_string(),
            ByteUnit::Megabytes => crate::model::format_size_as(crate::model::SizeFormat::Megabytes, false, bytes),
            ByteUnit::Gigabytes => crate::model::format_size_as(crate::model::SizeFormat::Gigabytes, false, bytes),
        }
    }
}

/// The numbers and path a status format can show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusInfo {
    /// Directories in the listing.
    pub dirs: u32,
    /// Files in the listing.
    pub files: u32,
    /// Bytes in the listing (files only).
    pub bytes: u64,
    /// Selected directories.
    pub sel_dirs: u32,
    /// Selected files.
    pub sel_files: u32,
    /// Bytes of the selected files.
    pub sel_bytes: u64,
    /// The directory shown by the Lister.
    pub path: String,
    /// The unit of `%b` and `%B`.
    pub unit: ByteUnit,
}

/// Expand `format` with the values in `info`.
///
/// Codes: `%d` `%f` `%b` are the total dirs, files and bytes, `%D` `%F` `%B`
/// the same for the selection, `%s` the number of selected entries, `%p` the
/// path and `%%` a literal percent sign. An unknown code, or a lone `%` at the
/// end, is copied as written so a typo stays visible instead of vanishing.
pub fn format_status(format: &str, info: &StatusInfo) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('d') => out.push_str(&info.dirs.to_string()),
            Some('f') => out.push_str(&info.files.to_string()),
            Some('b') => out.push_str(&info.unit.write(info.bytes)),
            Some('D') => out.push_str(&info.sel_dirs.to_string()),
            Some('F') => out.push_str(&info.sel_files.to_string()),
            Some('B') => out.push_str(&info.unit.write(info.sel_bytes)),
            Some('s') => out.push_str(&(info.sel_dirs + info.sel_files).to_string()),
            Some('p') => out.push_str(&info.path),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::DEFAULT_STATUS_FORMAT;

    fn info() -> StatusInfo {
        StatusInfo {
            dirs: 4,
            files: 10,
            bytes: 2048,
            sel_dirs: 1,
            sel_files: 2,
            sel_bytes: 300,
            path: "/tmp".to_string(),
            unit: ByteUnit::Bytes,
        }
    }

    #[test]
    fn totals_render_as_written() {
        assert_eq!(format_status("%f files, %b", &info()), "10 files, 2048");
    }

    #[test]
    fn uppercase_codes_are_the_selection() {
        assert_eq!(format_status("%D/%F/%B", &info()), "1/2/300");
        assert_eq!(format_status("%s selected", &info()), "3 selected");
    }

    #[test]
    fn the_unit_cycles_bytes_mega_giga() {
        assert_eq!(ByteUnit::Bytes.next(), ByteUnit::Megabytes);
        assert_eq!(ByteUnit::Megabytes.next(), ByteUnit::Gigabytes);
        assert_eq!(ByteUnit::Gigabytes.next(), ByteUnit::Bytes);
    }

    #[test]
    fn byte_totals_follow_the_unit() {
        let mut i = StatusInfo { bytes: 3 * 1024 * 1024, sel_bytes: 1024 * 1024 * 1024, ..info() };
        i.unit = ByteUnit::Megabytes;
        assert_eq!(format_status("%B/%b", &i), "1024.00 MB/3.00 MB");
        i.unit = ByteUnit::Gigabytes;
        assert_eq!(format_status("%B/%b", &i), "1.00 GB/0.00 GB");
    }

    #[test]
    fn path_and_literal_percent() {
        assert_eq!(format_status("%p 100%%", &info()), "/tmp 100%");
    }

    #[test]
    fn unknown_codes_and_a_trailing_percent_are_kept() {
        assert_eq!(format_status("%x-%", &info()), "%x-%");
    }

    #[test]
    fn the_default_format_matches_the_old_fixed_line() {
        assert_eq!(
            format_status(DEFAULT_STATUS_FORMAT, &info()),
            "D: [1/4]  F: [2/10]  B: [300/2048]"
        );
    }

    #[test]
    fn non_ascii_text_passes_through() {
        assert_eq!(format_status("é %d ✓", &info()), "é 4 ✓");
    }
}
