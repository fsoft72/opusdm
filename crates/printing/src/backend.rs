//! What the printer programs ask CUPS, as a trait so the windows and the
//! panel can be tested with [`FakeBackend`].

use crate::model::{Device, Driver, Job, JobState, NewPrinter, PrintError, Printer, PrinterEdit, PrinterOption, PrinterState};
use std::sync::Mutex;

/// The CUPS calls. Every method blocks: call them off the GTK thread.
pub trait PrintBackend: Send + Sync {
    /// Every printer, with the default one marked.
    fn printers(&self) -> Result<Vec<Printer>, PrintError>;
    /// The jobs not completed yet, of every printer.
    fn jobs(&self) -> Result<Vec<Job>, PrintError>;
    /// The devices found on the network and the local ports (slow: seconds).
    fn devices(&self) -> Result<Vec<Device>, PrintError>;
    /// Every driver CUPS knows (slow: seconds).
    fn drivers(&self) -> Result<Vec<Driver>, PrintError>;
    /// The default job options of `printer`.
    fn options(&self, printer: &str) -> Result<Vec<PrinterOption>, PrintError>;
    /// Add and enable a printer.
    fn add(&self, new: &NewPrinter) -> Result<(), PrintError>;
    /// Save the description, location and options of `printer`.
    fn modify(&self, printer: &str, edit: &PrinterEdit) -> Result<(), PrintError>;
    /// Remove `printer` (its jobs are canceled).
    fn remove(&self, printer: &str) -> Result<(), PrintError>;
    /// Make `printer` the default of the system.
    fn set_default(&self, printer: &str) -> Result<(), PrintError>;
    /// Enable (and accept jobs) or disable `printer`.
    fn set_enabled(&self, printer: &str, on: bool) -> Result<(), PrintError>;
    /// Print the CUPS test page on `printer`.
    fn test_page(&self, printer: &str) -> Result<(), PrintError>;
    /// Cancel the job `job`.
    fn cancel(&self, job: i32) -> Result<(), PrintError>;
    /// Cancel every job of `printer`.
    fn cancel_all(&self, printer: &str) -> Result<(), PrintError>;
    /// Hold the job `job`.
    fn hold(&self, job: i32) -> Result<(), PrintError>;
    /// Release the held job `job`.
    fn release(&self, job: i32) -> Result<(), PrintError>;
    /// Enable `printer` again and, for a stopped job, restart it.
    fn retry(&self, printer: &str, job: Option<i32>) -> Result<(), PrintError>;
}

/// A backend in memory, for the tests: every call is recorded, and `fail`
/// (or `fail_once`, for the next call only) makes the calls fail.
#[derive(Default)]
pub struct FakeBackend {
    pub printers: Mutex<Vec<Printer>>,
    pub jobs: Mutex<Vec<Job>>,
    pub devices: Mutex<Vec<crate::model::Device>>,
    pub drivers: Mutex<Vec<Driver>>,
    pub options: Mutex<Vec<PrinterOption>>,
    pub fail: Mutex<Option<PrintError>>,
    pub fail_once: Mutex<Option<PrintError>>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    /// The calls made so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Record `call`, then fail when asked to.
    fn call(&self, call: String) -> Result<(), PrintError> {
        self.calls.lock().unwrap().push(call);
        if let Some(e) = self.fail_once.lock().unwrap().take() { return Err(e); }
        match self.fail.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Change the job `id` with `change`.
    fn change_job(&self, id: i32, change: impl FnOnce(&mut Job)) {
        if let Some(j) = self.jobs.lock().unwrap().iter_mut().find(|j| j.id == id) { change(j); }
    }

    /// Change the printer `name` with `change`.
    fn change_printer(&self, name: &str, change: impl FnOnce(&mut Printer)) {
        if let Some(p) = self.printers.lock().unwrap().iter_mut().find(|p| p.name == name) { change(p); }
    }
}

impl PrintBackend for FakeBackend {
    fn printers(&self) -> Result<Vec<Printer>, PrintError> {
        self.call("printers".into())?;
        Ok(self.printers.lock().unwrap().clone())
    }

    fn jobs(&self) -> Result<Vec<Job>, PrintError> {
        self.call("jobs".into())?;
        Ok(self.jobs.lock().unwrap().clone())
    }

    fn devices(&self) -> Result<Vec<Device>, PrintError> {
        self.call("devices".into())?;
        Ok(self.devices.lock().unwrap().clone())
    }

    fn drivers(&self) -> Result<Vec<Driver>, PrintError> {
        self.call("drivers".into())?;
        Ok(self.drivers.lock().unwrap().clone())
    }

    fn options(&self, printer: &str) -> Result<Vec<PrinterOption>, PrintError> {
        self.call(format!("options {printer}"))?;
        Ok(self.options.lock().unwrap().clone())
    }

