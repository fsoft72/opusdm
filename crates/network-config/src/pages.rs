//! The General, Wi-Fi, Ethernet, IPv4 and IPv6 tabs of the editor.

use crate::form::{self, check, choice, entry, join_list, note, secret_entry, spin, split_list, EditorCtx, Form, SPACING};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_network::ipconfig::{Family, IpAddress, IpConfig, IpMethod};
use opusdm_network::profile::{Detail, EthernetDetail, Profile, ProfileKind, WifiDetail, WifiSecurity};
use std::rc::Rc;

/// The Wi-Fi modes: NetworkManager name and label.
const WIFI_MODES: [(&str, &str); 3] = [("infrastructure", "Infrastructure"), ("adhoc", "Ad-hoc"), ("ap", "Access point")];
/// The Wi-Fi securities offered.
const WIFI_SECURITIES: [&str; 3] = ["None", "WPA and WPA2 Personal", "WPA3 Personal"];
/// The largest MTU accepted.
const MTU_MAX: i64 = 9000;
/// The range of the autoconnect priority.
const PRIORITY_RANGE: i64 = 999;

/// The Wi-Fi details of `p`, if it is Wi-Fi.
fn wifi(p: &mut Profile) -> Option<&mut WifiDetail> {
    match &mut p.detail {
        Detail::Wifi(w) => Some(w),
        _ => None,
    }
}

/// The Ethernet details of `p`, if it is Ethernet.
fn ethernet(p: &mut Profile) -> Option<&mut EthernetDetail> {
    match &mut p.detail {
        Detail::Ethernet(e) => Some(e),
        _ => None,
    }
}

/// The General tab: start on its own, priority, device and companion VPN.
pub fn general(ctx: &Rc<EditorCtx>) -> gtk::Widget {
    let p = ctx.read(Profile::clone);
    let form = Form::new();
    form.indented(&check(ctx, "Connect automatically", p.autoconnect, |p, on| p.autoconnect = on));
    form.row(
        "Priority",
        &spin(ctx, "general.priority", i64::from(p.autoconnect_priority), -PRIORITY_RANGE, PRIORITY_RANGE, |p, v| p.autoconnect_priority = v as i32),
    );
    let (label, hint) = if p.kind() == ProfileKind::WireGuard {
        ("Interface name", "The network interface this VPN creates, such as wg0.")
    } else {
        ("Restrict to device", "Leave empty to use any device.")
    };
    let ifname = entry(ctx, "general.interface", &p.interface_name, |p, v| p.interface_name = v);
    ifname.set_placeholder_text(Some(hint));
    form.row(label, &ifname);
    if matches!(p.kind(), ProfileKind::Wifi | ProfileKind::Ethernet) && !ctx.vpns.is_empty() {
        form.row("Start with VPN", &secondary_vpn(ctx, &p));
    }
    form.widget().clone().upcast()
}

/// The VPN started together with this connection: the first of
/// `connection.secondaries`; the others are kept.
fn secondary_vpn(ctx: &Rc<EditorCtx>, p: &Profile) -> gtk::DropDown {
    let mut labels = vec!["None"];
    labels.extend(ctx.vpns.iter().map(|(_, name)| name.as_str()));
    let selected = p.secondaries.first().and_then(|u| ctx.vpns.iter().position(|(uuid, _)| uuid == u)).map(|i| i + 1).unwrap_or(0);
    let uuids: Vec<String> = ctx.vpns.iter().map(|(u, _)| u.clone()).collect();
    choice(ctx, "general.vpn", &labels, selected, move |p, i| {
        if !p.secondaries.is_empty() {
            p.secondaries.remove(0);
        }
        if let Some(uuid) = i.checked_sub(1).and_then(|i| uuids.get(i)) {
            p.secondaries.insert(0, uuid.clone());
        }
    })
}

