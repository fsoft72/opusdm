//! Keyboard bindings.
//!
//! [`resolve`] is a pure key -> action mapping so the whole binding table can
//! be unit tested; `window.rs` only has to execute the action it gets back.
//! The modal ("vim mode") bindings are plain single letters, which is why the
//! caller must pass the right [`Focus`]: while a text field has the keyboard,
//! `j` is a character, not a command.

use gtk4::gdk;

/// Where the keyboard focus is when the key arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    /// The file list, or anything else that is not a text field.
    List,
    /// A text entry or text view: only Escape and Ctrl chords are shortcuts.
    Editing,
}

/// Which way the list cursor moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    /// Next row.
    Down,
    /// Previous row.
    Up,
    /// First row.
    First,
    /// Last row.
    Last,
}

/// The resolved meaning of a key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Run the command palette entry with this id.
    Command(&'static str),
    /// Move the list cursor.
    Move(Motion),
    /// Open the row under the cursor (enter directory, open file).
    Enter,
    /// Toggle the selection of the row under the cursor, then move down.
    ToggleSelectAndAdvance,
    /// Raise the Lister holding the opposite role.
    SwapRoleFocus,
    /// Show the command palette.
    OpenPalette,
    /// Reveal the filter bar.
    RevealFilter,
    /// Show or hide the terminal drawer.
    ToggleTerminal,
    /// Quick Look, or recursive size when a directory is selected.
    PreviewOrSize,
    /// Context-dependent dismiss (filter, terminal, then selection).
    Escape,
}

