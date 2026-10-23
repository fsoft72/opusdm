//! The printer icon and its popover, in the right side of the panel. The
//! icon shows only while jobs are queued or a printer is in error.

use crate::printer_center::{Action, PrinterCenter};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_printing::model::{reason_words, Job, JobState, Printer, Severity};
use opusdm_printing::tray::{is_owner_paused, Look};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

/// The program that manages the printers, started by "Manage printers...".
const MANAGER_BIN: &str = "opusdm-printer-manager";
/// Size of the icon drawing in pixels (wider than tall: room for the badge).
const ICON_WIDTH: i32 = 26;
const ICON_HEIGHT: i32 = 20;
/// Width of the popover, in pixels.
const POPOVER_WIDTH: i32 = 320;
/// The job list scrolls above this height, in pixels.
const LIST_MAX_HEIGHT: i32 = 320;
/// Colours of the badge: jobs, error, printer disabled.
const BADGE_NORMAL: (f64, f64, f64) = (0.208, 0.518, 0.894);
const BADGE_ERROR: (f64, f64, f64) = (0.898, 0.282, 0.302);
const BADGE_PAUSED: (f64, f64, f64) = (0.5, 0.5, 0.5);

/// What the icon draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IconLook {
    look: Look,
    jobs: usize,
}

/// Text of the badge: the job count (`9+` above nine), or `!` for an error
/// with no job.
pub fn badge_text(look: Look, jobs: usize) -> Option<String> {
    match (look, jobs) {
        (Look::Hidden, _) => None,
        (Look::Error, 0) => Some("!".to_string()),
        (_, 0) => None,
        (_, 1..=9) => Some(jobs.to_string()),
        _ => Some("9+".to_string()),
    }
}

/// The button and its popover.
pub struct PrinterButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    shown: Rc<Cell<IconLook>>,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    message: gtk::Label,
    list: gtk::Box,
}

impl PrinterButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<PrinterCenter>) -> Rc<Self> {
        let _ = center;
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);
        let shown = Rc::new(Cell::new(IconLook { look: Look::Hidden, jobs: 0 }));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_WIDTH);
        icon.set_content_height(ICON_HEIGHT);
        {
            let shown = shown.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_printer(area, cr, w, h, shown.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(POPOVER_WIDTH);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        message.set_wrap(true);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(LIST_MAX_HEIGHT);
        scroll.set_child(Some(&list));
        let manage = gtk::Button::with_label("Manage printers...");
        manage.add_css_class("flat");
        content.append(&message);
        content.append(&scroll);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&manage);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));
        {
            let popover = popover.clone();
            manage.connect_clicked(move |_| {
                popover.popdown();
                open_manager();
            });
        }
        root.append(&button);
        Rc::new(Self { root, icon, shown, button, popover, message, list })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<PrinterCenter>) {
        let view = center.view();
        self.root.set_visible(view.look != Look::Hidden);
        if view.look == Look::Hidden { self.popover.popdown(); }
        self.shown.set(IconLook { look: view.look, jobs: view.jobs });
        self.icon.queue_draw();
        self.button.set_tooltip_text(Some(&view.tooltip));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        self.show_list(center);
    }

    /// Rebuild the printers with jobs or errors, and their jobs.
    fn show_list(&self, center: &Rc<PrinterCenter>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let s = center.snapshot();
        for p in &s.printers {
            let jobs: Vec<&Job> = s.jobs_of(&p.name).collect();
            if jobs.is_empty() && p.worst() < Severity::Warning { continue; }
            self.list.append(&printer_heading(p));
            for j in jobs {
                self.list.append(&job_row(center, p, j));
            }
        }
    }
}

