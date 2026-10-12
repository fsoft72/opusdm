//! Import of an OpenVPN client configuration (`.ovpn`) into the settings of
//! the NetworkManager OpenVPN plugin.

use crate::error::NetError;
use crate::import::{Imported, PendingFile};
use crate::profile::{ovpn_key, Detail, OpenVpnAuth, Profile, ProfileKind};
use std::path::{Path, PathBuf};

/// Directives that only matter to the `openvpn` command line client, or that
/// the plugin always sets itself: accepted without a note.
const HARMLESS: &[&str] = &[
    "client",
    "nobind",
    "persist-key",
    "persist-tun",
    "resolv-retry",
    "verb",
    "mute",
    "mute-replay-warnings",
    "pull",
    "tls-client",
    "auth-nocache",
    "explicit-exit-notify",
    "redirect-gateway",
    "remote-random",
    "setenv",
    "block-outside-dns",
];

/// The inline blocks: the tag and the `vpn.data` key that gets its file.
const INLINE: &[(&str, &str)] = &[
    ("ca", ovpn_key::CA),
    ("cert", ovpn_key::CERT),
    ("key", ovpn_key::KEY),
    ("tls-auth", ovpn_key::TA),
    ("tls-crypt", ovpn_key::TLS_CRYPT),
    ("secret", ovpn_key::STATIC_KEY),
];

/// What the directives say, before it becomes a profile.
#[derive(Default)]
struct Parsed {
    remotes: Vec<String>,
    port: Option<String>,
    proto: Option<String>,
    user_pass: bool,
    pkcs12: Option<String>,
    key_direction: Option<String>,
}

/// Read `text`, a configuration named `name`. Relative file names are found
/// in `base_dir` (the folder of the file); inline blocks become files in
/// `files_dir`, written only when the profile is saved.
pub fn parse(text: &str, name: &str, base_dir: &Path, files_dir: &Path) -> Result<Imported, NetError> {
    let mut profile = Profile::new(ProfileKind::OpenVpn)?;
    profile.id = name.to_string();
    let Detail::OpenVpn(vpn) = &mut profile.detail else { unreachable!("a new OpenVPN profile has OpenVPN details") };
    let mut parsed = Parsed::default();
    let mut ignored = Vec::new();
    let mut files = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, raw)) = lines.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
        let err = |what: &str| NetError::other(format!("Line {}: {what}", n + 1));
        if let Some(tag) = line.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
            let end = format!("</{tag}>");
            let mut body = String::new();
            loop {
                let (_, l) = lines.next().ok_or_else(|| err(&format!("<{tag}> is not closed")))?;
                if l.trim() == end { break; }
                body.push_str(l);
                body.push('\n');
            }
            match INLINE.iter().find(|(t, _)| *t == tag) {
                Some((_, key)) => {
                    let path = files_dir.join(format!("{tag}.pem"));
                    vpn.put(key, &path.to_string_lossy());
                    files.push(PendingFile { path, contents: body });
                }
                None => ignored.push(format!("<{tag}>")),
            }
            continue;
        }
        let words = split_words(line);
        let (directive, args) = words.split_first().ok_or_else(|| err("empty directive"))?;
        let arg = |i: usize| args.get(i).cloned().ok_or_else(|| err(&format!("{directive} needs a value")));
        let file = |i: usize| arg(i).map(|f| resolve(base_dir, &f));
        match directive.as_str() {
            "remote" => {
                let mut r = arg(0)?;
                if let Some(port) = args.get(1) {
                    r.push(':');
                    r.push_str(port);
                    if let Some(proto) = args.get(2) {
                        r.push(':');
                        r.push_str(proto);
                    }
                }
                parsed.remotes.push(r);
            }
            "port" | "rport" => parsed.port = Some(arg(0)?),
            "proto" => parsed.proto = Some(arg(0)?),
            "dev" => vpn.put(ovpn_key::DEV_TYPE, if arg(0)?.starts_with("tap") { "tap" } else { "tun" }),
            "dev-type" => vpn.put(ovpn_key::DEV_TYPE, &arg(0)?),
            "ca" => vpn.put(ovpn_key::CA, &file(0)?),
            "cert" => vpn.put(ovpn_key::CERT, &file(0)?),
            "key" => vpn.put(ovpn_key::KEY, &file(0)?),
            "pkcs12" => parsed.pkcs12 = Some(file(0)?),
            "tls-auth" => {
                // An inline block leaves `[inline]` as the file name.
                if arg(0)? != "[inline]" { vpn.put(ovpn_key::TA, &file(0)?); }
                if let Some(dir) = args.get(1) { parsed.key_direction = Some(dir.clone()); }
            }
            "tls-crypt" => {
                if arg(0)? != "[inline]" { vpn.put(ovpn_key::TLS_CRYPT, &file(0)?); }
            }
            "secret" => {
                if arg(0)? != "[inline]" { vpn.put(ovpn_key::STATIC_KEY, &file(0)?); }
                if let Some(dir) = args.get(1) { parsed.key_direction = Some(dir.clone()); }
            }
            "key-direction" => parsed.key_direction = Some(arg(0)?),
            "auth-user-pass" => {
                parsed.user_pass = true;
                if !args.is_empty() { ignored.push("auth-user-pass file (the password is asked or kept in the keyring)".to_string()); }
            }
            "cipher" => vpn.put(ovpn_key::CIPHER, &arg(0)?),
            "data-ciphers" => vpn.put("data-ciphers", &arg(0)?),
            "auth" => vpn.put(ovpn_key::AUTH, &arg(0)?),
            "comp-lzo" => vpn.put(ovpn_key::COMP_LZO, args.first().map(String::as_str).unwrap_or("adaptive")),
            "compress" => vpn.put(ovpn_key::COMPRESS, args.first().map(String::as_str).unwrap_or("yes")),
            "verify-x509-name" => {
                let kind = args.get(1).cloned().unwrap_or_else(|| "subject".to_string());
                vpn.put(ovpn_key::VERIFY_X509_NAME, &format!("{kind}:{}", arg(0)?));
            }
            "remote-cert-tls" => vpn.put(ovpn_key::REMOTE_CERT_TLS, &arg(0)?),
            "tun-mtu" => vpn.put(ovpn_key::TUNNEL_MTU, &arg(0)?),
            "mssfix" => vpn.put(ovpn_key::MSSFIX, args.first().map(String::as_str).unwrap_or("yes")),
            "ping" => vpn.put("ping", &arg(0)?),
            "ping-restart" => vpn.put("ping-restart", &arg(0)?),
            "ping-exit" => vpn.put("ping-exit", &arg(0)?),
            "reneg-sec" => vpn.put("reneg-seconds", &arg(0)?),
            "float" => vpn.put("float", "yes"),
            "tls-version-min" => vpn.put("tls-version-min", &arg(0)?),
            "tls-version-max" => vpn.put("tls-version-max", &arg(0)?),
            "tls-cipher" => vpn.put("tls-cipher", &arg(0)?),
            "ifconfig" => {
                vpn.put(ovpn_key::LOCAL_IP, &arg(0)?);
                vpn.put(ovpn_key::REMOTE_IP, &arg(1)?);
            }
            d if HARMLESS.contains(&d) => {}
            d => ignored.push(d.to_string()),
        }
    }
    finish(vpn, parsed)?;
    Ok(Imported { profile, ignored, files })
}

