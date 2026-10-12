//! Behaviour of the archive crate on real archives built by the tests.

use opusdm_archive::{extract, extract_entry, list, ArchiveError, EntryKind};
use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Names that a shell tool would read as options or wildcards.
const FILES: &[(&str, &str)] = &[("sub/a.txt", "nested"), ("-dash.txt", "dash"), ("st[ar]*.txt", "star")];

/// A tar stream holding `FILES` (names prefixed with `./`) and the folder `sub/`.
fn _tar_bytes() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut dir = tar::Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_mode(0o755);
    dir.set_size(0);
    builder.append_data(&mut dir, "./sub/", std::io::empty()).unwrap();
    for (name, body) in FILES {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, format!("./{name}"), body.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap()
}

/// Write `bytes` to `dir/name` and return the path.
fn _put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn _tar(dir: &Path) -> PathBuf {
    _put(dir, "a.tar", &_tar_bytes())
}

fn _tar_gz(dir: &Path) -> PathBuf {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&_tar_bytes()).unwrap();
    _put(dir, "a.tar.gz", &enc.finish().unwrap())
}

fn _tar_zst(dir: &Path) -> PathBuf {
    _put(dir, "a.tar.zst", &zstd::encode_all(&_tar_bytes()[..], 3).unwrap())
}

fn _zip(dir: &Path) -> PathBuf {
    let path = dir.join("a.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zip.add_directory("sub/", opts).unwrap();
    for (name, body) in FILES {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}

fn _seven_z() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.7z")
}

/// Every format of the fixtures, with a label for the failure message.
fn _all(dir: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![("tar", _tar(dir)), ("tar.gz", _tar_gz(dir)), ("tar.zst", _tar_zst(dir)), ("zip", _zip(dir)), ("7z", _seven_z())]
}

fn _never() -> bool {
    false
}

#[test]
fn every_format_lists_the_same_names() {
    let td = tempfile::tempdir().unwrap();
    for (label, arch) in _all(td.path()) {
        let entries = list(&arch).unwrap_or_else(|e| panic!("{label}: {e}"));
        let mut files: Vec<_> = entries.iter().filter(|e| e.kind == EntryKind::File).map(|e| e.path.as_str()).collect();
        files.sort();
        assert_eq!(files, ["-dash.txt", "st[ar]*.txt", "sub/a.txt"], "{label}");
        let nested = entries.iter().find(|e| e.path == "sub/a.txt").unwrap();
        assert_eq!(nested.size, 6, "{label}");
        assert!(entries.iter().all(|e| !e.path.starts_with("./") && !e.path.ends_with('/')), "{label}");
    }
}

#[test]
fn every_format_extracts_one_entry_with_an_odd_name() {
    let td = tempfile::tempdir().unwrap();
    for (label, arch) in _all(td.path()) {
        for (name, body) in FILES {
            let mut out = Vec::new();
            extract_entry(&arch, name, &mut out).unwrap_or_else(|e| panic!("{label} {name}: {e}"));
            assert_eq!(out, body.as_bytes(), "{label} {name}");
        }
    }
}

#[test]
fn a_wildcard_name_does_not_match_other_entries() {
    let td = tempfile::tempdir().unwrap();
    let arch = _tar(td.path());
    let err = extract_entry(&arch, "st*", &mut Vec::new()).unwrap_err();
    assert!(matches!(err, ArchiveError::NotFound(_)), "{err}");
}

#[test]
fn extract_all_writes_every_file() {
    let td = tempfile::tempdir().unwrap();
    for (label, arch) in _all(td.path()) {
        let dest = td.path().join(format!("out-{label}"));
        fs::create_dir(&dest).unwrap();
        let done = extract(&arch, &dest, None, &_never).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(done.written, 3, "{label}");
        for (name, body) in FILES {
            assert_eq!(fs::read_to_string(dest.join(name)).unwrap(), *body, "{label} {name}");
        }
    }
}

#[test]
fn a_selected_folder_brings_everything_below_it() {
    let td = tempfile::tempdir().unwrap();
    let arch = _zip(td.path());
    let dest = td.path().join("out");
    fs::create_dir(&dest).unwrap();
    extract(&arch, &dest, Some(&["sub".to_string()]), &_never).unwrap();
    assert!(dest.join("sub/a.txt").is_file());
    assert!(!dest.join("-dash.txt").exists());
}

#[test]
fn a_selected_file_is_extracted_alone() {
    let td = tempfile::tempdir().unwrap();
    let arch = _tar(td.path());
    let dest = td.path().join("out");
    fs::create_dir(&dest).unwrap();
    let done = extract(&arch, &dest, Some(&["st[ar]*.txt".to_string()]), &_never).unwrap();
    assert_eq!(done.written, 1);
    assert!(dest.join("st[ar]*.txt").is_file());
    assert!(!dest.join("-dash.txt").exists());
}

#[test]
fn an_existing_file_is_kept() {
    let td = tempfile::tempdir().unwrap();
    for (label, arch) in _all(td.path()) {
        let dest = td.path().join(format!("out-{label}"));
        fs::create_dir_all(dest.join("sub")).unwrap();
        fs::write(dest.join("-dash.txt"), "old").unwrap();
        let done = extract(&arch, &dest, None, &_never).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(fs::read_to_string(dest.join("-dash.txt")).unwrap(), "old", "{label}");
        assert_eq!((done.written, done.skipped), (2, 1), "{label}");
    }
}

/// A tar whose only file is named `name` (written raw, past the builder's checks).
fn _tar_with_raw_name(name: &str) -> Vec<u8> {
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, &b"evil"[..]).unwrap();
    builder.into_inner().unwrap()
}

