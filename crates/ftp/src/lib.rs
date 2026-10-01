//! Remote file access for OpusDM (F-39): FTP, FTPS and SFTP behind one small
//! [`RemoteFs`] trait, plus the address book, the known-hosts file and a safe
//! recursive download. No GTK; the Lister drives it from worker threads.

pub mod book;
mod ftp_backend;
mod sftp_backend;
pub mod transfer;
pub mod url;

use std::io::Write;
use std::path::PathBuf;

pub use book::{AddressBook, HostCheck, KnownHosts, Site};
pub use transfer::{download, RemoteItem};
pub use url::{is_remote, RemoteUrl, Scheme};

/// Everything that can go wrong talking to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// The host key was never seen: ask the user, then call
    /// [`book::trust_host`] and connect again.
    UnknownHost { host: String, port: u16, fingerprint: String },
    /// The host key differs from the one on record: refuse.
    HostChanged { host: String, port: u16, expected: String, found: String },
    /// A password is needed and none was given.
    AuthRequired,
    /// The server rejected the credentials.
    AuthFailed(String),
    /// The network, the protocol or the server failed.
    Failed(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::UnknownHost { host, fingerprint, .. } => write!(f, "unknown host key for {host}: {fingerprint}"),
            RemoteError::HostChanged { host, expected, found, .. } => {
                write!(f, "the host key of {host} CHANGED (expected {expected}, got {found}): refusing to connect")
            }
            RemoteError::AuthRequired => write!(f, "a password is required"),
            RemoteError::AuthFailed(m) => write!(f, "login failed: {m}"),
            RemoteError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RemoteError {}

impl From<std::io::Error> for RemoteError {
    fn from(e: std::io::Error) -> Self {
        RemoteError::Failed(e.to_string())
    }
}

/// One entry of a remote directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Name, no directory part.
    pub name: String,
    /// A directory (a symlink to one counts when the server resolves it).
    pub is_dir: bool,
    /// The entry itself is a symbolic link.
    pub is_symlink: bool,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub modified: i64,
    /// Permission bits when the protocol gives them, else 0.
    pub mode: u32,
}

/// A connected server.
pub trait RemoteFs: Send {
    /// List the directory at `path`, without `.` and `..`.
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, RemoteError>;
    /// Stream the file at `path` into `out`; returns the bytes written.
    fn read_file(&mut self, path: &str, out: &mut dyn Write) -> Result<u64, RemoteError>;
    /// Say goodbye to the server. Errors are not interesting here.
    fn close(&mut self);
}

/// How to reach a server.
#[derive(Debug, Clone)]
pub struct ConnectParams {
    /// Where to connect (the path is ignored).
    pub url: RemoteUrl,
    /// The password; `None` tries keys and the agent (SFTP) or asks for one.
    pub password: Option<String>,
    /// A private key file to try first (SFTP).
    pub identity: Option<PathBuf>,
    /// FTP passive mode.
    pub passive: bool,
    /// Verify the server certificate (FTPS).
    pub tls_verify: bool,
    /// The known-hosts file (SFTP).
    pub known_hosts: PathBuf,
}

impl ConnectParams {
    /// Defaults for `url`: passive mode, verified TLS, the standard known-hosts file.
    pub fn new(url: RemoteUrl) -> Self {
        ConnectParams { url, password: None, identity: None, passive: true, tls_verify: true, known_hosts: book::known_hosts_path() }
    }
}

/// Connect and log in.
pub fn connect(params: &ConnectParams) -> Result<Box<dyn RemoteFs>, RemoteError> {
    match params.url.scheme {
        Scheme::Sftp => Ok(Box::new(sftp_backend::SftpFs::connect(params)?)),
        Scheme::Ftp | Scheme::Ftps => Ok(Box::new(ftp_backend::FtpFs::connect(params)?)),
    }
}

/// Seconds to wait for a connection or a reply.
pub(crate) const TIMEOUT_SECS: u64 = 30;
