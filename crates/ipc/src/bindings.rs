//! Hotkeys, user menu and script-event bindings (F-03, F-04, F-05).
//!
//! Three JSON files under `~/.config/opusdm/` (`hotkeys.json`,
//! `usermenu.json`, `scripts.json`) hold ordinary lists that reference saved
//! functions by [`FunctionId`]. The hub loads them into one [`Bindings`],
//! sanitizes it, persists it and broadcasts it to every lister. See
//! `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`.

use crate::function::FunctionId;
use crate::funcref::{self, FuncRef};
use crate::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Modifier keys of a key combination. NumLock and CapsLock are never part
/// of a combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Alt (X11 Mod1).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Super (X11 Mod4).
    pub sup: bool,
}

impl Mods {
    /// Whether Ctrl, Alt or Super is held. Shift alone does not count: it
    /// only changes the character a key produces.
    pub fn has_command_modifier(self) -> bool {
        self.ctrl || self.alt || self.sup
    }
}

/// One parsed key combination: modifiers plus the GDK key name (`u`, `F7`,
/// `plus`, ...). GDK key values equal X11 keysyms, so the same name is
/// resolved by the lister and by the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    /// Held modifiers.
    pub mods: Mods,
    /// GDK key name as written.
    pub name: String,
}

impl KeyCombo {
    /// The key name with a single ASCII letter lower-cased and every other
    /// name (`F7`, `plus`, `ISO_Left_Tab`) unchanged.
    pub fn normalized_name(&self) -> String {
        if self.name.len() == 1 && self.name.is_ascii() {
            return self.name.to_ascii_lowercase();
        }
        self.name.clone()
    }

    /// What two combinations are compared by (duplicate detection, lookup).
    pub fn identity(&self) -> (Mods, String) {
        (self.mods, self.normalized_name())
    }
}

/// Parse `Ctrl+Alt+u` style text. Modifiers are case-insensitive and may
/// come in any order; the last `+`-separated part is the key name.
pub fn parse_key(text: &str) -> Result<KeyCombo, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty key".to_string());
    }
    let mut parts: Vec<&str> = text.split('+').collect();
    let name = parts.pop().unwrap_or_default();
    if name.is_empty() {
        return Err(format!("missing key name in '{text}'"));
    }
    let mut mods = Mods::default();
    for part in parts {
        let slot = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut mods.ctrl,
            "alt" => &mut mods.alt,
            "shift" => &mut mods.shift,
            "super" => &mut mods.sup,
            other => return Err(format!("unknown modifier '{other}' in '{text}'")),
        };
        if *slot {
            return Err(format!("repeated modifier '{part}' in '{text}'"));
        }
        *slot = true;
    }
    Ok(KeyCombo { mods, name: name.to_string() })
}

/// The canonical text for `combo`: modifiers in the fixed order Ctrl, Alt,
/// Shift, Super, then the normalized key name.
pub fn format_key(combo: &KeyCombo) -> String {
    let mut out = String::new();
    if combo.mods.ctrl {
        out.push_str("Ctrl+");
    }
    if combo.mods.alt {
        out.push_str("Alt+");
    }
    if combo.mods.shift {
        out.push_str("Shift+");
    }
    if combo.mods.sup {
        out.push_str("Super+");
    }
    out.push_str(&combo.normalized_name());
    out
}

/// One key bound to a function. `key` is the canonical text of a
/// [`KeyCombo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Hotkey {
    /// Canonical key text, e.g. `Ctrl+Alt+u`.
    pub key: String,
    /// The function to run.
    pub function: FunctionId,
    /// Grab the key system-wide (X11 only) instead of only in listers.
    pub global: bool,
}

/// One node of the user menu tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MenuNode {
    /// A leaf that runs a function.
    Entry { label: String, function: FunctionId },
    /// A named group of further nodes.
    Submenu { label: String, children: Vec<MenuNode> },
    /// A visual separator.
    Separator,
}

/// A system event a function can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptEvent {
    /// The hub started.
    Startup,
    /// The hub is quitting.
    Shutdown,
    /// A lister registered with the hub.
    ListerOpened,
    /// A lister went away.
    ListerClosed,
    /// Primary double click on the desktop background.
    DesktopDoubleClick,
    /// Middle double click on the desktop.
    DesktopMiddleDoubleClick,
    /// Secondary (right) double click on the desktop.
    DesktopRightDoubleClick,
    /// The environment was hidden (not emitted until F-38).
    Hidden,
    /// The environment was revealed (not emitted until F-38).
    Revealed,
}

impl ScriptEvent {
    /// Every variant, in editor display order.
    pub const ALL: [ScriptEvent; 9] = [
        ScriptEvent::Startup,
        ScriptEvent::Shutdown,
        ScriptEvent::ListerOpened,
        ScriptEvent::ListerClosed,
        ScriptEvent::DesktopDoubleClick,
        ScriptEvent::DesktopMiddleDoubleClick,
        ScriptEvent::DesktopRightDoubleClick,
        ScriptEvent::Hidden,
        ScriptEvent::Revealed,
    ];

    /// Whether the hub currently emits this event.
    pub fn is_emitted(self) -> bool {
        !matches!(self, ScriptEvent::Hidden | ScriptEvent::Revealed)
    }

