//! The "Change Permissions..." dialog.

use crate::host::FileActionHost;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::Op;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

/// Mode shown when the first selected entry cannot be read.
const FALLBACK_MODE: u32 = 0o644;

/// Row labels of the rwx grid, in the order of the mode's octal digits.
const CLASS_LABELS: [&str; 3] = ["Owner", "Group", "Other"];

/// Column labels of the rwx grid, most significant bit first.
const BIT_LABELS: [&str; 3] = ["r", "w", "x"];

/// Turn the 3x3 checkbox state (`[class][bit]`, owner first, r/w/x order)
/// into a mode.
pub fn mode_from_bits(bits: [[bool; 3]; 3]) -> u32 {
    bits.iter().enumerate().fold(0, |mode, (class, row)| {
        row.iter().enumerate().fold(mode, |m, (bit, on)| {
            if *on { m | (0o4 >> bit) << (3 * (2 - class)) } else { m }
        })
    })
}

/// The inverse of [`mode_from_bits`].
pub fn bits_from_mode(mode: u32) -> [[bool; 3]; 3] {
    let mut bits = [[false; 3]; 3];
    for (class, row) in bits.iter_mut().enumerate() {
        for (bit, on) in row.iter_mut().enumerate() {
            *on = mode & ((0o4 >> bit) << (3 * (2 - class))) != 0;
        }
    }
    bits
}

/// Parse an octal mode such as `644` or `0755` (at most three digits after
/// stripping leading zeros).
pub fn parse_octal(text: &str) -> Result<u32, String> {
    let t = text.trim().trim_start_matches('0');
    if t.is_empty() {
        return Ok(0);
    }
    if t.len() > 3 {
        return Err(format!("mode has too many digits: {text:?}"));
    }
    u32::from_str_radix(t, 8).map_err(|_| format!("not an octal mode: {text:?}"))
}

/// Open the permissions dialog for `srcs` (the context menu passes the
/// clicked entry or the selection that holds it).
pub fn open_permissions(host: &Rc<dyn FileActionHost>, srcs: Vec<std::path::PathBuf>) {
    let Some(first) = srcs.first() else {
        host.notify("nothing selected");
        return;
    };
    let initial = std::fs::metadata(first)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(FALLBACK_MODE);

    let shell = opusdm_ui::DialogShell::new(&host.parent_window(), "Change Permissions").size(360, 300);
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
    for (col, label) in BIT_LABELS.iter().enumerate() {
        grid.attach(&gtk::Label::new(Some(label)), col as i32 + 1, 0, 1, 1);
    }
    let checks: Vec<Vec<gtk::CheckButton>> = (0..3)
        .map(|class| {
            let label = gtk::Label::new(Some(CLASS_LABELS[class]));
            label.set_xalign(0.0);
            grid.attach(&label, 0, class as i32 + 1, 1, 1);
            (0..3)
                .map(|bit| {
                    let check = gtk::CheckButton::new();
                    grid.attach(&check, bit + 1, class as i32 + 1, 1, 1);
                    check
                })
                .collect()
        })
        .collect();
    shell.content().append(&grid);

    let octal = gtk::Entry::new();
    octal.set_max_length(4);
    shell.content().append(&gtk::Label::new(Some("Octal mode")));
    shell.content().append(&octal);
    let recursive = gtk::CheckButton::with_label("Apply recursively (files get this mode, directories also get x where r is set)");
    shell.content().append(&recursive);

    let read_grid = {
        let checks = checks.clone();
        move || {
            let mut bits = [[false; 3]; 3];
            for (class, row) in checks.iter().enumerate() {
                for (bit, c) in row.iter().enumerate() {
                    bits[class][bit] = c.is_active();
                }
            }
            mode_from_bits(bits)
        }
    };

    // The grid and the entry drive each other; the guard stops the echo.
    let syncing = Rc::new(Cell::new(false));
    let write_grid = {
        let checks = checks.clone();
        let syncing = syncing.clone();
        move |mode: u32| {
            syncing.set(true);
            let bits = bits_from_mode(mode);
            for (class, row) in checks.iter().enumerate() {
                for (bit, c) in row.iter().enumerate() {
                    c.set_active(bits[class][bit]);
                }
            }
            syncing.set(false);
        }
    };
    write_grid(initial);
    octal.set_text(&format!("{initial:03o}"));

    for row in &checks {
        for c in row {
            let (octal, read_grid, syncing) = (octal.clone(), read_grid.clone(), syncing.clone());
            c.connect_toggled(move |_| {
                if !syncing.get() {
                    octal.set_text(&format!("{:03o}", read_grid()));
                }
            });
        }
    }
    {
        let syncing = syncing.clone();
        let write_grid = write_grid.clone();
        octal.connect_changed(move |e| {
            if syncing.get() {
                return;
            }
            if let Ok(mode) = parse_octal(e.text().as_str()) {
                write_grid(mode);
            }
        });
    }

    let ok_btn = shell.button("Apply");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let host = host.clone();
    ok_btn.connect_clicked(move |_| {
        let mode = match parse_octal(octal.text().as_str()) {
            Ok(m) => m,
            Err(msg) => {
                host.notify(&msg);
                return;
            }
        };
        let op = Op::Chmod { mode, recurse: recursive.is_active() };
        host.submit_job(op, srcs.clone(), Some(host.folder()));
        window.close();
    });
    shell.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_and_bits_are_inverse() {
        for mode in [0o000, 0o644, 0o755, 0o777, 0o421] {
            assert_eq!(mode_from_bits(bits_from_mode(mode)), mode);
        }
        assert!(bits_from_mode(0o400)[0][0]);
        assert!(!bits_from_mode(0o400)[0][1]);
        assert!(bits_from_mode(0o001)[2][2]);
    }

    #[test]
    fn parses_octal_modes() {
        assert_eq!(parse_octal("644"), Ok(0o644));
        assert_eq!(parse_octal("0755"), Ok(0o755));
        assert_eq!(parse_octal("000"), Ok(0));
        assert_eq!(parse_octal(" 7 "), Ok(7));
        assert!(parse_octal("888").is_err());
        assert!(parse_octal("7777").is_err());
        assert!(parse_octal("rwx").is_err());
    }
}
