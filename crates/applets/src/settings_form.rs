//! The settings window of an applet, generated from the `settings` of its
//! manifest: one labelled row per setting (an entry for `string`, a spin
//! button for `number`, a switch for `bool`, a drop-down for `choice`), a
//! status line for errors, then Apply and Close.
//!
//! [`build`] knows nothing of the host: Apply reads the widgets, validates
//! them with [`collect`] and hands the values to `on_apply`; a validation
//! error is shown in the window. The caller shows its own errors (a save
//! that failed) with [`show_status`]. Close emits `close-request`, as the
//! title bar close does, then destroys the window.
//!
//! Ownership: the window owns its widgets and the button handlers; the
//! handlers hold only a `glib::WeakRef` of the window (a failed upgrade is
//! logged), so there is no reference cycle through the window.

use crate::manifest::{SettingDef, SettingKind};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{debug_log, err_log};
use serde_json::{Map, Number, Value};
use std::rc::Rc;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Widget name of the Apply button.
pub const APPLY_NAME: &str = "opusdm-settings-apply";
/// Widget name of the Close button.
pub const CLOSE_NAME: &str = "opusdm-settings-close";
/// Widget name of the status (error) line.
const STATUS_NAME: &str = "opusdm-settings-status";
/// Prefix of the widget name of a setting's field (then the key).
const FIELD_PREFIX: &str = "opusdm-setting-";
/// Text shown when the manifest declares no setting.
pub const NO_SETTINGS_TEXT: &str = "This applet has no settings";
/// Title of the window until the caller sets the applet's.
const DEFAULT_TITLE: &str = "Applet settings";

/// Padding around the window content.
const MARGIN: i32 = 12;
/// Gap between rows, and between a label and its control.
const SPACING: i32 = 8;
/// Width of the labels, in characters, so the controls line up.
const LABEL_CHARS: i32 = 22;
/// Default width of the window.
const DEFAULT_WIDTH: i32 = 380;
/// Range of a number field (wide: the manifest declares no bounds).
const NUMBER_MIN: f64 = -1e9;
/// See [`NUMBER_MIN`].
const NUMBER_MAX: f64 = 1e9;
/// Decimal digits of a number field whose value is fractional.
const FRACTION_DIGITS: u32 = 2;

/// The widget name of the field of setting `key`.
pub fn field_name(key: &str) -> String {
    format!("{FIELD_PREFIX}{key}")
}

/// The window of the settings `defs`, its fields showing `values` (a
/// missing key shows the default). Apply validates the fields and calls
/// `on_apply` with the values; Close emits `close-request` and destroys
/// the window. With no
/// setting the window says so and has only Close. Not presented.
pub fn build(defs: &[SettingDef], values: &Map<String, Value>, on_apply: Box<dyn Fn(Map<String, Value>)>) -> gtk::Window {
    let window = gtk::Window::builder().title(DEFAULT_TITLE).default_width(DEFAULT_WIDTH).build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    column.set_margin_start(MARGIN);
    column.set_margin_end(MARGIN);
    column.set_margin_top(MARGIN);
    column.set_margin_bottom(MARGIN);
    let fields: Vec<(SettingDef, gtk::Widget)> = defs.iter().map(|def| (def.clone(), field_for(def, values.get(&def.key).unwrap_or(&def.default)))).collect();
    if fields.is_empty() {
        let label = gtk::Label::new(Some(NO_SETTINGS_TEXT));
        label.set_xalign(0.0);
        column.append(&label);
    }
    for (def, widget) in &fields {
        column.append(&opusdm_ui::labeled_row(&def.label, opusdm_ui::LabelWidth::Chars(LABEL_CHARS), SPACING, widget));
    }
    let status = status_label();
    column.append(&status);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(SPACING);
    let close = gtk::Button::with_label("Close");
    close.set_widget_name(CLOSE_NAME);
    close.connect_clicked(close_handler(&window));
    buttons.append(&close);
    if !fields.is_empty() {
        let apply = gtk::Button::with_label("Apply");
        apply.set_widget_name(APPLY_NAME);
        apply.connect_clicked(apply_handler(&window, fields, Rc::from(on_apply)));
        buttons.append(&apply);
    }
    column.append(&buttons);
    window.set_child(Some(&column));
    window
}

/// Show `text` in the status line of settings window `window` (an empty
/// text hides it). A window without a status line is logged.
pub fn show_status(window: &gtk::Window, text: &str) {
    let Some(label) = status_of(window) else { return err_log!(APP_TAG, "settings window without a status line; not shown: {text}") };
    label.set_text(text);
    label.set_visible(!text.is_empty());
}

/// The text of the status line of `window`, `None` when it is hidden.
pub fn status_text(window: &gtk::Window) -> Option<String> {
    let label = status_of(window)?;
    if !label.get_visible() { return None; }
    Some(label.text().to_string())
}

