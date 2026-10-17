//! SFTP against a real `sshd` started on a loopback port by the test itself.
//! The tests skip (and say so) when `sshd` or `ssh-keygen` is not installed.

use opusdm_ftp::book::{load_known_hosts, trust_host, HostCheck};
use opusdm_ftp::{connect, download, ConnectParams, RemoteError, RemoteItem, RemoteUrl};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// A running sshd plus the files that belong to it.
struct Server {
    child: Child,
    dir: tempfile::TempDir,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_sshd() -> Option<PathBuf> {
    ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"].iter().map(PathBuf::from).find(|p| p.exists())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn keygen(path: &Path) -> bool {
    Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start sshd, or `None` when it cannot be started here.
fn start() -> Option<Server> {
    let sshd = find_sshd()?;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    if !keygen(&p.join("host_key")) || !keygen(&p.join("client_key")) {
        return None;
    }
    std::fs::copy(p.join("client_key.pub"), p.join("authorized_keys")).unwrap();
    let port = free_port();
    let config = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nAuthorizedKeysFile {d}/authorized_keys\n\
         PasswordAuthentication no\nUsePAM no\nStrictModes no\nPidFile {d}/sshd.pid\nSubsystem sftp internal-sftp\n",
        d = p.display()
    );
    std::fs::write(p.join("sshd_config"), config).unwrap();
    let child = Command::new(sshd)
        .args(["-D", "-e", "-f"])
        .arg(p.join("sshd_config"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut server = Server { child, dir, port };
    // Ready means the server answers with its SSH banner, not just accepts.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut probe) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = probe.set_read_timeout(Some(Duration::from_secs(2)));
            let mut banner = [0u8; 4];
            if std::io::Read::read_exact(&mut probe, &mut banner).is_ok() && &banner == b"SSH-" {
                break;
            }
        }
        if Instant::now() > deadline || server.child.try_wait().ok().flatten().is_some() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Keep `server` alive for the caller.
    server.port = port;
    Some(server)
}

fn params(server: &Server) -> ConnectParams {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let url = RemoteUrl::parse(&format!("sftp://{user}@127.0.0.1:{}/", server.port)).unwrap();
    let mut p = ConnectParams::new(url);
    p.identity = Some(server.dir.path().join("client_key"));
    p.known_hosts = server.dir.path().join("known_hosts.json");
    p
}

#[test]
fn first_connection_asks_to_trust_the_host_then_browses_and_downloads() {
    let Some(server) = start() else {
        eprintln!("skipped: no usable sshd on this machine");
        return;
    };
    let params = params(&server);

    // Never seen: the connection stops with the fingerprint to approve.
    let (host, port, fingerprint) = match connect(&params) {
        Err(RemoteError::UnknownHost { host, port, fingerprint }) => (host, port, fingerprint),
        Err(other) => panic!("expected UnknownHost, got {other}"),
        Ok(_) => panic!("expected UnknownHost, got a connection"),
    };
    assert!(fingerprint.contains(':'));
    trust_host(&params.known_hosts, &host, port, &fingerprint).unwrap();
    assert_eq!(load_known_hosts(&params.known_hosts).check(&host, port, &fingerprint), HostCheck::Known);

    // Something to browse.
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("sub")).unwrap();
    std::fs::write(data.path().join("hello.txt"), b"hello over sftp").unwrap();
    std::fs::write(data.path().join("sub/inner.txt"), b"inner").unwrap();

    let mut fs = match connect(&params) {
        Ok(fs) => fs,
        Err(e) => panic!("connect after trusting failed: {e}"),
    };
    let root = data.path().to_string_lossy().into_owned();
    let mut names: Vec<(String, bool)> = fs.list(&root).unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
    names.sort();
    assert_eq!(names, vec![("hello.txt".to_string(), false), ("sub".to_string(), true)]);

    // Copy a file and a folder to a local destination.
    let local = tempfile::tempdir().unwrap();
    let items = vec![
        RemoteItem { path: format!("{root}/hello.txt"), name: "hello.txt".into(), is_dir: false, is_symlink: false },
        RemoteItem { path: format!("{root}/sub"), name: "sub".into(), is_dir: true, is_symlink: false },
    ];
    download(fs.as_mut(), &items, local.path(), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(local.path().join("hello.txt")).unwrap(), b"hello over sftp");
    assert_eq!(std::fs::read(local.path().join("sub/inner.txt")).unwrap(), b"inner");
    fs.close();
}

#[test]
fn a_changed_host_key_is_refused() {
    let Some(server) = start() else {
        eprintln!("skipped: no usable sshd on this machine");
        return;
    };
    let params = params(&server);
    trust_host(&params.known_hosts, "127.0.0.1", server.port, "00:11").unwrap();
    match connect(&params) {
        Err(RemoteError::HostChanged { expected, .. }) => assert_eq!(expected, "00:11"),
        Err(other) => panic!("expected HostChanged, got {other}"),
        Ok(_) => panic!("expected HostChanged, got a connection"),
    }
}

#[test]
fn upload_and_remove_round_trip_on_the_server() {
    let Some(server) = start() else {
        eprintln!("skipped: no usable sshd on this machine");
        return;
    };
    let params = params(&server);
    trust_host(&params.known_hosts, "127.0.0.1", server.port, &match connect(&params) {
        Err(RemoteError::UnknownHost { fingerprint, .. }) => fingerprint,
        _ => panic!("expected UnknownHost"),
    })
    .unwrap();
    let mut fs = connect(&params).unwrap();

    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(local.path().join("d/sub")).unwrap();
    std::fs::write(local.path().join("d/a.txt"), b"AAA").unwrap();
    std::fs::write(local.path().join("d/sub/b.txt"), vec![5u8; 200_000]).unwrap();
    let remote = tempfile::tempdir().unwrap();
    let root = remote.path().to_string_lossy().into_owned();

    opusdm_ftp::upload(fs.as_mut(), &[local.path().join("d")], &root, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(remote.path().join("d/a.txt")).unwrap(), b"AAA");
    assert_eq!(std::fs::read(remote.path().join("d/sub/b.txt")).unwrap().len(), 200_000);
    // Uploading again over existing folders and files works.
    opusdm_ftp::upload(fs.as_mut(), &[local.path().join("d")], &root, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(opusdm_ftp::existing_remote(fs.as_mut(), &root, &["d".to_string(), "x".to_string()]).unwrap(), vec!["d".to_string()]);

    let item = RemoteItem { path: format!("{root}/d"), name: "d".into(), is_dir: true, is_symlink: false };
    opusdm_ftp::remove_tree(fs.as_mut(), &[item], &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(!remote.path().join("d").exists());
    fs.close();
}

#[test]
fn removing_a_link_to_a_folder_keeps_the_folder() {
    let Some(server) = start() else {
        eprintln!("skipped: no usable sshd on this machine");
        return;
    };
    let params = params(&server);
    trust_host(&params.known_hosts, "127.0.0.1", server.port, &match connect(&params) {
        Err(RemoteError::UnknownHost { fingerprint, .. }) => fingerprint,
        _ => panic!("expected UnknownHost"),
    })
    .unwrap();
    let mut fs = connect(&params).unwrap();

    let remote = tempfile::tempdir().unwrap();
    std::fs::create_dir(remote.path().join("target")).unwrap();
    std::fs::write(remote.path().join("target/keep.txt"), b"K").unwrap();
    std::os::unix::fs::symlink(remote.path().join("target"), remote.path().join("link")).unwrap();
    let root = remote.path().to_string_lossy().into_owned();

    let items = opusdm_ftp::resolve_items(fs.as_mut(), &[format!("{root}/link")]).unwrap();
    assert!(items[0].is_symlink && items[0].is_dir);
    opusdm_ftp::remove_tree(fs.as_mut(), &items, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(std::fs::symlink_metadata(remote.path().join("link")).is_err(), "the link is gone");
    assert_eq!(std::fs::read(remote.path().join("target/keep.txt")).unwrap(), b"K");
    fs.close();
}
