//! Writes a theme folder from a [`ThemeDraft`]. The theme is built in a
//! staging folder next to the others and swapped in only on full success, so
//! a failure never leaves a half-written theme and writing over the theme the
//! assets came from cannot lose them. Used by "Save theme" and the Theme
//! Editor.

use opusdm_ipc::desktop::BACKGROUND_IMAGE_EXTS;
use opusdm_ipc::look::{Fonts, HubLook, SurfaceImage, ThemeHub, ThemeHubBar, ThemeSurface, ThemeSurfaces};
use opusdm_ipc::theme::DEFAULT_THEME_SLUG;
use opusdm_ipc::themes::{
    is_plain_file_name, legacy_wallpaper, theme_dir, themes_dir, valid_slug, CustomThemeData, ThemeManifest, WALLPAPERS_DIR,
};
use opusdm_ipc::{Appearance, ColorSettings};
use std::fmt;
use std::path::{Path, PathBuf};

/// `manifest.json` `version` written when the draft has none.
const DEFAULT_VERSION: &str = "1";

/// The preview picture a theme may carry at its root.
const PREVIEW_FILE: &str = "preview.png";

/// The icon files a theme provides, without the `.png` extension.
pub const ICON_NAMES: [&str; 12] =
    ["archive", "audio", "broken", "code", "dir", "exec", "file", "image", "pdf", "symlink", "text", "video"];

/// Extensions (lower case) accepted for the start menu icon.
pub const HUB_ICON_EXTS: [&str; 2] = ["png", "svg"];

/// Extensions (lower case) accepted for files in `fonts/`.
pub const FONT_EXTS: [&str; 2] = ["ttf", "otf"];

/// Extensions (lower case) accepted for files in `sounds/`.
pub const AUDIO_EXTS: [&str; 6] = ["wav", "ogg", "oga", "mp3", "flac", "opus"];

/// A theme as the editor and Save see it. `None` means the theme does not set
/// that part, so applying it gives the default value. Pictures are absolute
/// paths; the writer copies them into the theme.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeDraft {
    pub label: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub colors: Option<ColorSettings>,
    pub appearance: Option<Appearance>,
    pub fonts: Option<Fonts>,
    pub lister: Option<SurfaceImage>,
    pub requester: Option<SurfaceImage>,
    pub hub: HubLook,
}

/// A change to the files carried by a theme, applied in order on top of the
/// source theme's own files.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetOp {
    AddFont(PathBuf),
    RemoveFont(String),
    AddSound(PathBuf),
    RemoveSound(String),
    AddWallpaper(PathBuf),
    RemoveWallpaper(String),
    /// Overwrite `icons/<name>.png` with `src`.
    ReplaceIcon { name: String, src: PathBuf },
}

/// Why writing a theme failed.
#[derive(Debug)]
pub enum BuildError {
    /// The slug is not a valid theme folder name.
    InvalidName(String),
    /// The draft or an asset operation is not acceptable.
    Invalid(String),
    Io { path: PathBuf, source: std::io::Error },
    Json(serde_json::Error),
}

impl fmt::Display for BuildError {
    /// Human readable message naming the offending name, file or JSON error.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::InvalidName(n) => write!(f, "invalid theme name: {n:?}"),
            BuildError::Invalid(m) => write!(f, "{m}"),
            BuildError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            BuildError::Json(e) => write!(f, "cannot write theme.json: {e}"),
        }
    }
}

/// Attach `path` to an I/O error.
fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> BuildError + '_ {
    move |source| BuildError::Io { path: path.to_path_buf(), source }
}

/// Whether `path` has one of `exts` (lower case), ignoring case.
fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
}

