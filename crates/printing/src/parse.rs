//! Parsers of what the CUPS command line tools print (run with `LC_ALL=C`),
//! and the rules for queue names.

use crate::model::{Device, Driver, PrinterOption};

/// The options shown first in the Options tab, in this order.
pub const COMMON_OPTION_KEYS: &[&str] = &[
    "PageSize",
    "Duplex",
    "KMDuplex",
    "ColorModel",
    "SelectColor",
    "print-color-mode",
    "cupsPrintQuality",
    "print-quality",
    "Resolution",
    "InputSlot",
    "MediaType",
];

/// Longest queue name CUPS accepts, in bytes.
pub const MAX_QUEUE_NAME: usize = 127;

/// The devices of `lpinfo -l -v`. The bare scheme entries (`ipp`, `socket`,
/// ...) are the kinds of manual address, not devices: they are left out.
pub fn parse_devices(out: &str) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for line in out.lines() {
        if let Some(uri) = line.strip_prefix("Device: uri = ") {
            devices.push(Device { uri: uri.trim().to_string(), ..Default::default() });
            continue;
        }
        let (Some(device), Some((key, value))) = (devices.last_mut(), line.split_once(" = ")) else { continue };
        let value = value.trim().to_string();
        match key.trim() {
            "class" => device.class = value,
            "info" => device.info = value,
            "make-and-model" => device.make_model = value,
            "device-id" => device.device_id = value,
            _ => {}
        }
    }
    devices.retain(|d| d.uri.contains("://"));
    devices
}

/// The drivers of `lpinfo -m`: the name, a space, the description.
pub fn parse_drivers(out: &str) -> Vec<Driver> {
    out.lines()
        .filter_map(|line| {
            let (name, description) = line.trim().split_once(' ')?;
            Some(Driver { name: name.to_string(), description: description.trim().to_string() })
        })
        .collect()
}

/// The drivers whose description holds every word of `query` (case
/// insensitive), at most `limit`. An empty query gives the first `limit`.
pub fn filter_drivers<'a>(drivers: &'a [Driver], query: &str, limit: usize) -> Vec<&'a Driver> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    drivers
        .iter()
        .filter(|d| {
            let description = d.description.to_lowercase();
            words.iter().all(|w| description.contains(w.as_str()))
        })
        .take(limit)
        .collect()
}

/// The options of `lpoptions -p NAME -l`: `Key/Label: c1 *c2 c3`, the starred
/// choice is the default (the first one when none is starred).
pub fn parse_options(out: &str) -> Vec<PrinterOption> {
    out.lines()
        .filter_map(|line| {
            let (head, choices) = line.split_once(": ")?;
            let (key, label) = head.split_once('/').unwrap_or((head, head));
            let mut default = None;
            let choices: Vec<String> = choices
                .split_whitespace()
                .map(|c| match c.strip_prefix('*') {
                    Some(starred) => {
                        default = Some(starred.to_string());
                        starred.to_string()
                    }
                    None => c.to_string(),
                })
                .collect();
            let default = default.or_else(|| choices.first().cloned()).unwrap_or_default();
            Some(PrinterOption { key: key.to_string(), label: label.to_string(), choices, default })
        })
        .collect()
}

/// The common options first (in the order of [`COMMON_OPTION_KEYS`]), then
/// the others in their order.
pub fn split_options(options: &[PrinterOption]) -> (Vec<&PrinterOption>, Vec<&PrinterOption>) {
    let common = COMMON_OPTION_KEYS.iter().filter_map(|k| options.iter().find(|o| o.key == *k)).collect();
    let rest = options.iter().filter(|o| !COMMON_OPTION_KEYS.contains(&o.key.as_str())).collect();
    (common, rest)
}

/// Whether `name` can be a CUPS queue name, with the reason when not.
pub fn validate_queue_name(name: &str) -> Result<(), String> {
    if name.is_empty() { return Err("The name is empty".to_string()); }
    if name.len() > MAX_QUEUE_NAME { return Err(format!("The name is longer than {MAX_QUEUE_NAME} characters")); }
    if !name.chars().all(_is_name_char) { return Err("The name cannot contain spaces, /, #, \\ or quotes".to_string()); }
    Ok(())
}

/// Whether `c` may appear in a queue name.
fn _is_name_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '/' | '#' | '\\' | '\'' | '"')
}

/// A valid queue name made from `make_model`, not in `taken`.
pub fn suggest_queue_name(make_model: &str, taken: &[String]) -> String {
    let mut words: Vec<&str> = make_model.split_whitespace().collect();
    // "KONICA MINOLTA KONICA MINOLTA bizhub": the maker is written twice.
    if let Some(k) = (1..=words.len() / 2).find(|&k| words[..k] == words[k..2 * k]) {
        words.drain(..k);
    }
    let mut base = String::new();
    for c in words.join("_").chars() {
        // Only letters, digits, - and . are kept: a valid name, easy to type.
        let c = if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' };
        if c == '_' && base.ends_with('_') { continue; }
        base.push(c);
    }
    let mut base = base.trim_matches('_').to_string();
    if base.is_empty() { base = "Printer".to_string(); }
    base.truncate(MAX_QUEUE_NAME - 4);
    if !taken.contains(&base) { return base; }
    (2..).map(|n| format!("{base}_{n}")).find(|n| !taken.contains(n)).unwrap_or(base)
}

