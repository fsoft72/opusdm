//! Virtual archive filesystem support for navigating and extracting Zip, Tar, 7z.

use crate::model::Row;
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ArchiveItem {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mode: u32,
}

/// Check if a path points to a supported archive file.
pub fn is_archive_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if name.ends_with(".tar.gz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.zst")
        || name.ends_with(".tgz")
    {
        return true;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    matches!(ext.as_str(), "zip" | "tar" | "tgz" | "7z")
}

/// Parse a virtual archive path formatted like `/path/to/archive.zip#sub/dir`.
/// Returns `(archive_path, inner_path)`.
pub fn parse_archive_path(path: &Path) -> Option<(PathBuf, String)> {
    let s = path.to_string_lossy();
    if let Some((arch_str, inner_str)) = s.split_once('#') {
        let arch_path = PathBuf::from(arch_str);
        if is_archive_file(&arch_path) {
            return Some((arch_path, inner_str.trim_matches('/').to_string()));
        }
    }
    None
}

/// Construct a virtual archive path from an archive path and inner path.
pub fn make_archive_path(arch_path: &Path, inner: &str) -> PathBuf {
    let clean = inner.trim_matches('/');
    if clean.is_empty() {
        PathBuf::from(format!("{}#", arch_path.display()))
    } else {
        PathBuf::from(format!("{}#{}", arch_path.display(), clean))
    }
}

/// Read all items contained in an archive.
pub fn read_archive_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let ext = arch_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let name = arch_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "zip" {
        read_zip_entries(arch_path)
    } else if name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.zst")
        || ext == "tar"
    {
        read_tar_entries(arch_path)
    } else if ext == "7z" {
        read_7z_entries(arch_path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unsupported archive format",
        ))
    }
}