/// Map a key press to an [`Action`], or `None` to let GTK handle it.
///
/// `shift` is only consulted for keys whose keyval does not already encode it
/// (F7); shifted characters such as `*`, `+`, `~` and `G` arrive as their own
/// keyvals.
pub fn resolve(key: gdk::Key, ctrl: bool, shift: bool, focus: Focus) -> Option<Action> {
    // Ctrl chords work wherever the focus is, including inside text fields.
    if ctrl {
        return match key {
            gdk::Key::p | gdk::Key::P => Some(Action::OpenPalette),
            gdk::Key::f | gdk::Key::F => Some(Action::RevealFilter),
            gdk::Key::a | gdk::Key::A => Some(Action::Command("select_all")),
            gdk::Key::r | gdk::Key::R => Some(Action::Command("reload")),
            gdk::Key::h | gdk::Key::H => Some(Action::Command("toggle_hidden")),
            _ => None,
        };
    }

    // Function keys and Escape are unambiguous, so they keep working while
    // typing. Everything below this point would otherwise eat text input.
    match key {
        gdk::Key::F4 => return Some(Action::ToggleTerminal),
        gdk::Key::Escape => return Some(Action::Escape),
        _ => {}
    }
    if focus == Focus::Editing {
        return None;
    }

    match key {
        // Movement: vim keys and the arrows that already work, plus g/G.
        gdk::Key::j => Some(Action::Move(Motion::Down)),
        gdk::Key::k => Some(Action::Move(Motion::Up)),
        gdk::Key::g => Some(Action::Move(Motion::First)),
        gdk::Key::G => Some(Action::Move(Motion::Last)),
        gdk::Key::l | gdk::Key::Return | gdk::Key::KP_Enter => Some(Action::Enter),
        gdk::Key::h | gdk::Key::BackSpace => Some(Action::Command("parent_dir")),
        gdk::Key::asciitilde => Some(Action::Command("home_dir")),

        // Selection.
        gdk::Key::Insert | gdk::Key::KP_Insert => Some(Action::ToggleSelectAndAdvance),
        gdk::Key::asterisk => Some(Action::Command("invert_selection")),
        gdk::Key::plus | gdk::Key::KP_Add => Some(Action::Command("select_pattern")),
        gdk::Key::minus | gdk::Key::KP_Subtract => Some(Action::Command("deselect_pattern")),

        // File operations.
        gdk::Key::c => Some(Action::Command("copy")),
        gdk::Key::m => Some(Action::Command("move")),
        gdk::Key::d | gdk::Key::Delete | gdk::Key::KP_Delete => Some(Action::Command("delete")),
        gdk::Key::F2 => Some(Action::Command("rename")),
        gdk::Key::F7 if shift => Some(Action::Command("newfile")),
        gdk::Key::F7 => Some(Action::Command("mkdir")),

        // Roles and panels.
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Some(Action::SwapRoleFocus),
        gdk::Key::grave => Some(Action::ToggleTerminal),
        gdk::Key::F3 => Some(Action::PreviewOrSize),
        gdk::Key::space | gdk::Key::KP_Space => Some(Action::PreviewOrSize),
        gdk::Key::slash => Some(Action::RevealFilter),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(key: gdk::Key) -> Option<Action> {
        resolve(key, false, false, Focus::List)
    }

    fn editing(key: gdk::Key) -> Option<Action> {
        resolve(key, false, false, Focus::Editing)
    }

    #[test]
    fn vim_motion_keys_move_the_cursor() {
        assert_eq!(list(gdk::Key::j), Some(Action::Move(Motion::Down)));
        assert_eq!(list(gdk::Key::k), Some(Action::Move(Motion::Up)));
        assert_eq!(list(gdk::Key::g), Some(Action::Move(Motion::First)));
        assert_eq!(list(gdk::Key::G), Some(Action::Move(Motion::Last)));
    }

    #[test]
    fn h_and_l_leave_and_enter_directories() {
        assert_eq!(list(gdk::Key::l), Some(Action::Enter));
        assert_eq!(list(gdk::Key::Return), Some(Action::Enter));
        assert_eq!(list(gdk::Key::h), Some(Action::Command("parent_dir")));
        assert_eq!(list(gdk::Key::BackSpace), Some(Action::Command("parent_dir")));
    }

    #[test]
    fn file_operation_letters_are_bound() {
        assert_eq!(list(gdk::Key::c), Some(Action::Command("copy")));
        assert_eq!(list(gdk::Key::m), Some(Action::Command("move")));
        assert_eq!(list(gdk::Key::d), Some(Action::Command("delete")));
        assert_eq!(list(gdk::Key::Delete), Some(Action::Command("delete")));
    }

    #[test]
    fn selection_keys_are_bound() {
        assert_eq!(list(gdk::Key::Insert), Some(Action::ToggleSelectAndAdvance));
        assert_eq!(list(gdk::Key::asterisk), Some(Action::Command("invert_selection")));
        assert_eq!(list(gdk::Key::plus), Some(Action::Command("select_pattern")));
        assert_eq!(list(gdk::Key::minus), Some(Action::Command("deselect_pattern")));
    }

    #[test]
    fn tab_swaps_the_role_focus() {
        assert_eq!(list(gdk::Key::Tab), Some(Action::SwapRoleFocus));
        assert_eq!(list(gdk::Key::ISO_Left_Tab), Some(Action::SwapRoleFocus));
    }

    #[test]
    fn backtick_opens_the_terminal_and_tilde_goes_home() {
        // Two different keyvals, so they can mean two different things.
        assert_eq!(list(gdk::Key::grave), Some(Action::ToggleTerminal));
        assert_eq!(list(gdk::Key::asciitilde), Some(Action::Command("home_dir")));
    }

    #[test]
    fn shift_only_matters_where_the_keyval_does_not_encode_it() {
        assert_eq!(resolve(gdk::Key::F7, false, false, Focus::List), Some(Action::Command("mkdir")));
        assert_eq!(resolve(gdk::Key::F7, false, true, Focus::List), Some(Action::Command("newfile")));
    }

    #[test]
    fn typing_in_a_text_field_swallows_every_single_letter_binding() {
        for key in [
            gdk::Key::j, gdk::Key::k, gdk::Key::h, gdk::Key::l,
            gdk::Key::c, gdk::Key::m, gdk::Key::d, gdk::Key::g,
            gdk::Key::space, gdk::Key::slash, gdk::Key::asterisk,
            gdk::Key::plus, gdk::Key::minus, gdk::Key::asciitilde,
            gdk::Key::Insert, gdk::Key::Tab, gdk::Key::grave,
        ] {
            assert_eq!(editing(key), None, "{key:?} must reach the text field");
        }
    }

    #[test]
    fn escape_and_function_keys_still_work_while_typing() {
        assert_eq!(editing(gdk::Key::Escape), Some(Action::Escape));
        assert_eq!(editing(gdk::Key::F4), Some(Action::ToggleTerminal));
    }

    #[test]
    fn ctrl_chords_work_in_both_focus_modes() {
        for focus in [Focus::List, Focus::Editing] {
            assert_eq!(resolve(gdk::Key::p, true, false, focus), Some(Action::OpenPalette));
            assert_eq!(resolve(gdk::Key::f, true, false, focus), Some(Action::RevealFilter));
            assert_eq!(resolve(gdk::Key::a, true, false, focus), Some(Action::Command("select_all")));
            assert_eq!(resolve(gdk::Key::r, true, false, focus), Some(Action::Command("reload")));
            assert_eq!(resolve(gdk::Key::h, true, false, focus), Some(Action::Command("toggle_hidden")));
        }
    }

    #[test]
    fn ctrl_does_not_leak_into_the_plain_letter_bindings() {
        // Ctrl+C must not delete-or-copy through the vim binding for `c`.
        assert_eq!(resolve(gdk::Key::c, true, false, Focus::List), None);
        assert_eq!(resolve(gdk::Key::d, true, false, Focus::List), None);
        assert_eq!(resolve(gdk::Key::j, true, false, Focus::List), None);
    }

    #[test]
    fn unbound_keys_are_left_to_gtk() {
        assert_eq!(list(gdk::Key::F9), None);
        assert_eq!(list(gdk::Key::Up), None, "arrows stay with the list widget");
        assert_eq!(list(gdk::Key::Down), None);
    }

    #[test]
    fn every_command_id_the_map_emits_exists_in_the_palette() {
        let known: Vec<&str> = crate::palette::get_commands().iter().map(|c| c.id).collect();
        // Sweep the whole keyval space we bind from, in both focus modes.
        let keys = [
            gdk::Key::j, gdk::Key::k, gdk::Key::g, gdk::Key::G, gdk::Key::l,
            gdk::Key::h, gdk::Key::asciitilde, gdk::Key::Insert, gdk::Key::asterisk,
            gdk::Key::plus, gdk::Key::minus, gdk::Key::c, gdk::Key::m, gdk::Key::d,
            gdk::Key::Delete, gdk::Key::F2, gdk::Key::F7, gdk::Key::Tab,
            gdk::Key::grave, gdk::Key::F3, gdk::Key::space, gdk::Key::slash,
            gdk::Key::BackSpace, gdk::Key::Return, gdk::Key::F4, gdk::Key::Escape,
            gdk::Key::p, gdk::Key::f, gdk::Key::a, gdk::Key::r,
        ];
        for key in keys {
            for (ctrl, shift) in [(false, false), (false, true), (true, false)] {
                let Some(Action::Command(id)) = resolve(key, ctrl, shift, Focus::List) else {
                    continue;
                };
                assert!(known.contains(&id), "{key:?} maps to unknown command {id:?}");
            }
        }
    }
}
