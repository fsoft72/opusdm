//! The draft of the fields of one printer: description, location and the
//! default options, as saved and as edited.

use opusdm_printing::model::{Printer, PrinterEdit, PrinterOption};

/// What the General and Options tabs edit.
#[derive(Clone, Debug, PartialEq)]
pub struct EditSession {
    pub printer: String,
    saved_info: String,
    saved_location: String,
    saved_options: Vec<(String, String)>,
    pub info: String,
    pub location: String,
    pub options: Vec<(String, String)>,
}

impl EditSession {
    /// A clean session for `p`; the options come later with [`Self::set_options`].
    pub fn new(p: &Printer) -> Self {
        EditSession {
            printer: p.name.clone(),
            saved_info: p.info.clone(),
            saved_location: p.location.clone(),
            saved_options: Vec::new(),
            info: p.info.clone(),
            location: p.location.clone(),
            options: Vec::new(),
        }
    }

    /// The options read from CUPS: saved and current values become their
    /// defaults. Edits of the description and location are kept.
    pub fn set_options(&mut self, options: &[PrinterOption]) {
        self.saved_options = options.iter().map(|o| (o.key.clone(), o.default.clone())).collect();
        self.options = self.saved_options.clone();
    }

    /// The current value of option `key`.
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Change option `key` to `value`.
    pub fn set_option(&mut self, key: &str, value: &str) {
        match self.options.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value.to_string(),
            None => self.options.push((key.to_string(), value.to_string())),
        }
    }

    /// Whether something differs from what is saved.
    pub fn is_dirty(&self) -> bool {
        self.info != self.saved_info || self.location != self.saved_location || self.options != self.saved_options
    }

    /// The changes to save; only the options that changed.
    pub fn edit(&self) -> PrinterEdit {
        let options = self.options.iter().filter(|o| !self.saved_options.contains(o)).cloned().collect();
        PrinterEdit { info: self.info.clone(), location: self.location.clone(), options }
    }

    /// Go back to what is saved.
    pub fn revert(&mut self) {
        self.info = self.saved_info.clone();
        self.location = self.saved_location.clone();
        self.options = self.saved_options.clone();
    }

    /// What is edited is now saved.
    pub fn mark_saved(&mut self) {
        self.saved_info = self.info.clone();
        self.saved_location = self.location.clone();
        self.saved_options = self.options.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_printing::backend::test_printer;

    /// The options of a small driver.
    fn options() -> Vec<PrinterOption> {
        vec![
            PrinterOption { key: "PageSize".into(), label: "Paper Size".into(), choices: vec!["A4".into(), "A3".into()], default: "A4".into() },
            PrinterOption { key: "KMDuplex".into(), label: "Duplex".into(), choices: vec!["False".into(), "True".into()], default: "False".into() },
        ]
    }

    /// A session of the test printer with its options read.
    fn session() -> EditSession {
        let mut s = EditSession::new(&test_printer("p"));
        s.set_options(&options());
        s
    }

    #[test]
    fn new_session_is_clean() {
        let s = session();
        assert!(!s.is_dirty());
        assert_eq!(s.info, "p info");
        assert_eq!(s.option("KMDuplex"), Some("False"));
        assert_eq!(s.edit().options, Vec::<(String, String)>::new());
    }

    #[test]
    fn changed_option_makes_dirty_and_edit_lists_only_it() {
        let mut s = session();
        s.set_option("KMDuplex", "True");
        assert!(s.is_dirty());
        let e = s.edit();
        assert_eq!(e.options, vec![("KMDuplex".to_string(), "True".to_string())]);
        assert_eq!(e.info, "p info");
        s.set_option("KMDuplex", "False");
        assert!(!s.is_dirty());
    }

    #[test]
    fn revert_and_mark_saved() {
        let mut s = session();
        s.info = "Desk".into();
        s.set_option("PageSize", "A3");
        s.revert();
        assert!(!s.is_dirty());
        assert_eq!(s.info, "p info");
        assert_eq!(s.option("PageSize"), Some("A4"));
        s.location = "Room 2".into();
        s.set_option("PageSize", "A3");
        s.mark_saved();
        assert!(!s.is_dirty());
        assert_eq!(s.location, "Room 2");
        assert!(s.edit().options.is_empty());
    }

    #[test]
    fn options_loaded_after_info_edit_keep_the_edit() {
        let mut s = EditSession::new(&test_printer("p"));
        s.info = "Desk".into();
        s.set_options(&options());
        assert_eq!(s.info, "Desk");
        assert!(s.is_dirty());
        assert!(s.edit().options.is_empty());
    }
}
