//! The Theme Editor's data side: loading an installed theme into a
//! `ThemeDraft`, creating / duplicating / deleting themes, and the pure
//! bookkeeping of pending asset operations. No GTK.

use crate::builder::slugify;
use crate::writer::{write_theme, AssetOp, BuildError, ThemeDraft};
use opusdm_ipc::look::{SurfaceImage, ThemeSurface};
use opusdm_ipc::themes::{theme_dir, valid_slug, CustomThemeData, ThemeManifest};
use std::path::{Path, PathBuf};

/// Load installed theme `slug` as an editable draft. Pictures are resolved to
/// absolute paths inside the theme folder. Errors name the file at fault; a
/// `theme.json` that exists but cannot be parsed is an error so the editor
/// never replaces it with an empty one.
pub fn load_draft(slug: &str) -> Result<ThemeDraft, String> {
    if !valid_slug(slug) {
        return Err(format!("invalid theme name: {slug:?}"));
    }
    let dir = theme_dir(slug);
    let manifest_path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: ThemeManifest = serde_json::from_str(&text).map_err(|e| format!("manifest.json: {e}"))?;

    let theme_path = dir.join("theme.json");
    let data: CustomThemeData = match std::fs::read_to_string(&theme_path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("theme.json: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CustomThemeData::default(),
        Err(e) => return Err(format!("{}: {e}", theme_path.display())),
    };

    let surface = |ts: Option<&ThemeSurface>| {
        ts.map(|t| {
            let mut s = SurfaceImage::default();
            crate::apply::merge_surface(&mut s, Some(t), slug);
            s
        })
    };
    let surfaces = data.surfaces.unwrap_or_default();
    Ok(ThemeDraft {
        label: manifest.label,
        author: manifest.author,
        version: manifest.version,
        colors: data.colors,
        appearance: data.appearance,
        fonts: data.fonts,
        lister: surface(surfaces.lister.as_ref()),
        requester: surface(surfaces.requester.as_ref()),
    })
}

/// The slug a new theme called `label` would get, or an error when the name
/// is unusable or a theme with that slug is installed.
fn free_slug(label: &str) -> Result<String, BuildError> {
    let slug = slugify(label).ok_or_else(|| BuildError::InvalidName(label.to_string()))?;
    if theme_dir(&slug).exists() {
        return Err(BuildError::Invalid(format!("a theme named \"{slug}\" already exists")));
    }
    Ok(slug)
}

/// Create an empty theme called `label` (it sets nothing yet). Returns its slug.
pub fn new_theme(label: &str) -> Result<String, BuildError> {
    let slug = free_slug(label)?;
    let draft = ThemeDraft { label: label.trim().to_string(), ..Default::default() };
    write_theme(&slug, &draft, None, &[])?;
    Ok(slug)
}

/// Copy installed theme `src_slug` under a new name. Returns the new slug.
pub fn duplicate_theme(src_slug: &str, label: &str) -> Result<String, BuildError> {
    let slug = free_slug(label)?;
    let mut draft = load_draft(src_slug).map_err(BuildError::Invalid)?;
    draft.label = label.trim().to_string();
    write_theme(&slug, &draft, Some(src_slug), &[])?;
    Ok(slug)
}

/// Delete an installed theme. The default theme cannot be deleted.
pub fn delete_theme(slug: &str) -> Result<(), String> {
    if slug == opusdm_ipc::theme::DEFAULT_THEME_SLUG {
        return Err("the default theme cannot be deleted".to_string());
    }
    if !valid_slug(slug) {
        return Err(format!("invalid theme name: {slug:?}"));
    }
    let dir = theme_dir(slug);
    if !dir.join("manifest.json").is_file() {
        return Err(format!("theme \"{slug}\" is not installed"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Whether the editor has unsaved work: the widgets describe something other
/// than what was loaded, or file operations are pending.
pub fn is_dirty(saved: &ThemeDraft, now: &ThemeDraft, ops: &[AssetOp]) -> bool {
    saved != now || !ops.is_empty()
}

/// Cancel any pending add of `name` and, when a file of that name is
/// `installed` in the theme, queue its removal once. `is_add` and `make_remove`
/// pick the font or sound variants.
fn queue_remove(
    ops: &mut Vec<AssetOp>,
    name: &str,
    installed: bool,
    is_add: fn(&AssetOp, &str) -> bool,
    make_remove: fn(String) -> AssetOp,
) {
    ops.retain(|op| !is_add(op, name));
    if !installed {
        return;
    }
    let remove = make_remove(name.to_string());
    if !ops.contains(&remove) {
        ops.push(remove);
    }
}

/// Queue the removal of font `name`. A pending add of that name is always
/// cancelled; the removal itself is queued only when `installed` (a font of
/// that name is in the theme folder) and not already queued.
pub fn queue_remove_font(ops: &mut Vec<AssetOp>, name: &str, installed: bool) {
    let is_add = |op: &AssetOp, n: &str| matches!(op, AssetOp::AddFont(p) if p.file_name().is_some_and(|f| f == n));
    queue_remove(ops, name, installed, is_add, AssetOp::RemoveFont);
}

/// Queue the removal of sound `name`; same rules as `queue_remove_font`.
pub fn queue_remove_sound(ops: &mut Vec<AssetOp>, name: &str, installed: bool) {
    let is_add = |op: &AssetOp, n: &str| matches!(op, AssetOp::AddSound(p) if p.file_name().is_some_and(|f| f == n));
    queue_remove(ops, name, installed, is_add, AssetOp::RemoveSound);
}

/// Queue the removal of wallpaper `name`; same rules as `queue_remove_font`.
pub fn queue_remove_wallpaper(ops: &mut Vec<AssetOp>, name: &str, installed: bool) {
    let is_add = |op: &AssetOp, n: &str| matches!(op, AssetOp::AddWallpaper(p) if p.file_name().is_some_and(|f| f == n));
    queue_remove(ops, name, installed, is_add, AssetOp::RemoveWallpaper);
}

/// Queue replacing icon `name` with `src`, superseding an earlier pending
/// replacement of the same icon.
pub fn queue_icon_replace(ops: &mut Vec<AssetOp>, name: &str, src: PathBuf) {
    ops.retain(|op| !matches!(op, AssetOp::ReplaceIcon { name: n, .. } if n == name));
    ops.push(AssetOp::ReplaceIcon { name: name.to_string(), src });
}

/// Names of the files directly inside `dir`, sorted. A missing or unreadable
/// folder gives an empty list.
pub fn list_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{install_source, picture, scratch_config, ENV_LOCK};
    use crate::writer::write_theme;
    use opusdm_ipc::desktop::ImageFit;
    use opusdm_ipc::look::{FontSpec, Fonts, SurfaceImage};
    use opusdm_ipc::themes::theme_dir;
    use opusdm_ipc::ColorSettings;

    #[test]
    fn load_draft_round_trips_what_write_theme_wrote() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        let d = ThemeDraft {
            label: "Round".into(),
            author: Some("me".into()),
            colors: Some(ColorSettings { file_fg: "#123456".into(), ..ColorSettings::default() }),
            fonts: Some(Fonts { list: FontSpec { family: "Iosevka".into(), size: 12 }, ui: FontSpec::default() }),
            lister: Some(SurfaceImage { image_path: Some(picture(td.path(), "l.png")), fit: ImageFit::Tile }),
            ..Default::default()
        };
        write_theme("rt", &d, None, &[]).unwrap();

        let back = load_draft("rt").unwrap();

        assert_eq!((back.label.as_str(), back.author.as_deref()), ("Round", Some("me")));
        assert_eq!(back.colors, d.colors);
        assert_eq!(back.fonts, d.fonts);
        let lister = back.lister.unwrap();
        assert!(lister.image_path.unwrap().ends_with("themes/rt/backgrounds/lister.png"));
        assert_eq!(lister.fit, ImageFit::Tile);
        assert!(back.requester.is_none() && back.appearance.is_none());
    }

    #[test]
    fn a_theme_json_that_does_not_parse_is_an_error_not_an_empty_theme() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("broken");
        std::fs::write(theme_dir("broken").join("theme.json"), "{ not json").unwrap();
        let err = load_draft("broken").unwrap_err();
        assert!(err.contains("theme.json"), "{err}");
    }

    #[test]
    fn a_theme_without_theme_json_loads_as_an_empty_draft() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("bare");
        assert_eq!(load_draft("bare").unwrap().colors, None);
    }

    #[test]
    fn a_missing_theme_is_an_error() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        assert!(load_draft("nope").is_err());
    }

    #[test]
    fn new_theme_creates_an_empty_valid_theme_and_refuses_duplicates() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        assert_eq!(new_theme("My New One").unwrap(), "my-new-one");
        assert_eq!(load_draft("my-new-one").unwrap().label, "My New One");
        assert!(new_theme("My New One").is_err(), "already exists");
        assert!(new_theme("***").is_err(), "no usable slug");
    }

    #[test]
    fn duplicate_theme_copies_everything_and_leaves_the_original() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("orig");
        let slug = duplicate_theme("orig", "Orig Copy").unwrap();
        assert_eq!(slug, "orig-copy");
        assert_eq!(load_draft("orig-copy").unwrap().label, "Orig Copy");
        for rel in ["icons/dir.png", "fonts/f.ttf", "sounds/a.wav"] {
            assert!(theme_dir("orig-copy").join(rel).is_file(), "{rel}");
            assert!(theme_dir("orig").join(rel).is_file(), "{rel} of the original");
        }
    }

    #[test]
    fn delete_theme_removes_a_theme_but_never_the_default() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("gone");
        delete_theme("gone").unwrap();
        assert!(!theme_dir("gone").exists());
        assert!(delete_theme("gone").is_err(), "unknown theme");
        install_source(opusdm_ipc::theme::DEFAULT_THEME_SLUG);
        assert!(delete_theme(opusdm_ipc::theme::DEFAULT_THEME_SLUG).is_err());
        assert!(theme_dir(opusdm_ipc::theme::DEFAULT_THEME_SLUG).exists());
    }

    /// Slugs that are not plain theme names touch nothing.
    #[test]
    fn unsafe_slugs_are_rejected_by_delete_and_load() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("keep");
        let outside = td.path().join("opusdm/x");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(delete_theme("../x").is_err());
        assert!(outside.exists(), "nothing outside themes/ may be touched");
        assert!(load_draft("a/b").is_err());
        assert!(theme_dir("keep").exists());
    }

    #[test]
    fn dirty_means_a_changed_draft_or_pending_operations() {
        let a = ThemeDraft { label: "A".into(), ..Default::default() };
        let mut b = a.clone();
        assert!(!is_dirty(&a, &b, &[]));
        b.label = "B".into();
        assert!(is_dirty(&a, &b, &[]));
        assert!(is_dirty(&a, &a, &[AssetOp::RemoveFont("x.ttf".into())]));
    }

    /// Removing a font that was only added this session cancels the add.
    #[test]
    fn removing_a_font_that_was_only_pending_cancels_the_add() {
        let mut ops = vec![AssetOp::AddFont("/tmp/new.ttf".into())];
        queue_remove_font(&mut ops, "new.ttf", false);
        assert!(ops.is_empty());
        queue_remove_font(&mut ops, "old.ttf", true);
        assert_eq!(ops, [AssetOp::RemoveFont("old.ttf".into())]);
    }

    /// Replacing an installed font then removing the row still removes the installed file.
    #[test]
    fn removing_an_installed_font_with_a_pending_add_queues_the_remove() {
        let mut ops = vec![AssetOp::AddFont("/tmp/x.ttf".into())];
        queue_remove_font(&mut ops, "x.ttf", true);
        assert_eq!(ops, [AssetOp::RemoveFont("x.ttf".into())]);
    }

    /// Removing the same installed font twice queues a single removal.
    #[test]
    fn removing_an_installed_font_twice_queues_one_remove() {
        let mut ops = Vec::new();
        queue_remove_font(&mut ops, "x.ttf", true);
        queue_remove_font(&mut ops, "x.ttf", true);
        assert_eq!(ops, [AssetOp::RemoveFont("x.ttf".into())]);
    }

    /// Remove, re-add, remove again of an installed font keeps one removal and no add.
    #[test]
    fn remove_readd_remove_of_an_installed_font_leaves_one_remove() {
        let mut ops = Vec::new();
        queue_remove_font(&mut ops, "x.ttf", true);
        ops.push(AssetOp::AddFont("/tmp/x.ttf".into()));
        assert_eq!(ops.len(), 2);
        queue_remove_font(&mut ops, "x.ttf", true);
        assert_eq!(ops, [AssetOp::RemoveFont("x.ttf".into())]);
    }

    /// Same rules for sounds: pending only, installed with pending add, installed twice.
    #[test]
    fn sound_removal_follows_the_same_rules() {
        let mut ops = vec![AssetOp::AddSound("/tmp/a.wav".into())];
        queue_remove_sound(&mut ops, "a.wav", false);
        assert!(ops.is_empty());
        let mut ops = vec![AssetOp::AddSound("/tmp/b.wav".into())];
        queue_remove_sound(&mut ops, "b.wav", true);
        queue_remove_sound(&mut ops, "b.wav", true);
        assert_eq!(ops, [AssetOp::RemoveSound("b.wav".into())]);
    }

    #[test]
    fn a_second_icon_replacement_supersedes_the_first() {
        let mut ops = Vec::new();
        queue_icon_replace(&mut ops, "dir", "/tmp/one.png".into());
        queue_icon_replace(&mut ops, "file", "/tmp/f.png".into());
        queue_icon_replace(&mut ops, "dir", "/tmp/two.png".into());
        assert_eq!(ops.len(), 2);
        assert!(ops.contains(&AssetOp::ReplaceIcon { name: "dir".into(), src: "/tmp/two.png".into() }));
    }

    #[test]
    fn list_names_returns_sorted_file_names_only() {
        let td = tempfile::tempdir().unwrap();
        for n in ["b.ttf", "a.ttf"] {
            std::fs::write(td.path().join(n), b"x").unwrap();
        }
        std::fs::create_dir(td.path().join("sub")).unwrap();
        assert_eq!(list_names(td.path()), ["a.ttf", "b.ttf"]);
        assert!(list_names(std::path::Path::new("/no/such/dir")).is_empty());
    }
}
