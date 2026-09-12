//! Repo maintenance commands run via `cargo run -p xtask -- <command>`.
//! Not part of the app's own build - `cargo build --workspace` does not
//! need this crate to succeed for the apps to run.

use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("package-themes") => package_themes(),
        _ => {
            eprintln!("usage: cargo run -p xtask -- package-themes");
            std::process::exit(1);
        }
    }
}

/// Zip every `themes/<slug>/` directory into `target/themes/<slug>.zip`.
fn package_themes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let themes_src = repo_root.join("themes");
    let out_dir = repo_root.join("target").join("themes");
    std::fs::create_dir_all(&out_dir).expect("create target/themes");

    for entry in std::fs::read_dir(&themes_src).expect("read themes/") {
        let entry = entry.expect("read themes/ entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let slug = path.file_name().unwrap().to_string_lossy().to_string();
        let zip_path = out_dir.join(format!("{slug}.zip"));
        let file = File::create(&zip_path).expect("create zip file");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();

        for walk_entry in walkdir::WalkDir::new(&path).into_iter().filter_map(|e| e.ok()) {
            let full_path = walk_entry.path();
            let rel_path = full_path.strip_prefix(&path).unwrap();
            if rel_path.as_os_str().is_empty() {
                continue;
            }
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");
            if full_path.is_dir() {
                zip.add_directory(format!("{rel_str}/"), options).expect("add dir");
            } else {
                zip.start_file(rel_str, options).expect("start file");
                let data = std::fs::read(full_path).expect("read theme file");
                zip.write_all(&data).expect("write zip entry");
            }
        }
        zip.finish().expect("finish zip");
        println!("wrote {}", zip_path.display());
    }
}
