//! Remote location syntax: `ftp://`, `ftps://` and `sftp://` URLs typed into
//! the path bar. A [`RemoteUrl`] never carries a password.

use serde::{Deserialize, Serialize};

/// The protocol of a remote site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    /// Plain FTP.
    Ftp,
    /// FTP over explicit TLS (`AUTH TLS`).
    Ftps,
    /// SFTP over SSH.
    #[default]
    Sftp,
}

impl Scheme {
    /// The scheme text, without `://`.
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Ftp => "ftp",
            Scheme::Ftps => "ftps",
            Scheme::Sftp => "sftp",
        }
    }

    /// The port used when a URL gives none.
    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Ftp | Scheme::Ftps => 21,
            Scheme::Sftp => 22,
        }
    }
}

/// A parsed remote location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    /// Protocol.
    pub scheme: Scheme,
    /// User name; `None` means anonymous (FTP) or the local user (SFTP).
    pub user: Option<String>,
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Absolute path on the server, `/` for the root, no trailing `/`.
    pub path: String,
}

/// Whether `text` starts with a remote scheme.
pub fn is_remote(text: &str) -> bool {
    ["ftp://", "ftps://", "sftp://"].iter().any(|p| text.len() >= p.len() && text[..p.len()].eq_ignore_ascii_case(p))
}

/// Collapse `//`, drop a trailing `/` and make the path absolute. `.` and
/// `..` segments are resolved, never climbing above the root.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

impl RemoteUrl {
    /// Parse `scheme://[user@]host[:port][/path]`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (scheme, rest) = [Scheme::Ftp, Scheme::Ftps, Scheme::Sftp]
            .into_iter()
            .find_map(|s| {
                let prefix = format!("{}://", s.as_str());
                (text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(&prefix)).then(|| (s, &text[prefix.len()..]))
            })
            .ok_or_else(|| format!("not a remote address (use ftp://, ftps:// or sftp://): {text:?}"))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (user, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h),
            Some((_, h)) => (None, h),
            None => (None, authority),
        };
        if user.as_deref().is_some_and(|u| u.contains(':')) {
            return Err("a password cannot be part of the address, it is asked when connecting".to_string());
        }
        let (host, port) = split_host_port(hostport, scheme)?;
        Ok(RemoteUrl { scheme, user, host, port, path: normalize_path(path) })
    }

    /// The same location one level up, `None` at the root.
    pub fn parent(&self) -> Option<RemoteUrl> {
        if self.path == "/" {
            return None;
        }
        let cut = self.path.rfind('/').unwrap_or(0);
        let path = if cut == 0 { "/".to_string() } else { self.path[..cut].to_string() };
        Some(RemoteUrl { path, ..self.clone() })
    }

    /// The location of the entry `name` inside this one.
    pub fn join(&self, name: &str) -> RemoteUrl {
        RemoteUrl { path: normalize_path(&format!("{}/{}", self.path, name)), ..self.clone() }
    }

    /// What identifies one connection: everything but the path.
    pub fn session_key(&self) -> String {
        format!("{}://{}@{}:{}", self.scheme.as_str(), self.user.as_deref().unwrap_or(""), self.host, self.port)
    }
}

/// Split `host[:port]`, accepting `[v6]:port`.
fn split_host_port(text: &str, scheme: Scheme) -> Result<(String, u16), String> {
    if text.is_empty() {
        return Err("the address has no host".to_string());
    }
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or("missing ']' in the address")?;
        (h, after.strip_prefix(':'))
    } else {
        match text.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (text, None),
        }
    };
    if host.is_empty() {
        return Err("the address has no host".to_string());
    }
    let port = match port {
        Some(p) => p.parse::<u16>().map_err(|_| format!("bad port {p:?}"))?,
        None => scheme.default_port(),
    };
    Ok((host.to_string(), port))
}

impl std::fmt::Display for RemoteUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", self.scheme.as_str())?;
        if let Some(u) = &self.user {
            write!(f, "{u}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            write!(f, "{}", self.host)?;
        }
        if self.port != self.scheme.default_port() {
            write!(f, ":{}", self.port)?;
        }
        write!(f, "{}", self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_part_and_defaults() {
        let u = RemoteUrl::parse("sftp://bob@example.org:2222/srv/data").unwrap();
        assert_eq!((u.scheme, u.user.as_deref(), u.host.as_str(), u.port, u.path.as_str()), (Scheme::Sftp, Some("bob"), "example.org", 2222, "/srv/data"));
        let u = RemoteUrl::parse("FTP://host").unwrap();
        assert_eq!((u.scheme, u.user, u.port, u.path.as_str()), (Scheme::Ftp, None, 21, "/"));
        assert_eq!(RemoteUrl::parse("ftps://h/x/").unwrap().path, "/x");
        assert_eq!(RemoteUrl::parse("sftp://[::1]:2200/a").unwrap().host, "::1");
    }

    #[test]
    fn rejects_bad_addresses() {
        assert!(RemoteUrl::parse("http://h/").is_err());
        assert!(RemoteUrl::parse("sftp://").is_err());
        assert!(RemoteUrl::parse("sftp://u:pw@h/").is_err());
        assert!(RemoteUrl::parse("sftp://h:99999/").is_err());
        assert!(RemoteUrl::parse("sftp://u@:22/").is_err());
    }

    #[test]
    fn display_round_trips_and_omits_the_default_port() {
        for text in ["sftp://bob@h/a/b", "ftp://h:2121/", "ftps://u@h/x", "sftp://[::1]:2200/a"] {
            assert_eq!(RemoteUrl::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(RemoteUrl::parse("sftp://h:22/a").unwrap().to_string(), "sftp://h/a");
    }

    #[test]
    fn parent_and_join_stay_inside_the_server() {
        let u = RemoteUrl::parse("sftp://h/a/b").unwrap();
        assert_eq!(u.parent().unwrap().path, "/a");
        assert_eq!(u.parent().unwrap().parent().unwrap().path, "/");
        assert!(u.parent().unwrap().parent().unwrap().parent().is_none());
        assert_eq!(u.join("c d").path, "/a/b/c d");
        assert_eq!(u.join("..").path, "/a");
        assert_eq!(u.join("../../..").path, "/");
    }

    #[test]
    fn session_key_ignores_the_path() {
        let a = RemoteUrl::parse("sftp://u@h/a").unwrap();
        let b = RemoteUrl::parse("sftp://u@h/b").unwrap();
        assert_eq!(a.session_key(), b.session_key());
        assert_ne!(a.session_key(), RemoteUrl::parse("sftp://v@h/a").unwrap().session_key());
    }

    #[test]
    fn is_remote_checks_the_scheme_only() {
        assert!(is_remote("sftp://h"));
        assert!(is_remote("FTPS://h"));
        assert!(!is_remote("/home/sftp://x"));
        assert!(!is_remote("ftp:/h"));
    }
}
