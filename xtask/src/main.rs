//! Repo maintenance commands run via `cargo run -p xtask -- <command>`.
//! Not part of the app's own build - `cargo build --workspace` does not
//! need this crate to succeed for the apps to run.

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
        opusdm_themepack::pack_theme_dir(&path, &zip_path).expect("pack theme folder");
        println!("wrote {}", zip_path.display());
    }
}
