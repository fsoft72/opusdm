//! The WireGuard and OpenVPN tabs of the editor, and the OpenVPN advanced
//! options dialog.

use crate::form::{check, choice, entry, file_entry, join_list, note, page_box, secret_entry, spin, split_list, EditorCtx, Form, SPACING};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_network::profile::{ovpn_key, Detail, OpenVpnAuth, OpenVpnDetail, Peer, Profile, Secret, WireGuardDetail};
use opusdm_network::wgkey;
use std::rc::Rc;

/// The largest port number.
const PORT_MAX: i64 = 65535;
/// The longest keepalive interval, in seconds.
const KEEPALIVE_MAX: i64 = 65535;
/// The largest MTU accepted.
const MTU_MAX: i64 = 9000;
/// The key directions of a TLS auth or static key: value and label.
const DIRECTIONS: [(&str, &str); 3] = [("", "None"), ("0", "0"), ("1", "1")];
/// The device types: value and label.
const DEV_TYPES: [(&str, &str); 2] = [("tun", "TUN"), ("tap", "TAP")];
/// The `remote-cert-tls` values: value and label.
const REMOTE_CERT: [(&str, &str); 3] = [("", "Do not check"), ("server", "Server"), ("client", "Client")];

/// The WireGuard details of `p`, if it is WireGuard.
fn wg(p: &mut Profile) -> Option<&mut WireGuardDetail> {
    match &mut p.detail {
        Detail::WireGuard(w) => Some(w),
        _ => None,
    }
}

/// The OpenVPN details of `p`, if it is OpenVPN.
fn ovpn(p: &mut Profile) -> Option<&mut OpenVpnDetail> {
    match &mut p.detail {
        Detail::OpenVpn(o) => Some(o),
        _ => None,
    }
}

/// The public key of a private key, or a hint when it is not a key.
fn public_text(private: &str) -> String {
    wgkey::public_of(private).unwrap_or_else(|_| "(no valid private key)".to_string())
}

/// The WireGuard tab: the interface keys, then the peers.
pub fn wireguard_page(ctx: &Rc<EditorCtx>) -> gtk::Widget {
    let Detail::WireGuard(w) = ctx.read(|p| p.detail.clone()) else { return note("Not a WireGuard connection").upcast() };
    let form = Form::new();
    let public = gtk::Label::new(Some(&public_text(w.private_key.as_str())));
    public.set_selectable(true);
    public.set_xalign(0.0);
    let private = {
        let public = public.clone();
        let e = secret_entry(ctx, "wireguard.private-key", w.private_key.as_str(), |p, v| if let Some(w) = wg(p) { w.private_key = Secret::new(v.trim()) });
        e.connect_changed(move |e| public.set_text(&public_text(&e.text())));
        e
    };
    let generate = gtk::Button::with_label("Generate");
    {
        let (private, window) = (private.clone(), ctx.window.clone());
        generate.connect_clicked(move |_| match wgkey::generate() {
            Ok((key, _)) => private.set_text(&key),
            Err(e) => opusdm_ui::requester(&window, "WireGuard", &e.text(), None, &[("OK", ())], |_| {}),
        });
    }
    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING / 2);
    private.set_hexpand(true);
    key_row.append(&private);
    key_row.append(&generate);
    form.row("Private key", &key_row);
    form.row("Public key", &public);
    form.row("Listen port (0 = any)", &spin(ctx, "wireguard.port", i64::from(w.listen_port), 0, PORT_MAX, |p, v| if let Some(w) = wg(p) { w.listen_port = v as u32 }));
    form.row("MTU (0 = automatic)", &spin(ctx, "wireguard.mtu", i64::from(w.mtu), 0, MTU_MAX, |p, v| if let Some(w) = wg(p) { w.mtu = v as u32 }));

    let peers = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    ctx.field("wireguard.peers", &peers);
    peer_frames(ctx, &peers);
    form.wide(&opusdm_ui::section_heading("Peers"));
    form.wide(&peers);
    let scroll = gtk::ScrolledWindow::builder().child(form.widget()).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    scroll.upcast()
}

