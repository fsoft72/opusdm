//! Encrypt / Decrypt (F-14): password-based file encryption.
//!
//! Format (`.enc`): the 8 byte magic `OPUSENC1`, a 16 byte random salt, a
//! 19 byte random nonce prefix, then the data in 64 KiB chunks sealed with
//! XChaCha20-Poly1305 in the STREAM construction (chunk counter plus a
//! last-chunk flag, so reordering and truncation are detected). The header is
//! bound to every chunk as associated data. The key comes from the password
//! through Argon2id with the per-file salt.

use crate::cancel::CancelToken;
use crate::types::Progress;
use argon2::Argon2;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::Payload;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// File type marker and format version.
const MAGIC: &[u8; 8] = b"OPUSENC1";
/// Argon2 salt length.
const SALT_LEN: usize = 16;
/// STREAM nonce prefix: 24 (XChaCha) - 4 (counter) - 1 (last flag).
const NONCE_LEN: usize = 19;
/// Plaintext bytes per chunk.
const CHUNK: usize = 64 * 1024;
/// Poly1305 tag appended to every chunk.
const TAG: usize = 16;
/// Suffix of encrypted files.
pub const ENC_SUFFIX: &str = ".enc";
/// Suffix given to a decrypted file whose name does not end in `.enc`.
const DEC_SUFFIX: &str = ".dec";

/// Why an encryption or decryption stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum CryptError {
    /// Reading or writing failed.
    Io(String),
    /// The input is not an OpusDM encrypted file.
    NotEncrypted,
    /// The first chunk did not authenticate: wrong password (or damage).
    WrongPassword,
    /// A later chunk did not authenticate, or the data is cut short.
    Corrupt,
    /// The job was cancelled.
    Cancelled,
}

impl std::fmt::Display for CryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptError::Io(m) => write!(f, "{m}"),
            CryptError::NotEncrypted => write!(f, "not an OpusDM encrypted file"),
            CryptError::WrongPassword => write!(f, "wrong password (or the file is damaged)"),
            CryptError::Corrupt => write!(f, "the file is damaged or truncated"),
            CryptError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl From<std::io::Error> for CryptError {
    fn from(e: std::io::Error) -> Self {
        CryptError::Io(e.to_string())
    }
}

/// Derive the 256 bit key for `password` and `salt`.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], CryptError> {
    let mut key = [0u8; 32];
    Argon2::default().hash_password_into(password.as_bytes(), salt, &mut key).map_err(|e| CryptError::Io(format!("key derivation failed: {e}")))?;
    Ok(key)
}

/// Fill `buf` from `r`, returning how many bytes arrived (fewer only at the end).
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Encrypt everything in `input` to `output`. Returns the plaintext bytes.
pub fn encrypt_stream(input: &mut impl Read, output: &mut impl Write, password: &str, cancel: &CancelToken) -> Result<u64, CryptError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| CryptError::Io(e.to_string()))?;
    getrandom::getrandom(&mut nonce).map_err(|e| CryptError::Io(e.to_string()))?;
    let key = derive_key(password, &salt)?;
    let mut header = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    output.write_all(&header)?;

    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut sealer = EncryptorBE32::from_aead(cipher, (&nonce).into());
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(CryptError::Cancelled);
        }
        let n = read_full(input, &mut buf)?;
        total += n as u64;
        let payload = Payload { msg: &buf[..n], aad: &header };
        if n == CHUNK {
            let sealed = sealer.encrypt_next(payload).map_err(|_| CryptError::Corrupt)?;
            output.write_all(&sealed)?;
        } else {
            // A short (possibly empty) chunk ends the stream.
            let sealed = sealer.encrypt_last(payload).map_err(|_| CryptError::Corrupt)?;
            output.write_all(&sealed)?;
            return Ok(total);
        }
    }
}

