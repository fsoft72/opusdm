//! The right side of the window: the name, the tabs of one profile and the
//! Connect, Revert and Save buttons.

use crate::form::{entry, note, EditorCtx, Form, MARGIN, SPACING};
use crate::{pages, vpn_pages};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_network::ipconfig::Family;
use opusdm_network::profile::ProfileKind;
use opusdm_network::ProfileSummary;
use std::rc::Rc;

/// The editor of one profile.
pub struct EditorView {
    pub root: gtk::Box,
    pub ctx: Rc<EditorCtx>,
    pub save: gtk::Button,
    pub revert: gtk::Button,
    pub connect: gtk::Button,
}

impl EditorView {
    /// Build the editor for the profile in `ctx`; `type_name` names a kind
    /// this program does not edit.
    pub fn new(ctx: &Rc<EditorCtx>, type_name: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        root.set_margin_top(MARGIN);
        root.set_margin_bottom(MARGIN);
        root.set_margin_start(MARGIN);
        root.set_margin_end(MARGIN);
        root.set_hexpand(true);
        let (kind, is_new, notes, name) = {
            let s = ctx.session.borrow();
            (s.current.kind(), s.is_new(), s.notes.clone(), s.current.id.clone())
        };
        if is_new {
            let mut text = "New connection, not saved yet.".to_string();
            if !notes.is_empty() {
                text.push_str(&format!(" The import left out: {}.", notes.join(", ")));
            }
            let banner = note(&text);
            banner.remove_css_class("dim-label");
            banner.add_css_class("warning");
            root.append(&banner);
        }
        let head = Form::new();
        head.widget().set_margin_top(0);
        head.widget().set_margin_bottom(0);
        let name_entry = entry(ctx, "general.id", &name, |p, v| p.id = v);
        name_entry.set_sensitive(kind != ProfileKind::Other);
        head.row("Name", &name_entry);
        root.append(head.widget());

        let tabs = gtk::Notebook::new();
        tabs.set_vexpand(true);
        if kind == ProfileKind::Other {
            tabs.append_page(&pages::other_page(type_name), Some(&gtk::Label::new(Some("Details"))));
        } else {
            tabs.append_page(&pages::general(ctx), Some(&gtk::Label::new(Some("General"))));
            let (page, label) = match kind {
                ProfileKind::Wifi => (pages::wifi_page(ctx), "Wi-Fi"),
                ProfileKind::Ethernet => (pages::ethernet_page(ctx), "Ethernet"),
                ProfileKind::WireGuard => (vpn_pages::wireguard_page(ctx), "WireGuard"),
                ProfileKind::OpenVpn => (vpn_pages::openvpn_page(ctx), "OpenVPN"),
                ProfileKind::Other => unreachable!("handled above"),
            };
            tabs.append_page(&page, Some(&gtk::Label::new(Some(label))));
            tabs.append_page(&pages::ip_page(ctx, Family::V4), Some(&gtk::Label::new(Some("IPv4"))));
            tabs.append_page(&pages::ip_page(ctx, Family::V6), Some(&gtk::Label::new(Some("IPv6"))));
            // A new or imported profile opens on its own tab.
            if is_new { tabs.set_current_page(Some(1)); }
        }
        root.append(&tabs);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        let connect = gtk::Button::with_label("Connect");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let revert = gtk::Button::with_label("Revert");
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        buttons.append(&connect);
        buttons.append(&spacer);
        buttons.append(&revert);
        buttons.append(&save);
        root.append(&buttons);
        Self { root, ctx: ctx.clone(), save, revert, connect }
    }

    /// Show what the buttons can do: Save with valid changes, Revert with
    /// changes to a saved profile, Connect or Disconnect for a saved one.
    pub fn update(&self, summary: Option<&ProfileSummary>) {
        let s = self.ctx.session.borrow();
        self.save.set_sensitive(s.can_save());
        self.revert.set_sensitive(!s.is_new() && s.is_dirty());
        let active = summary.is_some_and(|p| p.active || p.activating);
        self.connect.set_label(if active { "Disconnect" } else { "Connect" });
        self.connect.set_sensitive(!s.is_new() && summary.is_some());
        let errors = s.errors();
        let tip = if errors.is_empty() || !s.is_editable() { None } else { Some(errors.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n")) };
        self.save.set_tooltip_text(tip.as_deref());
    }
}

/// The placeholder shown when no profile is selected.
pub fn empty_view(text: &str) -> gtk::Widget {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    l.set_hexpand(true);
    l.set_vexpand(true);
    l.upcast()
}