    /// The events the hub emits, in display order.
    pub fn emitted() -> Vec<ScriptEvent> {
        Self::ALL.iter().copied().filter(|e| e.is_emitted()).collect()
    }

    /// Human-readable name for the editor.
    pub fn label(self) -> &'static str {
        match self {
            ScriptEvent::Startup => "Startup",
            ScriptEvent::Shutdown => "Shutdown",
            ScriptEvent::ListerOpened => "Lister opened",
            ScriptEvent::ListerClosed => "Lister closed",
            ScriptEvent::DesktopDoubleClick => "Desktop double click",
            ScriptEvent::DesktopMiddleDoubleClick => "Desktop middle double click",
            ScriptEvent::DesktopRightDoubleClick => "Desktop right double click",
            ScriptEvent::Hidden => "Hidden",
            ScriptEvent::Revealed => "Revealed",
        }
    }
}

/// A function bound to an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptBinding {
    /// The event.
    pub event: ScriptEvent,
    /// The function to run when it fires.
    pub function: FunctionId,
}

/// The three binding lists together: wire format, in-memory state, and the
/// content of the three files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Bindings {
    /// `hotkeys.json`.
    pub hotkeys: Vec<Hotkey>,
    /// `usermenu.json`.
    pub menu: Vec<MenuNode>,
    /// `scripts.json`.
    pub scripts: Vec<ScriptBinding>,
}

/// One reason a section of [`Bindings`] cannot be saved as it is. Messages
/// (via `Display`) never contain the row position: the editors show them
/// on the offending row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// The key text does not parse.
    HotkeyKeyInvalid { index: usize, reason: String },
    /// Same normalized key as the earlier hotkey `first`.
    HotkeyDuplicate { index: usize, first: usize },
    /// The function id is not a saved function.
    HotkeyUnknownFunction { index: usize },
    /// A global hotkey without Ctrl, Alt or Super.
    HotkeyGlobalNeedsModifier { index: usize },
    /// A menu label that is empty or blank.
    MenuEmptyLabel { path: Vec<usize> },
    /// A menu entry whose function id is not a saved function.
    MenuUnknownFunction { path: Vec<usize> },
    /// A script binding whose function id is not a saved function.
    ScriptUnknownFunction { index: usize },
    /// A script binding on an event the hub does not emit yet.
    ScriptEventNotEmitted { index: usize },
    /// The same (event, function) pair as the earlier binding `first`.
    ScriptDuplicate { index: usize, first: usize },
    /// An inline `bash:` or `exec:` command in a hotkey that cannot run.
    HotkeyInvalidCommand { index: usize, reason: String },
    /// An inline command in a menu entry that cannot run.
    MenuInvalidCommand { path: Vec<usize>, reason: String },
    /// An inline command in a script binding that cannot run.
    ScriptInvalidCommand { index: usize, reason: String },
}

impl BindingError {
    /// The hotkey row this error belongs to, if it is a hotkey error.
    pub fn hotkey_index(&self) -> Option<usize> {
        match self {
            BindingError::HotkeyKeyInvalid { index, .. }
            | BindingError::HotkeyDuplicate { index, .. }
            | BindingError::HotkeyUnknownFunction { index }
            | BindingError::HotkeyGlobalNeedsModifier { index }
            | BindingError::HotkeyInvalidCommand { index, .. } => Some(*index),
            _ => None,
        }
    }

    /// The script row this error belongs to, if it is a script error.
    pub fn script_index(&self) -> Option<usize> {
        match self {
            BindingError::ScriptUnknownFunction { index }
            | BindingError::ScriptEventNotEmitted { index }
            | BindingError::ScriptDuplicate { index, .. }
            | BindingError::ScriptInvalidCommand { index, .. } => Some(*index),
            _ => None,
        }
    }

    /// The menu node path this error belongs to, if it is a menu error.
    pub fn menu_path(&self) -> Option<&[usize]> {
        match self {
            BindingError::MenuEmptyLabel { path }
            | BindingError::MenuUnknownFunction { path }
            | BindingError::MenuInvalidCommand { path, .. } => Some(path),
            _ => None,
        }
    }
}

impl fmt::Display for BindingError {
    /// The message shown on the offending editor row.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingError::HotkeyKeyInvalid { reason, .. } => write!(f, "invalid key: {reason}"),
            BindingError::HotkeyDuplicate { first, .. } => {
                write!(f, "this key is already used by hotkey {}", first + 1)
            }
            BindingError::HotkeyUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::HotkeyGlobalNeedsModifier { .. } => {
                write!(f, "a global hotkey needs Ctrl, Alt or Super")
            }
            BindingError::MenuEmptyLabel { .. } => write!(f, "the label is empty"),
            BindingError::MenuUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::ScriptUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::ScriptEventNotEmitted { .. } => write!(f, "this event is not emitted yet"),
            BindingError::ScriptDuplicate { first, .. } => {
                write!(f, "already bound in row {}", first + 1)
            }
            BindingError::HotkeyInvalidCommand { reason, .. }
            | BindingError::MenuInvalidCommand { reason, .. }
            | BindingError::ScriptInvalidCommand { reason, .. } => write!(f, "invalid command: {reason}"),
        }
    }
}

