//! The state of the add printer wizard: device, driver, name, done. Pure,
//! so every rule of the Next button is tested without a display.

use opusdm_printing::model::{Device, DriverChoice, NewPrinter};
use opusdm_printing::parse::{is_driverless_uri, suggest_queue_name, validate_queue_name};

/// The pages of the wizard, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Device,
    Driver,
    Name,
    Done,
}

/// What the user chose so far.
#[derive(Clone, Debug)]
pub struct Wizard {
    pub step: Step,
    pub devices: Vec<Device>,
    pub device: Option<Device>,
    pub manual_uri: String,
    pub driver: Option<DriverChoice>,
    pub driverless: bool,
    pub name: String,
    pub info: String,
    pub location: String,
    pub make_default: bool,
    taken: Vec<String>,
    configured: Vec<String>,
}

impl Wizard {
    /// A wizard for a system with the queues `taken`, whose devices are
    /// `configured_uris`.
    pub fn new(taken: Vec<String>, configured_uris: Vec<String>) -> Self {
        Wizard {
            step: Step::Device,
            devices: Vec::new(),
            device: None,
            manual_uri: String::new(),
            driver: None,
            driverless: false,
            name: String::new(),
            info: String::new(),
            location: String::new(),
            make_default: false,
            taken,
            configured: configured_uris,
        }
    }

    /// Whether a printer already uses the device `uri`.
    pub fn is_configured(&self, uri: &str) -> bool {
        self.configured.iter().any(|u| u == uri)
    }

    /// The user picked a device found on the network.
    pub fn choose_device(&mut self, device: Device) {
        let model = if device.make_model.is_empty() || device.make_model == "Unknown" { &device.info } else { &device.make_model };
        self.name = suggest_queue_name(model, &self.taken);
        self.info = device.info.clone();
        self.manual_uri.clear();
        self._set_driverless(is_driverless_uri(&device.uri));
        self.device = Some(device);
    }

    /// The user typed an address.
    pub fn choose_manual(&mut self, uri: &str) {
        self.device = None;
        self.manual_uri = uri.to_string();
        self.info.clear();
        self.name = suggest_queue_name("", &self.taken);
        self._set_driverless(is_driverless_uri(uri.trim()));
    }

    /// Follow whether the device works without a driver: driverless is
    /// preselected when it does, and dropped when it does not.
    fn _set_driverless(&mut self, driverless: bool) {
        self.driverless = driverless;
        if driverless { self.driver = Some(DriverChoice::Driverless); }
        if !driverless && self.driver == Some(DriverChoice::Driverless) { self.driver = None; }
    }

    /// The address of the new printer.
    pub fn uri(&self) -> Option<String> {
        if let Some(d) = &self.device { return Some(d.uri.clone()); }
        let typed = self.manual_uri.trim();
        typed.contains("://").then(|| typed.to_string())
    }