/// Turn the collected directives into the plugin keys that depend on more
/// than one of them.
fn finish(vpn: &mut crate::profile::OpenVpnDetail, parsed: Parsed) -> Result<(), NetError> {
    if parsed.remotes.is_empty() { return Err(NetError::other("The file has no remote")); }
    vpn.put(ovpn_key::REMOTE, &parsed.remotes.join(", "));
    if let Some(port) = parsed.port { vpn.put(ovpn_key::PORT, &port); }
    if parsed.proto.as_deref().is_some_and(|p| p.starts_with("tcp")) { vpn.put(ovpn_key::PROTO_TCP, "yes"); }
    if let Some(p12) = parsed.pkcs12 {
        // The plugin reads a PKCS#12 bundle given as all three files.
        for key in [ovpn_key::CA, ovpn_key::CERT, ovpn_key::KEY] {
            if vpn.get(key).is_empty() { vpn.put(key, &p12); }
        }
    }
    if let Some(dir) = parsed.key_direction {
        let key = if vpn.get(ovpn_key::STATIC_KEY).is_empty() { ovpn_key::TA_DIR } else { ovpn_key::STATIC_KEY_DIRECTION };
        vpn.put(key, &dir);
    }
    let has_cert = !vpn.get(ovpn_key::CERT).is_empty();
    let auth = if !vpn.get(ovpn_key::STATIC_KEY).is_empty() {
        OpenVpnAuth::StaticKey
    } else if parsed.user_pass && has_cert {
        OpenVpnAuth::PasswordTls
    } else if parsed.user_pass {
        OpenVpnAuth::Password
    } else {
        OpenVpnAuth::Tls
    };
    vpn.put(ovpn_key::CONNECTION_TYPE, auth.name());
    Ok(())
}

/// Split a line in words; double or single quotes keep spaces in a word.
fn split_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started { out.push(word); }
    out
}