/// The Wi-Fi tab.
pub fn wifi_page(ctx: &Rc<EditorCtx>) -> gtk::Widget {
    let Detail::Wifi(w) = ctx.read(|p| p.detail.clone()) else { return note("Not a Wi-Fi connection").upcast() };
    let form = Form::new();
    form.row("Network name (SSID)", &entry(ctx, "wifi.ssid", &w.ssid, |p, v| if let Some(w) = wifi(p) { w.ssid = v }));
    let mode = WIFI_MODES.iter().position(|(m, _)| *m == w.mode).unwrap_or(0);
    let labels: Vec<&str> = WIFI_MODES.iter().map(|(_, l)| *l).collect();
    form.row("Mode", &choice(ctx, "wifi.mode", &labels, mode, |p, i| if let Some(w) = wifi(p) { w.mode = WIFI_MODES[i].0.to_string() }));
    form.indented(&check(ctx, "Hidden network", w.hidden, |p, on| if let Some(w) = wifi(p) { w.hidden = on }));
    if let WifiSecurity::Other(kind) = &w.security {
        form.wide(&note(&format!("Security {kind} (such as 802.1X) is not edited here; it is kept as it is.")));
        return form.widget().clone().upcast();
    }
    let password = secret_entry(ctx, "wifi.psk", w.psk.as_str(), |p, v| if let Some(w) = wifi(p) { w.psk = opusdm_network::profile::Secret::new(v) });
    let selected = match w.security {
        WifiSecurity::Open => 0,
        WifiSecurity::WpaPsk => 1,
        _ => 2,
    };
    password.set_sensitive(selected != 0);
    let security = choice(ctx, "wifi.security", &WIFI_SECURITIES, selected, |p, i| {
        if let Some(w) = wifi(p) {
            w.security = [WifiSecurity::Open, WifiSecurity::WpaPsk, WifiSecurity::Sae][i].clone();
        }
    });
    {
        let password = password.clone();
        security.connect_selected_notify(move |d| password.set_sensitive(d.selected() != 0));
    }
    form.row("Security", &security);
    form.row("Password", &password);
    form.widget().clone().upcast()
}

/// The Ethernet tab.
pub fn ethernet_page(ctx: &Rc<EditorCtx>) -> gtk::Widget {
    let Detail::Ethernet(e) = ctx.read(|p| p.detail.clone()) else { return note("Not an Ethernet connection").upcast() };
    let form = Form::new();
    let mac = entry(ctx, "ethernet.mac", &e.cloned_mac, |p, v| if let Some(e) = ethernet(p) { e.cloned_mac = v.trim().to_string() });
    mac.set_placeholder_text(Some("The device's own address"));
    form.row("Cloned MAC address", &mac);
    form.row("MTU (0 = automatic)", &spin(ctx, "ethernet.mtu", i64::from(e.mtu), 0, MTU_MAX, |p, v| if let Some(e) = ethernet(p) { e.mtu = v as u32 }));
    form.widget().clone().upcast()
}

/// The tab of a kind this program does not edit.
pub fn other_page(type_name: &str) -> gtk::Widget {
    let page = form::page_box();
    page.append(&note(&format!(
        "Connections of type {type_name} are not edited here. They can be connected, disconnected and deleted."
    )));
    page.upcast()
}

/// The IP methods offered for a family, with their labels; `current` is
/// added when it is one this program does not offer.
fn methods(family: Family, current: &IpMethod) -> Vec<(IpMethod, String)> {
    let mut out: Vec<(IpMethod, String)> = [
        (IpMethod::Auto, "Automatic"),
        (IpMethod::Manual, "Manual"),
        (IpMethod::LinkLocal, "Link-local only"),
        (IpMethod::Shared, "Shared to other computers"),
        (IpMethod::Disabled, if family == Family::V4 { "Disabled" } else { "Ignored" }),
    ]
    .into_iter()
    .map(|(m, l)| (m, l.to_string()))
    .collect();
    if let IpMethod::Other(name) = current {
        out.push((current.clone(), format!("Other ({name})")));
    }
    out
}

/// The prefix length a new address starts with.
fn default_prefix(family: Family) -> u32 {
    match family {
        Family::V4 => 24,
        Family::V6 => 64,
    }
}

/// The IP configuration of `family` in `p`.
fn ip_of(p: &mut Profile, family: Family) -> &mut IpConfig {
    match family {
        Family::V4 => &mut p.ipv4,
        Family::V6 => &mut p.ipv6,
    }
}