/// Why a function reference cannot be used: it names a saved function that
/// does not exist, or an inline command that cannot run.
enum RefIssue {
    /// A saved id that is not in the function list.
    Missing,
    /// An inline command with this problem.
    Invalid(String),
}

/// The issue with `reference`, if any. A saved id is checked against
/// `known`; an inline command by [`funcref::problem`].
fn check_reference(reference: &str, known: &HashSet<FunctionId>) -> Option<RefIssue> {
    match funcref::parse(reference) {
        FuncRef::Saved(id) => (!known.contains(&id)).then_some(RefIssue::Missing),
        inline => funcref::problem(&inline).map(RefIssue::Invalid),
    }
}

/// Every problem in a hotkey list, in row order.
pub fn validate_hotkeys(hotkeys: &[Hotkey], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    let mut seen: HashMap<(Mods, String), usize> = HashMap::new();
    for (index, hotkey) in hotkeys.iter().enumerate() {
        match parse_key(&hotkey.key) {
            Err(reason) => errors.push(BindingError::HotkeyKeyInvalid { index, reason }),
            Ok(combo) => {
                match seen.get(&combo.identity()) {
                    Some(&first) => errors.push(BindingError::HotkeyDuplicate { index, first }),
                    None => {
                        seen.insert(combo.identity(), index);
                    }
                }
                if hotkey.global && !combo.mods.has_command_modifier() {
                    errors.push(BindingError::HotkeyGlobalNeedsModifier { index });
                }
            }
        }
        match check_reference(&hotkey.function, known) {
            Some(RefIssue::Missing) => errors.push(BindingError::HotkeyUnknownFunction { index }),
            Some(RefIssue::Invalid(reason)) => errors.push(BindingError::HotkeyInvalidCommand { index, reason }),
            None => {}
        }
    }
    errors
}

/// Every problem in a menu tree, depth first.
pub fn validate_menu(menu: &[MenuNode], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    walk_menu(menu, &mut Vec::new(), known, &mut errors);
    errors
}

/// Recursive helper of [`validate_menu`]; `path` is the position of `nodes`.
fn walk_menu(nodes: &[MenuNode], path: &mut Vec<usize>, known: &HashSet<FunctionId>, errors: &mut Vec<BindingError>) {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        match node {
            MenuNode::Entry { label, function } => {
                if label.trim().is_empty() {
                    errors.push(BindingError::MenuEmptyLabel { path: path.clone() });
                }
                match check_reference(function, known) {
                    Some(RefIssue::Missing) => errors.push(BindingError::MenuUnknownFunction { path: path.clone() }),
                    Some(RefIssue::Invalid(reason)) => errors.push(BindingError::MenuInvalidCommand { path: path.clone(), reason }),
                    None => {}
                }
            }
            MenuNode::Submenu { label, children } => {
                if label.trim().is_empty() {
                    errors.push(BindingError::MenuEmptyLabel { path: path.clone() });
                }
                walk_menu(children, path, known, errors);
            }
            MenuNode::Separator => {}
        }
        path.pop();
    }
}

/// Every problem in a script-binding list, in row order.
pub fn validate_scripts(scripts: &[ScriptBinding], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    let mut seen: HashMap<(ScriptEvent, &str), usize> = HashMap::new();
    for (index, binding) in scripts.iter().enumerate() {
        match seen.get(&(binding.event, binding.function.as_str())) {
            Some(&first) => errors.push(BindingError::ScriptDuplicate { index, first }),
            None => {
                seen.insert((binding.event, binding.function.as_str()), index);
            }
        }
        if !binding.event.is_emitted() {
            errors.push(BindingError::ScriptEventNotEmitted { index });
        }
        match check_reference(&binding.function, known) {
            Some(RefIssue::Missing) => errors.push(BindingError::ScriptUnknownFunction { index }),
            Some(RefIssue::Invalid(reason)) => errors.push(BindingError::ScriptInvalidCommand { index, reason }),
            None => {}
        }
    }
    errors
}

/// The child list at `parent` (an empty path is the root list), or `None`
/// when the path does not lead to a submenu.
pub fn children_mut<'a>(tree: &'a mut Vec<MenuNode>, parent: &[usize]) -> Option<&'a mut Vec<MenuNode>> {
    let Some((&first, rest)) = parent.split_first() else { return Some(tree) };
    match tree.get_mut(first)? {
        MenuNode::Submenu { children, .. } => children_mut(children, rest),
        _ => None,
    }
}

/// Append `node` to the list at `parent`. `false` when `parent` is not a
/// submenu path.
pub fn add_node(tree: &mut Vec<MenuNode>, parent: &[usize], node: MenuNode) -> bool {
    let Some(list) = children_mut(tree, parent) else { return false };
    list.push(node);
    true
}

/// Remove the node at `path`. `false` when there is no such node.
pub fn remove_node(tree: &mut Vec<MenuNode>, path: &[usize]) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    if index >= list.len() {
        return false;
    }
    list.remove(index);
    true
}