/// Check a draft and its asset operations before anything is written: the
/// label is not empty, added files exist and have an allowed extension, and
/// names to remove or replace are acceptable.
pub fn validate_draft(draft: &ThemeDraft, ops: &[AssetOp]) -> Result<(), BuildError> {
    if draft.label.trim().is_empty() {
        return Err(BuildError::Invalid("the theme needs a name".to_string()));
    }
    let hub_pictures = draft.hub.bar.iter().filter_map(|b| b.image.image_path.as_ref()).map(|p| (p, &BACKGROUND_IMAGE_EXTS[..]));
    let menu_icon = draft.hub.menu_icon.iter().map(|p| (p, &HUB_ICON_EXTS[..]));
    for (src, exts) in hub_pictures.chain(menu_icon) {
        if !has_ext(src, exts) {
            return Err(BuildError::Invalid(format!("{} must be one of: {}", src.display(), exts.join(", "))));
        }
        if !src.is_file() {
            return Err(BuildError::Invalid(format!("{} does not exist", src.display())));
        }
    }
    for op in ops {
        let (src, exts): (Option<&PathBuf>, &[&str]) = match op {
            AssetOp::AddFont(p) => (Some(p), &FONT_EXTS),
            AssetOp::AddSound(p) => (Some(p), &AUDIO_EXTS),
            AssetOp::AddWallpaper(p) => (Some(p), &BACKGROUND_IMAGE_EXTS),
            AssetOp::ReplaceIcon { name, src } => {
                if !ICON_NAMES.contains(&name.as_str()) {
                    return Err(BuildError::Invalid(format!("{name:?} is not an icon name")));
                }
                (Some(src), &["png"])
            }
            AssetOp::RemoveFont(n) | AssetOp::RemoveSound(n) | AssetOp::RemoveWallpaper(n) => {
                if !is_plain_file_name(n) {
                    return Err(BuildError::Invalid(format!("{n:?} is not a file name")));
                }
                (None, &[])
            }
        };
        let Some(src) = src else { continue };
        if !has_ext(src, exts) {
            return Err(BuildError::Invalid(format!("{} must be one of: {}", src.display(), exts.join(", "))));
        }
        if !src.is_file() {
            return Err(BuildError::Invalid(format!("{} does not exist", src.display())));
        }
    }
    Ok(())
}

/// Whether theme `slug` must not be changed: the default theme, which ships
/// with OpusDM and is the fallback for every other one.
pub fn is_read_only(slug: &str) -> bool {
    slug == DEFAULT_THEME_SLUG
}

/// Write `draft` as theme `slug`, replacing any theme with that slug.
/// `source_slug` names an installed theme whose `icons/`, `fonts/`, `sounds/`,
/// `wallpapers/` (plus a legacy desktop picture) and `preview.png` are carried
/// over before `ops` are applied. The default theme is refused (see
/// [`is_read_only`]). Returns the installed folder.
pub fn write_theme(slug: &str, draft: &ThemeDraft, source_slug: Option<&str>, ops: &[AssetOp]) -> Result<PathBuf, BuildError> {
    if !valid_slug(slug) {
        return Err(BuildError::InvalidName(slug.to_string()));
    }
    if is_read_only(slug) {
        return Err(BuildError::Invalid("the default theme cannot be changed: duplicate it first".to_string()));
    }
    if let Some(source) = source_slug {
        if !valid_slug(source) {
            return Err(BuildError::InvalidName(source.to_string()));
        }
    }
    validate_draft(draft, ops)?;
    let root = themes_dir();
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    let staging = root.join(format!(".{slug}.tmp"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io_err(&staging))?;
    }
    let dest = theme_dir(slug);
    if let Err(e) = stage(slug, draft, source_slug, ops, &staging).and_then(|()| swap_in(&staging, &dest)) {
        if staging.exists() {
            if let Err(clean) = std::fs::remove_dir_all(&staging) {
                opusdm_ipc::err_log!("theme-config", "cannot remove {}: {clean}", staging.display());
            }
        }
        return Err(e);
    }
    Ok(dest)
}