/// Fill `box_` with one frame per peer and an Add button. Rebuilt after an
/// add or a remove.
fn peer_frames(ctx: &Rc<EditorCtx>, box_: &gtk::Box) {
    opusdm_ui::clear_children(box_);
    let peers = ctx.read(|p| match &p.detail {
        Detail::WireGuard(w) => w.peers.clone(),
        _ => Vec::new(),
    });
    let rebuild = {
        let (ctx, box_) = (ctx.clone(), box_.clone());
        move || {
            let (ctx, box_) = (ctx.clone(), box_.clone());
            // Not inside the handler of a button that is about to be removed.
            glib::idle_add_local_once(move || peer_frames(&ctx, &box_));
        }
    };
    for (i, peer) in peers.iter().enumerate() {
        box_.append(&peer_frame(ctx, i, peer, rebuild.clone()));
    }
    let add = gtk::Button::with_label("Add peer");
    add.set_halign(gtk::Align::Start);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| {
            ctx.edit(|p| if let Some(w) = wg(p) { w.peers.push(Peer::default()) });
            rebuild();
        });
    }
    box_.append(&add);
}

/// The frame of peer `i`.
fn peer_frame(ctx: &Rc<EditorCtx>, i: usize, peer: &Peer, rebuild: impl Fn() + 'static) -> gtk::Frame {
    /// Change peer `i` of the profile.
    fn with_peer(p: &mut Profile, i: usize, f: impl FnOnce(&mut Peer)) {
        if let Some(peer) = wg(p).and_then(|w| w.peers.get_mut(i)) {
            f(peer);
        }
    }
    let form = Form::new();
    form.row("Public key", &entry(ctx, "wireguard.peers", &peer.public_key, move |p, v| with_peer(p, i, |peer| peer.public_key = v.trim().to_string())));
    let endpoint = entry(ctx, "wireguard.endpoint", &peer.endpoint, move |p, v| with_peer(p, i, |peer| peer.endpoint = v.trim().to_string()));
    endpoint.set_placeholder_text(Some("host:port"));
    form.row("Endpoint", &endpoint);
    let allowed = entry(ctx, "wireguard.allowed-ips", &join_list(&peer.allowed_ips), move |p, v| with_peer(p, i, |peer| peer.allowed_ips = split_list(&v)));
    allowed.set_placeholder_text(Some("0.0.0.0/0, ::/0 sends everything through the VPN"));
    form.row("Allowed IPs", &allowed);
    form.row(
        "Preshared key",
        &secret_entry(ctx, "wireguard.peers", peer.preshared_key.as_str(), move |p, v| with_peer(p, i, |peer| peer.preshared_key = Secret::new(v.trim()))),
    );
    form.row(
        "Keepalive (seconds)",
        &spin(ctx, "wireguard.keepalive", i64::from(peer.keepalive), 0, KEEPALIVE_MAX, move |p, v| with_peer(p, i, |peer| peer.keepalive = v as u32)),
    );
    let remove = gtk::Button::with_label("Remove peer");
    remove.set_halign(gtk::Align::End);
    {
        let ctx = ctx.clone();
        remove.connect_clicked(move |_| {
            ctx.edit(|p| {
                if let Some(w) = wg(p) {
                    if i < w.peers.len() { w.peers.remove(i); }
                }
            });
            rebuild();
        });
    }
    form.wide(&remove);
    let frame = gtk::Frame::new(Some(&format!("Peer {}", i + 1)));
    frame.set_child(Some(form.widget()));
    frame
}

/// The OpenVPN tab. Changing the authentication rebuilds it, since the
/// fields depend on it.
pub fn openvpn_page(ctx: &Rc<EditorCtx>) -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    openvpn_fields(ctx, &page);
    let scroll = gtk::ScrolledWindow::builder().child(&page).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    scroll.upcast()
}

