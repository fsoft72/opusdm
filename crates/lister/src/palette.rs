//! Command Palette HUD dialog (Ctrl+P / Ctrl+Shift+P).

use crate::filter::fuzzy_match;
use crate::keys::CommandId;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{Op, Role, ToHub};
use std::borrow::Cow;
use std::rc::Rc;

#[derive(Clone)]
pub struct CommandItem {
    pub id: CommandId,
    pub title: Cow<'static, str>,
    pub category: Cow<'static, str>,
    pub shortcut: Option<&'static str>,
}

/// The built-in entries, built once from [`CommandId::BUILTIN`] and
/// [`CommandId::meta`] so `get_commands` just clones them.
static COMMANDS: std::sync::LazyLock<Vec<CommandItem>> = std::sync::LazyLock::new(|| {
    CommandId::BUILTIN
        .iter()
        .filter_map(|id| {
            let meta = id.meta()?;
            Some(CommandItem {
                id: id.clone(),
                title: Cow::Borrowed(meta.title),
                category: Cow::Borrowed(meta.category),
                shortcut: meta.shortcut,
            })
        })
        .collect()
});

/// Borrow the command palette's entries.
/// Every palette entry: the static commands, plus one dynamic "Function"
/// entry per saved function in `functions` (the Lister's copy, which the hub
/// keeps current).
pub fn get_commands(functions: &[opusdm_ipc::function::Function]) -> Vec<CommandItem> {
    let mut items: Vec<CommandItem> = COMMANDS.clone();
    for f in functions.iter().cloned() {
        items.push(CommandItem {
            id: CommandId::RunFunction(f.id),
            title: Cow::Owned(f.name),
            category: Cow::Borrowed("Function"),
            shortcut: None,
        });
    }
    items
}

/// The static built-in commands, without the dynamic per-function entries
/// (no disk read): what "Find Key" names a built-in key by.
pub fn builtin_commands() -> &'static [CommandItem] {
    &COMMANDS
}

