//! FTP against a tiny in-process server (USER, PASS, TYPE, PASV, LIST, RETR,
//! QUIT) that serves a fixed tree in passive mode.

use opusdm_ftp::{connect, download, ConnectParams, RemoteError, RemoteItem, RemoteUrl};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;

const LISTING: &str = "drwxr-xr-x   2 ftp ftp      4096 Jan 15 10:30 pub\r\n-rw-r--r--   1 ftp ftp        11 Jan 15  2020 readme.txt\r\n";

fn reply(out: &mut TcpStream, line: &str) {
    out.write_all(format!("{line}\r\n").as_bytes()).unwrap();
}

/// Serve one client; `password` is the only accepted password for user `bob`.
fn serve(stream: TcpStream, password: &'static str) {
    let mut out = stream.try_clone().unwrap();
    let mut input = BufReader::new(stream);
    reply(&mut out, "220 ready");
    let mut user = String::new();
    let mut data: Option<TcpListener> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let text = line.trim_end().to_string();
        let (cmd, arg) = text.split_once(' ').unwrap_or((text.as_str(), ""));
        match cmd.to_ascii_uppercase().as_str() {
            "USER" => {
                user = arg.to_string();
                reply(&mut out, "331 password please");
            }
            "PASS" => {
                if user == "anonymous" || (user == "bob" && arg == password) {
                    reply(&mut out, "230 welcome");
                } else {
                    reply(&mut out, "530 login incorrect");
                }
            }
            "TYPE" => reply(&mut out, "200 ok"),
            "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = listener.local_addr().unwrap().port();
                data = Some(listener);
                reply(&mut out, &format!("227 Entering Passive Mode (127,0,0,1,{},{})", port / 256, port % 256));
            }
            "LIST" | "RETR" => {
                let Some(listener) = data.take() else {
                    reply(&mut out, "425 no data connection");
                    continue;
                };
                let body: Option<&str> = match (cmd.to_ascii_uppercase().as_str(), arg) {
                    ("LIST", _) => Some(LISTING),
                    ("RETR", "/readme.txt") => Some("hello world"),
                    _ => None,
                };
                match body {
                    Some(body) => {
                        reply(&mut out, "150 opening data connection");
                        let (mut conn, _) = listener.accept().unwrap();
                        conn.write_all(body.as_bytes()).unwrap();
                        drop(conn);
                        reply(&mut out, "226 done");
                    }
                    None => reply(&mut out, "550 no such file"),
                }
            }
            "QUIT" => {
                reply(&mut out, "221 bye");
                return;
            }
            _ => reply(&mut out, "502 not implemented"),
        }
    }
}

/// Start the server on a loopback port.
fn start() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream, "secret"));
        }
    });
    port
}

fn params(port: u16, user: &str, password: Option<&str>) -> ConnectParams {
    let at = if user.is_empty() { String::new() } else { format!("{user}@") };
    let mut p = ConnectParams::new(RemoteUrl::parse(&format!("ftp://{at}127.0.0.1:{port}/")).unwrap());
    p.password = password.map(String::from);
    p
}

#[test]
fn anonymous_login_lists_and_downloads() {
    let port = start();
    let mut fs = connect(&params(port, "", None)).unwrap();
    let mut entries = fs.list("/").unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), vec![("pub", true), ("readme.txt", false)]);
    assert_eq!(entries[1].size, 11);

    let local = tempfile::tempdir().unwrap();
    let items = vec![RemoteItem { path: "/readme.txt".into(), name: "readme.txt".into(), is_dir: false }];
    download(fs.as_mut(), &items, local.path(), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(local.path().join("readme.txt")).unwrap(), b"hello world");
    fs.close();
}

#[test]
fn a_named_user_needs_the_right_password() {
    let port = start();
    assert_eq!(connect(&params(port, "bob", None)).err(), Some(RemoteError::AuthRequired));
    assert!(matches!(connect(&params(port, "bob", Some("wrong"))).err(), Some(RemoteError::AuthFailed(_))));
    assert!(connect(&params(port, "bob", Some("secret"))).is_ok());
}

#[test]
fn a_missing_file_fails_and_leaves_nothing_behind() {
    let port = start();
    let mut fs = connect(&params(port, "", None)).unwrap();
    let local = tempfile::tempdir().unwrap();
    let items = vec![RemoteItem { path: "/nope".into(), name: "nope".into(), is_dir: false }];
    assert!(download(fs.as_mut(), &items, local.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
    assert!(!local.path().join("nope").exists());
}