/// The IPv4 or IPv6 tab.
pub fn ip_page(ctx: &Rc<EditorCtx>, family: Family) -> gtk::Widget {
    let ip = ctx.read(|p| match family {
        Family::V4 => p.ipv4.clone(),
        Family::V6 => p.ipv6.clone(),
    });
    let is_vpn = ctx.read(|p| p.kind().is_vpn());
    let g = family.group();
    let form = Form::new();
    let offered = methods(family, &ip.method);
    let selected = offered.iter().position(|(m, _)| *m == ip.method).unwrap_or(0);
    let labels: Vec<&str> = offered.iter().map(|(_, l)| l.as_str()).collect();
    let choices: Vec<IpMethod> = offered.iter().map(|(m, _)| m.clone()).collect();
    form.row("Method", &choice(ctx, &format!("{g}.method"), &labels, selected, move |p, i| ip_of(p, family).method = choices[i].clone()));

    let rows = gtk::Box::new(gtk::Orientation::Vertical, SPACING / 2);
    ctx.field(&format!("{g}.addresses"), &rows);
    address_rows(ctx, family, &rows);
    form.row("Addresses", &rows);

    form.row("Gateway", &entry(ctx, &format!("{g}.gateway"), &ip.gateway, move |p, v| ip_of(p, family).gateway = v));
    let dns = entry(ctx, &format!("{g}.dns"), &join_list(&ip.dns), move |p, v| ip_of(p, family).dns = split_list(&v));
    dns.set_placeholder_text(Some("Separated by commas"));
    form.row("DNS servers", &dns);
    form.indented(&check(ctx, "Use only these DNS servers", ip.ignore_auto_dns, move |p, on| ip_of(p, family).ignore_auto_dns = on));
    let default_label = if is_vpn { "Use only for resources on its network" } else { "Never use as the default route" };
    form.indented(&check(ctx, default_label, ip.never_default, move |p, on| ip_of(p, family).never_default = on));
    form.widget().clone().upcast()
}

/// Fill `rows` with one line per address and an Add button. Rebuilt after
/// an add or a remove.
fn address_rows(ctx: &Rc<EditorCtx>, family: Family, rows: &gtk::Box) {
    opusdm_ui::clear_children(rows);
    let addresses = ctx.read(|p| match family {
        Family::V4 => p.ipv4.addresses.clone(),
        Family::V6 => p.ipv6.addresses.clone(),
    });
    let rebuild = {
        let (ctx, rows) = (ctx.clone(), rows.clone());
        move || {
            let (ctx, rows) = (ctx.clone(), rows.clone());
            // Not inside the handler of a button that is about to be removed.
            glib::idle_add_local_once(move || address_rows(&ctx, family, &rows));
        }
    };
    for (i, a) in addresses.iter().enumerate() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, SPACING / 2);
        let addr = gtk::Entry::new();
        addr.set_text(&a.address);
        addr.set_hexpand(true);
        let prefix = gtk::SpinButton::with_range(0.0, f64::from(family.max_prefix()), 1.0);
        prefix.set_value(f64::from(a.prefix));
        prefix.set_tooltip_text(Some("Prefix length"));
        let remove = gtk::Button::with_label("Remove");
        {
            let ctx = ctx.clone();
            addr.connect_changed(move |e| {
                let text = e.text().trim().to_string();
                ctx.edit(|p| if let Some(a) = ip_of(p, family).addresses.get_mut(i) { a.address = text });
            });
        }
        {
            let ctx = ctx.clone();
            prefix.connect_value_changed(move |s| {
                let v = s.value() as u32;
                ctx.edit(|p| if let Some(a) = ip_of(p, family).addresses.get_mut(i) { a.prefix = v });
            });
        }
        {
            let (ctx, rebuild) = (ctx.clone(), rebuild.clone());
            remove.connect_clicked(move |_| {
                ctx.edit(|p| {
                    let list = &mut ip_of(p, family).addresses;
                    if i < list.len() { list.remove(i); }
                });
                rebuild();
            });
        }
        line.append(&addr);
        line.append(&gtk::Label::new(Some("/")));
        line.append(&prefix);
        line.append(&remove);
        rows.append(&line);
    }
    let add = gtk::Button::with_label("Add address");
    add.set_halign(gtk::Align::Start);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| {
            ctx.edit(|p| ip_of(p, family).addresses.push(IpAddress { address: String::new(), prefix: default_prefix(family) }));
            rebuild();
        });
    }
    rows.append(&add);
}