/// Execute a command by ID. The match is exhaustive (no wildcard arm): a
/// `CommandId` added without a handler here is a compile error, not a
/// silently dead-ended shortcut. Every arm is one call; the longer bodies live
/// in `commands.rs`, grouped by category.
pub fn execute_command(ui: &Rc<Ui>, cmd_id: CommandId) {
    use crate::commands as c;
    match cmd_id {
        // View
        CommandId::Filter => ui.reveal_filter(),
        CommandId::OpenViewer => crate::context_menu::open_in_viewer(ui, ui.selected_paths()),
        CommandId::ToggleSidecar => crate::sidecar::toggle(ui),
        CommandId::ToggleHidden => c::toggle_hidden(ui),
        CommandId::SizeFormat => crate::size_format::open(ui),
        CommandId::FindFiles => crate::find_dialog::open(ui, false),
        CommandId::SearchInFiles => crate::find_dialog::open(ui, true),
        CommandId::ReadSmart => crate::viewer::open_from_selection(ui, crate::viewer_mode::Mode::Smart),
        CommandId::ReadText => crate::viewer::open_from_selection(ui, crate::viewer_mode::Mode::Text),
        CommandId::ReadAnsi => crate::viewer::open_from_selection(ui, crate::viewer_mode::Mode::Ansi),
        CommandId::ReadHex => crate::viewer::open_from_selection(ui, crate::viewer_mode::Mode::Hex),
        CommandId::ShowImages => crate::viewer::show_images(ui),
        CommandId::CacheList => crate::cache_list::open(ui),
        CommandId::FreeCaches => crate::cache_list::free_all(ui),
        CommandId::ListerFormat => crate::format_editor::open(ui),
        CommandId::SetShowPattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Show),
        CommandId::SetHidePattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Hide),
        CommandId::Reload => ui.reload_by_user(),

        // Selection
        CommandId::SelectAll => {
            ui.view.selection.select_all();
        }
        CommandId::DeselectAll => {
            ui.remember_selection();
            ui.view.selection.unselect_all();
        }
        CommandId::InvertSelection => c::invert_selection(ui),
        CommandId::SelectPattern => c::select_by_pattern(ui, true),
        CommandId::DeselectPattern => c::select_by_pattern(ui, false),
        CommandId::AdvancedSelect => crate::advanced_select::open(ui),
        CommandId::Reselect => ui.reselect(),

        // File operations
        CommandId::Copy => c::transfer_to_dest(ui, Op::Copy, opusdm_ipc::JobOptions::default()),
        CommandId::Move => c::transfer_to_dest(ui, Op::Move, opusdm_ipc::JobOptions::default()),
        CommandId::CopyNewer => {
            c::transfer_to_dest(ui, Op::Copy, opusdm_ipc::JobOptions { newer: true, ..Default::default() })
        }
        CommandId::CopyAs => crate::copy_as::open(ui, false),
        CommandId::MoveAs => crate::copy_as::open(ui, true),
        CommandId::Duplicate => c::duplicate(ui),
        CommandId::Delete => c::delete_selection(ui, false),
        CommandId::DeletePermanent => c::delete_selection(ui, true),
        CommandId::ShowTrash => crate::trash_ui::open(ui),
        CommandId::Mkdir => c::create_here(ui, false),
        CommandId::NewFile => c::create_here(ui, true),
        CommandId::Rename => c::rename_selection(ui),
        CommandId::BatchRename => crate::batch_rename::open_batch_rename_dialog(ui),
        CommandId::ChangePermissions => crate::attr_dialogs::open_permissions(ui),
        CommandId::SetTimestamp => crate::attr_dialogs::open_timestamp(ui),
        CommandId::MakeLink => crate::link_dialogs::open_link(ui),
        CommandId::JoinFiles => crate::link_dialogs::open_join(ui),
        CommandId::SplitFile => crate::link_dialogs::open_split(ui),
        CommandId::CheckFit => crate::checkfit_ui::start(ui),
        CommandId::Compress => crate::compress_dialog::open_compress(ui, ui.selected_paths()),
        CommandId::Encrypt => crate::crypt_dialog::open_crypt(ui, true, ui.selected_paths()),
        CommandId::Decrypt => crate::crypt_dialog::open_crypt(ui, false, ui.selected_paths()),

        // Clipboard
        CommandId::ClipCopy => crate::clip::copy_or_cut(ui, false),
        CommandId::ClipCut => crate::clip::copy_or_cut(ui, true),
        CommandId::ClipPaste => crate::clip::paste(ui),

        // Navigation
        CommandId::HistoryBack => ui.history_back(),
        CommandId::HistoryForward => ui.history_forward(),
        CommandId::ParentDir => c::go_parent(ui),
        CommandId::HomeDir => c::go_home(ui),
        CommandId::RootDir => c::go_root(ui),
        CommandId::ParentNew => c::open_parent_in_new(ui),
        CommandId::RootNew => c::open_root_in_new(ui),
        CommandId::Bookmarks => crate::bookmarks_ui::open_manager(ui),
        CommandId::AddBookmark => crate::bookmarks_ui::add_current_folder(ui),

        // Tools
        CommandId::Terminal => ui.toggle_terminal(),
        CommandId::CalcSizes => crate::tools::calculate_dir_sizes_async(ui, false),
        CommandId::CalcSizesForce => crate::tools::calculate_dir_sizes_async(ui, true),
        CommandId::CalcChecksums => crate::tools::open_checksum_dialog(ui),
        CommandId::CompareDirs => crate::tools::open_compare_dialog(ui),
        CommandId::ViewFont => crate::font_viewer::open(ui),

        // Roles
        CommandId::SetSource => c::request_role(ui, Role::Source),
        CommandId::SetDest => c::request_role(ui, Role::Dest),
        CommandId::SetNone => c::request_role(ui, Role::None),
        CommandId::TogglePin => c::toggle_pin(ui),

        // Functions
        CommandId::ManageFunctions => {
            if let Err(e) = crate::function_editor::launch_manager() {
                crate::actions::flash(ui, &format!("cannot open Manage Functions: {e}"));
            }
        }
        CommandId::ManageBanks => {
            ui.send_hub(&ToHub::OpenBankManager);
        }
        CommandId::ManageHotkeys => {
            if let Err(e) = crate::hotkey_editor::launch_editor() {
                crate::actions::flash(ui, &format!("cannot open Edit Hotkeys: {e}"));
            }
        }
        CommandId::FindKey => crate::hotkey_editor::open_find_key(ui),
        CommandId::ManageUserMenu => crate::menu_editor::open_menu_editor(ui),
        CommandId::ManageScripts => crate::scripts_editor::open_scripts_editor(ui),
        CommandId::JsScripts => {
            if let Err(e) = crate::scripts_ui::launch() {
                crate::actions::flash(ui, &format!("cannot open Scripts: {e}"));
            }
        }
        CommandId::CommandLine => crate::cli_dialog::open_command_line(ui),
        CommandId::ManageFiletypes => crate::filetypes_ui::open_editor(ui, None),
        CommandId::CreateFiletype => c::create_filetype(ui),
        CommandId::RunFunction(id) => c::run_function(ui, id),

        // Network
        CommandId::FtpBook => crate::remote_ui::open_address_book(ui),
        CommandId::FtpQuit => ui.quit_remote(),
    }
}

