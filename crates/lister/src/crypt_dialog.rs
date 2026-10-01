//! The Encrypt / Decrypt password requester (F-14).

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{JobOptions, Op, Secret};
use std::rc::Rc;

/// Check the two password fields: `confirm` is `None` when decrypting.
/// Returns the password, or what is wrong with the input.
pub fn check_passwords(password: &str, confirm: Option<&str>) -> Result<String, String> {
    if password.is_empty() {
        return Err("type a password".to_string());
    }
    if confirm.is_some_and(|c| c != password) {
        return Err("the two passwords differ".to_string());
    }
    Ok(password.to_string())
}

/// The paths a context menu action on `target` applies to: the whole
/// selection when `target` is part of it, otherwise just `target`.
pub fn targets_for(selected: Vec<std::path::PathBuf>, target: &std::path::Path) -> Vec<std::path::PathBuf> {
    if selected.iter().any(|p| p == target) {
        selected
    } else {
        vec![target.to_path_buf()]
    }
}

/// Ask for a password and encrypt (`encrypt`) or decrypt `srcs` into the
/// DEST Lister, or into this folder when there is no DEST.
pub fn open_crypt(ui: &Rc<Ui>, encrypt: bool, srcs: Vec<std::path::PathBuf>) {
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let title = if encrypt { "Encrypt" } else { "Decrypt" };
    let shell = opusdm_ui::DialogShell::new(&ui.window, title).size(400, 200);
    let root = shell.content();
    let summary = if encrypt { "Files are written as name.enc; existing files are never overwritten." } else { "Files named name.enc are written as name." };
    let note = gtk::Label::new(Some(summary));
    note.set_xalign(0.0);
    note.set_wrap(true);
    root.append(&note);

    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_activates_default(!encrypt);
    root.append(&gtk::Label::builder().label("Password").xalign(0.0).build());
    root.append(&password);
    let confirm = gtk::PasswordEntry::new();
    confirm.set_show_peek_icon(true);
    confirm.set_activates_default(true);
    if encrypt {
        root.append(&gtk::Label::builder().label("Repeat the password").xalign(0.0).build());
        root.append(&confirm);
    }
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");
    root.append(&error);

    let ok_btn = shell.button(title);
    shell.button_spacer();
    shell.close_button("Cancel");
    let window = shell.present();
    window.set_default_widget(Some(&ok_btn));
    password.grab_focus();

    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| {
        let confirm_text = confirm.text().to_string();
        let again = encrypt.then_some(confirm_text.as_str());
        match check_passwords(password.text().as_str(), again) {
            Ok(secret) => {
                let secret = Secret(secret);
                let op = if encrypt { Op::Encrypt { password: secret } } else { Op::Decrypt { password: secret } };
                let dest = crate::link_dialogs::dest_or_cwd(&ui);
                crate::job_submit::submit_job(&ui, op, srcs.clone(), dest, JobOptions::default());
                window.close();
            }
            Err(msg) => error.set_text(&msg),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_targets_follow_the_selection_only_when_it_holds_the_clicked_file() {
        use std::path::PathBuf;
        let (a, b, c) = (PathBuf::from("/d/a"), PathBuf::from("/d/b"), PathBuf::from("/d/c"));
        assert_eq!(targets_for(vec![a.clone(), b.clone()], &b), vec![a.clone(), b.clone()]);
        assert_eq!(targets_for(vec![a, b], &c), vec![c]);
    }

    #[test]
    fn passwords_must_be_present_and_match_when_confirmed() {
        assert!(check_passwords("", None).is_err());
        assert!(check_passwords("a", Some("b")).is_err());
        assert_eq!(check_passwords("a", Some("a")), Ok("a".to_string()));
        assert_eq!(check_passwords("a", None), Ok("a".to_string()));
    }
}
