//! The real backend: reads through IPP on the CUPS socket, changes through
//! the CUPS command line tools, which CUPS trusts through the same socket.

use crate::backend::PrintBackend;
use crate::ipp::{self, Request};
use crate::model::{Device, Driver, DriverChoice, Job, NewPrinter, PrintError, Printer, PrinterEdit, PrinterOption, JOB_ATTRS, PRINTER_ATTRS};
use crate::parse;
use crate::transport;
use std::process::Command;

/// The page printed by "Print test page".
pub const TEST_PAGE: &str = "/usr/share/cups/data/testprint";
/// Seconds `lpinfo -v` waits for the network backends.
pub const DISCOVERY_TIMEOUT_SECS: u32 = 10;

/// The CUPS of this machine.
#[derive(Clone, Debug, Default)]
pub struct CupsBackend;

impl CupsBackend {
    /// The backend of the local CUPS.
    pub fn new() -> Self {
        CupsBackend
    }

    /// Send an IPP request and decode the answer.
    fn ipp(&self, request: Request) -> Result<ipp::Response, PrintError> {
        let body = transport::post(&transport::socket_path(), &request.encode())?;
        ipp::decode(&body).map_err(PrintError::new)
    }

    /// The name of the default printer, if there is one.
    fn default_name(&self) -> Result<Option<String>, PrintError> {
        let r = self.ipp(Request::new(ipp::OP_CUPS_GET_DEFAULT).keywords("requested-attributes", &["printer-name"]))?;
        if r.status == ipp::STATUS_NOT_FOUND { return Ok(None); }
        if !r.is_ok() { return Err(_ipp_error("Reading the default printer", &r)); }
        let name = r.groups_of(ipp::TAG_PRINTER).find_map(|g| g.str("printer-name").map(str::to_string));
        Ok(name)
    }
}

/// The error of a refused IPP request.
fn _ipp_error(what: &str, r: &ipp::Response) -> PrintError {
    PrintError::new(format!("{what}: {}", r.status_message().map(str::to_string).unwrap_or_else(|| format!("CUPS status 0x{:04x}", r.status))))
}

/// The arguments of `lpadmin` that add `new`.
pub fn add_args(new: &NewPrinter) -> Vec<String> {
    let mut args: Vec<String> = ["-p", &new.name, "-E", "-v", &new.uri, "-D", &new.info, "-L", &new.location].iter().map(|s| s.to_string()).collect();
    match &new.driver {
        DriverChoice::Driverless => args.extend(["-m".to_string(), "everywhere".to_string()]),
        DriverChoice::Model(name) => args.extend(["-m".to_string(), name.clone()]),
        DriverChoice::PpdFile(path) => args.extend(["-P".to_string(), path.display().to_string()]),
    }
    args
}

/// The arguments of `lpadmin` that save `edit` on `printer`.
pub fn modify_args(printer: &str, edit: &PrinterEdit) -> Vec<String> {
    let mut args: Vec<String> = ["-p", printer, "-D", &edit.info, "-L", &edit.location].iter().map(|s| s.to_string()).collect();
    for (key, value) in &edit.options {
        args.push("-o".to_string());
        args.push(format!("{key}={value}"));
    }
    args
}

/// The text of a failed command: the tool and the first line it wrote.
pub fn command_error(tool: &str, stderr: &str, code: Option<i32>) -> PrintError {
    if let Some(line) = stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        let line = line.strip_prefix(&format!("{tool}:")).map(str::trim).unwrap_or(line);
        return PrintError::new(format!("{tool}: {line}"));
    }
    match code {
        Some(c) => PrintError::new(format!("{tool} failed (exit code {c})")),
        None => PrintError::new(format!("{tool} failed (killed)")),
    }
}