/// Whether the device at `uri` can be used without a driver (IPP Everywhere).
pub fn is_driverless_uri(uri: &str) -> bool {
    if uri.starts_with("ipp://") || uri.starts_with("ipps://") { return true; }
    uri.starts_with("dnssd://") && (uri.contains("._ipp._tcp") || uri.contains("._ipps._tcp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Part of `LC_ALL=C lpinfo -l -v` on the owner's machine.
    const DEVICES: &str = "Device: uri = ipp
        class = network
        info = Internet Printing Protocol (ipp)
        make-and-model = Unknown
        device-id =
        location =
Device: uri = dnssd://KONICA%20MINOLTA%20bizhub%20C224e(98%3A1F%3A7C)._pdl-datastream._tcp.local/
        class = network
        info = KONICA MINOLTA bizhub C224e(98:1F:7C)
        make-and-model = KONICA MINOLTA KONICA MINOLTA bizhub C224e
        device-id = MFG:KONICA MINOLTA;MDL:C364Series;CMD:PS;
        location =
";

    const DRIVERS: &str = "drv:///cupsfilters.drv/pwgrast.ppd Generic IPP Everywhere Printer
openprinting-ppds:0/ppd/openprinting/KONICA_MINOLTA/KO500UX.ppd KONICA MINOLTA 500/420/360PS(P)
openprinting-ppds:0/ppd/openprinting/KONICA_MINOLTA/KOC250UX.ppd KONICA MINOLTA C250 PS(P)
";

    const OPTIONS: &str = "PageSize/Paper Size: A3 *A4 A5
KMDuplex/Duplex: *False True
Resolution/Resolution: *600dpi
";

    /// An option with `key` and no choices.
    fn opt(key: &str) -> PrinterOption {
        PrinterOption { key: key.into(), label: key.into(), choices: Vec::new(), default: String::new() }
    }

    #[test]
    fn devices_from_lpinfo() {
        let d = parse_devices(DEVICES);
        assert_eq!(d.len(), 1);
        assert!(d[0].uri.starts_with("dnssd://KONICA"));
        assert_eq!(d[0].info, "KONICA MINOLTA bizhub C224e(98:1F:7C)");
        assert_eq!(d[0].make_model, "KONICA MINOLTA KONICA MINOLTA bizhub C224e");
        assert_eq!(d[0].device_id, "MFG:KONICA MINOLTA;MDL:C364Series;CMD:PS;");
        assert_eq!(d[0].class, "network");
    }

    #[test]
    fn drivers_and_filter() {
        let d = parse_drivers(DRIVERS);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].name, "drv:///cupsfilters.drv/pwgrast.ppd");
        assert_eq!(d[0].description, "Generic IPP Everywhere Printer");
        let found = filter_drivers(&d, "konica c250", 10);
        assert_eq!(found.len(), 1);
        assert!(found[0].name.ends_with("KOC250UX.ppd"));
        assert_eq!(filter_drivers(&d, "", 2).len(), 2);
        assert!(filter_drivers(&d, "epson", 10).is_empty());
    }

    #[test]
    fn options_from_lpoptions() {
        let o = parse_options(OPTIONS);
        assert_eq!(o.len(), 3);
        assert_eq!((o[0].key.as_str(), o[0].label.as_str(), o[0].default.as_str()), ("PageSize", "Paper Size", "A4"));
        assert_eq!(o[0].choices, vec!["A3", "A4", "A5"]);
        assert_eq!(o[1].default, "False");
        assert_eq!(o[2].default, "600dpi");
        let none_starred = parse_options("Fold/Fold: None Half\n");
        assert_eq!(none_starred[0].default, "None");
    }

    #[test]
    fn options_are_split_common_first() {
        let input = vec![opt("Staple"), opt("KMDuplex"), opt("PageSize"), opt("Finisher")];
        let (common, rest) = split_options(&input);
        let keys = |v: &[&PrinterOption]| v.iter().map(|o| o.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&common), vec!["PageSize", "KMDuplex"]);
        assert_eq!(keys(&rest), vec!["Staple", "Finisher"]);
    }

    #[test]
    fn queue_names() {
        assert!(validate_queue_name("C364Series").is_ok());
        assert!(validate_queue_name("KONICA_MINOLTA_bizhub_C224e").is_ok());
        assert_eq!(validate_queue_name(""), Err("The name is empty".to_string()));
        assert_eq!(validate_queue_name(&"a".repeat(128)), Err("The name is longer than 127 characters".to_string()));
        assert!(validate_queue_name(&"a".repeat(127)).is_ok());
        for bad in ["my printer", "a/b", "a#b", "a\\b", "a'b", "a\"b", "città"] {
            assert_eq!(validate_queue_name(bad), Err("The name cannot contain spaces, /, #, \\ or quotes".to_string()), "{bad}");
        }
    }

    #[test]
    fn suggested_names() {
        let name = suggest_queue_name("KONICA MINOLTA KONICA MINOLTA bizhub C224e", &[]);
        assert_eq!(name, "KONICA_MINOLTA_bizhub_C224e");
        assert!(validate_queue_name(&name).is_ok());
        assert_eq!(suggest_queue_name("KONICA MINOLTA bizhub C224e", std::slice::from_ref(&name)), "KONICA_MINOLTA_bizhub_C224e_2");
        assert_eq!(suggest_queue_name("", &[]), "Printer");
        assert_eq!(suggest_queue_name("HP LaserJet (A/B)", &[]), "HP_LaserJet_A_B");
    }

    #[test]
    fn driverless_uris() {
        assert!(is_driverless_uri("ipp://10.0.20.5/ipp/print"));
        assert!(is_driverless_uri("ipps://host/ipp/print"));
        assert!(is_driverless_uri("dnssd://Brother%20HL._ipp._tcp.local/"));
        assert!(is_driverless_uri("dnssd://Brother%20HL._ipps._tcp.local/"));
        assert!(!is_driverless_uri("dnssd://KONICA%20MINOLTA%20bizhub%20C224e(98%3A1F%3A7C)._pdl-datastream._tcp.local/"));
        assert!(!is_driverless_uri("socket://127.0.0.1:9"));
    }
}