/// Decrypt everything in `input` to `output`. Returns the plaintext bytes.
pub fn decrypt_stream(input: &mut impl Read, output: &mut impl Write, password: &str, cancel: &CancelToken) -> Result<u64, CryptError> {
    let mut header = vec![0u8; MAGIC.len() + SALT_LEN + NONCE_LEN];
    if read_full(input, &mut header)? != header.len() || &header[..MAGIC.len()] != MAGIC {
        return Err(CryptError::NotEncrypted);
    }
    let salt = &header[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce: [u8; NONCE_LEN] = header[MAGIC.len() + SALT_LEN..].try_into().map_err(|_| CryptError::NotEncrypted)?;
    let key = derive_key(password, salt)?;

    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut opener = DecryptorBE32::from_aead(cipher, (&nonce).into());
    let mut buf = vec![0u8; CHUNK + TAG];
    let mut total = 0u64;
    let mut index = 0u64;
    let fail = |index: u64| if index == 0 { CryptError::WrongPassword } else { CryptError::Corrupt };
    loop {
        if cancel.is_cancelled() {
            return Err(CryptError::Cancelled);
        }
        let n = read_full(input, &mut buf)?;
        let payload = Payload { msg: &buf[..n], aad: &header };
        if n == CHUNK + TAG {
            let plain = opener.decrypt_next(payload).map_err(|_| fail(index))?;
            output.write_all(&plain)?;
            total += plain.len() as u64;
            index += 1;
        } else {
            let plain = opener.decrypt_last(payload).map_err(|_| fail(index))?;
            output.write_all(&plain)?;
            return Ok(total + plain.len() as u64);
        }
    }
}

/// The name a file gets in the destination: `name.enc` when encrypting; the
/// name without `.enc` (or with `.dec` added) when decrypting.
pub fn target_name(name: &str, encrypt: bool) -> String {
    if encrypt {
        return format!("{name}{ENC_SUFFIX}");
    }
    let cut = name.len().checked_sub(ENC_SUFFIX.len()).filter(|c| *c > 0 && name.is_char_boundary(*c));
    match cut {
        Some(c) if name[c..].eq_ignore_ascii_case(ENC_SUFFIX) => name[..c].to_string(),
        _ => format!("{name}{DEC_SUFFIX}"),
    }
}

/// Encrypt or decrypt one file to `target`, which must not exist. The data
/// goes to a `.part` file first, so a failure never leaves a half file under
/// the final name.
fn crypt_file(src: &Path, target: &Path, password: &str, encrypt: bool, cancel: &CancelToken) -> Result<(), CryptError> {
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(CryptError::Io(format!("{} already exists", target.display())));
    }
    let part = target.with_file_name(format!(".{}.part", target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    let mut input = File::open(src)?;
    let mut output = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&part)?;
    let result = if encrypt {
        encrypt_stream(&mut input, &mut output, password, cancel)
    } else {
        decrypt_stream(&mut input, &mut output, password, cancel)
    };
    let result = result.and_then(|_| output.sync_all().map_err(CryptError::from));
    drop(output);
    match result {
        Ok(()) => std::fs::rename(&part, target).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            CryptError::from(e)
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Every regular file under `src` paired with its target below `dest`.
/// A directory keeps its tree; a file lands directly in `dest`. Entries that
/// are neither files nor directories are reported through `errors`.
fn plan(srcs: &[PathBuf], dest: &Path, encrypt: bool, errors: &mut Vec<(PathBuf, String)>) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for src in srcs {
        let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            errors.push((src.clone(), "source has no file name".into()));
            continue;
        };
        let meta = match std::fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => {
                errors.push((src.clone(), e.to_string()));
                continue;
            }
        };
        if meta.is_dir() {
            let root = dest.join(&name);
            for entry in walkdir::WalkDir::new(src).follow_links(false).sort_by_file_name() {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
                let file_name = entry.file_name().to_string_lossy();
                let target = root.join(rel.with_file_name(target_name(&file_name, encrypt)));
                out.push((entry.path().to_path_buf(), target));
            }
        } else if meta.is_file() {
            out.push((src.clone(), dest.join(target_name(&name, encrypt))));
        } else {
            errors.push((src.clone(), "not a regular file or folder".into()));
        }
    }
    out
}

