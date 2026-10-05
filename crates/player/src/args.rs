//! Command line of `opusdm-player`.

/// Usage text, shown on a bad command line.
pub const USAGE: &str = "usage: opusdm-player [--loop] <file or URL>";

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Path or URL of the file to play.
    pub target: String,
    /// Start again at the end.
    pub looping: bool,
}

/// Parse the arguments after the program name: one target and an optional `--loop`.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut target = None;
    let mut looping = false;
    for arg in args {
        if arg == "--loop" { looping = true; continue; }
        if arg.starts_with("--") { return Err(format!("unknown option {arg}\n{USAGE}")); }
        if target.replace(arg).is_some() { return Err(format!("only one file can be played\n{USAGE}")); }
    }
    let target = target.ok_or_else(|| USAGE.to_string())?;
    Ok(Args { target, looping })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Args, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn a_file_alone() {
        assert_eq!(p(&["a.mp3"]).unwrap(), Args { target: "a.mp3".into(), looping: false });
    }

    #[test]
    fn loop_flag_in_any_position() {
        assert!(p(&["--loop", "a.mp3"]).unwrap().looping);
        assert!(p(&["a.mp3", "--loop"]).unwrap().looping);
    }

    #[test]
    fn errors_carry_the_usage() {
        assert_eq!(p(&[]).unwrap_err(), USAGE);
        assert!(p(&["a", "b"]).unwrap_err().contains("only one file"));
        assert!(p(&["--x", "a"]).unwrap_err().contains("unknown option --x"));
    }
}
