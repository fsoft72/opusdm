//! Keyboard bindings.
//!
//! [`resolve`] is a pure key -> action mapping so the whole binding table can
//! be unit tested; `window.rs` only has to execute the action it gets back.
//! There are no single-letter commands: plain letters over the file list start
//! type-to-find (see `typeahead.rs`). The caller still passes the right
//! [`Focus`], because while a text field has the keyboard `+`, `-`, `/` and
//! space are characters, not commands.

use gtk4::gdk;

/// Declares [`CommandId`] together with what the command palette shows for each
/// built-in command: title, category and shortcut. One entry per command is the
/// only place a new command needs describing (the palette entry order is the
/// order written here); `palette::execute_command` still has to handle it, and
/// its `match` is exhaustive, so forgetting that is a compile error.
macro_rules! commands {
    ($( $(#[$doc:meta])* $name:ident => ($title:expr, $category:expr, $shortcut:expr) ),* $(,)?) => {
        /// One entry in the command palette / a target `Action::Command` can name.
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub enum CommandId {
            $( $(#[$doc])* $name, )*
            /// Run a saved Function by id (not a built-in: it has no [`CommandMeta`]).
            RunFunction(opusdm_ipc::function::FunctionId),
        }

        impl CommandId {
            /// Every built-in command, in palette order.
            pub const BUILTIN: &'static [CommandId] = &[ $( CommandId::$name, )* ];

            /// Title, category and shortcut of a built-in command; `None` for
            /// [`CommandId::RunFunction`], whose title is the function's name.
            pub fn meta(&self) -> Option<CommandMeta> {
                match self {
                    $( CommandId::$name => Some(CommandMeta { title: $title, category: $category, shortcut: $shortcut }), )*
                    CommandId::RunFunction(_) => None,
                }
            }
        }
    };
}

/// What the command palette shows for a built-in command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMeta {
    pub title: &'static str,
    pub category: &'static str,
    pub shortcut: Option<&'static str>,
}

commands! {
    Filter => ("Filter Files in Current View", "View", Some("/")),
    /// Open the selection (files and folders) in `opusdm-viewer`.
    OpenViewer => ("Open in Viewer", "View", Some("F3")),
    /// Open or close the Sidecar `opusdm-viewer` that follows the selection.
    ToggleSidecar => ("Toggle Sidecar Viewer", "View", Some("Shift+F3")),
    SelectAll => ("Select All", "Selection", Some("Ctrl+A")),
    DeselectAll => ("Deselect All", "Selection", Some("Esc")),
    InvertSelection => ("Invert Selection", "Selection", Some("*")),
    SelectPattern => ("Select by Pattern (*.rs, etc.)", "Selection", Some("+")),
    /// Open the Advanced Select requester.
    AdvancedSelect => ("Advanced Select... (name, size, date, type)", "Selection", None),
    DeselectPattern => ("Deselect by Pattern", "Selection", Some("-")),
    ToggleHidden => ("Toggle Hidden Files", "View", Some("Ctrl+H")),
    /// Open the Size Format requester (how the Size column is written).
    SizeFormat => ("Size Format...", "View", None),
    /// Open the Find dialog on the Name field.
    FindFiles => ("Find Files...", "View", None),
    /// Open the Find dialog on the Contains field.
    SearchInFiles => ("Search in Files...", "View", None),
    /// Open the Viewer in SmartRead mode.
    ReadSmart => ("Read (SmartRead)", "View", None),
    /// Open the Viewer as text.
    ReadText => ("Read as Text", "View", None),
    /// Open the Viewer as ANSI art.
    ReadAnsi => ("Read as ANSI", "View", None),
    /// Open the Viewer as a hex dump.
    ReadHex => ("Read as Hex", "View", None),
    /// Open the image slideshow.
    ShowImages => ("Show (image slideshow)", "View", None),
    /// Open the Cache List.
    CacheList => ("Cache List...", "View", None),
    /// Drop every Lister's directory cache.
    FreeCaches => ("Free Caches", "View", None),
    /// Open the Lister Format editor.
    ListerFormat => ("Lister Format...", "View", None),
    /// Ask for the persistent Show pattern list.
    SetShowPattern => ("Set Show Pattern...", "View", None),
    /// Ask for the persistent Hide pattern list.
    SetHidePattern => ("Set Hide Pattern...", "View", None),
    Copy => ("Copy (Source -> Dest)", "File Ops", Some("F5")),
    Move => ("Move (Source -> Dest)", "File Ops", Some("F6")),
    Delete => ("Delete Selected Items", "File Ops", Some("Del")),
    /// Copy to DEST under names built from a pattern.
    CopyAs => ("Copy As... (Source -> Dest, new names)", "File Ops", None),
    /// Move to DEST under names built from a pattern.
    MoveAs => ("Move As... (Source -> Dest, new names)", "File Ops", None),
    /// Copy each selected entry next to itself as `name.copy`.
    Duplicate => ("Duplicate Selected Items", "File Ops", None),
    /// Copy to DEST only what is missing there or newer than the DEST copy.
    CopyNewer => ("Copy (newer only, Source -> Dest)", "File Ops", None),
    /// Open the permissions dialog.
    ChangePermissions => ("Change Permissions...", "File Ops", None),
    /// Open the timestamp dialog.
    SetTimestamp => ("Set Timestamp...", "File Ops", None),
    /// Open the link dialog.
    MakeLink => ("Make Link... (hard or symbolic, Source -> Dest)", "File Ops", None),
    /// Open the join dialog.
    JoinFiles => ("Join Files...", "File Ops", None),
    /// Open the split dialog.
    SplitFile => ("Split File...", "File Ops", None),
    /// Check whether the selection fits in the free space at DEST.
    CheckFit => ("Check Fit (Source -> Dest free space)...", "File Ops", None),
    /// Select again what the last job or Deselect All cleared.
    Reselect => ("Reselect Previous Selection", "Selection", Some("Ctrl+Shift+A")),
    ClipCopy => ("Copy to Clipboard", "Clipboard", Some("Ctrl+C")),
    ClipCut => ("Cut to Clipboard", "Clipboard", Some("Ctrl+X")),
    ClipPaste => ("Paste from Clipboard", "Clipboard", Some("Ctrl+V")),
    Mkdir => ("Make Directory...", "File Ops", Some("Ctrl+N")),
    NewFile => ("New File...", "File Ops", Some("Ctrl+Shift+N")),
    Rename => ("Rename...", "File Ops", Some("F2")),
    BatchRename => ("Batch Rename Selected Files (Regex/Numbering)...", "File Ops", Some("F2 (multi)")),
    Reload => ("Reload / Refresh Current Directory", "View", Some("Ctrl+R")),
    /// Go back in the history.
    HistoryBack => ("History Back", "Navigation", Some("Alt+Left")),
    /// Go forward in the history.
    HistoryForward => ("History Forward", "Navigation", Some("Alt+Right")),
    ParentDir => ("Go to Parent Directory", "Navigation", Some("Backspace")),
    HomeDir => ("Go to Home Directory (~)", "Navigation", Some("~")),
    Terminal => ("Toggle Terminal Drawer", "Tools", Some("F4 / `")),
    CalcSizes => ("Calculate Directory Sizes", "Tools", Some("Space (on dir)")),
    /// Calculate directory sizes again, ignoring the ones already known.
    CalcSizesForce => ("Calculate Directory Sizes (force rescan)", "Tools", None),
    CalcChecksums => ("Calculate Checksums (SHA-256 / MD5)...", "Tools", None),
    CompareDirs => ("Compare / Sync Directories (Visual Diff)...", "Tools", None),
    SetSource => ("Set Role: SOURCE", "Roles", None),
    SetDest => ("Set Role: DEST", "Roles", None),
    SetNone => ("Set Role: NONE", "Roles", None),
    TogglePin => ("Toggle Role Pin / Lock", "Roles", None),
    /// Open the Function Editor's manager window (list/new/edit/delete).
    ManageFunctions => ("Manage Functions...", "Functions", None),
    ManageBanks => ("Button Banks...", "Functions", None),
    /// Open the hotkey editor.
    ManageHotkeys => ("Edit Hotkeys...", "Functions", None),
    /// Ask for a key combination and say what it does.
    FindKey => ("Find Key...", "Functions", None),
    /// Open the user menu editor.
    ManageUserMenu => ("Edit User Menu...", "Functions", None),
    /// Open the script events editor.
    ManageScripts => ("Edit Script Events...", "Functions", None),
    /// Pack the selection into an archive.
    Compress => ("Compress... (zip, tar.gz, 7z, ... Source -> Dest)", "File Ops", None),
    /// Encrypt the selection with a password.
    Encrypt => ("Encrypt... (password, Source -> Dest as .enc)", "File Ops", None),
    /// Decrypt the selected `.enc` files with a password.
    Decrypt => ("Decrypt... (.enc files, Source -> Dest)", "File Ops", None),
    /// Open the font previewer.
    ViewFont => ("View Font...", "Tools", None),
    /// Open the bookmark manager.
    Bookmarks => ("Bookmarks...", "Navigation", None),
    /// Bookmark the current folder.
    AddBookmark => ("Add Bookmark for This Folder...", "Navigation", None),
    /// Go to the root of the current tree.
    RootDir => ("Go to Root", "Navigation", None),
    /// Open the parent folder in a new Lister.
    ParentNew => ("Open Parent in NEW Lister", "Navigation", None),
    /// Open the root of the current tree in a new Lister.
    RootNew => ("Open Root in NEW Lister", "Navigation", None),
    /// Open the internal command line.
    CommandLine => ("Command Line...", "Functions", None),
    /// Open the FTP address book.
    FtpBook => ("FTP / SFTP Address Book...", "Network", None),
    /// Close every FTP / SFTP connection.
    FtpQuit => ("FTP Quit (close all connections)", "Network", None),
    /// Open the File Types editor.
    ManageFiletypes => ("Edit File Types...", "Functions", None),
    /// Open the File Types editor with a new type derived from the selected file.
    CreateFiletype => ("Filetype Creator (from selected file)...", "Functions", None),
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
    /// Ctrl+Return: open the folder under the cursor in a new Lister.
    EnterNew,
    /// Toggle the selection of the row under the cursor, then move down.
    ToggleSelectAndAdvance,
    /// Raise the Lister holding the opposite role.
    SwapRoleFocus,
    /// Show the command palette.
    OpenPalette,
    /// Reveal the filter bar.
    RevealFilter,
    /// Ctrl+L: focus the path bar with its whole text selected.
    FocusPath,
    /// Show or hide the terminal drawer.
    ToggleTerminal,
    /// Context-dependent dismiss (filter, terminal, then selection).
    Escape,
}

/// Map a key press to an [`Action`], or `None` to let GTK handle it.
///
/// `shift` is only consulted for keys whose keyval does not already encode it
/// (Ctrl+N versus Ctrl+Shift+N); shifted characters such as `*`, `+`, `~` and `G` arrive as their own
/// keyvals.
pub fn resolve(key: gdk::Key, ctrl: bool, shift: bool, focus: Focus) -> Option<Action> {
    // Ctrl chords work wherever the focus is, including inside text fields.
    if ctrl {
        // New folder / new file only fire over the list: inside a text field
        // Ctrl+N must not create anything.
        if focus == Focus::List && matches!(key, gdk::Key::n | gdk::Key::N) {
            return Some(Action::Command(if shift { CommandId::NewFile } else { CommandId::Mkdir }));
        }
        if focus == Focus::List && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
            return Some(Action::EnterNew);
        }
        return match key {
            gdk::Key::p | gdk::Key::P => Some(Action::OpenPalette),
            gdk::Key::f | gdk::Key::F => Some(Action::RevealFilter),
            gdk::Key::l | gdk::Key::L => Some(Action::FocusPath),
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
        // `~` is AltGr+ì on an Italian layout, where GTK may report the dead
        // tilde keyval instead of `asciitilde`, so the character is checked too.
        gdk::Key::asciitilde | gdk::Key::dead_tilde => Some(Action::Command(CommandId::HomeDir)),
        k if k.to_unicode() == Some('~') => Some(Action::Command(CommandId::HomeDir)),

        // Selection.
        gdk::Key::Insert | gdk::Key::KP_Insert => Some(Action::ToggleSelectAndAdvance),
        gdk::Key::asterisk | gdk::Key::KP_Multiply => Some(Action::Command(CommandId::InvertSelection)),
        gdk::Key::plus | gdk::Key::KP_Add => Some(Action::Command(CommandId::SelectPattern)),
        gdk::Key::minus | gdk::Key::KP_Subtract => Some(Action::Command(CommandId::DeselectPattern)),

        // File operations.
        gdk::Key::F5 => Some(Action::Command(CommandId::Copy)),
        gdk::Key::F6 => Some(Action::Command(CommandId::Move)),
        gdk::Key::Delete | gdk::Key::KP_Delete => Some(Action::Command(CommandId::Delete)),
        gdk::Key::F2 => Some(Action::Command(CommandId::Rename)),

        // Roles and panels.
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Some(Action::SwapRoleFocus),
        gdk::Key::grave => Some(Action::ToggleTerminal),
        gdk::Key::F3 if shift => Some(Action::Command(CommandId::ToggleSidecar)),
        gdk::Key::F3 => Some(Action::Command(CommandId::OpenViewer)),
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
    fn ctrl_return_enters_in_a_new_lister_only_over_the_list() {
        assert_eq!(resolve(gdk::Key::Return, true, false, Focus::List), Some(Action::EnterNew));
        assert_eq!(resolve(gdk::Key::KP_Enter, true, false, Focus::List), Some(Action::EnterNew));
        assert_eq!(resolve(gdk::Key::Return, true, false, Focus::Editing), None);
    }

    #[test]
    fn file_operations_are_on_function_keys() {
        assert_eq!(list(gdk::Key::F5), Some(Action::Command(CommandId::Copy)));
        assert_eq!(list(gdk::Key::F6), Some(Action::Command(CommandId::Move)));
        assert_eq!(list(gdk::Key::F8), None, "F8 is no longer bound");
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
        assert_eq!(list(gdk::Key::KP_Multiply), Some(Action::Command(CommandId::InvertSelection)));
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
    fn every_tilde_keyval_goes_home() {
        for key in [gdk::Key::asciitilde, gdk::Key::dead_tilde] {
            assert_eq!(list(key), Some(Action::Command(CommandId::HomeDir)), "{key:?}");
        }
    }

    #[test]
    fn shift_only_matters_where_the_keyval_does_not_encode_it() {
        assert_eq!(resolve(gdk::Key::n, true, false, Focus::List), Some(Action::Command(CommandId::Mkdir)));
        assert_eq!(resolve(gdk::Key::N, true, true, Focus::List), Some(Action::Command(CommandId::NewFile)));
        assert_eq!(resolve(gdk::Key::F7, false, false, Focus::List), None, "F7 is no longer bound");
        assert_eq!(resolve(gdk::Key::F7, false, true, Focus::List), None, "Shift+F7 is no longer bound");
        assert_eq!(resolve(gdk::Key::n, true, false, Focus::Editing), None, "Ctrl+N stays out of text fields");
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
    fn f3_opens_the_viewer_and_space_is_unbound() {
        assert_eq!(list(gdk::Key::F3), Some(Action::Command(CommandId::OpenViewer)));
        assert_eq!(resolve(gdk::Key::F3, false, true, Focus::List), Some(Action::Command(CommandId::ToggleSidecar)));
        assert_eq!(list(gdk::Key::space), None);
        assert_eq!(list(gdk::Key::KP_Space), None);
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
            assert_eq!(resolve(gdk::Key::l, true, false, focus), Some(Action::FocusPath));
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
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        // Sweep the whole keyval space we bind from, in both focus modes.
        let keys = [
            gdk::Key::asciitilde, gdk::Key::Insert, gdk::Key::asterisk,
            gdk::Key::plus, gdk::Key::minus, gdk::Key::c, gdk::Key::h,
            gdk::Key::F5, gdk::Key::F6, gdk::Key::F8, gdk::Key::n, gdk::Key::N,
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
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageFunctions));
        assert!(known.contains(&CommandId::ManageBanks));
    }

    #[test]
    fn hotkey_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageHotkeys));
        assert!(known.contains(&CommandId::FindKey));
    }

    #[test]
    fn user_menu_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageUserMenu));
    }

    #[test]
    fn script_events_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageScripts));
    }

    #[test]
    fn view_font_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ViewFont));
    }

    #[test]
    fn bookmark_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::Bookmarks) && known.contains(&CommandId::AddBookmark));
    }

    #[test]
    fn root_and_parent_variants_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        for id in [CommandId::RootDir, CommandId::ParentNew, CommandId::RootNew] {
            assert!(known.contains(&id));
        }
    }

    #[test]
    fn force_size_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::CalcSizesForce));
    }

    #[test]
    fn command_line_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::CommandLine));
    }

    #[test]
    fn crypt_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::Encrypt));
        assert!(known.contains(&CommandId::Decrypt));
    }

    #[test]
    fn ftp_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::FtpBook));
        assert!(known.contains(&CommandId::FtpQuit));
    }

    #[test]
    fn filetype_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands(&[]).iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageFiletypes));
        assert!(known.contains(&CommandId::CreateFiletype));
    }

    #[test]
    fn every_builtin_command_has_a_distinct_title_and_a_category() {
        let mut titles = std::collections::HashSet::new();
        for id in CommandId::BUILTIN {
            let meta = id.meta().expect("a built-in command has metadata");
            assert!(!meta.title.is_empty() && !meta.category.is_empty(), "{id:?}");
            assert!(titles.insert(meta.title), "duplicate palette title: {}", meta.title);
        }
        assert!(CommandId::RunFunction(Default::default()).meta().is_none());
    }
}