    /// Whether Next may go on from this step, with the reason when not.
    pub fn can_next(&self) -> Result<(), String> {
        match self.step {
            Step::Device if self.uri().is_none() => Err("Choose a printer or type its address".to_string()),
            Step::Driver if self.driver.is_none() => Err("Choose a driver".to_string()),
            Step::Name => {
                validate_queue_name(&self.name)?;
                if self.taken.iter().any(|t| t == &self.name) { return Err("A printer with this name exists".to_string()); }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Go to the next step, when allowed.
    pub fn next(&mut self) {
        if self.can_next().is_err() { return; }
        self.step = match self.step {
            Step::Device => Step::Driver,
            Step::Driver => Step::Name,
            Step::Name | Step::Done => Step::Done,
        };
    }

    /// Go back one step. The printer is added when Done is reached, so Done
    /// stays.
    pub fn back(&mut self) {
        self.step = match self.step {
            Step::Device | Step::Driver => Step::Device,
            Step::Name => Step::Driver,
            Step::Done => Step::Done,
        };
    }

    /// The printer to add, once the name is valid.
    pub fn request(&self) -> Option<NewPrinter> {
        if self.step != Step::Name || self.can_next().is_err() { return None; }
        Some(NewPrinter {
            name: self.name.clone(),
            uri: self.uri()?,
            info: self.info.clone(),
            location: self.location.clone(),
            driver: self.driver.clone()?,
            make_default: self.make_default,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bizhub as `lpinfo -l -v` shows it.
    fn bizhub() -> Device {
        Device {
            uri: "dnssd://KONICA%20MINOLTA%20bizhub%20C224e(98%3A1F%3A7C)._pdl-datastream._tcp.local/".into(),
            info: "KONICA MINOLTA bizhub C224e(98:1F:7C)".into(),
            make_model: "KONICA MINOLTA KONICA MINOLTA bizhub C224e".into(),
            device_id: "MFG:KONICA MINOLTA;MDL:C364Series;CMD:PS;".into(),
            class: "network".into(),
        }
    }

    /// A printer that speaks IPP.
    fn ipp_device() -> Device {
        Device { uri: "ipp://10.0.20.7/ipp/print".into(), info: "Brother HL-L2350DW".into(), make_model: "Brother HL-L2350DW series".into(), ..Default::default() }
    }

    #[test]
    fn bizhub_is_not_driverless_and_gets_a_name() {
        let mut w = Wizard::new(vec!["C364Series".into()], vec![bizhub().uri]);
        assert!(w.is_configured(&bizhub().uri));
        w.choose_device(bizhub());
        assert!(!w.driverless);
        assert_eq!(w.driver, None);
        assert_eq!(w.name, "KONICA_MINOLTA_bizhub_C224e");
        assert_eq!(w.info, "KONICA MINOLTA bizhub C224e(98:1F:7C)");
        assert_eq!(w.uri(), Some(bizhub().uri));
    }

    #[test]
    fn ipp_device_preselects_driverless() {
        let mut w = Wizard::new(Vec::new(), Vec::new());
        w.choose_device(ipp_device());
        assert!(w.driverless);
        assert_eq!(w.driver, Some(DriverChoice::Driverless));
        assert_eq!(w.name, "Brother_HL-L2350DW_series");
    }

    #[test]
    fn manual_uri_needs_a_scheme() {
        let mut w = Wizard::new(Vec::new(), Vec::new());
        w.choose_manual("10.0.20.7");
        assert_eq!(w.uri(), None);
        assert!(w.can_next().is_err());
        w.choose_manual("  socket://10.0.20.7:9100 ");
        assert_eq!(w.uri(), Some("socket://10.0.20.7:9100".into()));
        assert_eq!(w.device, None);
        assert!(!w.driverless);
        assert_eq!(w.name, "Printer");
        w.choose_manual("ipp://10.0.20.7/ipp/print");
        assert!(w.driverless);
    }

    #[test]
    fn next_is_blocked_with_a_reason() {
        let mut w = Wizard::new(Vec::new(), Vec::new());
        assert_eq!(w.can_next(), Err("Choose a printer or type its address".into()));
        w.next();
        assert_eq!(w.step, Step::Device);
        w.choose_device(bizhub());
        w.next();
        assert_eq!(w.step, Step::Driver);
        assert_eq!(w.can_next(), Err("Choose a driver".into()));
        w.driver = Some(DriverChoice::Model("drv:///x.ppd".into()));
        w.next();
        assert_eq!(w.step, Step::Name);
        w.name = "my printer".into();
        assert!(w.can_next().is_err());
        assert_eq!(w.request(), None);
        w.back();
        assert_eq!(w.step, Step::Driver);
        w.back();
        w.back();
        assert_eq!(w.step, Step::Device);
    }

    #[test]
    fn taken_names_are_refused() {
        let mut w = Wizard::new(vec!["KONICA_MINOLTA_bizhub_C224e".into()], Vec::new());
        w.choose_device(bizhub());
        assert_eq!(w.name, "KONICA_MINOLTA_bizhub_C224e_2", "the suggestion avoids taken names");
        w.driver = Some(DriverChoice::Driverless);
        w.step = Step::Name;
        w.name = "KONICA_MINOLTA_bizhub_C224e".into();
        assert_eq!(w.can_next(), Err("A printer with this name exists".into()));
    }

    #[test]
    fn request_from_a_full_wizard() {
        let mut w = Wizard::new(Vec::new(), Vec::new());
        w.choose_device(ipp_device());
        w.next();
        w.next();
        w.location = "Office".into();
        w.make_default = true;
        let r = w.request().unwrap();
        assert_eq!(r, NewPrinter { name: "Brother_HL-L2350DW_series".into(), uri: "ipp://10.0.20.7/ipp/print".into(), info: "Brother HL-L2350DW".into(), location: "Office".into(), driver: DriverChoice::Driverless, make_default: true });
        w.next();
        assert_eq!(w.step, Step::Done);
        w.next();
        assert_eq!(w.step, Step::Done);
    }
}