/// Encrypt (`encrypt`) or decrypt every source into `dest_dir`. Files are
/// handled one by one; a failure is recorded and the rest still run.
/// Existing targets are never overwritten.
pub fn run_crypt(srcs: &[PathBuf], dest_dir: &Path, password: &str, encrypt: bool, tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    if password.is_empty() {
        errors.push((dest_dir.to_path_buf(), "the password must not be empty".into()));
        return Progress::Done { copied: 0, skipped: 0, cancelled: false, errors };
    }
    let work = plan(srcs, dest_dir, encrypt, &mut errors);
    let mut done = 0u64;
    for (src, target) in &work {
        if cancel.is_cancelled() {
            return Progress::Done { copied: done, skipped: 0, cancelled: true, errors };
        }
        let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let _ = tx.send(Progress::File {
            path: target.clone(),
            done_bytes: 0,
            total_bytes: 0,
            files_done: done,
            files_total: work.len() as u64,
            file_bytes_done: 0,
            file_bytes_total: size,
        });
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                errors.push((target.clone(), e.to_string()));
                continue;
            }
        }
        match crypt_file(src, target, password, encrypt, cancel) {
            Ok(()) => done += 1,
            Err(CryptError::Cancelled) => return Progress::Done { copied: done, skipped: 0, cancelled: true, errors },
            Err(e) => errors.push((src.clone(), e.to_string())),
        }
    }
    Progress::Done { copied: done, skipped: 0, cancelled: false, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn never() -> CancelToken {
        CancelToken::new()
    }

    fn round_trip(data: &[u8]) {
        let mut sealed = Vec::new();
        encrypt_stream(&mut &data[..], &mut sealed, "pw", &never()).unwrap();
        let mut plain = Vec::new();
        let n = decrypt_stream(&mut &sealed[..], &mut plain, "pw", &never()).unwrap();
        assert_eq!(plain, data);
        assert_eq!(n as usize, data.len());
    }

    #[test]
    fn round_trips_empty_small_and_chunk_boundary_data() {
        round_trip(b"");
        round_trip(b"hello");
        round_trip(&vec![7u8; CHUNK - 1]);
        round_trip(&vec![7u8; CHUNK]);
        round_trip(&vec![7u8; CHUNK + 1]);
        round_trip(&(0..CHUNK * 2 + 123).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    }

    #[test]
    fn the_ciphertext_hides_the_data_and_differs_every_time() {
        let data = b"attack at dawn attack at dawn";
        let (mut a, mut b) = (Vec::new(), Vec::new());
        encrypt_stream(&mut &data[..], &mut a, "pw", &never()).unwrap();
        encrypt_stream(&mut &data[..], &mut b, "pw", &never()).unwrap();
        assert_ne!(a, b, "salt and nonce are random");
        assert!(!a.windows(6).any(|w| w == b"attack"));
        assert!(a.starts_with(MAGIC));
    }

    #[test]
    fn a_wrong_password_fails_cleanly() {
        let mut sealed = Vec::new();
        encrypt_stream(&mut &b"secret"[..], &mut sealed, "right", &never()).unwrap();
        let mut plain = Vec::new();
        assert_eq!(decrypt_stream(&mut &sealed[..], &mut plain, "wrong", &never()), Err(CryptError::WrongPassword));
        assert!(plain.is_empty(), "nothing is released for a wrong password");
    }

    #[test]
    fn tampering_truncation_and_foreign_files_are_detected() {
        let data = vec![1u8; CHUNK * 2 + 10];
        let mut sealed = Vec::new();
        encrypt_stream(&mut &data[..], &mut sealed, "pw", &never()).unwrap();
        let dec = |bytes: &[u8]| decrypt_stream(&mut &bytes[..], &mut Vec::new(), "pw", &never());

        let mut flipped = sealed.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert_eq!(dec(&flipped), Err(CryptError::Corrupt));

        // Cut exactly after the first chunk: the closing chunk is missing.
        let header = MAGIC.len() + SALT_LEN + NONCE_LEN;
        assert!(dec(&sealed[..header + CHUNK + TAG]).is_err());
        // Cut in the middle of a chunk.
        assert!(dec(&sealed[..sealed.len() - 5]).is_err());

        let mut bad_header = sealed.clone();
        bad_header[MAGIC.len()] ^= 1;
        assert!(dec(&bad_header).is_err(), "the header is authenticated");

        assert_eq!(dec(b"plain text, not encrypted at all......"), Err(CryptError::NotEncrypted));
        assert_eq!(dec(b""), Err(CryptError::NotEncrypted));
    }

    #[test]
    fn target_names() {
        assert_eq!(target_name("a.txt", true), "a.txt.enc");
        assert_eq!(target_name("a.txt.enc", false), "a.txt");
        assert_eq!(target_name("A.TXT.ENC", false), "A.TXT");
        assert_eq!(target_name("a.txt", false), "a.txt.dec");
        assert_eq!(target_name(".enc", false), ".enc.dec");
    }

    fn run(srcs: &[PathBuf], dest: &Path, pw: &str, encrypt: bool) -> (u64, Vec<(PathBuf, String)>) {
        let (tx, _rx) = channel();
        match run_crypt(srcs, dest, pw, encrypt, &tx, &never()) {
            Progress::Done { copied, errors, .. } => (copied, errors),
            _ => panic!("unexpected progress"),
        }
    }

    #[test]
    fn files_and_folders_round_trip_through_the_job() {
        let td = tempfile::tempdir().unwrap();
        let (plain, enc, back) = (td.path().join("plain"), td.path().join("enc"), td.path().join("back"));
        for d in [&plain, &enc, &back] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(plain.join("a.txt"), b"alpha").unwrap();
        std::fs::create_dir_all(plain.join("dir/sub")).unwrap();
        std::fs::write(plain.join("dir/sub/b.bin"), vec![9u8; CHUNK + 5]).unwrap();

        let (n, errors) = run(&[plain.join("a.txt"), plain.join("dir")], &enc, "pw", true);
        assert_eq!((n, errors.len()), (2, 0), "{errors:?}");
        assert!(enc.join("a.txt.enc").is_file());
        assert!(enc.join("dir/sub/b.bin.enc").is_file());
        assert_ne!(std::fs::read(enc.join("a.txt.enc")).unwrap(), b"alpha");

        let (n, errors) = run(&[enc.join("a.txt.enc"), enc.join("dir")], &back, "pw", false);
        assert_eq!((n, errors.len()), (2, 0), "{errors:?}");
        assert_eq!(std::fs::read(back.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(back.join("dir/sub/b.bin")).unwrap(), vec![9u8; CHUNK + 5]);
    }

    #[test]
    fn a_wrong_password_leaves_no_output_and_the_rest_still_runs() {
        let td = tempfile::tempdir().unwrap();
        let (src, enc, out) = (td.path().join("src"), td.path().join("enc"), td.path().join("out"));
        for d in [&src, &enc, &out] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(src.join("a"), b"A").unwrap();
        run(&[src.join("a")], &enc, "right", true);
        std::fs::write(enc.join("b.enc"), b"this is not encrypted data at all, sorry").unwrap();

        let (n, errors) = run(&[enc.join("a.enc"), enc.join("b.enc")], &out, "wrong", false);
        assert_eq!(n, 0);
        assert_eq!(errors.len(), 2);
        assert!(errors[0].1.contains("wrong password"), "{errors:?}");
        assert!(errors[1].1.contains("not an OpusDM"), "{errors:?}");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "no partial files");
    }

    #[test]
    fn an_existing_target_is_never_overwritten_and_empty_passwords_are_refused() {
        let td = tempfile::tempdir().unwrap();
        let (src, dest) = (td.path().join("src"), td.path().join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("a"), b"A").unwrap();
        std::fs::write(dest.join("a.enc"), b"keep me").unwrap();
        let (n, errors) = run(&[src.join("a")], &dest, "pw", true);
        assert_eq!((n, errors.len()), (0, 1));
        assert_eq!(std::fs::read(dest.join("a.enc")).unwrap(), b"keep me");

        let (_, errors) = run(&[src.join("a")], &dest, "", true);
        assert!(errors[0].1.contains("password"));
    }

    #[test]
    fn a_cancelled_job_stops_and_reports_it() {
        let td = tempfile::tempdir().unwrap();
        let (src, dest) = (td.path().join("src"), td.path().join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("a"), b"A").unwrap();
        let cancel = never();
        cancel.cancel();
        let (tx, _rx) = channel();
        match run_crypt(&[src.join("a")], &dest, "pw", true, &tx, &cancel) {
            Progress::Done { cancelled, copied, .. } => assert!(cancelled && copied == 0),
            _ => panic!("unexpected progress"),
        }
    }

    #[test]
    fn the_job_engine_dispatches_encrypt_and_decrypt() {
        use opusdm_ipc::{JobOptions, Op, Secret};
        let td = tempfile::tempdir().unwrap();
        let (src, enc, back) = (td.path().join("src"), td.path().join("enc"), td.path().join("back"));
        for d in [&src, &enc, &back] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(src.join("a.txt"), b"through the engine").unwrap();
        let job = |op: Op, srcs: Vec<PathBuf>, dest: &Path| {
            let (tx, rx) = channel();
            crate::run_job(crate::JobSpec { op, srcs, dest_dir: dest.to_path_buf(), options: JobOptions::default() }, tx, never());
            rx.into_iter().find_map(|p| match p {
                Progress::Done { copied, errors, .. } => Some((copied, errors)),
                _ => None,
            })
        };
        let pw = || Secret("pw".into());
        assert_eq!(job(Op::Encrypt { password: pw() }, vec![src.join("a.txt")], &enc).unwrap().0, 1);
        assert_eq!(job(Op::Decrypt { password: pw() }, vec![enc.join("a.txt.enc")], &back).unwrap().0, 1);
        assert_eq!(std::fs::read(back.join("a.txt")).unwrap(), b"through the engine");
    }
}
