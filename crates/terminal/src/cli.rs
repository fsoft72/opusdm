//! Command line of `opusdm-terminal`.

use std::path::PathBuf;

/// Usage text printed by `--help` and after a bad argument.
pub const USAGE: &str = "Usage: opusdm-terminal [-d DIR] [-e COMMAND | -x PROGRAM [ARGS...]]
  -d, --working-directory DIR   start the shell in DIR
  -e, --command COMMAND         run COMMAND through the shell instead of a shell
  -x, --execute PROGRAM ARGS    run PROGRAM with the remaining arguments
  -h, --help                    show this help";

/// What the terminal was asked to start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Directory the first shell starts in; the current one when missing.
    pub working_directory: Option<PathBuf>,
    /// Program and arguments to run instead of the user's shell.
    pub command: Option<Vec<String>>,
    /// Whether only the help was asked for.
    pub help: bool,
}

/// Parse the arguments after the program name.
pub fn parse_args(args: &[String]) -> Result<LaunchOptions, String> {
    let mut options = LaunchOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut value = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg {
            "-h" | "--help" => options.help = true,
            "-d" | "--working-directory" => options.working_directory = Some(PathBuf::from(value(arg)?)),
            "-e" | "--command" => {
                let shell = crate::pane::detect_user_shell();
                options.command = Some(vec![shell, "-c".into(), value(arg)?]);
            }
            "-x" | "--execute" => {
                let rest = args[i + 1..].to_vec();
                if rest.is_empty() { return Err(format!("{arg} needs a program")); }
                options.command = Some(rest);
                break;
            }
            _ => match arg.strip_prefix("--working-directory=") {
                Some(dir) => options.working_directory = Some(PathBuf::from(dir)),
                None => return Err(format!("unknown argument {arg:?}")),
            },
        }
        i += 1;
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_give_a_plain_shell() {
        assert_eq!(parse_args(&[]), Ok(LaunchOptions::default()));
    }

    #[test]
    fn the_working_directory_is_read_in_both_forms() {
        let short = parse_args(&args(&["-d", "/tmp"])).unwrap();
        let long = parse_args(&args(&["--working-directory=/tmp"])).unwrap();
        assert_eq!(short.working_directory, Some(PathBuf::from("/tmp")));
        assert_eq!(long, short);
    }

    #[test]
    fn execute_takes_every_remaining_argument() {
        let options = parse_args(&args(&["-d", "/", "-x", "htop", "-d", "5"])).unwrap();
        assert_eq!(options.command, Some(args(&["htop", "-d", "5"])));
        assert_eq!(options.working_directory, Some(PathBuf::from("/")));
    }

    #[test]
    fn command_runs_through_the_shell() {
        let options = parse_args(&args(&["-e", "ls -l"])).unwrap();
        let command = options.command.unwrap();
        assert_eq!(&command[1..], &args(&["-c", "ls -l"])[..]);
    }

    #[test]
    fn bad_arguments_are_reported() {
        assert!(parse_args(&args(&["-d"])).is_err());
        assert!(parse_args(&args(&["-x"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }
}