/// Validate the raw `(key, text)` pairs read from the form against `defs`:
/// a number must parse (finite), a bool is `true` or `false`, a choice must
/// be one of its choices, a string is kept as typed. Every declared key
/// must be there; undeclared keys are ignored. The error names the key.
pub fn collect(defs: &[SettingDef], raw: &[(String, String)]) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for def in defs {
        let Some((_, text)) = raw.iter().find(|(k, _)| *k == def.key) else { return Err(format!("setting '{}' ({}): no value", def.key, def.label)) };
        out.insert(def.key.clone(), parse_value(def, text)?);
    }
    Ok(out)
}

/// The value of setting `def` typed as `text`, or an error naming it.
fn parse_value(def: &SettingDef, text: &str) -> Result<Value, String> {
    let bad = |what: &str| format!("setting '{}' ({}): \"{text}\" is not {what}", def.key, def.label);
    match &def.kind {
        SettingKind::String => Ok(Value::String(text.to_string())),
        SettingKind::Number => parse_number(text).ok_or_else(|| bad("a number")),
        SettingKind::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(bad("true or false")),
        },
        SettingKind::Choice(choices) => {
            if !choices.iter().any(|c| c == text) { return Err(bad(&format!("one of {}", choices.join(", ")))); }
            Ok(Value::String(text.to_string()))
        }
    }
}

/// `text` (trimmed) as a finite JSON number: a whole number is an integer.
fn parse_number(text: &str) -> Option<Value> {
    let n: f64 = text.trim().parse().ok().filter(|n: &f64| n.is_finite())?;
    if n.fract() == 0.0 && n.abs() < 9.0e15 { return Some(Value::from(n as i64)); }
    Number::from_f64(n).map(Value::Number)
}

/// The control of setting `def`, showing `value` (the default when the
/// value has the wrong type), named after its key.
fn field_for(def: &SettingDef, value: &Value) -> gtk::Widget {
    let widget: gtk::Widget = match &def.kind {
        SettingKind::String => {
            let entry = gtk::Entry::new();
            entry.set_text(value.as_str().or(def.default.as_str()).unwrap_or_default());
            entry.set_hexpand(true);
            entry.upcast()
        }
        SettingKind::Number => {
            let current = value.as_f64().or(def.default.as_f64()).unwrap_or_default();
            let fractional = [def.default.as_f64(), value.as_f64()].into_iter().flatten().any(|n| n.fract() != 0.0);
            let step = if fractional { 0.1 } else { 1.0 };
            let spin = gtk::SpinButton::with_range(NUMBER_MIN, NUMBER_MAX, step);
            spin.set_digits(if fractional { FRACTION_DIGITS } else { 0 });
            // Letters cannot be typed: the click on Apply makes the spin
            // button parse its text first, and it would turn "abc" into 0.
            spin.set_numeric(true);
            spin.connect_input(_keep_value_when_blank);
            spin.set_value(current);
            spin.set_hexpand(true);
            spin.upcast()
        }
        SettingKind::Bool => {
            let switch = gtk::Switch::new();
            switch.set_active(value.as_bool().or(def.default.as_bool()).unwrap_or_default());
            switch.set_halign(gtk::Align::Start);
            switch.set_valign(gtk::Align::Center);
            switch.upcast()
        }
        SettingKind::Choice(choices) => {
            let names: Vec<&str> = choices.iter().map(String::as_str).collect();
            let dropdown = gtk::DropDown::from_strings(&names);
            let wanted = value.as_str().or(def.default.as_str()).unwrap_or_default();
            let index = choices.iter().position(|c| c == wanted).unwrap_or(0);
            dropdown.set_selected(u32::try_from(index).unwrap_or(0));
            dropdown.set_hexpand(true);
            dropdown.upcast()
        }
    };
    widget.set_widget_name(&field_name(&def.key));
    widget
}

/// The `input` handler of a number field: GTK's own parser reads an empty
/// text as 0, so an empty or blank text gives the current value instead
/// (GTK then shows it again); any other text goes to GTK's parser (`None`).
/// The current value is returned, not an input error: with the default
/// update policy (`Always`) GTK would use an unset value on an error.
fn _keep_value_when_blank(spin: &gtk::SpinButton) -> Option<Result<f64, ()>> {
    if !spin.text().trim().is_empty() { return None; }
    debug_log!(APP_TAG, "number field '{}' left blank: its value {} is kept", spin.widget_name(), spin.value());
    Some(Ok(spin.value()))
}

/// The text of the control `widget` of setting `def`, for [`collect`].
fn raw_text(def: &SettingDef, widget: &gtk::Widget) -> String {
    match &def.kind {
        SettingKind::String => widget.downcast_ref::<gtk::Entry>().map(|e| e.text().to_string()),
        // The spin button prints its value with the locale's decimal
        // separator; `collect` parses a dot.
        SettingKind::Number => widget.downcast_ref::<gtk::SpinButton>().map(|s| s.text().replace(',', ".")),
        SettingKind::Bool => widget.downcast_ref::<gtk::Switch>().map(|s| s.is_active().to_string()),
        SettingKind::Choice(choices) => widget.downcast_ref::<gtk::DropDown>().map(|d| {
            let index = usize::try_from(d.selected()).unwrap_or(usize::MAX);
            choices.get(index).cloned().unwrap_or_default()
        }),
    }
    .unwrap_or_else(|| {
        err_log!(APP_TAG, "internal: setting '{}' has a field of the wrong type {}", def.key, widget.type_());
        String::new()
    })
}

