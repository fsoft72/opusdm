//! Keyboard bindings.
//!
//! [`resolve`] is a pure key -> action mapping so the whole binding table can
//! be unit tested; `window.rs` only has to execute the action it gets back.
//! There are no single-letter commands: plain letters over the file list start
//! type-to-find (see `typeahead.rs`). The caller still passes the right
//! [`Focus`], because while a text field has the keyboard `+`, `-`, `/` and
//! space are characters, not commands.

use gtk4::gdk;

/// One entry in the command palette / a target `Action::Command` can name.
/// A single enum shared by `keys::resolve`, `palette::get_commands` and
/// `palette::execute_command` so a typo in any one of them is a compile
/// error instead of a silently dead-ending shortcut.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommandId {
    Filter,
    Preview,
    SelectAll,
    DeselectAll,
    InvertSelection,
    SelectPattern,
    /// Open the Advanced Select requester.
    AdvancedSelect,
    DeselectPattern,
    ToggleHidden,
    Copy,
    Move,
    Delete,
    /// Copy to DEST under names built from a pattern.
    CopyAs,
    /// Move to DEST under names built from a pattern.
    MoveAs,
    /// Copy each selected entry next to itself as `name.copy`.
    Duplicate,
    /// Copy to DEST only what is missing there or newer than the DEST copy.
    CopyNewer,
    /// Open the permissions dialog.
    ChangePermissions,
    /// Open the timestamp dialog.
    SetTimestamp,
    /// Open the link dialog.
    MakeLink,
    /// Open the join dialog.
    JoinFiles,
    /// Open the split dialog.
    SplitFile,
    /// Check whether the selection fits in the free space at DEST.
    CheckFit,
    /// Select again what the last job or Deselect All cleared.
    Reselect,
    ClipCopy,
    ClipCut,
    ClipPaste,
    Mkdir,
    NewFile,
    Rename,
    BatchRename,
    Reload,
    ParentDir,
    HomeDir,
    Terminal,
    CalcSizes,
    CalcChecksums,
    CompareDirs,
    SetSource,
    SetDest,
    SetNone,
    TogglePin,
    /// Open the Lister Format editor.
    ListerFormat,
    /// Ask for the persistent Show pattern list.
    SetShowPattern,
    /// Ask for the persistent Hide pattern list.
    SetHidePattern,
    /// Open the Find dialog on the Name field.
    FindFiles,
    /// Open the Find dialog on the Contains field.
    SearchInFiles,
    /// Open the Viewer in SmartRead mode.
    ReadSmart,
    /// Open the Viewer as text.
    ReadText,
    /// Open the Viewer as ANSI art.
    ReadAnsi,
    /// Open the Viewer as a hex dump.
    ReadHex,
    /// Open the image slideshow.
    ShowImages,
    /// Open the Cache List.
    CacheList,
    /// Drop every Lister's directory cache.
    FreeCaches,
    /// Go back in the history.
    HistoryBack,
    /// Go forward in the history.
    HistoryForward,
    /// Open the Function Editor's manager window (list/new/edit/delete).
    ManageFunctions,
    /// Run a saved Function by id.
    RunFunction(opusdm_ipc::function::FunctionId),
    /// Open the hotkey editor.
    ManageHotkeys,
    /// Ask for a key combination and say what it does.
    FindKey,
    /// Open the user menu editor.
    ManageUserMenu,
    /// Open the script events editor.
    ManageScripts,
}

/// Where the keyboard focus is when the key arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    /// The file list, or anything else that is not a text field.
    List,
    /// A text entry or text view: only Escape and Ctrl chords are shortcuts.
    Editing,
}