/// Swap the node at `path` with its neighbour `delta` places away (`-1` up,
/// `1` down). `false` when it is already at that edge or does not exist.
pub fn move_node(tree: &mut Vec<MenuNode>, path: &[usize], delta: isize) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    let Some(target) = index.checked_add_signed(delta) else { return false };
    if index >= list.len() || target >= list.len() {
        return false;
    }
    list.swap(index, target);
    true
}

/// Set the label of the entry or submenu at `path`. `false` for a separator
/// or a path that does not exist.
pub fn rename_node(tree: &mut Vec<MenuNode>, path: &[usize], new_label: &str) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    match list.get_mut(index) {
        Some(MenuNode::Entry { label, .. }) | Some(MenuNode::Submenu { label, .. }) => {
            *label = new_label.to_string();
            true
        }
        _ => false,
    }
}

/// The hub's tolerant clean-up of a [`Bindings`] it received or loaded: drop
/// what can never work, keep the first of any duplicate, canonicalize hotkey
/// text, and log each removal. Dangling function ids are kept (they are
/// resolved at run time and flagged by the editors).
pub fn sanitize(bindings: Bindings) -> Bindings {
    let mut hotkeys = Vec::new();
    let mut seen: HashSet<(Mods, String)> = HashSet::new();
    for hotkey in bindings.hotkeys {
        let combo = match parse_key(&hotkey.key) {
            Ok(combo) => combo,
            Err(reason) => {
                crate::err_log!("opusdm", "dropping hotkey '{}': {reason}", hotkey.key);
                continue;
            }
        };
        if hotkey.global && !combo.mods.has_command_modifier() {
            crate::err_log!("opusdm", "dropping global hotkey '{}': it needs Ctrl, Alt or Super", hotkey.key);
            continue;
        }
        if !seen.insert(combo.identity()) {
            crate::err_log!("opusdm", "dropping hotkey '{}': the key is already bound", hotkey.key);
            continue;
        }
        hotkeys.push(Hotkey { key: format_key(&combo), ..hotkey });
    }
    let mut scripts: Vec<ScriptBinding> = Vec::new();
    for binding in bindings.scripts {
        if scripts.contains(&binding) {
            crate::err_log!("opusdm", "dropping repeated script binding for {:?}", binding.event);
            continue;
        }
        scripts.push(binding);
    }
    Bindings { hotkeys, menu: bindings.menu, scripts }
}

/// `menu` without entries whose function is neither a saved function in
/// `known` nor a runnable inline command, and without the submenus that end
/// up empty. Separators are kept.
pub fn prune(menu: &[MenuNode], known: &HashSet<FunctionId>) -> Vec<MenuNode> {
    menu.iter()
        .filter_map(|node| match node {
            MenuNode::Entry { function, .. } => check_reference(function, known).is_none().then(|| node.clone()),
            MenuNode::Submenu { label, children } => {
                let kept = prune(children, known);
                (!kept.is_empty()).then(|| MenuNode::Submenu { label: label.clone(), children: kept })
            }
            MenuNode::Separator => Some(MenuNode::Separator),
        })
        .collect()
}

/// Path to `hotkeys.json` under [`config_dir`].
pub fn hotkeys_path() -> PathBuf {
    config_dir().join("hotkeys.json")
}

/// Path to `usermenu.json` under [`config_dir`].
pub fn usermenu_path() -> PathBuf {
    config_dir().join("usermenu.json")
}

/// Path to `scripts.json` under [`config_dir`].
pub fn scripts_path() -> PathBuf {
    config_dir().join("scripts.json")
}

/// Load the three files from the standard config directory. A missing or
/// malformed file yields an empty section.
pub fn load_bindings() -> Bindings {
    load_bindings_from(&config_dir())
}

/// Load one binding file. When the file exists but does not parse (a typo in
/// an event name, say), its content is copied to `<name>.bak` first: the
/// section loads as empty, and the next save rewrites the file from memory,
/// which would otherwise destroy what the user wrote.
fn load_section<T: serde::de::DeserializeOwned + Default>(dir: &Path, file: &str, entity: &str) -> T {
    let path = dir.join(file);
    load_json(&path, "opusdm", entity)
}

/// Testable core of [`load_bindings`].
pub fn load_bindings_from(dir: &Path) -> Bindings {
    Bindings {
        hotkeys: load_section(dir, "hotkeys.json", "hotkeys"),
        menu: load_section(dir, "usermenu.json", "user menu"),
        scripts: load_section(dir, "scripts.json", "script events"),
    }
}

/// Id of the saved function that switches to the previous virtual desktop.
pub const DESKTOP_PREV_ID: &str = "builtin-desktop-prev";

/// Id of the saved function that switches to the next virtual desktop.
pub const DESKTOP_NEXT_ID: &str = "builtin-desktop-next";

/// The two saved functions the default desktop hotkeys run.
pub fn default_desktop_functions() -> Vec<crate::function::Function> {
    use crate::function::{FnStep, Function, InternalArgs, InternalCmd};
    [(DESKTOP_PREV_ID, InternalCmd::DesktopPrev), (DESKTOP_NEXT_ID, InternalCmd::DesktopNext)]
        .into_iter()
        .map(|(id, cmd)| Function {
            id: id.to_string(),
            name: cmd.label().to_string(),
            steps: vec![FnStep::Internal { cmd, args: InternalArgs::default() }],
        })
        .collect()
}