/// New scroll offset that brings the `[top, bottom]` span fully into a
/// viewport currently showing `[value, value + page]`, or `None` when it is
/// already fully visible.
fn reveal_offset(value: f64, page: f64, top: f64, bottom: f64) -> Option<f64> {
    if top < value { return Some(top); }
    if bottom > value + page { return Some(bottom - page); }
    None
}

/// Select `row` and scroll `scroller` so it is visible. Keyboard focus
/// stays in the search entry, so the ListBox never scrolls on its own.
fn select_and_reveal(list_box: &gtk::ListBox, scroller: &gtk::ScrolledWindow, row: &gtk::ListBoxRow) {
    list_box.select_row(Some(row));
    let Some(bounds) = row.compute_bounds(list_box) else { return };
    let adj = scroller.vadjustment();
    let top = f64::from(bounds.y());
    let bottom = top + f64::from(bounds.height());
    if let Some(v) = reveal_offset(adj.value(), adj.page_size(), top, bottom) {
        adj.set_value(v);
    }
}

/// One palette row: its entry's category, title and shortcut in a line.
fn build_command_row(cmd: &CommandItem) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(3);
    hbox.set_margin_bottom(3);

    let cat_label = gtk::Label::new(Some(cmd.category.as_ref()));
    cat_label.add_css_class("palette-cat");
    cat_label.set_size_request(80, -1);
    cat_label.set_xalign(0.0);

    let title_label = gtk::Label::new(Some(cmd.title.as_ref()));
    title_label.set_xalign(0.0);
    title_label.set_hexpand(true);
    title_label.add_css_class("palette-title");

    hbox.append(&cat_label);
    hbox.append(&title_label);

    if let Some(sc) = cmd.shortcut {
        let sc_label = gtk::Label::new(Some(sc));
        sc_label.add_css_class("palette-shortcut");
        hbox.append(&sc_label);
    }

    row.set_child(Some(&hbox));
    row
}

/// Hide the rows that do not match the search text and select the first one left.
fn connect_filter(entry: &gtk::SearchEntry, rows: &PaletteRows, list_box: &gtk::ListBox, scroller: &gtk::ScrolledWindow) {
    let rows = rows.clone();
    let list_box = list_box.clone();
    let scroller = scroller.clone();
    entry.connect_search_changed(move |e| {
        let q = e.text().to_string();
        let mut first_visible: Option<&gtk::ListBoxRow> = None;
        for (row, cmd) in &rows {
            let haystack = format!("{} {}", cmd.title, cmd.category);
            let visible = fuzzy_match(&q, &haystack);
            row.set_visible(visible);
            if visible && first_visible.is_none() {
                first_visible = Some(row);
            }
        }
        if let Some(r) = first_visible {
            select_and_reveal(&list_box, &scroller, r);
        }
    });
}

/// The index to select when moving one visible row `down` or up from
/// `current` among `len` visible rows; `None` at either end.
fn neighbour_index(len: usize, current: usize, down: bool) -> Option<usize> {
    if down {
        (current + 1 < len).then_some(current + 1)
    } else {
        current.checked_sub(1)
    }
}

/// Move the selection one visible row down or up.
fn move_selection(rows: &PaletteRows, list_box: &gtk::ListBox, scroller: &gtk::ScrolledWindow, down: bool) {
    let visible_rows: Vec<&gtk::ListBoxRow> = rows.iter().filter(|(r, _)| r.is_visible()).map(|(r, _)| r).collect();
    let Some(sel) = list_box.selected_row() else { return };
    let Some(pos) = visible_rows.iter().position(|r| **r == sel) else { return };
    if let Some(next) = neighbour_index(visible_rows.len(), pos, down) {
        select_and_reveal(list_box, scroller, visible_rows[next]);
    }
}

