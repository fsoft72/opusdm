//! Command line arguments of `opusdm-compositor`.

/// Default size of the output.
pub const DEFAULT_SIZE: (i32, i32) = (1920, 1080);

/// Which backend drives the compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// A window on the host session (sandbox).
    Nested,
    /// No window, used by tests.
    Headless,
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub backend: BackendKind,
    pub size: (i32, i32),
    pub startup: Option<String>,
}

/// Parses `WxH` into two positive integers.
fn _parse_size(text: &str) -> Result<(i32, i32), String> {
    let bad = || format!("bad --size '{text}', expected WxH with positive numbers");
    let (w, h) = text.split_once('x').ok_or_else(bad)?;
    let w: i32 = w.parse().map_err(|_| bad())?;
    let h: i32 = h.parse().map_err(|_| bad())?;
    if w <= 0 || h <= 0 {
        return Err(bad());
    }
    Ok((w, h))
}

/// Parses the arguments (without the program name).
///
/// Exactly one of `--nested` or `--headless` is required; `--size WxH` and
/// `--startup COMMAND` are optional.
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut backend: Option<BackendKind> = None;
    let mut size = DEFAULT_SIZE;
    let mut startup: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let kind = match arg.as_str() {
            "--nested" => Some(BackendKind::Nested),
            "--headless" => Some(BackendKind::Headless),
            _ => None,
        };
        if let Some(kind) = kind {
            if backend.is_some() {
                return Err("only one of --nested or --headless is allowed".into());
            }
            backend = Some(kind);
            continue;
        }
        match arg.as_str() {
            "--size" => size = _parse_size(it.next().ok_or("--size needs a value")?)?,
            "--startup" => startup = Some(it.next().ok_or("--startup needs a value")?.clone()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let backend = backend.ok_or("one of --nested or --headless is required")?;
    Ok(Args { backend, size, startup })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn nested_with_size_and_startup() {
        let a = parse_args(&s(&["--nested", "--size", "1280x800", "--startup", "sh /tmp/x"])).unwrap();
        assert_eq!(a.backend, BackendKind::Nested);
        assert_eq!(a.size, (1280, 800));
        assert_eq!(a.startup.as_deref(), Some("sh /tmp/x"));
    }

    #[test]
    fn headless_takes_the_default_size() {
        let a = parse_args(&s(&["--headless"])).unwrap();
        assert_eq!((a.backend, a.size, a.startup), (BackendKind::Headless, DEFAULT_SIZE, None));
    }

    #[test]
    fn a_backend_is_required_and_bad_values_fail() {
        assert!(parse_args(&s(&[])).unwrap_err().contains("--nested or --headless"));
        assert!(parse_args(&s(&["--nested", "--size", "big"])).is_err());
        assert!(parse_args(&s(&["--nested", "--size", "0x600"])).is_err());
        assert!(parse_args(&s(&["--nested", "--startup"])).is_err());
        assert!(parse_args(&s(&["--nested", "--headless"])).is_err());
        assert!(parse_args(&s(&["--nested", "--bogus"])).is_err());
    }
}
