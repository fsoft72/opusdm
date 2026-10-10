//! The commands of the terminal that can be bound to a hotkey or picked from
//! the context menu. Pure data: running them lives in `window.rs`.

/// One terminal command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Split the pane in two, one above the other.
    SplitHorizontal,
    /// Split the pane in two, side by side.
    SplitVertical,
    ClosePane,
    NewTab,
    NextTab,
    PrevTab,
    NewWindow,
    FocusNext,
    FocusPrev,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    Copy,
    Paste,
    SelectAll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ResetClear,
    Export,
    Preferences,
}

impl Action {
    /// Every action, in the order the Preferences dialog lists them.
    pub const ALL: [Action; 22] = [
        Action::SplitHorizontal,
        Action::SplitVertical,
        Action::ClosePane,
        Action::NewTab,
        Action::NextTab,
        Action::PrevTab,
        Action::NewWindow,
        Action::FocusNext,
        Action::FocusPrev,
        Action::FocusUp,
        Action::FocusDown,
        Action::FocusLeft,
        Action::FocusRight,
        Action::Copy,
        Action::Paste,
        Action::SelectAll,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::ZoomReset,
        Action::ResetClear,
        Action::Export,
        Action::Preferences,
    ];

    /// Stable id, used as the key in `terminal.json` and as the GAction name.
    pub fn id(self) -> &'static str {
        match self {
            Action::SplitHorizontal => "split-horizontal",
            Action::SplitVertical => "split-vertical",
            Action::ClosePane => "close-pane",
            Action::NewTab => "new-tab",
            Action::NextTab => "next-tab",
            Action::PrevTab => "prev-tab",
            Action::NewWindow => "new-window",
            Action::FocusNext => "focus-next",
            Action::FocusPrev => "focus-prev",
            Action::FocusUp => "focus-up",
            Action::FocusDown => "focus-down",
            Action::FocusLeft => "focus-left",
            Action::FocusRight => "focus-right",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::SelectAll => "select-all",
            Action::ZoomIn => "zoom-in",
            Action::ZoomOut => "zoom-out",
            Action::ZoomReset => "zoom-reset",
            Action::ResetClear => "reset-clear",
            Action::Export => "export",
            Action::Preferences => "preferences",
        }
    }

    /// Label shown in the context menu and in the Preferences dialog.
    pub fn label(self) -> &'static str {
        match self {
            Action::SplitHorizontal => "Split Horizontally",
            Action::SplitVertical => "Split Vertically",
            Action::ClosePane => "Close Terminal",
            Action::NewTab => "New Tab",
            Action::NextTab => "Next Tab",
            Action::PrevTab => "Previous Tab",
            Action::NewWindow => "New Window",
            Action::FocusNext => "Focus Next Terminal",
            Action::FocusPrev => "Focus Previous Terminal",
            Action::FocusUp => "Focus Terminal Above",
            Action::FocusDown => "Focus Terminal Below",
            Action::FocusLeft => "Focus Terminal Left",
            Action::FocusRight => "Focus Terminal Right",
            Action::Copy => "Copy",
            Action::Paste => "Paste",
            Action::SelectAll => "Select All",
            Action::ZoomIn => "Zoom In",
            Action::ZoomOut => "Zoom Out",
            Action::ZoomReset => "Normal Size",
            Action::ResetClear => "Reset and Clear",
            Action::Export => "Export Contents...",
            Action::Preferences => "Preferences...",
        }
    }

    /// Default accelerator in GTK syntax, modelled on Terminator.
    pub fn default_hotkey(self) -> &'static str {
        match self {
            Action::SplitHorizontal => "<Ctrl><Shift>o",
            Action::SplitVertical => "<Ctrl><Shift>e",
            Action::ClosePane => "<Ctrl><Shift>w",
            Action::NewTab => "<Ctrl><Shift>t",
            Action::NextTab => "<Ctrl>Page_Down",
            Action::PrevTab => "<Ctrl>Page_Up",
            Action::NewWindow => "<Ctrl><Shift>i",
            Action::FocusNext => "<Ctrl><Shift>n",
            Action::FocusPrev => "<Ctrl><Shift>p",
            Action::FocusUp => "<Alt>Up",
            Action::FocusDown => "<Alt>Down",
            Action::FocusLeft => "<Alt>Left",
            Action::FocusRight => "<Alt>Right",
            Action::Copy => "<Ctrl><Shift>c",
            Action::Paste => "<Ctrl><Shift>v",
            Action::SelectAll => "<Ctrl><Shift>a",
            Action::ZoomIn => "<Ctrl>plus",
            Action::ZoomOut => "<Ctrl>minus",
            Action::ZoomReset => "<Ctrl>0",
            Action::ResetClear => "<Ctrl><Shift>g",
            Action::Export => "<Ctrl><Shift>s",
            Action::Preferences => "<Ctrl>comma",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_default_hotkeys_are_unique() {
        let ids: HashSet<_> = Action::ALL.iter().map(|a| a.id()).collect();
        let keys: HashSet<_> = Action::ALL.iter().map(|a| a.default_hotkey()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        assert_eq!(keys.len(), Action::ALL.len());
    }
}
