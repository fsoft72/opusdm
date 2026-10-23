//! The widgets of the three tabs of a printer: General, Options and Queue.
//! They hold no state: the window fills them and reads them.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_printing::model::{reason_severity, reason_words, Job, Printer, PrinterOption, Severity};
use opusdm_printing::parse::split_options;
use opusdm_ui::{labeled_row, LabelWidth};

/// Pixels between widgets.
pub const SPACING: i32 = 8;
/// Pixels around a page.
pub const MARGIN: i32 = 12;
/// Width of the labels of the General rows, in characters.
const LABEL_CHARS: i32 = 12;
/// Width of the labels of the option rows, in characters.
const OPTION_LABEL_CHARS: i32 = 22;
/// The choice shown for an option without a default.
const NOT_SET: &str = "(not set)";

/// A page with the usual margins.
fn page() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    b.set_margin_top(MARGIN);
    b.set_margin_bottom(MARGIN);
    b.set_margin_start(MARGIN);
    b.set_margin_end(MARGIN);
    b
}

/// A read-only value label.
fn value_label() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_selectable(true);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

/// A button with `label` and `tip`, appended to `row`.
fn button(row: &gtk::Box, label: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_tooltip_text(Some(tip));
    row.append(&b);
    b
}

/// The General tab.
pub struct GeneralPage {
    pub root: gtk::Box,
    pub name: gtk::Label,
    pub info: gtk::Entry,
    pub location: gtk::Entry,
    pub model: gtk::Label,
    pub address: gtk::Label,
    pub state: gtk::Label,
    pub lpoptions_note: gtk::Label,
    pub set_default: gtk::Button,
    pub enable: gtk::Button,
    pub test_page: gtk::Button,
    pub revert: gtk::Button,
    pub save: gtk::Button,
}

impl GeneralPage {
    /// Build the empty tab.
    pub fn new() -> Self {
        let root = page();
        let name = value_label();
        let info = gtk::Entry::new();
        info.set_hexpand(true);
        let location = gtk::Entry::new();
        location.set_hexpand(true);
        let model = value_label();
        let address = value_label();
        let state = value_label();
        for (text, widget) in [("Name", name.upcast_ref::<gtk::Widget>()), ("Description", info.upcast_ref()), ("Location", location.upcast_ref()), ("Model", model.upcast_ref()), ("Address", address.upcast_ref()), ("State", state.upcast_ref())] {
            root.append(&labeled_row(text, LabelWidth::Chars(LABEL_CHARS), SPACING, widget));
        }
        let lpoptions_note = gtk::Label::new(Some("Your own options in ~/.cups/lpoptions override these defaults."));
        lpoptions_note.set_xalign(0.0);
        lpoptions_note.add_css_class("warning");
        lpoptions_note.set_visible(false);
        root.append(&lpoptions_note);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        actions.set_margin_top(SPACING);
        let set_default = button(&actions, "Set as default", "Print to this printer when no printer is chosen");
        let enable = button(&actions, "Disable", "Stop or start printing on this printer");
        let test_page = button(&actions, "Print test page", "Print the CUPS test page");
        root.append(&actions);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        root.append(&spacer);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        bottom.set_halign(gtk::Align::End);
        let revert = button(&bottom, "Revert", "Put back the saved description, location and options");
        let save = button(&bottom, "Save", "Save the description, location and options");
        save.add_css_class("suggested-action");
        root.append(&bottom);
        GeneralPage { root, name, info, location, model, address, state, lpoptions_note, set_default, enable, test_page, revert, save }
    }

    /// Show the read-only facts of `p`.
    pub fn show(&self, p: &Printer) {
        self.name.set_text(&p.name);
        self.model.set_text(&p.make_model);
        self.address.set_text(&p.device_uri);
        self.state.set_text(&state_text(p));
        self.set_default.set_sensitive(!p.is_default);
        self.enable.set_label(if p.is_paused() { "Enable" } else { "Disable" });
    }
}

/// The state of `p` in words: the state, its message and the reasons.
pub fn state_text(p: &Printer) -> String {
    let mut parts = vec![p.state.label().to_string()];
    if !p.accepting { parts.push("not accepting jobs".to_string()); }
    if !p.state_message.is_empty() { parts.push(p.state_message.clone()); }
    for r in p.reasons.iter().filter(|r| r.as_str() != "none" && r.as_str() != "paused") {
        let level = match reason_severity(r) {
            Severity::Error => "error: ",
            Severity::Warning => "warning: ",
            _ => "",
        };
        parts.push(format!("{level}{}", reason_words(r)));
    }
    parts.join(" - ")
}

/// The Options tab: a scrolled box the window fills with [`option_rows`].
pub struct OptionsPage {
    pub root: gtk::ScrolledWindow,
    pub content: gtk::Box,
}