/// Replace `dest` with `staging`. An existing `dest` is first moved to a
/// sibling `.<name>.old` backup, which is restored if the final rename fails
/// and removed once it succeeds, so the old folder is never lost.
fn swap_in(staging: &Path, dest: &Path) -> Result<(), BuildError> {
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let backup = dest.with_file_name(format!(".{name}.old"));
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(io_err(&backup))?;
    }
    let had_old = dest.exists();
    if had_old {
        std::fs::rename(dest, &backup).map_err(io_err(dest))?;
    }
    if let Err(e) = std::fs::rename(staging, dest) {
        if had_old {
            if let Err(restore) = std::fs::rename(&backup, dest) {
                opusdm_ipc::err_log!("theme-config", "cannot restore {}: {restore}", dest.display());
            }
        }
        return Err(BuildError::Io { path: staging.to_path_buf(), source: e });
    }
    if had_old {
        if let Err(clean) = std::fs::remove_dir_all(&backup) {
            opusdm_ipc::err_log!("theme-config", "cannot remove {}: {clean}", backup.display());
        }
    }
    Ok(())
}

/// Build the whole theme inside `dir`.
fn stage(slug: &str, draft: &ThemeDraft, source_slug: Option<&str>, ops: &[AssetOp], dir: &Path) -> Result<(), BuildError> {
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let backgrounds = dir.join("backgrounds");
    let data = CustomThemeData {
        colors: draft.colors.clone(),
        appearance: draft.appearance.clone(),
        background: None,
        fonts: draft.fonts.clone(),
        surfaces: theme_surfaces(draft, &backgrounds)?,
        hub: theme_hub(&draft.hub, &backgrounds)?,
    };
    if let Some(source) = source_slug {
        for part in ["icons", "fonts", "sounds", WALLPAPERS_DIR] {
            copy_dir_if_exists(&theme_dir(source).join(part), &dir.join(part))?;
        }
        copy_file_if_regular(&theme_dir(source).join(PREVIEW_FILE), &dir.join(PREVIEW_FILE))?;
        carry_legacy_wallpaper(source, &dir.join(WALLPAPERS_DIR))?;
    }
    for op in ops {
        apply_op(op, dir)?;
    }
    let manifest = ThemeManifest {
        name: slug.to_string(),
        label: draft.label.trim().to_string(),
        author: draft.author.clone().filter(|a| !a.trim().is_empty()),
        version: Some(draft.version.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_VERSION.to_string())),
    };
    write_json(&dir.join("manifest.json"), &manifest)?;
    write_json(&dir.join("theme.json"), &data)
}

/// Serialize `value` to `path` as pretty JSON.
fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), BuildError> {
    let text = serde_json::to_string_pretty(value).map_err(BuildError::Json)?;
    std::fs::write(path, text + "\n").map_err(io_err(path))
}

/// Copy the desktop picture a legacy `theme.json` of theme `source` names
/// into `wallpapers` (the staged theme's folder), unless a wallpaper with that
/// name is already there, so saving an old theme keeps its wallpaper.
fn carry_legacy_wallpaper(source: &str, wallpapers: &Path) -> Result<(), BuildError> {
    let Some(src) = legacy_wallpaper(source) else { return Ok(()) };
    let name = file_name(&src)?;
    if wallpapers.join(&name).exists() {
        return Ok(());
    }
    copy_into(&src, wallpapers, &name)
}

/// The `surfaces` section, or `None` when neither surface is set.
fn theme_surfaces(draft: &ThemeDraft, backgrounds: &Path) -> Result<Option<ThemeSurfaces>, BuildError> {
    if draft.lister.is_none() && draft.requester.is_none() {
        return Ok(None);
    }
    Ok(Some(ThemeSurfaces {
        lister: draft.lister.as_ref().map(|s| theme_surface(s, backgrounds, "lister")).transpose()?,
        requester: draft.requester.as_ref().map(|s| theme_surface(s, backgrounds, "requester")).transpose()?,
    }))
}