/// The status (error) line: hidden until there is something to show.
fn status_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_widget_name(STATUS_NAME);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    label.add_css_class("error");
    label.set_visible(false);
    label
}

/// The status line of `window`, found by its name.
fn status_of(window: &gtk::Window) -> Option<gtk::Label> {
    find_named(&window.child()?, STATUS_NAME)?.downcast::<gtk::Label>().ok()
}

/// The first widget named `name` under `root`, depth first.
fn find_named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name { return Some(root.clone()); }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_named(&c, name) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

/// Apply: read the fields, validate them; an error goes to the status line
/// (and the log), good values to `on_apply`. Holds a weak window.
fn apply_handler(window: &gtk::Window, fields: Vec<(SettingDef, gtk::Widget)>, on_apply: Rc<dyn Fn(Map<String, Value>)>) -> impl Fn(&gtk::Button) + 'static {
    let weak = window.downgrade();
    move |_| {
        let Some(window) = weak.upgrade() else { return err_log!(APP_TAG, "settings Apply clicked after its window was destroyed, ignored") };
        let defs: Vec<SettingDef> = fields.iter().map(|(d, _)| d.clone()).collect();
        let raw: Vec<(String, String)> = fields.iter().map(|(d, w)| (d.key.clone(), raw_text(d, w))).collect();
        match collect(&defs, &raw) {
            Ok(values) => {
                show_status(&window, "");
                on_apply(values);
            }
            Err(e) => {
                err_log!(APP_TAG, "settings not applied: {e}");
                show_status(&window, &e);
            }
        }
    }
}

/// Close: what the title bar close does, `close-request` then destroy.
/// Holds a weak window.
fn close_handler(window: &gtk::Window) -> impl Fn(&gtk::Button) + 'static {
    let weak: glib::WeakRef<gtk::Window> = window.downgrade();
    move |_| {
        let Some(window) = weak.upgrade() else { return err_log!(APP_TAG, "settings Close clicked after its window was destroyed, ignored") };
        // As `gtk_window_close`, which does nothing on a window that is not
        // realized: `close-request` first (the host forgets the window),
        // then destroy unless a handler kept it.
        let kept = window.emit_by_name::<bool>("close-request", &[]);
        if !kept { window.destroy(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A setting `key` of `kind` with `default`.
    fn def(key: &str, kind: SettingKind, default: Value) -> SettingDef {
        SettingDef { key: key.into(), kind, label: format!("Label {key}"), default }
    }

    /// `(key, text)` pairs.
    fn raw(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn collect_rejects_a_non_numeric_number() {
        let defs = [def("refreshMinutes", SettingKind::Number, json!(30))];
        for bad in ["abc", "", "12x", "NaN", "inf"] {
            let err = collect(&defs, &raw(&[("refreshMinutes", bad)])).unwrap_err();
            assert!(err.contains("refreshMinutes"), "{bad}: {err}");
        }
        let ok = collect(&defs, &raw(&[("refreshMinutes", " 15 ")])).unwrap();
        assert_eq!(ok["refreshMinutes"], json!(15));
        let ok = collect(&defs, &raw(&[("refreshMinutes", "2.5")])).unwrap();
        assert_eq!(ok["refreshMinutes"], json!(2.5));
        let ok = collect(&defs, &raw(&[("refreshMinutes", "-3")])).unwrap();
        assert_eq!(ok["refreshMinutes"], json!(-3));
    }

    #[test]
    fn collect_keeps_the_choice_inside_choices() {
        let defs = [def("units", SettingKind::Choice(vec!["C".into(), "F".into()]), json!("C"))];
        assert_eq!(collect(&defs, &raw(&[("units", "F")])).unwrap()["units"], json!("F"));
        for bad in ["K", "", "c"] {
            let err = collect(&defs, &raw(&[("units", bad)])).unwrap_err();
            assert!(err.contains("units"), "{bad}: {err}");
        }
    }

    #[test]
    fn collect_reads_strings_and_bools_and_names_a_missing_key() {
        let defs = [def("city", SettingKind::String, json!("Rome")), def("on", SettingKind::Bool, json!(true))];
        let got = collect(&defs, &raw(&[("city", " Milan "), ("on", "false"), ("extra", "x")])).unwrap();
        assert_eq!(got["city"], json!(" Milan "), "a string is kept as typed");
        assert_eq!(got["on"], json!(false));
        assert!(!got.contains_key("extra"), "a key that is not declared is not collected");
        let err = collect(&defs, &raw(&[("city", "x"), ("on", "maybe")])).unwrap_err();
        assert!(err.contains("on"), "{err}");
        let err = collect(&defs, &raw(&[("city", "x")])).unwrap_err();
        assert!(err.contains("'on'"), "{err}");
    }
}
