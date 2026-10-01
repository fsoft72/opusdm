//! Button banks: named, free-floating windows of buttons. Each button has a
//! label and/or an icon and up to three Functions (left, right, middle
//! click). One JSON file per bank under `~/.config/opusdm/buttons/`. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

use crate::function::FunctionId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Most buttons one bank may hold.
pub const MAX_BUTTONS: usize = 256;
/// Smallest and largest number of buttons per row (or per column).
pub const MIN_COLUMNS: u32 = 1;
pub const MAX_COLUMNS: u32 = 32;
/// Buttons per row (or per column) of a new bank.
pub const DEFAULT_COLUMNS: u32 = 4;
/// Saved window positions outside `0..=MAX_POSITION` are dropped on load.
const MAX_POSITION: i32 = 32_000;

/// A window whose top-left corner is closer than this to the right or bottom
/// edge of a monitor is treated as off screen.
const MIN_VISIBLE_PX: i32 = 40;

/// The saved position `pos` if it lands on one of `monitors` (each
/// `(x, y, width, height)`), so a bank saved on a monitor that is gone is
/// left to the window manager instead of opening off screen. With no monitor
/// information the position is kept.
pub fn position_on_screen(pos: (i32, i32), monitors: &[(i32, i32, i32, i32)]) -> Option<(i32, i32)> {
    if monitors.is_empty() {
        return Some(pos);
    }
    let (x, y) = pos;
    let visible = monitors.iter().any(|&(mx, my, w, h)| x >= mx && x < mx + w - MIN_VISIBLE_PX && y >= my && y < my + h - MIN_VISIBLE_PX);
    visible.then_some(pos)
}

/// Base directory of the bank files (`~/.config/opusdm/buttons`).
pub fn banks_dir() -> PathBuf {
    crate::settings::config_dir().join("buttons")
}

/// How the buttons are laid out: rows filled left to right, or columns
/// filled top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// The mouse button that was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// What a bank command asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankAction {
    Open,
    Close,
    Toggle,
    Hide,
}

/// The visible state of one bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankView {
    /// No window.
    Closed,
    /// A window that is open but hidden.
    Hidden,
    /// A visible window.
    Shown,
}

impl BankAction {
    /// The state a bank in `current` ends up in. `Hide` keeps a closed bank
    /// closed; `Toggle` closes a shown bank and shows anything else.
    pub fn apply(self, current: BankView) -> BankView {
        match (self, current) {
            (BankAction::Open, _) => BankView::Shown,
            (BankAction::Close, _) => BankView::Closed,
            (BankAction::Hide, BankView::Closed) => BankView::Closed,
            (BankAction::Hide, _) => BankView::Hidden,
            (BankAction::Toggle, BankView::Shown) => BankView::Closed,
            (BankAction::Toggle, _) => BankView::Shown,
        }
    }
}

/// One button: what it shows and the Function each mouse button runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Btn {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<FunctionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<FunctionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub middle: Option<FunctionId>,
}

impl Btn {
    /// The Function bound to `button`, if any.
    pub fn function_for(&self, button: MouseButton) -> Option<&str> {
        match button {
            MouseButton::Left => self.left.as_deref(),
            MouseButton::Right => self.right.as_deref(),
            MouseButton::Middle => self.middle.as_deref(),
        }
    }
}

/// One bank: its file id, appearance, saved window state and buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonBank {
    /// File stem of the bank's JSON file. Not stored inside the file.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub orientation: Orientation,
    pub borderless: bool,
    /// Buttons per row (Horizontal) or per column (Vertical).
    pub columns: u32,
    /// Whether the window was open when the hub last ran.
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    pub buttons: Vec<Btn>,
}

impl Default for ButtonBank {
    fn default() -> Self {
        ButtonBank {
            id: String::new(),
            name: String::new(),
            orientation: Orientation::Horizontal,
            borderless: false,
            columns: DEFAULT_COLUMNS,
            open: false,
            x: None,
            y: None,
            buttons: Vec::new(),
        }
    }
}

impl ButtonBank {
    /// Clamp `columns`, cap the button count and drop a saved position that
    /// is negative, beyond `MAX_POSITION` or only half there.
    pub fn cleaned(mut self) -> ButtonBank {
        self.columns = self.columns.clamp(MIN_COLUMNS, MAX_COLUMNS);
        self.buttons.truncate(MAX_BUTTONS);
        let ok = matches!((self.x, self.y), (Some(x), Some(y)) if (0..=MAX_POSITION).contains(&x) && (0..=MAX_POSITION).contains(&y));
        if !ok {
            self.x = None;
            self.y = None;
        }
        self
    }

