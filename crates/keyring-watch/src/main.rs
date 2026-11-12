//! `opusdm-keyring-watch`: tells the user when the keyring is locked or gone.
//!
//! No arguments: the watcher (started by the hub). `--unlock`: the password
//! window, also the way out when there is no tray. `--ask-password
//! unlock|restart`: the same window, started by the watcher.

use opusdm_keyring_watch::app::{self, Mode};

/// Exit code for a command line that means nothing.
const EXIT_USAGE: i32 = 2;

/// The usage text.
const USAGE: &str = "usage: opusdm-keyring-watch [--unlock | --ask-password unlock|restart]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match app::parse_args(&refs) {
        Mode::Watch => app::run(),
        Mode::Ask(mode) => opusdm_keyring_watch::ask::run(mode),
        Mode::Invalid => {
            eprintln!("{USAGE}");
            EXIT_USAGE
        }
    };
    std::process::exit(code);
}
