//! opusdm-compositor: the native Wayland compositor of OpusDM.

mod args;
mod backend;
// Used by the backends from Task 3 on.
#[allow(dead_code)]
mod child;
// Used by the backends from Task 3 on.
#[allow(dead_code)]
mod geometry;

use std::process::ExitCode;

const USAGE: &str = "usage: opusdm-compositor --nested|--headless [--size WxH] [--startup COMMAND]";

/// Initializes logging to stderr: `warn`, or `debug` when `OPUSDM_DEBUG` is set.
fn init_logging() {
    let level = if std::env::var_os("OPUSDM_DEBUG").is_some() { "debug" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::new(level);
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

/// Parses the arguments, starts logging and runs the selected backend.
fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match args::parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("opusdm-compositor: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    init_logging();
    if let Err(e) = backend::run(&args) {
        opusdm_ipc::err_log!("opusdm-compositor", "{e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