/// The resolved meaning of a key press.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// Run the command palette entry with this id.
    Command(CommandId),
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
            gdk::Key::a | gdk::Key::A if shift => Some(Action::Command(CommandId::Reselect)),
            gdk::Key::a | gdk::Key::A => Some(Action::Command(CommandId::SelectAll)),
            gdk::Key::r | gdk::Key::R => Some(Action::Command(CommandId::Reload)),
            gdk::Key::h | gdk::Key::H => Some(Action::Command(CommandId::ToggleHidden)),
            gdk::Key::c | gdk::Key::C => Some(Action::Command(CommandId::ClipCopy)),
            gdk::Key::x | gdk::Key::X => Some(Action::Command(CommandId::ClipCut)),
            gdk::Key::v | gdk::Key::V => Some(Action::Command(CommandId::ClipPaste)),
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
        // Navigation. Arrows are left to the list widget.
        gdk::Key::Return | gdk::Key::KP_Enter => Some(Action::Enter),
        gdk::Key::BackSpace => Some(Action::Command(CommandId::ParentDir)),
        gdk::Key::asciitilde => Some(Action::Command(CommandId::HomeDir)),

        // Selection.
        gdk::Key::Insert | gdk::Key::KP_Insert => Some(Action::ToggleSelectAndAdvance),
        gdk::Key::asterisk => Some(Action::Command(CommandId::InvertSelection)),
        gdk::Key::plus | gdk::Key::KP_Add => Some(Action::Command(CommandId::SelectPattern)),
        gdk::Key::minus | gdk::Key::KP_Subtract => Some(Action::Command(CommandId::DeselectPattern)),

        // File operations.
        gdk::Key::F5 => Some(Action::Command(CommandId::Copy)),
        gdk::Key::F6 => Some(Action::Command(CommandId::Move)),
        gdk::Key::F8 | gdk::Key::Delete | gdk::Key::KP_Delete => Some(Action::Command(CommandId::Delete)),
        gdk::Key::F2 => Some(Action::Command(CommandId::Rename)),
        gdk::Key::F7 if shift => Some(Action::Command(CommandId::NewFile)),
        gdk::Key::F7 => Some(Action::Command(CommandId::Mkdir)),

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
    fn return_enters_and_backspace_goes_to_the_parent() {
        assert_eq!(list(gdk::Key::Return), Some(Action::Enter));
        assert_eq!(list(gdk::Key::BackSpace), Some(Action::Command(CommandId::ParentDir)));
    }

    #[test]
    fn file_operations_are_on_function_keys() {
        assert_eq!(list(gdk::Key::F5), Some(Action::Command(CommandId::Copy)));
        assert_eq!(list(gdk::Key::F6), Some(Action::Command(CommandId::Move)));
        assert_eq!(list(gdk::Key::F8), Some(Action::Command(CommandId::Delete)));
        assert_eq!(list(gdk::Key::Delete), Some(Action::Command(CommandId::Delete)));
    }

    #[test]
    fn plain_letters_are_not_commands() {
        // They belong to type-to-find.
        for key in [
            gdk::Key::j, gdk::Key::k, gdk::Key::g, gdk::Key::G, gdk::Key::h,
            gdk::Key::l, gdk::Key::c, gdk::Key::m, gdk::Key::d, gdk::Key::a,
        ] {
            assert_eq!(list(key), None, "{key:?} must stay free for type-to-find");
        }
    }

    #[test]
    fn selection_keys_are_bound() {
        assert_eq!(list(gdk::Key::Insert), Some(Action::ToggleSelectAndAdvance));
        assert_eq!(list(gdk::Key::asterisk), Some(Action::Command(CommandId::InvertSelection)));
        assert_eq!(list(gdk::Key::plus), Some(Action::Command(CommandId::SelectPattern)));
        assert_eq!(list(gdk::Key::minus), Some(Action::Command(CommandId::DeselectPattern)));
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
        assert_eq!(list(gdk::Key::asciitilde), Some(Action::Command(CommandId::HomeDir)));
    }

    #[test]
    fn shift_only_matters_where_the_keyval_does_not_encode_it() {
        assert_eq!(resolve(gdk::Key::F7, false, false, Focus::List), Some(Action::Command(CommandId::Mkdir)));
        assert_eq!(resolve(gdk::Key::F7, false, true, Focus::List), Some(Action::Command(CommandId::NewFile)));
    }

    #[test]
    fn typing_in_a_text_field_swallows_every_single_letter_binding() {
        for key in [
            gdk::Key::j, gdk::Key::c, gdk::Key::d,
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
    fn ctrl_shift_a_reselects_while_ctrl_a_selects_all() {
        assert_eq!(resolve(gdk::Key::A, true, true, Focus::List), Some(Action::Command(CommandId::Reselect)));
        assert_eq!(resolve(gdk::Key::a, true, false, Focus::List), Some(Action::Command(CommandId::SelectAll)));
    }

    #[test]
    fn ctrl_chords_work_in_both_focus_modes() {
        for focus in [Focus::List, Focus::Editing] {
            assert_eq!(resolve(gdk::Key::p, true, false, focus), Some(Action::OpenPalette));
            assert_eq!(resolve(gdk::Key::f, true, false, focus), Some(Action::RevealFilter));
            assert_eq!(resolve(gdk::Key::a, true, false, focus), Some(Action::Command(CommandId::SelectAll)));
            assert_eq!(resolve(gdk::Key::r, true, false, focus), Some(Action::Command(CommandId::Reload)));
            assert_eq!(resolve(gdk::Key::h, true, false, focus), Some(Action::Command(CommandId::ToggleHidden)));
            assert_eq!(resolve(gdk::Key::c, true, false, focus), Some(Action::Command(CommandId::ClipCopy)));
            assert_eq!(resolve(gdk::Key::x, true, false, focus), Some(Action::Command(CommandId::ClipCut)));
            assert_eq!(resolve(gdk::Key::v, true, false, focus), Some(Action::Command(CommandId::ClipPaste)));
        }
    }

    #[test]
    fn ctrl_does_not_leak_into_the_plain_letter_bindings() {
        // Ctrl+C is the clipboard copy, and unbound Ctrl+letters stay unbound.
        assert_eq!(resolve(gdk::Key::c, true, false, Focus::List), Some(Action::Command(CommandId::ClipCopy)));
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
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        // Sweep the whole keyval space we bind from, in both focus modes.
        let keys = [
            gdk::Key::asciitilde, gdk::Key::Insert, gdk::Key::asterisk,
            gdk::Key::plus, gdk::Key::minus, gdk::Key::c, gdk::Key::h,
            gdk::Key::F5, gdk::Key::F6, gdk::Key::F8,
            gdk::Key::Delete, gdk::Key::F2, gdk::Key::F7, gdk::Key::Tab,
            gdk::Key::grave, gdk::Key::F3, gdk::Key::space, gdk::Key::slash,
            gdk::Key::BackSpace, gdk::Key::Return, gdk::Key::F4, gdk::Key::Escape,
            gdk::Key::p, gdk::Key::f, gdk::Key::a, gdk::Key::r, gdk::Key::x, gdk::Key::v,
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

    #[test]
    fn manage_functions_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageFunctions));
    }

    #[test]
    fn hotkey_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageHotkeys));
        assert!(known.contains(&CommandId::FindKey));
    }

    #[test]
    fn user_menu_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageUserMenu));
    }

    #[test]
    fn script_events_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageScripts));
    }
}