/// The `hub` section, or `None` when the draft sets nothing about the hub.
/// Pictures are copied into `backgrounds/` as `hub-bar.<ext>` and
/// `hub-menu-icon.<ext>`.
fn theme_hub(hub: &HubLook, backgrounds: &Path) -> Result<Option<ThemeHub>, BuildError> {
    if *hub == HubLook::default() {
        return Ok(None);
    }
    let bar = hub
        .bar
        .as_ref()
        .map(|b| -> Result<ThemeHubBar, BuildError> {
            let b = b.clone().sanitized();
            let image = b.image.image_path.as_ref().map(|p| copy_picture(p, backgrounds, "hub-bar")).transpose()?;
            Ok(ThemeHubBar { kind: b.kind, color: b.color, start: b.start, end: b.end, angle: b.angle, image, fit: Some(b.image.fit) })
        })
        .transpose()?;
    let menu_icon = hub.menu_icon.as_ref().map(|p| copy_picture(p, backgrounds, "hub-menu-icon")).transpose()?;
    Ok(Some(ThemeHub { bar, menu_icon, menu_font: hub.menu_font.clone() }))
}

/// One surface as a theme section, copying its picture (named `stem.<ext>`).
fn theme_surface(surface: &SurfaceImage, backgrounds: &Path, stem: &str) -> Result<ThemeSurface, BuildError> {
    let image = surface.image_path.as_ref().map(|p| copy_picture(p, backgrounds, stem)).transpose()?;
    Ok(ThemeSurface { image, fit: Some(surface.fit) })
}

/// Copy `src` into `backgrounds/` as `<stem>.<original extension>` and
/// return that file name.
fn copy_picture(src: &Path, backgrounds: &Path, stem: &str) -> Result<String, BuildError> {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
    let name = format!("{stem}.{ext}");
    copy_into(src, backgrounds, &name)?;
    Ok(name)
}

/// Copy `src` to `folder/name`, creating `folder`.
fn copy_into(src: &Path, folder: &Path, name: &str) -> Result<(), BuildError> {
    std::fs::create_dir_all(folder).map_err(io_err(folder))?;
    std::fs::copy(src, folder.join(name)).map_err(io_err(src))?;
    Ok(())
}

/// Apply one asset operation inside the staged theme `dir`.
fn apply_op(op: &AssetOp, dir: &Path) -> Result<(), BuildError> {
    match op {
        AssetOp::AddFont(src) => copy_into(src, &dir.join("fonts"), &file_name(src)?),
        AssetOp::AddSound(src) => copy_into(src, &dir.join("sounds"), &file_name(src)?),
        AssetOp::AddWallpaper(src) => copy_into(src, &dir.join(WALLPAPERS_DIR), &file_name(src)?),
        AssetOp::ReplaceIcon { name, src } => copy_into(src, &dir.join("icons"), &format!("{name}.png")),
        AssetOp::RemoveFont(name) => remove_if_exists(&dir.join("fonts").join(name)),
        AssetOp::RemoveSound(name) => remove_if_exists(&dir.join("sounds").join(name)),
        AssetOp::RemoveWallpaper(name) => remove_if_exists(&dir.join(WALLPAPERS_DIR).join(name)),
    }
}

/// The final component of `path` as a string.
fn file_name(path: &Path) -> Result<String, BuildError> {
    path.file_name().map(|n| n.to_string_lossy().to_string()).ok_or_else(|| BuildError::Invalid(format!("{} has no file name", path.display())))
}

/// Delete `path`; a file that is already gone is fine.
fn remove_if_exists(path: &Path) -> Result<(), BuildError> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(BuildError::Io { path: path.to_path_buf(), source: e }),
        _ => Ok(()),
    }
}

/// Copy `src` to `dst` when `src` is a regular file (not a link); anything
/// else, including a missing file, is skipped.
fn copy_file_if_regular(src: &Path, dst: &Path) -> Result<(), BuildError> {
    let Ok(meta) = std::fs::symlink_metadata(src) else { return Ok(()) };
    if !meta.file_type().is_file() {
        return Ok(());
    }
    std::fs::copy(src, dst).map_err(io_err(src))?;
    Ok(())
}

