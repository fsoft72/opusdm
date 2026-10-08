//! Which parts of the desktop settings an Apply really changed, so the GTK
//! side redoes only that work. Re-applying an unchanged part (reloading the
//! compositor, re-laying out the icons, moving the dock) is visible as a
//! flicker of the whole desktop. No GTK, no I/O - fully unit-testable.

use opusdm_ipc::DesktopSettings;

/// One flag per group of settings that is applied as a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopChanges {
    /// Window transparency: the compositor config is rewritten and reloaded.
    pub transparency: bool,
    /// Number of virtual screens.
    pub virtual_screens: bool,
    /// Window snapping on or off.
    pub snap: bool,
    /// Icon size of the desktop icon field.
    pub icon_size: bool,
    /// Grid spacing or gap of the desktop icon field.
    pub grid: bool,
    /// Panel edge: moves the panel and the icon area margin.
    pub panel_position: bool,
    /// Dock edge or icon size, or the panel edge it must avoid.
    pub dock_layout: bool,
    /// Folder shown as the desktop.
    pub desktop_dir: bool,
}

impl DesktopChanges {
    /// Whether anything at all changed.
    pub fn any(&self) -> bool {
        *self != DesktopChanges::default()
    }
}

/// Compare the settings last applied (`old`) with the new ones.
pub fn diff(old: &DesktopSettings, new: &DesktopSettings) -> DesktopChanges {
    DesktopChanges {
        transparency: old.transparency_enabled != new.transparency_enabled
            || old.inactive_opacity != new.inactive_opacity
            || old.active_opacity != new.active_opacity,
        virtual_screens: old.virtual_screens != new.virtual_screens,
        snap: old.snap_enabled != new.snap_enabled,
        icon_size: old.icon_size != new.icon_size,
        grid: old.grid_spacing != new.grid_spacing || old.grid_gap != new.grid_gap,
        panel_position: old.panel_position != new.panel_position,
        dock_layout: old.dock_position != new.dock_position
            || old.dock_icon_size != new.dock_icon_size
            || old.panel_position != new.panel_position,
        desktop_dir: old.desktop_dir != new.desktop_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::{DockPosition, PanelPosition};
    use std::path::PathBuf;

    #[test]
    fn identical_settings_change_nothing() {
        let ds = DesktopSettings::default();
        assert!(!diff(&ds, &ds.clone()).any());
    }

    #[test]
    fn only_the_opacity_touches_the_compositor() {
        let old = DesktopSettings::default();
        let new = DesktopSettings { active_opacity: old.active_opacity - 5, ..old.clone() };
        let c = diff(&old, &new);
        assert_eq!(c, DesktopChanges { transparency: true, ..DesktopChanges::default() });
    }

    #[test]
    fn the_panel_edge_moves_the_panel_and_the_dock() {
        let old = DesktopSettings { panel_position: PanelPosition::Top, ..DesktopSettings::default() };
        let new = DesktopSettings { panel_position: PanelPosition::Bottom, ..old.clone() };
        let c = diff(&old, &new);
        assert!(c.panel_position && c.dock_layout);
        assert!(!c.transparency && !c.icon_size && !c.grid && !c.desktop_dir);
    }

    #[test]
    fn dock_edge_and_icon_size_change_only_the_dock() {
        let old = DesktopSettings::default();
        let new = DesktopSettings { dock_position: DockPosition::Left, dock_icon_size: 40, ..old.clone() };
        assert_eq!(diff(&old, &new), DesktopChanges { dock_layout: true, ..DesktopChanges::default() });
    }

    #[test]
    fn icon_field_settings_are_told_apart() {
        let old = DesktopSettings::default();
        let size = DesktopSettings { icon_size: old.icon_size + 8, ..old.clone() };
        assert_eq!(diff(&old, &size), DesktopChanges { icon_size: true, ..DesktopChanges::default() });
        let gap = DesktopSettings { grid_gap: old.grid_gap + 2, ..old.clone() };
        assert_eq!(diff(&old, &gap), DesktopChanges { grid: true, ..DesktopChanges::default() });
    }

    #[test]
    fn screens_snap_and_folder_are_flagged() {
        let old = DesktopSettings::default();
        let new = DesktopSettings {
            virtual_screens: old.virtual_screens + 1,
            snap_enabled: !old.snap_enabled,
            desktop_dir: Some(PathBuf::from("/tmp/d")),
            ..old.clone()
        };
        let c = diff(&old, &new);
        assert!(c.virtual_screens && c.snap && c.desktop_dir);
        assert!(!c.transparency && !c.dock_layout);
    }

    #[test]
    fn settings_the_desktop_does_not_apply_are_ignored() {
        let old = DesktopSettings::default();
        let new = DesktopSettings { open_lister_at_start: !old.open_lister_at_start, ..old.clone() };
        assert!(!diff(&old, &new).any());
    }
}
