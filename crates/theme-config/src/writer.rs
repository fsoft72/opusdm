//! Writes a theme folder from a [`ThemeDraft`]. The theme is built in a
//! staging folder next to the others and swapped in only on full success, so
//! a failure never leaves a half-written theme and writing over the theme the
//! assets came from cannot lose them. Used by "Save theme" and the Theme
//! Editor.

use opusdm_ipc::desktop::{Background, BackgroundMode};
use opusdm_ipc::look::{Fonts, SurfaceImage, ThemeSurface, ThemeSurfaces};
use opusdm_ipc::themes::{
    is_plain_file_name, theme_dir, themes_dir, valid_slug, CustomThemeData, ThemeBackground, ThemeDesktopMetrics, ThemeManifest,
};
use opusdm_ipc::{Appearance, Behavior, ColorSettings};
use std::fmt;
use std::path::{Path, PathBuf};

/// `manifest.json` `version` written when the draft has none.
const DEFAULT_VERSION: &str = "1";

/// The icon files a theme provides, without the `.png` extension.
pub const ICON_NAMES: [&str; 12] =
    ["archive", "audio", "broken", "code", "dir", "exec", "file", "image", "pdf", "symlink", "text", "video"];

/// Extensions (lower case) accepted for files in `fonts/`.
pub const FONT_EXTS: [&str; 2] = ["ttf", "otf"];

/// Extensions (lower case) accepted for files in `sounds/`.
pub const AUDIO_EXTS: [&str; 6] = ["wav", "ogg", "oga", "mp3", "flac", "opus"];

/// A theme as the editor and Save see it. `None` means the theme does not set
/// that part, so applying it leaves the user's own value. Pictures are
/// absolute paths; the writer copies them into the theme.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeDraft {
    pub label: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub colors: Option<ColorSettings>,
    pub appearance: Option<Appearance>,
    pub behavior: Option<Behavior>,
    pub desktop: Option<ThemeDesktopMetrics>,
    pub background: Option<Background>,
    pub fonts: Option<Fonts>,
    pub lister: Option<SurfaceImage>,
    pub requester: Option<SurfaceImage>,
}