/// The name of `p` and its state, in the colour of its worst problem.
fn printer_heading(p: &Printer) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let name = gtk::Label::new(Some(&p.name));
    name.set_xalign(0.0);
    name.add_css_class("heading");
    b.append(&name);
    let mut parts: Vec<String> = Vec::new();
    if is_owner_paused(p) { parts.push("Disabled".to_string()); }
    if !p.state_message.is_empty() && p.state_message != "Paused" { parts.push(p.state_message.clone()); }
    parts.extend(p.reasons.iter().filter(|r| !matches!(r.as_str(), "none" | "paused")).map(|r| reason_words(r)));
    if parts.is_empty() { return b; }
    let state = gtk::Label::new(Some(&parts.join(" - ")));
    state.set_xalign(0.0);
    state.set_wrap(true);
    state.add_css_class(if p.worst() == Severity::Error { "error" } else { "dim-label" });
    b.append(&state);
    b
}

/// One job: its name and state, with the buttons it can use.
fn job_row(center: &Rc<PrinterCenter>, p: &Printer, j: &Job) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name = gtk::Label::new(Some(&j.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    line.append(&name);
    let state = gtk::Label::new(Some(j.state.label()));
    state.add_css_class(if j.state == JobState::Stopped { "error" } else { "dim-label" });
    line.append(&state);
    let mut actions: Vec<(&str, Action)> = Vec::new();
    match j.state {
        JobState::Held => actions.push(("Resume", Action::Release(j.id))),
        JobState::Pending => actions.push(("Hold", Action::Hold(j.id))),
        _ => {}
    }
    let stopped_printer = p.state == opusdm_printing::model::PrinterState::Stopped && !is_owner_paused(p);
    if j.state == JobState::Stopped || stopped_printer {
        let job = (j.state == JobState::Stopped).then_some(j.id);
        actions.push(("Retry", Action::Retry(p.name.clone(), job)));
    }
    actions.push(("Cancel", Action::Cancel(j.id)));
    for (label, action) in actions {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        let center = center.clone();
        b.connect_clicked(move |_| center.send(action.clone()));
        line.append(&b);
    }
    line
}

/// Start the printer manager program, from the folder of the panel first.
fn open_manager() {
    let sibling = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(MANAGER_BIN)));
    let path = sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from(MANAGER_BIN));
    if let Err(e) = std::process::Command::new(&path).spawn() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start {}: {e}", path.display());
    }
}

/// A printer in the text colour: the body, the paper going in on top and
/// the sheet coming out, with a badge of the jobs.
fn draw_printer(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, _w: i32, h: i32, shown: IconLook) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
    let h = h as f64;
    let (left, right) = (2.0, 18.0);
    let (top, bottom) = (h * 0.38, h * 0.78);
    cr.set_line_width(1.6);
    // Paper in, body, sheet out.
    cr.rectangle(6.0, h * 0.12, 8.0, top - h * 0.12);
    let _ = cr.stroke();
    cr.rectangle(left, top, right - left, bottom - top);
    let _ = cr.stroke();
    cr.rectangle(6.0, bottom - 3.0, 8.0, h * 0.95 - (bottom - 3.0));
    let _ = cr.fill();
    let Some(text) = badge_text(shown.look, shown.jobs) else { return };
    let rgb = match shown.look {
        Look::Error => BADGE_ERROR,
        Look::Paused => BADGE_PAUSED,
        _ => BADGE_NORMAL,
    };
    let (bx, by, br) = (right + 1.5, top - 1.0, 6.0);
    cr.set_source_rgb(rgb.0, rgb.1, rgb.2);
    cr.arc(bx, by, br, 0.0, 2.0 * std::f64::consts::PI);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
    cr.set_font_size(if text.len() > 1 { 7.0 } else { 8.5 });
    if let Ok(ext) = cr.text_extents(&text) {
        cr.move_to(bx - ext.width() / 2.0 - ext.x_bearing(), by - ext.height() / 2.0 - ext.y_bearing());
        let _ = cr.show_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_texts() {
        assert_eq!(badge_text(Look::Hidden, 3), None);
        assert_eq!(badge_text(Look::Normal, 1).as_deref(), Some("1"));
        assert_eq!(badge_text(Look::Normal, 12).as_deref(), Some("9+"));
        assert_eq!(badge_text(Look::Error, 0).as_deref(), Some("!"));
        assert_eq!(badge_text(Look::Error, 2).as_deref(), Some("2"));
        assert_eq!(badge_text(Look::Paused, 1).as_deref(), Some("1"));
    }
}
