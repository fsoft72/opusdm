//! Local-time text conversion for the "Set Timestamp" dialog, done with libc
//! so no date library is needed. Format: `YYYY-MM-DD HH:MM:SS`.

/// Text format shown to and accepted from the user.
pub const TIME_FORMAT_HINT: &str = "YYYY-MM-DD HH:MM:SS";

/// The local-time fields of `secs` since the Unix epoch.
fn local_tm(secs: i64) -> Option<libc::tm> {
    let t: libc::time_t = secs;
    // SAFETY: `tm` is plain data, zero is a valid initial value, and
    // `localtime_r` only writes into the buffer we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    ok.then_some(tm)
}

/// Format `secs` (Unix time) as local `YYYY-MM-DD HH:MM:SS`.
pub fn format_local(secs: i64) -> String {
    match local_tm(secs) {
        Some(tm) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        ),
        None => secs.to_string(),
    }
}

/// Parse local `YYYY-MM-DD HH:MM:SS` (`T` may replace the space, seconds may
/// be left out) into Unix time. Impossible dates such as Feb 30 are rejected.
pub fn parse_local(text: &str) -> Result<i64, String> {
    let bad = || format!("expected {TIME_FORMAT_HINT}, got {text:?}");
    let text = text.trim().replace('T', " ");
    let (date, time) = text.split_once(' ').ok_or_else(bad)?;
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || !(2..=3).contains(&t.len()) {
        return Err(bad());
    }
    let num = |s: &str| s.trim().parse::<i32>().map_err(|_| bad());
    let (year, mon, day) = (num(d[0])?, num(d[1])?, num(d[2])?);
    let (hour, min) = (num(t[0])?, num(t[1])?);
    let sec = if t.len() == 3 { num(t[2])? } else { 0 };

    // SAFETY: zeroed `tm` is valid; `mktime` normalises it in place.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = mon - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = min;
    tm.tm_sec = sec;
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) } as i64;
    // `mktime` silently rolls Feb 30 over to March: compare the fields back.
    let same = tm.tm_year + 1900 == year
        && tm.tm_mon + 1 == mon
        && tm.tm_mday == day
        && tm.tm_hour == hour
        && tm.tm_min == min
        && tm.tm_sec == sec;
    if !same || secs == -1 {
        return Err(bad());
    }
    Ok(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_round_trip() {
        let secs = parse_local("2024-03-05 14:07:09").unwrap();
        assert_eq!(format_local(secs), "2024-03-05 14:07:09");
    }

    #[test]
    fn accepts_t_separator_and_missing_seconds() {
        assert_eq!(parse_local("2024-03-05T14:07").unwrap(), parse_local("2024-03-05 14:07:00").unwrap());
    }

    #[test]
    fn rejects_garbage_and_impossible_dates() {
        for bad in ["", "yesterday", "2024-02-30 10:00:00", "2024-13-01 00:00:00", "2024-01-01", "2024-01-01 25:00:00"] {
            assert!(parse_local(bad).is_err(), "{bad:?}");
        }
    }
}
