//! SFTP through libssh2.

use crate::book::{load_known_hosts, HostCheck};
use crate::{ConnectParams, RemoteEntry, RemoteError, RemoteFs, TIMEOUT_SECS};
use ssh2::{HashType, Session, Sftp};
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// An open SFTP session.
pub struct SftpFs {
    session: Session,
    sftp: Sftp,
}

fn failed(e: ssh2::Error) -> RemoteError {
    RemoteError::Failed(e.to_string())
}

/// Lower-case hex of `bytes`, `:` separated.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl SftpFs {
    /// Open the connection, verify the host key and authenticate.
    pub fn connect(params: &ConnectParams) -> Result<Self, RemoteError> {
        let url = &params.url;
        let timeout = Duration::from_secs(TIMEOUT_SECS);
        let addr = (url.host.as_str(), url.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| RemoteError::Failed(format!("cannot resolve {}", url.host)))?;
        let tcp = TcpStream::connect_timeout(&addr, timeout).map_err(|e| RemoteError::Failed(format!("{}:{}: {e}", url.host, url.port)))?;
        let mut session = Session::new().map_err(failed)?;
        session.set_tcp_stream(tcp);
        session.set_timeout((TIMEOUT_SECS * 1000) as u32);
        session.handshake().map_err(failed)?;

        let fingerprint = session.host_key_hash(HashType::Sha256).map(hex).ok_or_else(|| RemoteError::Failed("the server sent no host key".to_string()))?;
        match load_known_hosts(&params.known_hosts).check(&url.host, url.port, &fingerprint) {
            HostCheck::Known => {}
            HostCheck::Unknown => return Err(RemoteError::UnknownHost { host: url.host.clone(), port: url.port, fingerprint }),
            HostCheck::Changed { expected } => {
                return Err(RemoteError::HostChanged { host: url.host.clone(), port: url.port, expected, found: fingerprint })
            }
        }

        let user = url.user.clone().or_else(|| std::env::var("USER").ok()).unwrap_or_default();
        authenticate(&session, &user, params)?;
        let sftp = session.sftp().map_err(failed)?;
        Ok(SftpFs { session, sftp })
    }
}

/// Try the given key, the agent and the default keys, then the password.
fn authenticate(session: &Session, user: &str, params: &ConnectParams) -> Result<(), RemoteError> {
    if let Some(key) = &params.identity {
        if session.userauth_pubkey_file(user, None, key, params.password.as_deref()).is_ok() && session.authenticated() {
            return Ok(());
        }
    }
    if let Some(password) = &params.password {
        return session.userauth_password(user, password).map_err(|e| RemoteError::AuthFailed(e.to_string()));
    }
    if session.userauth_agent(user).is_ok() && session.authenticated() {
        return Ok(());
    }
    if let Some(home) = std::env::var_os("HOME") {
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let key = Path::new(&home).join(".ssh").join(name);
            if key.exists() && session.userauth_pubkey_file(user, None, &key, None).is_ok() && session.authenticated() {
                return Ok(());
            }
        }
    }
    Err(RemoteError::AuthRequired)
}

impl RemoteFs for SftpFs {
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, RemoteError> {
        let entries = self.sftp.readdir(Path::new(path)).map_err(failed)?;
        let mut out = Vec::with_capacity(entries.len());
        for (full, stat) in entries {
            let Some(name) = full.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            if name == "." || name == ".." {
                continue;
            }
            let is_symlink = stat.file_type() == ssh2::FileType::Symlink;
            // A link is shown as what it points to when that can be found out.
            let target = if is_symlink { self.sftp.stat(&full).ok() } else { None };
            let shown = target.as_ref().unwrap_or(&stat);
            out.push(RemoteEntry {
                name,
                is_dir: shown.is_dir(),
                is_symlink,
                size: shown.size.unwrap_or(0),
                modified: shown.mtime.unwrap_or(0) as i64,
                mode: stat.perm.unwrap_or(0) & 0o7777,
            });
        }
        Ok(out)
    }

    fn read_file(&mut self, path: &str, out: &mut dyn Write) -> Result<u64, RemoteError> {
        let mut file = self.sftp.open(Path::new(path)).map_err(failed)?;
        Ok(std::io::copy(&mut file, out)?)
    }

    fn close(&mut self) {
        let _ = self.session.disconnect(None, "bye", None);
    }
}