/// Recursively copy `src` to `dst`; a missing `src` is not an error.
/// Symbolic links are skipped (and logged), never followed.
fn copy_dir_if_exists(src: &Path, dst: &Path) -> Result<(), BuildError> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(io_err(dst))?;
    for entry in std::fs::read_dir(src).map_err(io_err(src))? {
        let entry = entry.map_err(io_err(src))?;
        let kind = entry.file_type().map_err(io_err(&entry.path()))?;
        let to = dst.join(entry.file_name());
        if kind.is_symlink() {
            opusdm_ipc::err_log!("theme-config", "skipping symbolic link {}", entry.path().display());
        } else if kind.is_dir() {
            copy_dir_if_exists(&entry.path(), &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to).map_err(io_err(&entry.path()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{install_source, picture, scratch_config, ENV_LOCK};
    use opusdm_ipc::desktop::ImageFit;
    use opusdm_ipc::themes::{load_theme_data, theme_dir, themes_dir};

    fn draft(label: &str) -> ThemeDraft {
        ThemeDraft { label: label.into(), ..Default::default() }
    }

    fn folder_names() -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(themes_dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_draft_with_nothing_set_writes_an_empty_theme_json() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let dir = write_theme("plain", &draft("Plain"), None, &[]).unwrap();
        let data = load_theme_data("plain");
        assert!(data.colors.is_none() && data.fonts.is_none() && data.surfaces.is_none() && data.background.is_none());
        let manifest: opusdm_ipc::themes::ThemeManifest = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!((manifest.name.as_str(), manifest.label.as_str()), ("plain", "Plain"));
    }

    #[test]
    fn pictures_are_copied_in_and_referenced_by_relative_name() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        let mut d = draft("Pics");
        d.lister = Some(SurfaceImage { image_path: Some(picture(td.path(), "l.png")), fit: ImageFit::Center });
        let dir = write_theme("pics", &d, None, &[]).unwrap();
        let data = load_theme_data("pics");
        assert_eq!(std::fs::read(dir.join("backgrounds/lister.png")).unwrap(), b"l.png");
        let lister = data.surfaces.unwrap().lister.unwrap();
        assert_eq!((lister.image.as_deref(), lister.fit), (Some("lister.png"), Some(ImageFit::Center)));
    }

    #[test]
    fn the_hub_look_is_written_with_its_pictures_and_loads_back() {
        use opusdm_ipc::look::{FontSpec, HubBar, HubBarKind, HubLook};
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        let mut d = draft("Hub");
        d.hub = HubLook {
            bar: Some(HubBar {
                kind: HubBarKind::Image,
                image: SurfaceImage { image_path: Some(picture(td.path(), "bar.png")), fit: ImageFit::Tile },
                ..HubBar::default()
            }),
            menu_icon: Some(picture(td.path(), "m.svg")),
            menu_font: Some(FontSpec { family: "Cantarell".into(), size: 13 }),
        };
        let dir = write_theme("hub", &d, None, &[]).unwrap();
        assert_eq!(std::fs::read(dir.join("backgrounds/hub-bar.png")).unwrap(), b"bar.png");
        assert_eq!(std::fs::read(dir.join("backgrounds/hub-menu-icon.svg")).unwrap(), b"m.svg");
        let hub = load_theme_data("hub").hub.unwrap();
        assert_eq!(hub.menu_icon.as_deref(), Some("hub-menu-icon.svg"));
        assert_eq!(hub.bar.unwrap().image.as_deref(), Some("hub-bar.png"));
    }

    #[test]
    fn a_draft_without_hub_settings_writes_no_hub_section() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        write_theme("nohub", &draft("No Hub"), None, &[]).unwrap();
        assert!(load_theme_data("nohub").hub.is_none());
    }

    #[test]
    fn a_menu_icon_that_is_not_png_or_svg_is_rejected() {
        use opusdm_ipc::look::HubLook;
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        let mut d = draft("T");
        d.hub = HubLook { menu_icon: Some(picture(td.path(), "m.jpg")), ..HubLook::default() };
        assert!(validate_draft(&d, &[]).is_err());
        d.hub.menu_icon = Some("/no/such/m.png".into());
        assert!(validate_draft(&d, &[]).is_err());
    }

    #[test]
    fn wallpapers_are_carried_and_edited_by_ops() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("src");
        let walls = theme_dir("src").join(WALLPAPERS_DIR);
        std::fs::create_dir_all(&walls).unwrap();
        std::fs::write(walls.join("old.png"), b"old").unwrap();
        std::fs::write(walls.join("keep.jpg"), b"keep").unwrap();
        let ops = [AssetOp::RemoveWallpaper("old.png".into()), AssetOp::AddWallpaper(picture(td.path(), "new.webp"))];
        let dir = write_theme("out", &draft("Out"), Some("src"), &ops).unwrap();
        assert!(!dir.join("wallpapers/old.png").exists());
        assert!(dir.join("wallpapers/keep.jpg").is_file() && dir.join("wallpapers/new.webp").is_file());
    }

    #[test]
    fn a_legacy_desktop_picture_becomes_a_wallpaper_and_the_section_is_dropped() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("old");
        let dir = theme_dir("old");
        std::fs::create_dir_all(dir.join("backgrounds")).unwrap();
        std::fs::write(dir.join("backgrounds/desktop.jpg"), b"wall").unwrap();
        std::fs::write(dir.join("theme.json"), r#"{"background":{"mode":"image","image":"desktop.jpg"}}"#).unwrap();
        let out = write_theme("old", &draft("Old"), Some("old"), &[]).unwrap();
        assert_eq!(std::fs::read(out.join("wallpapers/desktop.jpg")).unwrap(), b"wall");
        assert!(load_theme_data("old").background.is_none());
    }

    #[test]
    fn the_default_theme_is_read_only() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        assert!(is_read_only(DEFAULT_THEME_SLUG) && !is_read_only("mine"));
        let r = write_theme(DEFAULT_THEME_SLUG, &draft("Breeze"), None, &[]);
        assert!(matches!(r, Err(BuildError::Invalid(_))));
    }

    #[test]
    fn assets_are_carried_over_and_ops_apply_in_order() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("src");
        let ops = [
            AssetOp::RemoveFont("f.ttf".into()),
            AssetOp::AddFont(picture(td.path(), "new.otf")),
            AssetOp::AddSound(picture(td.path(), "b.ogg")),
            AssetOp::ReplaceIcon { name: "dir".into(), src: picture(td.path(), "dir.png") },
        ];
        let dir = write_theme("out", &draft("Out"), Some("src"), &ops).unwrap();
        assert!(!dir.join("fonts/f.ttf").exists());
        assert!(dir.join("fonts/new.otf").is_file());
        assert!(dir.join("sounds/a.wav").is_file() && dir.join("sounds/b.ogg").is_file());
        assert_eq!(std::fs::read(dir.join("icons/dir.png")).unwrap(), b"dir.png");
    }

    #[test]
    fn writing_over_the_source_theme_keeps_its_assets() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        write_theme("src", &draft("Src"), Some("src"), &[]).unwrap();
        for rel in ["icons/dir.png", "fonts/f.ttf", "sounds/a.wav", "theme.json"] {
            assert!(theme_dir("src").join(rel).is_file(), "{rel}");
        }
    }

    #[test]
    fn a_missing_picture_fails_naming_it_and_leaves_nothing_behind() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        let mut d = draft("T");
        d.requester = Some(SurfaceImage { image_path: Some("/no/such/r.png".into()), fit: ImageFit::Fill });
        let err = write_theme("t", &d, Some("src"), &[]).unwrap_err().to_string();
        assert!(err.contains("/no/such/r.png"), "{err}");
        assert_eq!(folder_names(), ["src"], "no half-written theme and no staging folder");
    }

    #[test]
    fn a_failed_write_keeps_the_existing_theme_intact() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        let mut d = draft("Src");
        d.requester = Some(SurfaceImage { image_path: Some("/no/such/r.png".into()), fit: ImageFit::Fill });
        assert!(write_theme("src", &d, Some("src"), &[]).is_err());
        assert!(theme_dir("src").join("icons/dir.png").is_file());
        assert_eq!(folder_names(), ["src"], "no staging or backup folder is left");
    }

    #[test]
    fn swap_in_replaces_an_existing_folder_and_leaves_no_backup() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let root = themes_dir();
        std::fs::create_dir_all(root.join("dest")).unwrap();
        std::fs::write(root.join("dest/old.txt"), "old").unwrap();
        std::fs::create_dir_all(root.join(".dest.tmp")).unwrap();
        std::fs::write(root.join(".dest.tmp/new.txt"), "new").unwrap();
        swap_in(&root.join(".dest.tmp"), &root.join("dest")).unwrap();
        assert!(root.join("dest/new.txt").is_file() && !root.join("dest/old.txt").exists());
        assert_eq!(folder_names(), ["dest"]);
    }

    #[test]
    fn swap_in_failure_keeps_the_old_folder_and_leaves_no_backup() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let root = themes_dir();
        std::fs::create_dir_all(root.join("dest")).unwrap();
        std::fs::write(root.join("dest/old.txt"), "old").unwrap();
        assert!(swap_in(&root.join(".dest.tmp"), &root.join("dest")).is_err());
        assert_eq!(std::fs::read(root.join("dest/old.txt")).unwrap(), b"old");
        assert_eq!(folder_names(), ["dest"]);
    }

    /// The source theme's `preview.png` survives a save, also over the same slug.
    #[test]
    fn preview_png_is_carried_over_from_the_source_theme() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        install_source("src");
        std::fs::write(theme_dir("src").join("preview.png"), b"preview-bytes").unwrap();
        let copy = write_theme("copy", &draft("Copy"), Some("src"), &[]).unwrap();
        assert_eq!(std::fs::read(copy.join("preview.png")).unwrap(), b"preview-bytes");
        write_theme("src", &draft("Src"), Some("src"), &[]).unwrap();
        assert_eq!(std::fs::read(theme_dir("src").join("preview.png")).unwrap(), b"preview-bytes");
    }

    /// Symlinks in the source theme are skipped: no loop, no link target content.
    #[test]
    fn symlinks_in_the_source_theme_are_not_followed() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        install_source("src");
        let icons = theme_dir("src").join("icons");
        std::os::unix::fs::symlink(".", icons.join("loop")).unwrap();
        let secret = picture(td.path(), "secret.txt");
        std::os::unix::fs::symlink(&secret, icons.join("leak.png")).unwrap();
        std::os::unix::fs::symlink(&secret, theme_dir("src").join("preview.png")).unwrap();
        let dir = write_theme("out", &draft("Out"), Some("src"), &[]).unwrap();
        assert!(dir.join("icons/dir.png").is_file());
        assert!(std::fs::symlink_metadata(dir.join("icons/loop")).is_err());
        assert!(std::fs::symlink_metadata(dir.join("icons/leak.png")).is_err());
        assert!(std::fs::symlink_metadata(dir.join("preview.png")).is_err());
    }

    /// A source slug that is not a valid theme name is refused.
    #[test]
    fn an_invalid_source_slug_is_rejected() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let r = write_theme("ok", &draft("Ok"), Some("../.."), &[]);
        assert!(matches!(r, Err(BuildError::InvalidName(_))));
        assert!(!themes_dir().join("ok").exists());
    }

    #[test]
    fn validation_rejects_bad_names_extensions_and_ops() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = scratch_config();
        assert!(matches!(write_theme("Bad Slug", &draft("T"), None, &[]), Err(BuildError::InvalidName(_))));
        assert!(matches!(write_theme("t", &draft("  "), None, &[]), Err(BuildError::Invalid(_))));
        let cases = [
            AssetOp::AddFont(picture(td.path(), "x.txt")),
            AssetOp::AddSound(picture(td.path(), "x.exe")),
            AssetOp::AddFont("/no/such/x.ttf".into()),
            AssetOp::RemoveSound("../x.wav".into()),
            AssetOp::AddWallpaper(picture(td.path(), "w.svg")),
            AssetOp::RemoveWallpaper("a/b.png".into()),
            AssetOp::ReplaceIcon { name: "nope".into(), src: picture(td.path(), "a.png") },
            AssetOp::ReplaceIcon { name: "dir".into(), src: picture(td.path(), "a.jpg") },
        ];
        for op in cases {
            assert!(validate_draft(&draft("T"), std::slice::from_ref(&op)).is_err(), "{op:?}");
        }
    }
}