    /// The `(row, column)` of button `index`: rows fill first when
    /// Horizontal, columns fill first when Vertical.
    pub fn grid_position(&self, index: usize) -> (usize, usize) {
        let columns = self.columns.clamp(MIN_COLUMNS, MAX_COLUMNS) as usize;
        match self.orientation {
            Orientation::Horizontal => (index / columns, index % columns),
            Orientation::Vertical => (index % columns, index / columns),
        }
    }

    /// Why this bank cannot be saved, if it cannot. A referenced Function
    /// only has to be a non-empty id: it may be deleted later, which the
    /// window shows instead.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("The bank name is empty".to_string());
        }
        if !(MIN_COLUMNS..=MAX_COLUMNS).contains(&self.columns) {
            return Err(format!("columns must be between {MIN_COLUMNS} and {MAX_COLUMNS}"));
        }
        if self.buttons.len() > MAX_BUTTONS {
            return Err(format!("A bank holds at most {MAX_BUTTONS} buttons"));
        }
        for (i, b) in self.buttons.iter().enumerate() {
            for id in [&b.left, &b.right, &b.middle].into_iter().flatten() {
                if id.trim().is_empty() {
                    return Err(format!("Button {}: empty Function id", i + 1));
                }
            }
        }
        Ok(())
    }

    /// Insert `button` before `index` (clamped to the end). Returns the
    /// index it ended up at.
    pub fn insert_button(&mut self, index: usize, button: Btn) -> Result<usize, String> {
        if self.buttons.len() >= MAX_BUTTONS {
            return Err(format!("A bank holds at most {MAX_BUTTONS} buttons"));
        }
        let index = index.min(self.buttons.len());
        self.buttons.insert(index, button);
        Ok(index)
    }

    /// Remove and return button `index`, or `None` when it does not exist.
    pub fn remove_button(&mut self, index: usize) -> Option<Btn> {
        if index >= self.buttons.len() {
            return None;
        }
        Some(self.buttons.remove(index))
    }

    /// Move button `index` by `delta` places. Returns its new index, or
    /// `None` when it would leave the list.
    pub fn move_button(&mut self, index: usize, delta: isize) -> Option<usize> {
        let target = index.checked_add_signed(delta)?;
        if index >= self.buttons.len() || target >= self.buttons.len() {
            return None;
        }
        self.buttons.swap(index, target);
        Some(target)
    }
}

/// Every bank, in file-name order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Banks {
    pub banks: Vec<ButtonBank>,
}

impl Banks {
    /// The bank with file id `id`.
    pub fn get(&self, id: &str) -> Option<&ButtonBank> {
        self.banks.iter().find(|b| b.id == id)
    }

    /// The bank called `name`, ignoring case.
    pub fn get_by_name(&self, name: &str) -> Option<&ButtonBank> {
        let name = name.trim().to_lowercase();
        self.banks.iter().find(|b| b.name.to_lowercase() == name)
    }

    /// Every bank name, in file-name order.
    pub fn names(&self) -> Vec<String> {
        self.banks.iter().map(|b| b.name.clone()).collect()
    }