    fn add(&self, new: &NewPrinter) -> Result<(), PrintError> {
        self.call(format!("add {}", new.name))?;
        let mut p = test_printer(&new.name);
        p.device_uri = new.uri.clone();
        p.info = new.info.clone();
        p.location = new.location.clone();
        self.printers.lock().unwrap().push(p);
        if new.make_default { self.set_default(&new.name)?; }
        Ok(())
    }

    fn modify(&self, printer: &str, edit: &PrinterEdit) -> Result<(), PrintError> {
        self.call(format!("modify {printer}"))?;
        self.change_printer(printer, |p| {
            p.info = edit.info.clone();
            p.location = edit.location.clone();
        });
        let mut options = self.options.lock().unwrap();
        for (key, value) in &edit.options {
            if let Some(o) = options.iter_mut().find(|o| &o.key == key) { o.default = value.clone(); }
        }
        Ok(())
    }

    fn remove(&self, printer: &str) -> Result<(), PrintError> {
        self.call(format!("remove {printer}"))?;
        self.printers.lock().unwrap().retain(|p| p.name != printer);
        self.jobs.lock().unwrap().retain(|j| j.printer != printer);
        Ok(())
    }

    fn set_default(&self, printer: &str) -> Result<(), PrintError> {
        self.call(format!("default {printer}"))?;
        for p in self.printers.lock().unwrap().iter_mut() { p.is_default = p.name == printer; }
        Ok(())
    }

    fn set_enabled(&self, printer: &str, on: bool) -> Result<(), PrintError> {
        self.call(format!("enable {printer} {on}"))?;
        self.change_printer(printer, |p| p.state = if on { PrinterState::Idle } else { PrinterState::Stopped });
        Ok(())
    }

    fn test_page(&self, printer: &str) -> Result<(), PrintError> {
        self.call(format!("test page {printer}"))?;
        let mut jobs = self.jobs.lock().unwrap();
        let id = jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        jobs.push(test_job(id, printer));
        Ok(())
    }

    fn cancel(&self, job: i32) -> Result<(), PrintError> {
        self.call(format!("cancel {job}"))?;
        self.jobs.lock().unwrap().retain(|j| j.id != job);
        Ok(())
    }

    fn cancel_all(&self, printer: &str) -> Result<(), PrintError> {
        self.call(format!("cancel all {printer}"))?;
        self.jobs.lock().unwrap().retain(|j| j.printer != printer);
        Ok(())
    }

    fn hold(&self, job: i32) -> Result<(), PrintError> {
        self.call(format!("hold {job}"))?;
        self.change_job(job, |j| j.state = JobState::Held);
        Ok(())
    }

    fn release(&self, job: i32) -> Result<(), PrintError> {
        self.call(format!("release {job}"))?;
        self.change_job(job, |j| j.state = JobState::Pending);
        Ok(())
    }

    fn retry(&self, printer: &str, job: Option<i32>) -> Result<(), PrintError> {
        self.call(format!("retry {printer} {job:?}"))?;
        self.change_printer(printer, |p| p.state = PrinterState::Idle);
        if let Some(id) = job { self.change_job(id, |j| j.state = JobState::Pending); }
        Ok(())
    }
}

/// A printer named `name`, ready, for the tests of this crate and others.
pub fn test_printer(name: &str) -> Printer {
    Printer {
        name: name.to_string(),
        info: format!("{name} info"),
        location: String::new(),
        make_model: "Generic".to_string(),
        device_uri: "socket://127.0.0.1:9".to_string(),
        state_message: String::new(),
        state: PrinterState::Idle,
        reasons: vec!["none".to_string()],
        accepting: true,
        is_default: false,
    }
}

/// A waiting job `id` of `printer`, for the tests.
pub fn test_job(id: i32, printer: &str) -> Job {
    Job { id, printer: printer.to_string(), name: format!("doc{id}.pdf"), user: "fabio".to_string(), state: JobState::Pending, reasons: Vec::new(), created: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DriverChoice;

    #[test]
    fn fake_records_calls_and_fails() {
        let fake = FakeBackend::default();
        fake.jobs.lock().unwrap().push(test_job(41, "p"));
        let new = NewPrinter { name: "p".into(), uri: "socket://h".into(), info: String::new(), location: String::new(), driver: DriverChoice::Driverless, make_default: false };
        fake.add(&new).unwrap();
        assert_eq!(fake.printers().unwrap().len(), 1);
        fake.cancel(41).unwrap();
        assert!(fake.jobs().unwrap().is_empty());
        assert_eq!(fake.calls(), vec!["add p", "printers", "cancel 41", "jobs"]);
        *fake.fail_once.lock().unwrap() = Some(PrintError::new("once"));
        assert_eq!(fake.printers().unwrap_err().0, "once");
        assert!(fake.printers().is_ok());
        *fake.fail.lock().unwrap() = Some(PrintError::new("down"));
        assert_eq!(fake.jobs().unwrap_err().0, "down");
        assert_eq!(fake.jobs().unwrap_err().0, "down");
    }
}