/// A closure that closes the palette and runs the selected row's command.
fn make_execute_selected(ui: &Rc<Ui>, dlg: &gtk::Window, list_box: &gtk::ListBox, rows: &PaletteRows) -> Rc<dyn Fn()> {
    let ui = ui.clone();
    let dlg = dlg.clone();
    let list_box = list_box.clone();
    let rows = rows.clone();
    Rc::new(move || {
        let Some(selected_row) = list_box.selected_row() else { return };
        if let Some((_, cmd)) = rows.iter().find(|(r, _)| *r == selected_row) {
            dlg.close();
            execute_command(&ui, cmd.id.clone());
        }
    })
}

/// Enter, Esc, Up and Down in the search entry.
fn connect_navigation_keys(
    entry: &gtk::SearchEntry,
    dlg: &gtk::Window,
    rows: &PaletteRows,
    list_box: &gtk::ListBox,
    scroller: &gtk::ScrolledWindow,
    exec: Rc<dyn Fn()>,
) {
    let key_ctrl = gtk::EventControllerKey::new();
    // Capture phase: the entry's inner text widget handles Enter itself
    // (it emits `activate` and stops the event), so a bubble-phase
    // controller on the entry never sees it.
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let dlg = dlg.clone();
    let list_box = list_box.clone();
    let scroller = scroller.clone();
    let rows = rows.clone();
    key_ctrl.connect_key_pressed(move |_, keyval, _, _| match keyval {
        gdk::Key::Escape => {
            dlg.close();
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter => {
            exec();
            glib::Propagation::Stop
        }
        gdk::Key::Down => {
            move_selection(&rows, &list_box, &scroller, true);
            glib::Propagation::Stop
        }
        gdk::Key::Up => {
            move_selection(&rows, &list_box, &scroller, false);
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    entry.add_controller(key_ctrl);
}

/// The palette's rows, each paired with the command it runs.
type PaletteRows = Vec<(gtk::ListBoxRow, CommandItem)>;

/// Open the Command Palette modal HUD.
pub fn open_command_palette(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "OpusDM Command Palette")
        .size(520, 380)
        .resizable(false);
    let root = shell.content();
    root.add_css_class("command-palette");

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Type a command... (Esc to cancel)"));
    entry.add_css_class("palette-search");
    root.append(&entry);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    list_box.add_css_class("palette-list");

    let rows: PaletteRows = get_commands(&ui.hub.functions.borrow())
        .into_iter()
        .map(|cmd| {
            let row = build_command_row(&cmd);
            list_box.append(&row);
            (row, cmd)
        })
        .collect();

    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).hexpand(true).build();
    root.append(&scroller);

    if let Some((first_row, _)) = rows.first() {
        list_box.select_row(Some(first_row));
    }

    connect_filter(&entry, &rows, &list_box, &scroller);
    let execute_selected = make_execute_selected(ui, shell.window(), &list_box, &rows);
    {
        let exec = execute_selected.clone();
        list_box.connect_row_activated(move |_, _| exec());
    }
    connect_navigation_keys(&entry, shell.window(), &rows, &list_box, &scroller, execute_selected);

    shell.present();
    entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_down_stops_at_the_last_visible_row() {
        assert_eq!(neighbour_index(3, 0, true), Some(1));
        assert_eq!(neighbour_index(3, 2, true), None);
    }

    #[test]
    fn moving_up_stops_at_the_first_visible_row() {
        assert_eq!(neighbour_index(3, 2, false), Some(1));
        assert_eq!(neighbour_index(3, 0, false), None);
    }

    #[test]
    fn reveal_offset_leaves_a_fully_visible_row_alone() {
        assert_eq!(reveal_offset(100.0, 300.0, 150.0, 180.0), None);
    }

    #[test]
    fn reveal_offset_scrolls_down_to_a_row_below_the_viewport() {
        assert_eq!(reveal_offset(0.0, 300.0, 290.0, 320.0), Some(20.0));
    }

    #[test]
    fn reveal_offset_scrolls_up_to_a_row_above_the_viewport() {
        assert_eq!(reveal_offset(100.0, 300.0, 60.0, 90.0), Some(60.0));
    }
}