/// A `vpn.data` value of the profile being edited.
fn data(ctx: &EditorCtx, key: &str) -> String {
    ctx.read(|p| match &p.detail {
        Detail::OpenVpn(o) => o.get(key).to_string(),
        _ => String::new(),
    })
}

/// An entry bound to the `vpn.data` key `key`.
fn data_entry(ctx: &Rc<EditorCtx>, field: &str, key: &'static str) -> gtk::Entry {
    entry(ctx, field, &data(ctx, key), move |p, v| if let Some(o) = ovpn(p) { o.put(key, v.trim()) })
}

/// A file chooser bound to the `vpn.data` key `key`.
fn data_file(ctx: &Rc<EditorCtx>, field: &str, key: &'static str) -> gtk::Box {
    file_entry(ctx, field, &data(ctx, key), move |p, v| if let Some(o) = ovpn(p) { o.put(key, v.trim()) })
}

/// A drop-down over `options` (value, label) bound to the `vpn.data` key `key`.
fn data_choice(ctx: &Rc<EditorCtx>, key: &'static str, options: &'static [(&'static str, &'static str)]) -> gtk::DropDown {
    let current = data(ctx, key);
    let selected = options.iter().position(|(v, _)| *v == current).unwrap_or(0);
    let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
    choice(ctx, &format!("openvpn.{key}"), &labels, selected, move |p, i| if let Some(o) = ovpn(p) { o.put(key, options[i].0) })
}

/// Fill `page` with the fields of the current authentication.
fn openvpn_fields(ctx: &Rc<EditorCtx>, page: &gtk::Box) {
    opusdm_ui::clear_children(page);
    let Detail::OpenVpn(o) = ctx.read(|p| p.detail.clone()) else { return page.append(&note("Not an OpenVPN connection")) };
    let auth = o.auth();
    let form = Form::new();
    let remote = data_entry(ctx, "openvpn.remote", ovpn_key::REMOTE);
    remote.set_placeholder_text(Some("host or host:port, several separated by commas"));
    form.row("Gateway", &remote);
    let labels: Vec<&str> = OpenVpnAuth::ALL.iter().map(|a| a.label()).collect();
    let selected = OpenVpnAuth::ALL.iter().position(|a| *a == auth).unwrap_or(0);
    let kind = choice(ctx, "openvpn.auth", &labels, selected, |p, i| if let Some(o) = ovpn(p) { o.put(ovpn_key::CONNECTION_TYPE, OpenVpnAuth::ALL[i].name()) });
    {
        let (ctx, page) = (ctx.clone(), page.clone());
        kind.connect_selected_notify(move |_| {
            let (ctx, page) = (ctx.clone(), page.clone());
            glib::idle_add_local_once(move || openvpn_fields(&ctx, &page));
        });
    }
    form.row("Authentication", &kind);
    if auth == OpenVpnAuth::StaticKey {
        form.row("Static key", &data_file(ctx, "openvpn.static-key", ovpn_key::STATIC_KEY));
        form.row("Key direction", &data_choice(ctx, ovpn_key::STATIC_KEY_DIRECTION, &DIRECTIONS));
        form.row("Local IP address", &data_entry(ctx, "openvpn.local-ip", ovpn_key::LOCAL_IP));
        form.row("Remote IP address", &data_entry(ctx, "openvpn.remote-ip", ovpn_key::REMOTE_IP));
    } else {
        form.row("CA certificate", &data_file(ctx, "openvpn.ca", ovpn_key::CA));
    }
    if auth.uses_certificate() {
        form.row("User certificate", &data_file(ctx, "openvpn.cert", ovpn_key::CERT));
        form.row("Private key", &data_file(ctx, "openvpn.key", ovpn_key::KEY));
        form.row(
            "Private key password",
            &secret_entry(ctx, "openvpn.cert-pass", o.cert_pass.as_str(), |p, v| if let Some(o) = ovpn(p) { o.cert_pass = Secret::new(v) }),
        );
    }
    if auth.uses_password() {
        form.row("User name", &data_entry(ctx, "openvpn.username", ovpn_key::USERNAME));
        let password = secret_entry(ctx, "openvpn.password", o.password.as_str(), |p, v| if let Some(o) = ovpn(p) { o.password = Secret::new(v) });
        password.set_sensitive(!o.ask_password);
        form.row("Password", &password);
        let ask = check(ctx, "Ask for the password at every connect", o.ask_password, |p, on| if let Some(o) = ovpn(p) { o.ask_password = on });
        ask.connect_toggled(move |c| password.set_sensitive(!c.is_active()));
        form.indented(&ask);
    }
    let advanced = gtk::Button::with_label("Advanced...");
    advanced.set_halign(gtk::Align::End);
    {
        let ctx = ctx.clone();
        advanced.connect_clicked(move |_| advanced_dialog(&ctx));
    }
    form.wide(&advanced);
    page.append(form.widget());
}