/// A file name of the configuration as an absolute path.
fn resolve(base_dir: &Path, file: &str) -> String {
    let path = PathBuf::from(file);
    if path.is_absolute() { return file.to_string(); }
    base_dir.join(path).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
client
dev tun
proto tcp
remote 212.31.246.91 443
remote 80.169.235.10 443 tcp
resolv-retry infinite
nobind
persist-key
cipher AES-256-GCM
auth-user-pass
tls-version-min 1.2
reneg-sec 28800
ping 10
remote-cert-tls server
cert client.crt
key-direction 1
route-nopull
<ca>
-----BEGIN CERTIFICATE-----
MIIB
-----END CERTIFICATE-----
</ca>
<key>
-----BEGIN PRIVATE KEY-----
MIIE
-----END PRIVATE KEY-----
</key>
<tls-auth>
-----BEGIN OpenVPN Static key V1-----
abcd
-----END OpenVPN Static key V1-----
</tls-auth>
";

    fn import(text: &str) -> Result<Imported, NetError> {
        parse(text, "work", Path::new("/home/u/vpn"), Path::new("/data/u1"))
    }

    fn data(imported: &Imported) -> &crate::profile::OpenVpnDetail {
        match &imported.profile.detail {
            Detail::OpenVpn(v) => v,
            _ => panic!("not OpenVPN"),
        }
    }

    #[test]
    fn a_full_file_is_read() {
        let imported = import(SAMPLE).unwrap();
        let v = data(&imported);
        assert_eq!(imported.profile.id, "work");
        assert_eq!(v.get(ovpn_key::REMOTE), "212.31.246.91:443, 80.169.235.10:443:tcp");
        assert_eq!(v.get(ovpn_key::PROTO_TCP), "yes");
        assert_eq!(v.get(ovpn_key::DEV_TYPE), "tun");
        assert_eq!(v.get(ovpn_key::CIPHER), "AES-256-GCM");
        assert_eq!(v.get(ovpn_key::REMOTE_CERT_TLS), "server");
        assert_eq!(v.get("reneg-seconds"), "28800");
        assert_eq!(v.get(ovpn_key::CERT), "/home/u/vpn/client.crt");
        assert_eq!(v.get(ovpn_key::CA), "/data/u1/ca.pem");
        assert_eq!(v.get(ovpn_key::KEY), "/data/u1/key.pem");
        assert_eq!(v.get(ovpn_key::TA), "/data/u1/tls-auth.pem");
        assert_eq!(v.get(ovpn_key::TA_DIR), "1");
        assert_eq!(v.auth(), OpenVpnAuth::PasswordTls);
        assert_eq!(imported.ignored, vec!["route-nopull".to_string()]);
        assert_eq!(imported.files.len(), 3);
        assert_eq!(imported.files[0].path, PathBuf::from("/data/u1/ca.pem"));
        assert_eq!(imported.files[0].contents, "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n");
    }

    #[test]
    fn the_authentication_follows_the_directives() {
        let base = "remote h\nca ca.crt\n";
        let auth = |extra: &str| data(&import(&format!("{base}{extra}")).unwrap()).auth();
        assert_eq!(auth(""), OpenVpnAuth::Tls);
        assert_eq!(auth("auth-user-pass\n"), OpenVpnAuth::Password);
        assert_eq!(auth("auth-user-pass\ncert c.crt\n"), OpenVpnAuth::PasswordTls);
        assert_eq!(auth("secret s.key 0\n"), OpenVpnAuth::StaticKey);
        let v = import(&format!("{base}secret s.key 0\n")).unwrap();
        assert_eq!(data(&v).get(ovpn_key::STATIC_KEY_DIRECTION), "0");
    }

    #[test]
    fn a_pkcs12_bundle_fills_the_three_files() {
        let v = import("remote h\npkcs12 me.p12\n").unwrap();
        for key in [ovpn_key::CA, ovpn_key::CERT, ovpn_key::KEY] {
            assert_eq!(data(&v).get(key), "/home/u/vpn/me.p12");
        }
    }

    #[test]
    fn quoted_names_keep_their_spaces() {
        assert_eq!(split_words("ca \"my ca.crt\""), vec!["ca".to_string(), "my ca.crt".to_string()]);
        assert_eq!(split_words("x '' y"), vec!["x".to_string(), String::new(), "y".to_string()]);
        let v = import("remote h\nca \"/etc/my ca.crt\"\n").unwrap();
        assert_eq!(data(&v).get(ovpn_key::CA), "/etc/my ca.crt");
    }

    #[test]
    fn broken_files_say_why() {
        assert!(import("client\n").unwrap_err().text().contains("no remote"));
        assert!(import("remote h\n<ca>\nabc\n").unwrap_err().text().contains("Line 2"));
        assert!(import("remote h\nca\n").unwrap_err().text().contains("ca needs a value"));
    }
}