/// The default global hotkeys: `Alt+1` for the previous desktop, `Alt+2` for
/// the next one, `Ctrl+\` for `opusdm-terminal`, `Ctrl+Print` for a screenshot and
/// `Ctrl+Shift+Space` for the run dialog.
pub fn default_hotkeys() -> Vec<Hotkey> {
    vec![
        Hotkey { key: "Alt+1".into(), function: DESKTOP_PREV_ID.into(), global: true },
        Hotkey { key: "Alt+2".into(), function: DESKTOP_NEXT_ID.into(), global: true },
        Hotkey { key: "Ctrl+backslash".into(), function: "exec:opusdm-terminal".into(), global: true },
        Hotkey { key: "Ctrl+Print".into(), function: "exec:opusdm-shot".into(), global: true },
        Hotkey { key: "Ctrl+Shift+space".into(), function: "exec:opusdm-run".into(), global: true },
    ]
}

/// First run: write [`default_hotkeys`] to `hotkeys.json` and add the saved
/// functions they run. Does nothing when `hotkeys.json` exists (even empty or
/// malformed), so what the user edited or deleted stays that way. Returns
/// whether the defaults were written.
pub fn seed_default_hotkeys() -> bool {
    seed_default_hotkeys_in(&config_dir(), &crate::function::functions_path())
}

/// Testable core of [`seed_default_hotkeys`]. A failure is logged, never fatal.
pub fn seed_default_hotkeys_in(dir: &Path, functions_file: &Path) -> bool {
    let hotkeys_file = dir.join("hotkeys.json");
    if hotkeys_file.exists() {
        return false;
    }
    let mut functions: Vec<crate::function::Function> = load_json(functions_file, "opusdm", "functions");
    for default in default_desktop_functions() {
        if !functions.iter().any(|f| f.id == default.id) {
            functions.push(default);
        }
    }
    let written = save_json(functions_file, &functions).and_then(|()| save_json(&hotkeys_file, &default_hotkeys()));
    if let Err(e) = &written {
        crate::err_log!("opusdm", "cannot write the default hotkeys to {}: {e}", hotkeys_file.display());
    }
    written.is_ok()
}

/// Save the three files under the standard config directory.
pub fn save_bindings(bindings: &Bindings) -> io::Result<()> {
    save_bindings_to(&config_dir(), bindings)
}