fn read_zip_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("zipinfo")
        .arg("-s")
        .arg(arch_path)
        .output()?;

    if !output.status.success() {
        // Fallback to unzip -Z -1
        let out2 = Command::new("unzip")
            .arg("-Z")
            .arg("-1")
            .arg(arch_path)
            .output()?;
        let mut items = Vec::new();
        for line in out2.stdout.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let is_dir = trimmed.ends_with('/');
            items.push(ArchiveItem {
                path: trimmed.trim_end_matches('/').to_string(),
                is_dir,
                size: 0,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
        return Ok(items);
    }

    let mut items = Vec::new();
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.starts_with("Archive:")
            || trimmed.starts_with("Zip file size:")
            || trimmed.ends_with("compressed:")
            || trimmed.is_empty()
        {
            continue;
        }

        // Example: -rw-rw-r--  3.0 unx        5 tx stor 26-Sep-03 15:27 file.txt
        let is_dir = trimmed.starts_with('d');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 7 {
            let size = parts.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let path = parts[7..].join(" ");
            let clean_path = path.trim_end_matches('/').to_string();
            items.push(ArchiveItem {
                path: clean_path,
                is_dir: is_dir || path.ends_with('/'),
                size,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
    }

    Ok(items)
}

fn read_tar_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("tar")
        .arg("-tvf")
        .arg(arch_path)
        .output()?;

    let mut items = Vec::new();
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Example: -rw-rw-r-- fabio/fabio 6 2026-09-03 15:27 hello.txt
        let is_dir = trimmed.starts_with('d');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 6 {
            let size = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let path = parts[5..].join(" ");
            let clean_path = path.trim_end_matches('/').to_string();
            items.push(ArchiveItem {
                path: clean_path,
                is_dir: is_dir || path.ends_with('/'),
                size,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
    }

    Ok(items)
}

fn read_7z_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("7z")
        .arg("l")
        .arg(arch_path)
        .output()?;

    let mut items = Vec::new();
    let mut in_files = false;
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.starts_with("-------------------") {
            in_files = !in_files;
            continue;
        }
        if in_files && trimmed.len() > 53 {
            let is_dir = trimmed.chars().nth(20) == Some('D');
            let size_str = &trimmed[26..38].trim();
            let size = size_str.parse::<u64>().unwrap_or(0);
            let path = trimmed[53..].trim().trim_end_matches('/').to_string();
            if !path.is_empty() {
                items.push(ArchiveItem {
                    path,
                    is_dir,
                    size,
                    mode: if is_dir { 0o755 } else { 0o644 },
                });
            }
        }
    }

    Ok(items)
}

/// Read rows for the given inner prefix within an archive.
pub fn read_archive_rows(arch_path: &Path, inner_prefix: &str) -> io::Result<Vec<Row>> {
    let items = read_archive_entries(arch_path)?;

    let prefix = if inner_prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", inner_prefix.trim_matches('/'))
    };

    let mut seen_dirs = HashSet::new();
    let mut rows = Vec::new();

    for item in items {
        if !item.path.starts_with(&prefix) && !prefix.is_empty() {
            continue;
        }
        let rel = if prefix.is_empty() {
            &item.path[..]
        } else {
            &item.path[prefix.len()..]
        };

        if rel.is_empty() {
            continue;
        }

        if let Some((dir_name, _)) = rel.split_once('/') {
            if seen_dirs.insert(dir_name.to_string()) {
                rows.push(Row {
                    name: dir_name.to_string(),
                    is_dir: true,
                    is_symlink: false,
                    broken_link: false,
                    size: 0,
                    modified_unix: 0,
                    uid: 1000,
                    gid: 1000,
                    mode: 0o755,
                });
            }
        } else {
            let is_dir = item.is_dir;
            let name = rel.to_string();
            rows.push(Row {
                name,
                is_dir,
                is_symlink: false,
                broken_link: false,
                size: if is_dir { 0 } else { item.size },
                modified_unix: 0,
                uid: 1000,
                gid: 1000,
                mode: item.mode,
            });
        }
    }

    Ok(rows)
}

/// Extract a single file from the archive into a temporary file for viewing.
pub fn extract_single_file_temp(arch: &Path, inner_file: &str) -> io::Result<PathBuf> {
    let temp_dir = std::env::temp_dir().join("opusdm-preview");
    std::fs::create_dir_all(&temp_dir)?;

    let filename = Path::new(inner_file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "preview_file".into());
    let dest_path = temp_dir.join(filename);

    let ext = arch
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "zip" {
        let output = Command::new("unzip")
            .arg("-p")
            .arg(arch)
            .arg(inner_file)
            .output()?;
        std::fs::write(&dest_path, &output.stdout)?;
    } else {
        Command::new("tar")
            .arg("-xf")
            .arg(arch)
            .arg("-C")
            .arg(&temp_dir)
            .arg(inner_file)
            .output()?;
    }

    Ok(dest_path)
}

/// Extract items from archive into `dest_dir`.
pub fn extract_items_to_dir(
    arch: &Path,
    items: &[String],
    dest_dir: &Path,
) -> io::Result<Vec<PathBuf>> {
    let ext = arch
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut extracted = Vec::new();

    if ext == "zip" {
        let mut cmd = Command::new("unzip");
        cmd.arg("-o").arg(arch);
        for item in items {
            cmd.arg(item);
        }
        cmd.arg("-d").arg(dest_dir);
        let status = cmd.status()?;
        if !status.success() {
            return Err(io::Error::other("Failed to extract zip items"));
        }
    } else {
        let mut cmd = Command::new("tar");
        cmd.arg("-xf").arg(arch).arg("-C").arg(dest_dir);
        for item in items {
            cmd.arg(item);
        }
        let status = cmd.status()?;
        if !status.success() {
            return Err(io::Error::other("Failed to extract tar items"));
        }
    }

    for item in items {
        let p = dest_dir.join(item);
        extracted.push(p);
    }

    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_archive_file() {
        assert!(is_archive_file(Path::new("archive.zip")));
        assert!(is_archive_file(Path::new("bundle.tar.gz")));
        assert!(is_archive_file(Path::new("archive.tar.bz2")));
        assert!(is_archive_file(Path::new("data.tar.xz")));
        assert!(is_archive_file(Path::new("project.tgz")));
        assert!(is_archive_file(Path::new("archive.7z")));
        assert!(!is_archive_file(Path::new("code.rs")));
        assert!(!is_archive_file(Path::new("data.json")));
    }

    #[test]
    fn test_archive_path_parsing() {
        let p = Path::new("/home/user/test.zip#sub/dir");
        let (arch, inner) = parse_archive_path(p).unwrap();
        assert_eq!(arch, PathBuf::from("/home/user/test.zip"));
        assert_eq!(inner, "sub/dir");

        let p_root = Path::new("/home/user/test.zip#");
        let (arch, inner) = parse_archive_path(p_root).unwrap();
        assert_eq!(arch, PathBuf::from("/home/user/test.zip"));
        assert_eq!(inner, "");
    }
}