/// A change to the files carried by a theme, applied in order on top of the
/// source theme's own files.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetOp {
    AddFont(PathBuf),
    RemoveFont(String),
    AddSound(PathBuf),
    RemoveSound(String),
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
    for op in ops {
        let (src, exts): (Option<&PathBuf>, &[&str]) = match op {
            AssetOp::AddFont(p) => (Some(p), &FONT_EXTS),
            AssetOp::AddSound(p) => (Some(p), &AUDIO_EXTS),
            AssetOp::ReplaceIcon { name, src } => {
                if !ICON_NAMES.contains(&name.as_str()) {
                    return Err(BuildError::Invalid(format!("{name:?} is not an icon name")));
                }
                (Some(src), &["png"])
            }
            AssetOp::RemoveFont(n) | AssetOp::RemoveSound(n) => {
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

/// Write `draft` as theme `slug`, replacing any theme with that slug.
/// `source_slug` names an installed theme whose `icons/`, `fonts/` and
/// `sounds/` are carried over before `ops` are applied. Returns the installed
/// folder.
pub fn write_theme(slug: &str, draft: &ThemeDraft, source_slug: Option<&str>, ops: &[AssetOp]) -> Result<PathBuf, BuildError> {
    if !valid_slug(slug) {
        return Err(BuildError::InvalidName(slug.to_string()));
    }
    validate_draft(draft, ops)?;
    let root = themes_dir();
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    let staging = root.join(format!(".{slug}.tmp"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io_err(&staging))?;
    }
    if let Err(e) = stage(slug, draft, source_slug, ops, &staging) {
        if let Err(clean) = std::fs::remove_dir_all(&staging) {
            opusdm_ipc::err_log!("theme-config", "cannot remove {}: {clean}", staging.display());
        }
        return Err(e);
    }
    let dest = theme_dir(slug);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(io_err(&dest))?;
    }
    std::fs::rename(&staging, &dest).map_err(io_err(&dest))?;
    Ok(dest)
}

/// Build the whole theme inside `dir`.
fn stage(slug: &str, draft: &ThemeDraft, source_slug: Option<&str>, ops: &[AssetOp], dir: &Path) -> Result<(), BuildError> {
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let backgrounds = dir.join("backgrounds");
    let data = CustomThemeData {
        colors: draft.colors.clone(),
        appearance: draft.appearance.clone(),
        behavior: draft.behavior.clone(),
        desktop: draft.desktop.clone(),
        background: draft.background.as_ref().map(|b| theme_background(b, &backgrounds)).transpose()?,
        fonts: draft.fonts.clone(),
        surfaces: theme_surfaces(draft, &backgrounds)?,
    };
    if let Some(source) = source_slug {
        for part in ["icons", "fonts", "sounds"] {
            copy_dir_if_exists(&theme_dir(source).join(part), &dir.join(part))?;
        }
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

/// The desktop background as a theme section. A picture is only carried when
/// the mode uses it: the path is kept in other modes so switching back is
/// lossless, but it may be stale.
fn theme_background(bg: &Background, backgrounds: &Path) -> Result<ThemeBackground, BuildError> {
    let image = match (bg.mode, &bg.image_path) {
        (BackgroundMode::Image, Some(p)) => Some(copy_picture(p, backgrounds, "desktop")?),
        _ => None,
    };
    Ok(ThemeBackground {
        mode: Some(bg.mode),
        solid: Some(bg.solid.clone()),
        gradient_start: Some(bg.gradient_start.clone()),
        gradient_end: Some(bg.gradient_end.clone()),
        gradient_angle: Some(bg.gradient_angle),
        image,
        image_fit: Some(bg.image_fit),
    })
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
        AssetOp::ReplaceIcon { name, src } => copy_into(src, &dir.join("icons"), &format!("{name}.png")),
        AssetOp::RemoveFont(name) => remove_if_exists(&dir.join("fonts").join(name)),
        AssetOp::RemoveSound(name) => remove_if_exists(&dir.join("sounds").join(name)),
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

/// Recursively copy `src` to `dst`; a missing `src` is not an error.
fn copy_dir_if_exists(src: &Path, dst: &Path) -> Result<(), BuildError> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(io_err(dst))?;
    for entry in std::fs::read_dir(src).map_err(io_err(src))? {
        let entry = entry.map_err(io_err(src))?;
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir_if_exists(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).map_err(io_err(&entry.path()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{install_source, picture, scratch_config, ENV_LOCK};
    use opusdm_ipc::desktop::{BackgroundMode, ImageFit};
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
        d.background = Some(Background { mode: BackgroundMode::Image, image_path: Some(picture(td.path(), "wall.jpg")), image_fit: ImageFit::Tile, ..Background::default() });
        d.lister = Some(SurfaceImage { image_path: Some(picture(td.path(), "l.png")), fit: ImageFit::Center });
        let dir = write_theme("pics", &d, None, &[]).unwrap();
        let data = load_theme_data("pics");
        assert_eq!(data.background.unwrap().image.as_deref(), Some("desktop.jpg"));
        assert_eq!(std::fs::read(dir.join("backgrounds/desktop.jpg")).unwrap(), b"wall.jpg");
        let lister = data.surfaces.unwrap().lister.unwrap();
        assert_eq!((lister.image.as_deref(), lister.fit), (Some("lister.png"), Some(ImageFit::Center)));
    }

    #[test]
    fn a_desktop_picture_left_over_from_another_mode_is_not_copied() {
        let _g = ENV_LOCK.lock().unwrap();
        let _td = scratch_config();
        let mut d = draft("T");
        d.background = Some(Background { mode: BackgroundMode::Solid, image_path: Some("/no/such/old.png".into()), ..Background::default() });
        write_theme("t", &d, None, &[]).unwrap();
        assert_eq!(load_theme_data("t").background.unwrap().image, None);
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
            AssetOp::ReplaceIcon { name: "nope".into(), src: picture(td.path(), "a.png") },
            AssetOp::ReplaceIcon { name: "dir".into(), src: picture(td.path(), "a.jpg") },
        ];
        for op in cases {
            assert!(validate_draft(&draft("T"), std::slice::from_ref(&op)).is_err(), "{op:?}");
        }
    }
}