#[test]
fn a_climbing_name_is_refused_and_nothing_escapes() {
    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("out");
    fs::create_dir(&dest).unwrap();
    let arch = _put(td.path(), "evil.tar", &_tar_with_raw_name("../evil.txt"));
    let err = extract(&arch, &dest, None, &_never).unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "{err}");
    assert!(!td.path().join("evil.txt").exists());
}

#[test]
fn a_symlink_cannot_redirect_a_later_entry_out_of_the_folder() {
    let td = tempfile::tempdir().unwrap();
    let outside = td.path().join("outside");
    let dest = td.path().join("out");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&dest).unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder.append_link(&mut link, "link", &outside).unwrap();
    let mut file = tar::Header::new_gnu();
    file.set_size(4);
    file.set_mode(0o644);
    builder.append_data(&mut file, "link/evil.txt", &b"evil"[..]).unwrap();
    let arch = _put(td.path(), "link.tar", &builder.into_inner().unwrap());
    let _ = extract(&arch, &dest, None, &_never);
    assert!(!outside.join("evil.txt").exists());
}

#[test]
fn cancel_stops_the_extraction() {
    let td = tempfile::tempdir().unwrap();
    let arch = _tar(td.path());
    let dest = td.path().join("out");
    fs::create_dir(&dest).unwrap();
    let calls = Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };
    let err = extract(&arch, &dest, None, &cancel).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "{err}");
}

#[test]
fn a_missing_selection_is_reported() {
    let td = tempfile::tempdir().unwrap();
    let arch = _tar(td.path());
    let dest = td.path().join("out");
    fs::create_dir(&dest).unwrap();
    let err = extract(&arch, &dest, Some(&["nope.txt".to_string()]), &_never).unwrap_err();
    assert!(matches!(err, ArchiveError::NotFound(ref n) if n == "nope.txt"), "{err}");
}

#[test]
fn a_folder_is_not_a_file() {
    let td = tempfile::tempdir().unwrap();
    let arch = _tar(td.path());
    assert!(extract_entry(&arch, "sub", &mut Vec::new()).is_err());
}

#[test]
fn a_plain_text_file_is_not_an_archive() {
    let td = tempfile::tempdir().unwrap();
    let path = _put(td.path(), "notes.zip", b"this is not an archive at all\n");
    assert!(list(&path).is_err());
}

#[test]
fn a_missing_file_is_an_open_error() {
    let err = list(Path::new("/nonexistent/none.zip")).unwrap_err();
    assert!(matches!(err, ArchiveError::Open { .. }), "{err}");
}
