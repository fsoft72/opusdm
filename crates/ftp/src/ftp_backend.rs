//! FTP and explicit FTPS through suppaftp.

use crate::{ConnectParams, RemoteEntry, RemoteError, RemoteFs, Scheme, TIMEOUT_SECS};
use suppaftp::list::File;
use suppaftp::native_tls::TlsConnector;
use suppaftp::types::FileType;
use suppaftp::{FtpError, FtpStream, Mode, NativeTlsConnector, NativeTlsFtpStream};
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::time::{Duration, UNIX_EPOCH};

/// An FTP control connection, with or without TLS.
enum Conn {
    Plain(FtpStream),
    Tls(NativeTlsFtpStream),
}

/// Run `$body` with `$s` bound to the stream, whichever kind it is.
macro_rules! with_stream {
    ($conn:expr, $s:ident => $body:expr) => {
        match $conn {
            Conn::Plain($s) => $body,
            Conn::Tls($s) => $body,
        }
    };
}

/// An open FTP session.
pub struct FtpFs {
    conn: Conn,
}

fn failed(e: FtpError) -> RemoteError {
    RemoteError::Failed(e.to_string())
}

impl FtpFs {
    /// Connect, switch to TLS for `ftps`, and log in.
    pub fn connect(params: &ConnectParams) -> Result<Self, RemoteError> {
        let url = &params.url;
        let user = url.user.clone().unwrap_or_else(|| "anonymous".to_string());
        let password = match (&url.user, &params.password) {
            (_, Some(p)) => p.clone(),
            (None, None) => "anonymous@".to_string(),
            (Some(_), None) => return Err(RemoteError::AuthRequired),
        };
        let addr = (url.host.as_str(), url.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| RemoteError::Failed(format!("cannot resolve {}", url.host)))?;
        let timeout = Duration::from_secs(TIMEOUT_SECS);
        let mode = if params.passive { Mode::Passive } else { Mode::Active };
        let login_failed = |e: FtpError| RemoteError::AuthFailed(e.to_string());

        let conn = if url.scheme == Scheme::Ftps {
            let tls = TlsConnector::builder()
                .danger_accept_invalid_certs(!params.tls_verify)
                .danger_accept_invalid_hostnames(!params.tls_verify)
                .build()
                .map_err(|e| RemoteError::Failed(e.to_string()))?;
            let mut s = NativeTlsFtpStream::connect_timeout(addr, timeout)
                .map_err(failed)?
                .into_secure(NativeTlsConnector::from(tls), &url.host)
                .map_err(failed)?;
            s.login(user.as_str(), password.as_str()).map_err(login_failed)?;
            s.set_mode(mode);
            s.transfer_type(FileType::Binary).map_err(failed)?;
            Conn::Tls(s)
        } else {
            let mut s = FtpStream::connect_timeout(addr, timeout).map_err(failed)?;
            s.login(user.as_str(), password.as_str()).map_err(login_failed)?;
            s.set_mode(mode);
            s.transfer_type(FileType::Binary).map_err(failed)?;
            Conn::Plain(s)
        };
        Ok(FtpFs { conn })
    }
}

/// Turn one `LIST` line into an entry; lines that are not entries (`total 4`)
/// give `None`.
pub(crate) fn parse_list_line(line: &str) -> Option<RemoteEntry> {
    let file = File::try_from(line).ok()?;
    let name = file.name().to_string();
    if name == "." || name == ".." {
        return None;
    }
    let modified = file.modified().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    Some(RemoteEntry {
        name,
        is_dir: file.is_directory(),
        is_symlink: file.is_symlink(),
        size: file.size() as u64,
        modified,
        mode: 0,
    })
}

impl RemoteFs for FtpFs {
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, RemoteError> {
        let lines = with_stream!(&mut self.conn, s => s.list(Some(path))).map_err(failed)?;
        Ok(lines.iter().filter_map(|l| parse_list_line(l)).collect())
    }

    fn read_file(&mut self, path: &str, out: &mut dyn Write) -> Result<u64, RemoteError> {
        with_stream!(&mut self.conn, s => {
            let mut data = s.retr_as_stream(path).map_err(failed)?;
            let copied = std::io::copy(&mut data, out)?;
            s.finalize_retr_stream(data).map_err(failed)?;
            Ok(copied)
        })
    }

    fn write_file(&mut self, path: &str, data: &mut dyn Read) -> Result<u64, RemoteError> {
        let mut reader = data;
        with_stream!(&mut self.conn, s => s.put_file(path, &mut reader)).map_err(failed)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), RemoteError> {
        let made = with_stream!(&mut self.conn, s => s.mkdir(path));
        match made {
            Ok(()) => Ok(()),
            // Already there: listing it works.
            Err(e) => match with_stream!(&mut self.conn, s => s.list(Some(path))) {
                Ok(_) => Ok(()),
                Err(_) => Err(failed(e)),
            },
        }
    }

    fn remove_file(&mut self, path: &str) -> Result<(), RemoteError> {
        with_stream!(&mut self.conn, s => s.rm(path)).map_err(failed)
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), RemoteError> {
        with_stream!(&mut self.conn, s => s.rmdir(path)).map_err(failed)
    }

    fn close(&mut self) {
        let _ = with_stream!(&mut self.conn, s => s.quit());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_list_lines_are_parsed() {
        let e = parse_list_line("drwxr-xr-x   2 user group      4096 Jan 15 10:30 My Dir").unwrap();
        assert_eq!((e.name.as_str(), e.is_dir), ("My Dir", true));
        let e = parse_list_line("-rw-r--r--   1 user group      1234 Jan 15  2020 a.txt").unwrap();
        assert_eq!((e.name.as_str(), e.is_dir, e.size), ("a.txt", false, 1234));
        let e = parse_list_line("lrwxrwxrwx   1 user group         5 Jan 15  2020 ln -> a.txt").unwrap();
        assert!(e.is_symlink);
    }

    #[test]
    fn dos_list_lines_are_parsed() {
        let e = parse_list_line("01-15-20  10:30AM       <DIR>          Folder").unwrap();
        assert_eq!((e.name.as_str(), e.is_dir), ("Folder", true));
    }

    #[test]
    fn dot_entries_and_noise_are_dropped() {
        assert!(parse_list_line("total 12").is_none());
        assert!(parse_list_line("drwxr-xr-x   2 user group      4096 Jan 15 10:30 .").is_none());
        assert!(parse_list_line("drwxr-xr-x   2 user group      4096 Jan 15 10:30 ..").is_none());
    }
}