/// Testable core of [`save_bindings`].
pub fn save_bindings_to(dir: &Path, bindings: &Bindings) -> io::Result<()> {
    save_json(&dir.join("hotkeys.json"), &bindings.hotkeys)?;
    save_json(&dir.join("usermenu.json"), &bindings.menu)?;
    save_json(&dir.join("scripts.json"), &bindings.scripts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(ids: &[&str]) -> HashSet<FunctionId> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn hk(key: &str, function: &str, global: bool) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global }
    }

    #[test]
    fn seed_default_hotkeys_writes_valid_defaults_once_and_keeps_existing_functions() {
        let dir = tempfile::tempdir().unwrap();
        let functions_file = dir.path().join("functions.json");
        let mine = crate::function::Function::new("mine");
        save_json(&functions_file, std::slice::from_ref(&mine)).unwrap();
        assert!(seed_default_hotkeys_in(dir.path(), &functions_file));
        let hotkeys: Vec<Hotkey> = load_json(&dir.path().join("hotkeys.json"), "opusdm", "hotkeys");
        assert_eq!(hotkeys, default_hotkeys());
        let functions: Vec<crate::function::Function> = load_json(&functions_file, "opusdm", "functions");
        assert_eq!(functions.len(), 3);
        assert!(functions.contains(&mine));
        let ids: HashSet<FunctionId> = functions.iter().map(|f| f.id.clone()).collect();
        // The exec: programs may not be installed where the tests run.
        let errors = validate_hotkeys(&hotkeys, &ids);
        assert!(errors.iter().all(|e| matches!(e, BindingError::HotkeyInvalidCommand { .. })), "{errors:?}");
        assert!(!seed_default_hotkeys_in(dir.path(), &functions_file));
    }

    #[test]
    fn seed_default_hotkeys_never_touches_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let functions_file = dir.path().join("functions.json");
        std::fs::write(dir.path().join("hotkeys.json"), "[]").unwrap();
        assert!(!seed_default_hotkeys_in(dir.path(), &functions_file));
        assert!(!functions_file.exists());
    }

    #[test]
    fn validate_hotkeys_accepts_a_good_inline_command_and_reports_a_bad_one() {
        let list = [hk("Ctrl+u", "bash:ls", false), hk("Ctrl+j", "exec:sh -c true", true), hk("Ctrl+k", "bash:  ", false), hk("Ctrl+l", "exec:opusdm-nope-xyz", false)];
        let errors = validate_hotkeys(&list, &known(&[]));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(matches!(&errors[0], BindingError::HotkeyInvalidCommand { index: 2, .. }));
        assert!(matches!(&errors[1], BindingError::HotkeyInvalidCommand { index: 3, reason } if reason.contains("was not found")));
        assert_eq!(errors[0].hotkey_index(), Some(2));
    }

    #[test]
    fn validate_menu_and_scripts_accept_inline_commands() {
        let menu = vec![
            MenuNode::Entry { label: "ok".into(), function: "bash:ls".into() },
            MenuNode::Entry { label: "bad".into(), function: "exec:".into() },
        ];
        let errors = validate_menu(&menu, &known(&[]));
        assert!(matches!(errors.as_slice(), [BindingError::MenuInvalidCommand { path, .. }] if path == &vec![1]));
        let scripts = [
            ScriptBinding { event: ScriptEvent::Startup, function: "bash:ls".into() },
            ScriptBinding { event: ScriptEvent::Shutdown, function: "bash:".into() },
        ];
        let errors = validate_scripts(&scripts, &known(&[]));
        assert!(matches!(errors.as_slice(), [BindingError::ScriptInvalidCommand { index: 1, .. }]));
        assert_eq!(errors[0].script_index(), Some(1));
    }

    #[test]
    fn a_saved_id_that_is_missing_is_still_the_old_error() {
        let errors = validate_hotkeys(&[hk("Ctrl+u", "gone", false)], &known(&["a"]));
        assert_eq!(errors, vec![BindingError::HotkeyUnknownFunction { index: 0 }]);
    }

    #[test]
    fn prune_keeps_valid_inline_entries_and_drops_broken_ones() {
        let menu = vec![
            MenuNode::Entry { label: "keep".into(), function: "bash:ls".into() },
            MenuNode::Entry { label: "drop".into(), function: "exec:".into() },
        ];
        let pruned = prune(&menu, &known(&[]));
        assert_eq!(pruned.len(), 1);
        assert!(matches!(&pruned[0], MenuNode::Entry { label, .. } if label == "keep"));
    }

    #[test]
    fn sanitize_keeps_inline_hotkeys_even_when_the_program_is_missing() {
        let dirty = Bindings { hotkeys: vec![hk("ctrl+U", "exec:opusdm-nope-xyz", true)], ..Bindings::default() };
        let clean = sanitize(dirty);
        assert_eq!(clean.hotkeys, vec![hk("Ctrl+u", "exec:opusdm-nope-xyz", true)]);
    }

    #[test]
    fn inline_references_round_trip_through_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = Bindings {
            hotkeys: vec![hk("Ctrl+Alt+t", "exec:kitty --class \"my term\"", true)],
            menu: vec![MenuNode::Entry { label: "L".into(), function: "bash+out:echo 'a b' && ls".into() }],
            scripts: vec![ScriptBinding { event: ScriptEvent::Startup, function: "bash:notify-send hi".into() }],
        };
        save_bindings_to(dir.path(), &bindings).unwrap();
        assert_eq!(load_bindings_from(dir.path()), bindings);
    }

    #[test]
    fn parse_key_reads_modifiers_and_name() {
        let combo = parse_key("Ctrl+Alt+u").unwrap();
        assert_eq!(combo.mods, Mods { ctrl: true, alt: true, shift: false, sup: false });
        assert_eq!(combo.name, "u");
    }

    #[test]
    fn parse_key_accepts_a_bare_key_and_lowercase_modifiers() {
        let bare = parse_key("F7").unwrap();
        assert_eq!(bare.mods, Mods::default());
        assert_eq!(bare.name, "F7");
        let shifted = parse_key("ctrl+SHIFT+F7").unwrap();
        assert!(shifted.mods.ctrl && shifted.mods.shift && !shifted.mods.alt);
    }

    #[test]
    fn parse_key_rejects_bad_input() {
        for bad in ["", "   ", "Ctrl+", "Ctrl+Ctrl+u", "Hyper+u", "+"] {
            assert!(parse_key(bad).is_err(), "'{bad}' must not parse");
        }
    }

    #[test]
    fn format_key_uses_the_fixed_modifier_order() {
        let combo = parse_key("Super+Shift+Alt+Ctrl+u").unwrap();
        assert_eq!(format_key(&combo), "Ctrl+Alt+Shift+Super+u");
    }

    #[test]
    fn identity_ignores_the_case_of_a_single_letter_only() {
        let a = parse_key("Ctrl+U").unwrap();
        let b = parse_key("ctrl+u").unwrap();
        assert_eq!(a.identity(), b.identity());
        assert_eq!(format_key(&a), "Ctrl+u");
        let f7 = parse_key("F7").unwrap();
        let lower = parse_key("f7").unwrap();
        assert_ne!(f7.identity(), lower.identity());
    }

    #[test]
    fn command_modifier_excludes_shift_alone() {
        assert!(!parse_key("Shift+u").unwrap().mods.has_command_modifier());
        assert!(parse_key("Alt+u").unwrap().mods.has_command_modifier());
        assert!(parse_key("Super+u").unwrap().mods.has_command_modifier());
    }

    #[test]
    fn validate_hotkeys_flags_a_duplicate_whatever_the_global_flag() {
        let list = [hk("Ctrl+u", "a", false), hk("ctrl+U", "b", true)];
        let errors = validate_hotkeys(&list, &known(&["a", "b"]));
        assert_eq!(errors, vec![BindingError::HotkeyDuplicate { index: 1, first: 0 }]);
    }

    #[test]
    fn validate_hotkeys_flags_invalid_key_unknown_function_and_bare_global() {
        let list = [hk("Ctrl+", "a", false), hk("Ctrl+j", "gone", false), hk("j", "a", true)];
        let errors = validate_hotkeys(&list, &known(&["a"]));
        assert!(errors.contains(&BindingError::HotkeyKeyInvalid { index: 0, reason: "missing key name in 'Ctrl+'".into() }));
        assert!(errors.contains(&BindingError::HotkeyUnknownFunction { index: 1 }));
        assert!(errors.contains(&BindingError::HotkeyGlobalNeedsModifier { index: 2 }));
    }

    #[test]
    fn validate_hotkeys_accepts_a_clean_list() {
        let list = [hk("Ctrl+u", "a", false), hk("Ctrl+Alt+u", "a", true)];
        assert!(validate_hotkeys(&list, &known(&["a"])).is_empty());
    }

    #[test]
    fn validate_menu_reports_paths_for_nested_problems() {
        let menu = vec![
            MenuNode::Entry { label: "ok".into(), function: "a".into() },
            MenuNode::Submenu {
                label: "sub".into(),
                children: vec![
                    MenuNode::Entry { label: " ".into(), function: "a".into() },
                    MenuNode::Entry { label: "x".into(), function: "gone".into() },
                ],
            },
        ];
        let errors = validate_menu(&menu, &known(&["a"]));
        assert_eq!(
            errors,
            vec![
                BindingError::MenuEmptyLabel { path: vec![1, 0] },
                BindingError::MenuUnknownFunction { path: vec![1, 1] },
            ]
        );
    }

    #[test]
    fn validate_scripts_flags_unknown_function_unemitted_event_and_duplicates() {
        let scripts = [
            ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Hidden, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Shutdown, function: "gone".into() },
        ];
        let errors = validate_scripts(&scripts, &known(&["a"]));
        assert!(errors.contains(&BindingError::ScriptDuplicate { index: 1, first: 0 }));
        assert!(errors.contains(&BindingError::ScriptEventNotEmitted { index: 2 }));
        assert!(errors.contains(&BindingError::ScriptUnknownFunction { index: 3 }));
    }

    #[test]
    fn sanitize_keeps_the_first_of_a_duplicate_and_canonicalizes_keys() {
        let dirty = Bindings {
            hotkeys: vec![
                hk("ctrl+U", "a", false),
                hk("Ctrl+u", "b", true),
                hk("nonsense+", "c", false),
                hk("u", "d", true),
            ],
            ..Bindings::default()
        };
        let clean = sanitize(dirty);
        assert_eq!(clean.hotkeys, vec![hk("Ctrl+u", "a", false)]);
    }

    #[test]
    fn sanitize_drops_repeated_script_pairs_but_keeps_dangling_ids() {
        let dirty = Bindings {
            scripts: vec![
                ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
                ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
                ScriptBinding { event: ScriptEvent::Startup, function: "gone".into() },
            ],
            ..Bindings::default()
        };
        let clean = sanitize(dirty);
        assert_eq!(clean.scripts.len(), 2);
    }

    #[test]
    fn prune_drops_missing_functions_and_empty_submenus() {
        let menu = vec![
            MenuNode::Entry { label: "keep".into(), function: "a".into() },
            MenuNode::Entry { label: "drop".into(), function: "gone".into() },
            MenuNode::Separator,
            MenuNode::Submenu {
                label: "empty after prune".into(),
                children: vec![MenuNode::Entry { label: "x".into(), function: "gone".into() }],
            },
            MenuNode::Submenu {
                label: "kept".into(),
                children: vec![MenuNode::Entry { label: "y".into(), function: "a".into() }],
            },
        ];
        let pruned = prune(&menu, &known(&["a"]));
        assert_eq!(pruned.len(), 3);
        assert!(matches!(&pruned[0], MenuNode::Entry { label, .. } if label == "keep"));
        assert!(matches!(pruned[1], MenuNode::Separator));
        assert!(matches!(&pruned[2], MenuNode::Submenu { label, .. } if label == "kept"));
    }

    #[test]
    fn script_event_and_menu_node_use_the_documented_json_shape() {
        assert_eq!(serde_json::to_string(&ScriptEvent::ListerOpened).unwrap(), "\"lister_opened\"");
        assert_eq!(serde_json::to_string(&MenuNode::Separator).unwrap(), "{\"type\":\"separator\"}");
        let entry: MenuNode = serde_json::from_str("{\"type\":\"entry\",\"label\":\"L\",\"function\":\"f\"}").unwrap();
        assert_eq!(entry, MenuNode::Entry { label: "L".into(), function: "f".into() });
    }

    #[test]
    fn only_the_seven_wired_events_are_emitted() {
        let emitted = ScriptEvent::emitted();
        assert_eq!(emitted.len(), 7);
        assert!(!emitted.contains(&ScriptEvent::Hidden));
        assert!(!emitted.contains(&ScriptEvent::Revealed));
    }

    #[test]
    fn bindings_roundtrip_through_the_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = Bindings {
            hotkeys: vec![hk("Ctrl+u", "a", false)],
            menu: vec![MenuNode::Entry { label: "L".into(), function: "a".into() }, MenuNode::Separator],
            scripts: vec![ScriptBinding { event: ScriptEvent::Startup, function: "a".into() }],
        };
        save_bindings_to(dir.path(), &bindings).unwrap();
        assert!(dir.path().join("hotkeys.json").is_file());
        assert!(dir.path().join("usermenu.json").is_file());
        assert!(dir.path().join("scripts.json").is_file());
        assert_eq!(load_bindings_from(dir.path()), bindings);
    }

    #[test]
    fn a_missing_or_malformed_file_yields_an_empty_section() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_bindings_from(dir.path()), Bindings::default());
        std::fs::write(dir.path().join("hotkeys.json"), b"{ not json").unwrap();
        std::fs::write(
            dir.path().join("scripts.json"),
            b"[{\"event\":\"startup\",\"function\":\"a\"}]",
        )
        .unwrap();
        let loaded = load_bindings_from(dir.path());
        assert!(loaded.hotkeys.is_empty());
        assert_eq!(loaded.scripts.len(), 1);
    }

    fn entry(label: &str) -> MenuNode {
        MenuNode::Entry { label: label.into(), function: "a".into() }
    }

    fn sub(label: &str, children: Vec<MenuNode>) -> MenuNode {
        MenuNode::Submenu { label: label.into(), children }
    }

    #[test]
    fn add_node_appends_at_the_root_or_inside_a_submenu() {
        let mut tree = vec![entry("a"), sub("s", vec![])];
        assert!(add_node(&mut tree, &[], entry("b")));
        assert!(add_node(&mut tree, &[1], entry("inner")));
        assert_eq!(tree.len(), 3);
        assert!(matches!(&tree[1], MenuNode::Submenu { children, .. } if children.len() == 1));
    }

    #[test]
    fn add_node_refuses_a_path_that_is_not_a_submenu() {
        let mut tree = vec![entry("a")];
        assert!(!add_node(&mut tree, &[0], entry("b")), "an entry has no children");
        assert!(!add_node(&mut tree, &[5], entry("b")), "no such node");
    }

    #[test]
    fn remove_node_deletes_by_path() {
        let mut tree = vec![entry("a"), sub("s", vec![entry("x"), entry("y")])];
        assert!(remove_node(&mut tree, &[1, 0]));
        assert!(matches!(&tree[1], MenuNode::Submenu { children, .. } if children.len() == 1));
        assert!(remove_node(&mut tree, &[0]));
        assert_eq!(tree.len(), 1);
        assert!(!remove_node(&mut tree, &[9]));
        assert!(!remove_node(&mut tree, &[]));
    }

    #[test]
    fn move_node_swaps_with_a_neighbour_and_stops_at_the_edges() {
        let mut tree = vec![entry("a"), entry("b"), entry("c")];
        assert!(move_node(&mut tree, &[1], -1));
        assert!(matches!(&tree[0], MenuNode::Entry { label, .. } if label == "b"));
        assert!(!move_node(&mut tree, &[0], -1), "already first");
        assert!(move_node(&mut tree, &[1], 1));
        assert!(matches!(&tree[2], MenuNode::Entry { label, .. } if label == "a"));
        assert!(!move_node(&mut tree, &[2], 1), "already last");
    }

    #[test]
    fn move_node_works_inside_a_submenu() {
        let mut tree = vec![sub("s", vec![entry("x"), entry("y")])];
        assert!(move_node(&mut tree, &[0, 1], -1));
        assert!(matches!(&tree[0], MenuNode::Submenu { children, .. }
            if matches!(&children[0], MenuNode::Entry { label, .. } if label == "y")));
    }

    #[test]
    fn rename_node_changes_entries_and_submenus_but_not_separators() {
        let mut tree = vec![entry("a"), sub("s", vec![]), MenuNode::Separator];
        assert!(rename_node(&mut tree, &[0], "A"));
        assert!(rename_node(&mut tree, &[1], "S"));
        assert!(!rename_node(&mut tree, &[2], "x"));
        assert!(matches!(&tree[0], MenuNode::Entry { label, .. } if label == "A"));
        assert!(matches!(&tree[1], MenuNode::Submenu { label, .. } if label == "S"));
    }

    #[test]
    fn an_unparsable_file_is_backed_up_before_it_can_be_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        // Valid JSON, but an event name the enum does not know: the whole
        // list fails to parse and loads as empty.
        let broken = "[{\"event\":\"startup2\",\"function\":\"a\"}]";
        std::fs::write(dir.path().join("scripts.json"), broken).unwrap();
        let loaded = load_bindings_from(dir.path());
        assert!(loaded.scripts.is_empty());
        let backup = std::fs::read_to_string(dir.path().join("scripts.json.bak")).unwrap();
        assert_eq!(backup, broken, "the broken content must survive in a .bak file");
    }

    #[test]
    fn a_good_or_missing_file_leaves_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hotkeys.json"), "[]").unwrap();
        load_bindings_from(dir.path());
        assert!(!dir.path().join("hotkeys.json.bak").exists());
        assert!(!dir.path().join("usermenu.json.bak").exists());
    }
}
