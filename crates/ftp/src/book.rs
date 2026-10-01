//! The address book (`ftp.json`) and the known-hosts file for SFTP host key
//! fingerprints.

use crate::url::{RemoteUrl, Scheme};
use opusdm_ipc::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// One saved remote site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Site {
    /// Name shown in the address book.
    pub name: String,
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// User name; empty means anonymous (FTP) or the local user (SFTP).
    pub user: String,
    /// Protocol.
    pub protocol: Scheme,
    /// FTP passive mode (the default and the only one that crosses NAT).
    pub passive: bool,
    /// Verify the server certificate (FTPS only).
    pub tls_verify: bool,
    /// Folder to open first.
    pub path: String,
}

impl Default for Site {
    fn default() -> Self {
        Site {
            name: String::new(),
            host: String::new(),
            port: Scheme::default().default_port(),
            user: String::new(),
            protocol: Scheme::default(),
            passive: true,
            tls_verify: true,
            path: "/".to_string(),
        }
    }
}

impl Site {
    /// The location this site opens.
    pub fn url(&self) -> RemoteUrl {
        RemoteUrl {
            scheme: self.protocol,
            user: (!self.user.is_empty()).then(|| self.user.clone()),
            host: self.host.clone(),
            port: self.port,
            path: if self.path.starts_with('/') { self.path.clone() } else { format!("/{}", self.path) },
        }
    }

    /// The first problem that makes the site unusable.
    pub fn problem(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("the site has no name".to_string());
        }
        if self.host.trim().is_empty() {
            return Some(format!("{}: the host is empty", self.name));
        }
        if self.port == 0 {
            return Some(format!("{}: the port must not be 0", self.name));
        }
        None
    }
}

/// The saved sites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AddressBook {
    /// Sites in display order.
    pub sites: Vec<Site>,
}

impl AddressBook {
    /// The site called `name`, case-insensitively.
    pub fn find(&self, name: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

/// Path to `ftp.json`.
pub fn book_path() -> PathBuf {
    config_dir().join("ftp.json")
}

/// Load the address book from the standard path (empty when missing or bad).
pub fn load_book() -> AddressBook {
    load_json(&book_path(), "opusdm", "FTP address book")
}

/// Save the address book to the standard path.
pub fn save_book(book: &AddressBook) -> io::Result<()> {
    save_json(&book_path(), book)
}

/// What the known-hosts file says about a server's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCheck {
    /// The fingerprint matches the stored one.
    Known,
    /// Never seen: the user must approve it.
    Unknown,
    /// A different fingerprint is stored: refuse to connect.
    Changed {
        /// The fingerprint on record.
        expected: String,
    },
}

/// Trusted SFTP host key fingerprints, by `host:port`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KnownHosts {
    /// `host:port` to hex SHA-256 fingerprint.
    pub hosts: BTreeMap<String, String>,
}

impl KnownHosts {
    fn key(host: &str, port: u16) -> String {
        format!("{}:{port}", host.to_ascii_lowercase())
    }

    /// Compare `fingerprint` with the record for `host:port`.
    pub fn check(&self, host: &str, port: u16, fingerprint: &str) -> HostCheck {
        match self.hosts.get(&Self::key(host, port)) {
            None => HostCheck::Unknown,
            Some(stored) if stored.eq_ignore_ascii_case(fingerprint) => HostCheck::Known,
            Some(stored) => HostCheck::Changed { expected: stored.clone() },
        }
    }

    /// Remember `fingerprint` for `host:port`.
    pub fn trust(&mut self, host: &str, port: u16, fingerprint: &str) {
        self.hosts.insert(Self::key(host, port), fingerprint.to_string());
    }
}

/// Path to the known-hosts file.
pub fn known_hosts_path() -> PathBuf {
    config_dir().join("known_hosts.json")
}

/// Load the known hosts from `path` (empty when missing or bad).
pub fn load_known_hosts(path: &Path) -> KnownHosts {
    load_json(path, "opusdm", "known hosts")
}

/// Save the known hosts to `path`.
pub fn save_known_hosts(path: &Path, hosts: &KnownHosts) -> io::Result<()> {
    save_json(path, hosts)
}

/// Approve `fingerprint` for `host:port` in the file at `path`.
pub fn trust_host(path: &Path, host: &str, port: u16, fingerprint: &str) -> io::Result<()> {
    let mut hosts = load_known_hosts(path);
    hosts.trust(host, port, fingerprint);
    save_known_hosts(path, &hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_url_and_problems() {
        let site = Site { name: "Home".into(), host: "nas".into(), user: "me".into(), path: "data".into(), port: 2222, ..Site::default() };
        assert_eq!(site.url().to_string(), "sftp://me@nas:2222/data");
        assert!(site.problem().is_none());
        assert!(Site { name: " ".into(), ..site.clone() }.problem().is_some());
        assert!(Site { host: "".into(), ..site.clone() }.problem().is_some());
        assert!(Site { port: 0, ..site }.problem().is_some());
    }

    #[test]
    fn a_site_missing_fields_gets_defaults() {
        let site: Site = serde_json::from_str(r#"{"name":"x","host":"h"}"#).unwrap();
        assert!(site.passive && site.tls_verify);
        assert_eq!((site.protocol, site.port), (Scheme::Sftp, 22));
    }

    #[test]
    fn known_hosts_distinguish_unknown_known_and_changed() {
        let mut hosts = KnownHosts::default();
        assert_eq!(hosts.check("H", 22, "aa"), HostCheck::Unknown);
        hosts.trust("h", 22, "aa");
        assert_eq!(hosts.check("H", 22, "AA"), HostCheck::Known);
        assert_eq!(hosts.check("h", 22, "bb"), HostCheck::Changed { expected: "aa".into() });
        assert_eq!(hosts.check("h", 2222, "aa"), HostCheck::Unknown);
    }

    #[test]
    fn trust_host_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kh.json");
        trust_host(&path, "h", 22, "ff").unwrap();
        assert_eq!(load_known_hosts(&path).check("h", 22, "ff"), HostCheck::Known);
    }
}