/// The rarely changed OpenVPN options, edited live like the tab.
fn advanced_dialog(ctx: &Rc<EditorCtx>) {
    let shell = opusdm_ui::DialogShell::new(&ctx.window, "OpenVPN advanced options").scrolled().size(560, 560);
    let general = Form::new();
    general.row("Port (empty = 1194)", &data_entry(ctx, "openvpn.port", ovpn_key::PORT));
    let tcp = data(ctx, ovpn_key::PROTO_TCP) == "yes";
    general.indented(&check(ctx, "Use a TCP connection", tcp, |p, on| if let Some(o) = ovpn(p) { o.put(ovpn_key::PROTO_TCP, if on { "yes" } else { "" }) }));
    general.row("Device type", &data_choice(ctx, ovpn_key::DEV_TYPE, &DEV_TYPES));
    general.row("Tunnel MTU", &data_entry(ctx, "openvpn.tunnel-mtu", ovpn_key::TUNNEL_MTU));
    general.row("TCP MSS fix", &data_entry(ctx, "openvpn.mssfix", ovpn_key::MSSFIX));
    general.row("Compression", &data_entry(ctx, "openvpn.compress", ovpn_key::COMPRESS));
    let security = Form::new();
    let cipher = data_entry(ctx, "openvpn.cipher", ovpn_key::CIPHER);
    cipher.set_placeholder_text(Some("Default (such as AES-256-GCM)"));
    security.row("Cipher", &cipher);
    let hmac = data_entry(ctx, "openvpn.auth-digest", ovpn_key::AUTH);
    hmac.set_placeholder_text(Some("Default (such as SHA256)"));
    security.row("HMAC authentication", &hmac);
    let tls = Form::new();
    let verify = data_entry(ctx, "openvpn.verify-x509-name", ovpn_key::VERIFY_X509_NAME);
    verify.set_placeholder_text(Some("type:name, such as name:vpn.example.com"));
    tls.row("Verify server name", &verify);
    tls.row("Check the peer certificate", &data_choice(ctx, ovpn_key::REMOTE_CERT_TLS, &REMOTE_CERT));
    tls.row("TLS auth key", &data_file(ctx, "openvpn.ta", ovpn_key::TA));
    tls.row("TLS auth direction", &data_choice(ctx, ovpn_key::TA_DIR, &DIRECTIONS));
    tls.row("TLS crypt key", &data_file(ctx, "openvpn.tls-crypt", ovpn_key::TLS_CRYPT));
    let tabs = gtk::Notebook::new();
    tabs.append_page(general.widget(), Some(&gtk::Label::new(Some("General"))));
    tabs.append_page(security.widget(), Some(&gtk::Label::new(Some("Security"))));
    tabs.append_page(tls.widget(), Some(&gtk::Label::new(Some("TLS"))));
    let content = page_box();
    content.append(&tabs);
    content.append(&note("Changes apply to the connection being edited; save it to keep them."));
    shell.content().append(&content);
    shell.close_button("Close");
    let window = shell.present();
    // The new fields are marked like the others.
    window.connect_map({
        let ctx = ctx.clone();
        move |_| ctx.refresh()
    });
}