    /// Whether a bank other than `except` already has `name`, ignoring case.
    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.trim().to_lowercase();
        self.banks.iter().any(|b| Some(b.id.as_str()) != except && b.name.to_lowercase() == name)
    }

    /// Create an empty bank called `name` (trimmed). Its id is a slug that
    /// also avoids every `<id>.json` already in `dir`, files that failed to
    /// load included, so a new bank can never overwrite a file it did not
    /// read. Returns the id. Empty and duplicate names are refused.
    pub fn create_in_dir(&mut self, dir: &Path, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The bank name is empty".to_string());
        }
        if self.name_taken(name, None) {
            return Err(format!("A bank named \"{name}\" already exists"));
        }
        let base = crate::groups::slugify(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() || dir.join(format!("{id}.json")).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.banks.push(ButtonBank { id: id.clone(), name: name.to_string(), ..ButtonBank::default() });
        self.banks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(id)
    }

    /// Replace bank `id` with `bank` after validating it. The id and the
    /// runtime state (`open`, `x`, `y`) of the stored bank are kept, so an
    /// editor holding an older copy cannot undo what the hub did meanwhile.
    pub fn replace(&mut self, id: &str, bank: ButtonBank) -> Result<(), String> {
        bank.validate()?;
        if self.name_taken(&bank.name, Some(id)) {
            return Err(format!("A bank named \"{}\" already exists", bank.name.trim()));
        }
        let slot = self.banks.iter_mut().find(|b| b.id == id).ok_or_else(|| format!("No bank \"{id}\""))?;
        let bank = ButtonBank { id: slot.id.clone(), open: slot.open, x: slot.x, y: slot.y, name: bank.name.trim().to_string(), ..bank };
        *slot = bank.cleaned();
        Ok(())
    }

    /// Remove bank `id` from the list. Returns whether it existed. The file
    /// is removed with [`Banks::remove_bank_file`].
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.banks.len();
        self.banks.retain(|b| b.id != id);
        self.banks.len() != before
    }

    /// Record whether bank `id` is open and where its window is.
    pub fn set_runtime(&mut self, id: &str, open: bool, pos: Option<(i32, i32)>) {
        let Some(bank) = self.banks.iter_mut().find(|b| b.id == id) else { return };
        bank.open = open;
        if let Some((x, y)) = pos {
            bank.x = Some(x);
            bank.y = Some(y);
        }
    }

    /// Load every `*.json` file of `dir` (sorted by name). A missing
    /// directory gives no banks. A file that cannot be read or parsed is
    /// skipped with a log line; a parse failure also copies it to
    /// `<file>.json.bak` (an existing backup is kept), so it survives even if
    /// something else later writes a bank with the same id.
    pub fn load_from_dir(dir: &Path) -> Banks {
        let Ok(read) = std::fs::read_dir(dir) else { return Banks::default() };
        let mut files: Vec<PathBuf> =
            read.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        files.sort();
        let mut banks = Vec::new();
        for path in files {
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let Some(mut bank) = crate::settings::read_json::<ButtonBank>(&path, "buttonbank", "bank file") else {
                continue;
            };
            bank.id = id.clone();
            if bank.name.trim().is_empty() {
                bank.name = id;
            }
            banks.push(bank.cleaned());
        }
        Banks { banks }
    }

    /// [`Banks::load_from_dir`] the standard directory.
    pub fn load() -> Banks {
        Banks::load_from_dir(&banks_dir())
    }

    /// Write bank `id` to `<dir>/<id>.json`, creating `dir`. An unknown id is
    /// an error.
    pub fn save_bank_to(&self, dir: &Path, id: &str) -> io::Result<()> {
        let bank = self.get(id).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no bank \"{id}\"")))?;
        crate::settings::save_json(&dir.join(format!("{id}.json")), bank)
    }

    /// [`Banks::save_bank_to`] the standard directory.
    pub fn save_bank(&self, id: &str) -> io::Result<()> {
        self.save_bank_to(&banks_dir(), id)
    }

    /// Delete `<dir>/<id>.json`. A file that is already gone is not an error.
    pub fn remove_bank_file_from(dir: &Path, id: &str) -> io::Result<()> {
        match std::fs::remove_file(dir.join(format!("{id}.json"))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// [`Banks::remove_bank_file_from`] the standard directory.
    pub fn remove_bank_file(id: &str) -> io::Result<()> {
        Banks::remove_bank_file_from(&banks_dir(), id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn(label: &str) -> Btn {
        Btn { label: label.to_string(), ..Btn::default() }
    }

    fn bank_with(n: usize) -> ButtonBank {
        let mut b = ButtonBank { name: "Tools".to_string(), ..ButtonBank::default() };
        for i in 0..n {
            b.buttons.push(btn(&format!("b{i}")));
        }
        b
    }

    #[test]
    fn defaults_are_four_columns_horizontal() {
        let b = ButtonBank::default();
        assert_eq!(b.columns, DEFAULT_COLUMNS);
        assert_eq!(b.orientation, Orientation::Horizontal);
        assert!(!b.open && !b.borderless);
    }

    #[test]
    fn a_file_without_the_new_fields_still_parses() {
        let b: ButtonBank = serde_json::from_str(r#"{"name":"Old"}"#).unwrap();
        assert_eq!(b.name, "Old");
        assert_eq!(b.columns, DEFAULT_COLUMNS);
        assert!(b.buttons.is_empty());
    }

    #[test]
    fn serde_round_trip_keeps_everything_but_the_id() {
        let mut b = bank_with(2);
        b.id = "tools".to_string();
        b.orientation = Orientation::Vertical;
        b.buttons[0].left = Some("f1".to_string());
        b.buttons[0].icon = Some(PathBuf::from("/a/b.png"));
        b.x = Some(10);
        b.y = Some(20);
        let back: ButtonBank = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.id, "");
        assert_eq!(ButtonBank { id: "tools".to_string(), ..back }, b);
    }

    #[test]
    fn function_for_maps_each_mouse_button() {
        let b = Btn { left: Some("l".into()), middle: Some("m".into()), ..Btn::default() };
        assert_eq!(b.function_for(MouseButton::Left), Some("l"));
        assert_eq!(b.function_for(MouseButton::Middle), Some("m"));
        assert_eq!(b.function_for(MouseButton::Right), None);
    }

    #[test]
    fn grid_position_fills_rows_when_horizontal_and_columns_when_vertical() {
        let mut b = bank_with(6);
        b.columns = 3;
        assert_eq!(b.grid_position(0), (0, 0));
        assert_eq!(b.grid_position(2), (0, 2));
        assert_eq!(b.grid_position(3), (1, 0));
        b.orientation = Orientation::Vertical;
        assert_eq!(b.grid_position(0), (0, 0));
        assert_eq!(b.grid_position(2), (2, 0));
        assert_eq!(b.grid_position(3), (0, 1));
    }

    #[test]
    fn validate_rejects_empty_name_bad_columns_and_too_many_buttons() {
        assert!(bank_with(1).validate().is_ok());
        let mut b = bank_with(1);
        b.name = "  ".to_string();
        assert!(b.validate().unwrap_err().contains("name"));
        let mut b = bank_with(1);
        b.columns = 0;
        assert!(b.validate().unwrap_err().contains("columns"));
        b.columns = MAX_COLUMNS + 1;
        assert!(b.validate().unwrap_err().contains("columns"));
        assert!(bank_with(MAX_BUTTONS + 1).validate().unwrap_err().contains("buttons"));
    }

    #[test]
    fn validate_rejects_an_empty_function_id() {
        let mut b = bank_with(1);
        b.buttons[0].left = Some(" ".to_string());
        assert!(b.validate().unwrap_err().contains("Function"));
    }

    #[test]
    fn insert_remove_and_move_buttons() {
        let mut b = bank_with(3);
        assert_eq!(b.insert_button(1, btn("new")), Ok(1));
        assert_eq!(b.buttons[1].label, "new");
        assert_eq!(b.insert_button(99, btn("end")), Ok(4));
        assert_eq!(b.buttons.last().unwrap().label, "end");
        assert_eq!(b.remove_button(1).unwrap().label, "new");
        assert!(b.remove_button(99).is_none());
        // move up / down, refused at the edges
        assert_eq!(b.move_button(1, -1), Some(0));
        assert_eq!(b.buttons[0].label, "b1");
        assert_eq!(b.move_button(0, -1), None);
        let last = b.buttons.len() - 1;
        assert_eq!(b.move_button(last, 1), None);
        assert_eq!(b.move_button(0, 1), Some(1));
    }

    #[test]
    fn insert_is_refused_at_the_button_limit() {
        let mut b = bank_with(MAX_BUTTONS);
        assert!(b.insert_button(0, btn("x")).is_err());
        assert_eq!(b.buttons.len(), MAX_BUTTONS);
    }

    #[test]
    fn cleaned_clamps_columns_and_drops_absurd_positions() {
        let b = ButtonBank { columns: 500, x: Some(-5), y: Some(10), ..bank_with(1) }.cleaned();
        assert_eq!(b.columns, MAX_COLUMNS);
        assert_eq!((b.x, b.y), (None, None));
        let b = ButtonBank { columns: 0, x: Some(40_000), y: Some(0), ..bank_with(1) }.cleaned();
        assert_eq!(b.columns, MIN_COLUMNS);
        assert_eq!((b.x, b.y), (None, None));
        let b = ButtonBank { x: Some(100), y: Some(200), ..bank_with(1) }.cleaned();
        assert_eq!((b.x, b.y), (Some(100), Some(200)));
    }

    #[test]
    fn action_state_table() {
        use BankAction::*;
        use BankView::*;
        for (action, from, to) in [
            (Open, Closed, Shown),
            (Open, Hidden, Shown),
            (Open, Shown, Shown),
            (Close, Closed, Closed),
            (Close, Hidden, Closed),
            (Close, Shown, Closed),
            (Hide, Closed, Closed),
            (Hide, Hidden, Hidden),
            (Hide, Shown, Hidden),
            (Toggle, Closed, Shown),
            (Toggle, Hidden, Shown),
            (Toggle, Shown, Closed),
        ] {
            assert_eq!(action.apply(from), to, "{action:?} on {from:?}");
        }
    }

    #[test]
    fn create_gives_distinct_ids_to_names_with_the_same_slug() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let a = banks.create_in_dir(td.path(), "My Bank").unwrap();
        let b = banks.create_in_dir(td.path(), "my-bank").unwrap();
        assert_eq!(a, "my-bank");
        assert_eq!(b, "my-bank-2");
    }

    #[test]
    fn create_refuses_empty_and_duplicate_names_ignoring_case() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        assert!(banks.create_in_dir(td.path(), "  ").is_err());
        banks.create_in_dir(td.path(), "Tools").unwrap();
        assert!(banks.create_in_dir(td.path(), "tOOLS").unwrap_err().contains("already exists"));
    }

    #[test]
    fn create_never_reuses_an_id_whose_file_exists_on_disk() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("tools.json"), "not json").unwrap();
        let mut banks = Banks::default();
        assert_eq!(banks.create_in_dir(td.path(), "Tools").unwrap(), "tools-2");
    }

    #[test]
    fn replace_keeps_the_runtime_state_and_checks_the_name() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let id = banks.create_in_dir(td.path(), "Tools").unwrap();
        let other = banks.create_in_dir(td.path(), "Other").unwrap();
        banks.set_runtime(&id, true, Some((5, 6)));
        let mut edited = bank_with(2);
        edited.name = "Renamed".to_string();
        banks.replace(&id, edited).unwrap();
        let got = banks.get(&id).unwrap();
        assert_eq!(got.name, "Renamed");
        assert_eq!(got.id, id);
        assert!(got.open);
        assert_eq!((got.x, got.y), (Some(5), Some(6)));
        let mut clash = bank_with(1);
        clash.name = "renamed".to_string();
        assert!(banks.replace(&other, clash).is_err());
        assert!(banks.replace("missing", bank_with(1)).is_err());
    }

    #[test]
    fn save_and_load_round_trip_in_name_order() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let b = banks.create_in_dir(td.path(), "Bravo").unwrap();
        let a = banks.create_in_dir(td.path(), "Alpha").unwrap();
        banks.save_bank_to(td.path(), &b).unwrap();
        banks.save_bank_to(td.path(), &a).unwrap();
        let loaded = Banks::load_from_dir(td.path());
        assert_eq!(loaded.names(), vec!["Alpha".to_string(), "Bravo".to_string()]);
        assert_eq!(loaded.get("alpha").unwrap().name, "Alpha");
        assert_eq!(loaded.get_by_name("BRAVO").unwrap().id, "bravo");
    }

    #[test]
    fn a_file_that_does_not_parse_is_backed_up_and_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("broken.json");
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = Banks::load_from_dir(td.path());
        assert!(loaded.banks.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert_eq!(std::fs::read_to_string(td.path().join("broken.json.bak")).unwrap(), "{ not json");
        // an existing backup is never overwritten
        std::fs::write(&path, "{ other").unwrap();
        Banks::load_from_dir(td.path());
        assert_eq!(std::fs::read_to_string(td.path().join("broken.json.bak")).unwrap(), "{ not json");
    }

    #[test]
    fn an_empty_name_in_a_file_falls_back_to_the_file_name() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("noname.json"), r#"{"name":""}"#).unwrap();
        assert_eq!(Banks::load_from_dir(td.path()).names(), vec!["noname".to_string()]);
    }

    #[test]
    fn delete_and_remove_file() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let id = banks.create_in_dir(td.path(), "Tools").unwrap();
        banks.save_bank_to(td.path(), &id).unwrap();
        assert!(banks.delete(&id));
        assert!(!banks.delete(&id));
        Banks::remove_bank_file_from(td.path(), &id).unwrap();
        assert!(!td.path().join("tools.json").exists());
        Banks::remove_bank_file_from(td.path(), &id).unwrap();
    }

    #[test]
    fn saving_an_unknown_id_is_an_error() {
        let td = tempfile::tempdir().unwrap();
        assert!(Banks::default().save_bank_to(td.path(), "nope").is_err());
    }

    #[test]
    fn position_on_screen_drops_a_position_no_monitor_shows() {
        let one = [(0, 0, 1920, 1080)];
        assert_eq!(position_on_screen((300, 200), &one), Some((300, 200)));
        assert_eq!(position_on_screen((2500, 100), &one), None);
        assert_eq!(position_on_screen((1900, 100), &one), None);
        assert_eq!(position_on_screen((100, 1070), &one), None);
        let two = [(0, 0, 1920, 1080), (1920, 0, 1280, 1024)];
        assert_eq!(position_on_screen((2500, 100), &two), Some((2500, 100)));
        assert_eq!(position_on_screen((100, 100), &[]), Some((100, 100)));
    }
}