impl OptionsPage {
    /// Build the empty tab.
    pub fn new() -> Self {
        let content = page();
        let root = gtk::ScrolledWindow::builder().child(&content).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        OptionsPage { root, content }
    }

    /// Show only `text` (loading, nothing selected, an error).
    pub fn show_text(&self, text: &str) {
        opusdm_ui::clear_children(&self.content);
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        l.add_css_class("dim-label");
        self.content.append(&l);
    }

    /// Show `options`, the common ones first, the others in an expander;
    /// `value` gives the current choice of a key and `on_change` gets the
    /// new choice.
    pub fn show_options(&self, options: &[PrinterOption], value: impl Fn(&str) -> Option<String>, on_change: impl Fn(&str, &str) + Clone + 'static) {
        opusdm_ui::clear_children(&self.content);
        if options.is_empty() { return self.show_text("This printer has no options."); }
        let (common, rest) = split_options(options);
        for o in &common {
            self.content.append(&option_row(o, value(&o.key), on_change.clone()));
        }
        if rest.is_empty() { return; }
        let more = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        for o in &rest {
            more.append(&option_row(o, value(&o.key), on_change.clone()));
        }
        let expander = gtk::Expander::new(Some(&format!("More options ({})", rest.len())));
        expander.set_child(Some(&more));
        expander.set_margin_top(SPACING);
        self.content.append(&expander);
    }
}

/// One option: its label and a drop-down of its choices.
fn option_row(o: &PrinterOption, current: Option<String>, on_change: impl Fn(&str, &str) + 'static) -> gtk::Box {
    // An option the driver gives no default gets a first "not set" choice,
    // so no value is shown as if it were the default.
    let mut choices = o.choices.clone();
    if o.default.is_empty() { choices.insert(0, String::new()); }
    let names: Vec<&str> = choices.iter().map(|c| if c.is_empty() { NOT_SET } else { c.as_str() }).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    dropdown.set_hexpand(true);
    let current = current.unwrap_or_else(|| o.default.clone());
    if let Some(i) = choices.iter().position(|c| *c == current) { dropdown.set_selected(i as u32); }
    let key = o.key.clone();
    dropdown.connect_selected_notify(move |d| {
        if let Some(choice) = choices.get(d.selected() as usize) { on_change(&key, choice); }
    });
    labeled_row(&o.label, LabelWidth::Chars(OPTION_LABEL_CHARS), SPACING, &dropdown)
}

/// The Queue tab.
pub struct QueuePage {
    pub root: gtk::Box,
    pub list: gtk::ListBox,
    pub message: gtk::Label,
    pub cancel: gtk::Button,
    pub hold: gtk::Button,
    pub release: gtk::Button,
    pub retry: gtk::Button,
    pub cancel_all: gtk::Button,
}

impl QueuePage {
    /// Build the empty tab.
    pub fn new() -> Self {
        let root = page();
        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        root.append(&message);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        scroll.add_css_class("frame");
        root.append(&scroll);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        let cancel = button(&actions, "Cancel job", "Cancel the selected job");
        let hold = button(&actions, "Hold", "Keep the selected job in the queue without printing it");
        let release = button(&actions, "Resume", "Print the held job");
        let retry = button(&actions, "Retry", "Start the stopped printer or job again");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        let cancel_all = button(&actions, "Cancel all", "Cancel every job of this printer");
        root.append(&actions);
        QueuePage { root, list, message, cancel, hold, release, retry, cancel_all }
    }
}

/// One job line of the queue: id, document, user, state and reasons.
pub fn job_row(j: &Job) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    line.set_margin_start(SPACING);
    line.set_margin_end(SPACING);
    line.set_margin_top(SPACING / 2);
    line.set_margin_bottom(SPACING / 2);
    let id = gtk::Label::new(Some(&j.id.to_string()));
    id.set_width_chars(5);
    id.set_xalign(0.0);
    id.add_css_class("dim-label");
    line.append(&id);
    let name = gtk::Label::new(Some(&j.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    line.append(&name);
    let user = gtk::Label::new(Some(&j.user));
    user.add_css_class("dim-label");
    line.append(&user);
    let mut state = j.state.label().to_string();
    let reasons: Vec<String> = j.reasons.iter().filter(|r| !matches!(r.as_str(), "none" | "job-printing" | "job-incoming" | "job-queued" | "job-hold-until-specified")).map(|r| reason_words(r)).collect();
    if !reasons.is_empty() { state = format!("{state} ({})", reasons.join(", ")); }
    let state = gtk::Label::new(Some(&state));
    if j.state == opusdm_printing::model::JobState::Stopped { state.add_css_class("error"); }
    line.append(&state);
    line
}