/// Run `tool` with `args` in the C locale; its standard output when it worked.
fn _run(tool: &str, args: &[String]) -> Result<String, PrintError> {
    let out = Command::new(tool)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .map_err(|e| PrintError::new(format!("{tool}: cannot run it: {e}")))?;
    if !out.status.success() { return Err(command_error(tool, &String::from_utf8_lossy(&out.stderr), out.status.code())); }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `_run` with string slices.
fn _run_str(tool: &str, args: &[&str]) -> Result<String, PrintError> {
    _run(tool, &args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
}

impl PrintBackend for CupsBackend {
    fn printers(&self) -> Result<Vec<Printer>, PrintError> {
        let r = self.ipp(Request::new(ipp::OP_CUPS_GET_PRINTERS).keywords("requested-attributes", PRINTER_ATTRS))?;
        if r.status == ipp::STATUS_NOT_FOUND { return Ok(Vec::new()); }
        if !r.is_ok() { return Err(_ipp_error("Reading the printers", &r)); }
        let default = self.default_name()?;
        let mut printers: Vec<Printer> = r.groups_of(ipp::TAG_PRINTER).filter_map(Printer::from_group).collect();
        for p in &mut printers { p.is_default = default.as_deref() == Some(p.name.as_str()); }
        Ok(printers)
    }

    fn jobs(&self) -> Result<Vec<Job>, PrintError> {
        let user = std::env::var("USER").unwrap_or_else(|_| "opusdm".to_string());
        let request = Request::new(ipp::OP_GET_JOBS)
            .uri("printer-uri", "ipp://localhost/")
            .name("requesting-user-name", &user)
            .keyword("which-jobs", "not-completed")
            .boolean("my-jobs", false)
            .keywords("requested-attributes", JOB_ATTRS);
        let r = self.ipp(request)?;
        if r.status == ipp::STATUS_NOT_FOUND { return Ok(Vec::new()); }
        if !r.is_ok() { return Err(_ipp_error("Reading the print queue", &r)); }
        Ok(r.groups_of(ipp::TAG_JOB).filter_map(Job::from_group).collect())
    }

    fn devices(&self) -> Result<Vec<Device>, PrintError> {
        let out = _run("lpinfo", &["-l".into(), "-v".into(), "--timeout".into(), DISCOVERY_TIMEOUT_SECS.to_string()])?;
        Ok(parse::parse_devices(&out))
    }

    fn drivers(&self) -> Result<Vec<Driver>, PrintError> {
        Ok(parse::parse_drivers(&_run_str("lpinfo", &["-m"])?))
    }

    fn options(&self, printer: &str) -> Result<Vec<PrinterOption>, PrintError> {
        Ok(parse::parse_options(&_run_str("lpoptions", &["-p", printer, "-l"])?))
    }

    fn add(&self, new: &NewPrinter) -> Result<(), PrintError> {
        _run("lpadmin", &add_args(new))?;
        if new.make_default { self.set_default(&new.name)?; }
        Ok(())
    }

    fn modify(&self, printer: &str, edit: &PrinterEdit) -> Result<(), PrintError> {
        _run("lpadmin", &modify_args(printer, edit)).map(|_| ())
    }

    fn remove(&self, printer: &str) -> Result<(), PrintError> {
        _run_str("lpadmin", &["-x", printer]).map(|_| ())
    }

    fn set_default(&self, printer: &str) -> Result<(), PrintError> {
        _run_str("lpadmin", &["-d", printer]).map(|_| ())
    }

    fn set_enabled(&self, printer: &str, on: bool) -> Result<(), PrintError> {
        if !on { return _run_str("cupsdisable", &[printer]).map(|_| ()); }
        _run_str("cupsenable", &[printer])?;
        _run_str("cupsaccept", &[printer]).map(|_| ())
    }

    fn test_page(&self, printer: &str) -> Result<(), PrintError> {
        _run_str("lp", &["-d", printer, "-t", "Test page", TEST_PAGE]).map(|_| ())
    }

    fn cancel(&self, job: i32) -> Result<(), PrintError> {
        _run("cancel", &[job.to_string()]).map(|_| ())
    }

    fn cancel_all(&self, printer: &str) -> Result<(), PrintError> {
        _run_str("cancel", &["-a", printer]).map(|_| ())
    }

    fn hold(&self, job: i32) -> Result<(), PrintError> {
        _run("lp", &["-i".into(), job.to_string(), "-H".into(), "hold".into()]).map(|_| ())
    }

    fn release(&self, job: i32) -> Result<(), PrintError> {
        _run("lp", &["-i".into(), job.to_string(), "-H".into(), "resume".into()]).map(|_| ())
    }

    fn retry(&self, printer: &str, job: Option<i32>) -> Result<(), PrintError> {
        _run_str("cupsenable", &[printer])?;
        let Some(id) = job else { return Ok(()) };
        _run("lp", &["-i".into(), id.to_string(), "-H".into(), "restart".into()]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A new printer with `driver`.
    fn new_printer(driver: DriverChoice) -> NewPrinter {
        NewPrinter { name: "p1".into(), uri: "socket://127.0.0.1:9".into(), info: "Office".into(), location: "Room 2".into(), driver, make_default: false }
    }

    #[test]
    fn add_arguments_per_driver() {
        let base = ["-p", "p1", "-E", "-v", "socket://127.0.0.1:9", "-D", "Office", "-L", "Room 2"];
        let with = |extra: &[&str]| base.iter().chain(extra).map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(add_args(&new_printer(DriverChoice::Driverless)), with(&["-m", "everywhere"]));
        assert_eq!(add_args(&new_printer(DriverChoice::Model("drv:///x.ppd".into()))), with(&["-m", "drv:///x.ppd"]));
        assert_eq!(add_args(&new_printer(DriverChoice::PpdFile(PathBuf::from("/tmp/a.ppd")))), with(&["-P", "/tmp/a.ppd"]));
    }

    #[test]
    fn modify_arguments() {
        let edit = PrinterEdit { info: "Desk".into(), location: "".into(), options: vec![("PageSize".into(), "A4".into()), ("KMDuplex".into(), "True".into())] };
        let args = modify_args("p1", &edit);
        assert_eq!(args, vec!["-p", "p1", "-D", "Desk", "-L", "", "-o", "PageSize=A4", "-o", "KMDuplex=True"]);
    }

    #[test]
    fn command_errors_name_the_tool() {
        assert_eq!(command_error("lpadmin", "lpadmin: Unable to connect\nmore\n", Some(1)).0, "lpadmin: Unable to connect");
        assert_eq!(command_error("cancel", "cancel: client-error-not-found\n", Some(1)).0, "cancel: client-error-not-found");
        assert_eq!(command_error("lp", "", Some(2)).0, "lp failed (exit code 2)");
        assert_eq!(command_error("lp", "", None).0, "lp failed (killed)");
    }

    /// Runs against the real CUPS of the machine with a probe queue that is
    /// removed at the end: `cargo test -p opusdm-printing -- --ignored`.
    #[test]
    #[ignore]
    fn live_probe_queue() {
        let cups = CupsBackend::new();
        let name = "opusdm_probe_test";
        let new = NewPrinter { name: name.into(), uri: "socket://127.0.0.1:9".into(), info: "OpusDM probe".into(), location: "Nowhere".into(), driver: DriverChoice::Model("drv:///cupsfilters.drv/pwgrast.ppd".into()), make_default: false };
        cups.add(&new).unwrap();
        let result = std::panic::catch_unwind(|| {
            let p = cups.printers().unwrap().into_iter().find(|p| p.name == name).expect("the probe is listed");
            assert_eq!(p.info, "OpusDM probe");
            assert_eq!(p.location, "Nowhere");
            assert!(!p.is_default);
            assert!(!cups.options(name).unwrap().is_empty());
            cups.modify(name, &PrinterEdit { info: "OpusDM probe 2".into(), location: "Here".into(), options: Vec::new() }).unwrap();
            assert_eq!(cups.printers().unwrap().into_iter().find(|p| p.name == name).unwrap().info, "OpusDM probe 2");
            cups.set_enabled(name, false).unwrap();
            cups.test_page(name).unwrap();
            let job = cups.jobs().unwrap().into_iter().find(|j| j.printer == name).expect("the test page is queued");
            cups.hold(job.id).unwrap();
            cups.release(job.id).unwrap();
            cups.cancel(job.id).unwrap();
            assert!(!cups.jobs().unwrap().iter().any(|j| j.id == job.id));
        });
        cups.remove(name).unwrap();
        assert!(!cups.printers().unwrap().iter().any(|p| p.name == name));
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}
