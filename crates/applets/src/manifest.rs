//! The `applet.json` manifest of an applet.

use serde_json::Value;

/// Default entry script, relative to the applet folder.
pub const DEFAULT_ENTRY: &str = "main.js";
/// Default window width in pixels.
pub const DEFAULT_WIDTH: i32 = 300;
/// Default window height in pixels.
pub const DEFAULT_HEIGHT: i32 = 200;
/// Default minimum window size (both axes), capped to the window size.
pub const DEFAULT_MIN_SIZE: i32 = 50;

/// Permission names an applet may declare. They are never enforced.
pub const PERMISSIONS: [&str; 3] = ["net", "fs", "exec"];

/// A parsed and validated `applet.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry: String,
    pub window: WindowSpec,
    pub settings: Vec<SettingDef>,
    pub permissions: Vec<String>,
}

/// Size and behaviour of the applet window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSpec {
    pub width: i32,
    pub height: i32,
    pub min_width: i32,
    pub min_height: i32,
    pub resizable: bool,
    pub transparent: bool,
}

/// One user setting declared by the applet.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingDef {
    pub key: String,
    pub kind: SettingKind,
    pub label: String,
    pub default: Value,
}

/// Type of a setting; `Choice` carries the allowed values.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    String,
    Number,
    Bool,
    Choice(Vec<String>),
}

// Raw JSON shapes, validated by `parse`.

#[derive(serde::Deserialize)]
struct RawManifest {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    entry: Option<String>,
    #[serde(default)]
    window: RawWindow,
    #[serde(default)]
    settings: Vec<RawSetting>,
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawWindow {
    width: Option<i64>,
    height: Option<i64>,
    min_width: Option<i64>,
    min_height: Option<i64>,
    #[serde(default)]
    resizable: bool,
    #[serde(default)]
    transparent: bool,
}

#[derive(serde::Deserialize)]
struct RawSetting {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    label: String,
    default: Option<Value>,
    choices: Option<Vec<String>>,
}

/// Parses and validates the text of an `applet.json`.
///
/// Errors are meant for a person: they name the offending field or key.
/// Unknown top-level fields are ignored.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let raw: RawManifest = serde_json::from_str(text).map_err(|e| format!("invalid applet.json: {e}"))?;
    if raw.name.trim().is_empty() { return Err("'name' must not be empty".into()); }
    let entry = raw.entry.unwrap_or_else(|| DEFAULT_ENTRY.to_string());
    check_entry(&entry)?;
    let window = build_window(&raw.window)?;
    let mut settings: Vec<SettingDef> = Vec::new();
    for s in raw.settings {
        if settings.iter().any(|d| d.key == s.key) { return Err(format!("duplicate setting key '{}'", s.key)); }
        settings.push(build_setting(s)?);
    }
    for p in &raw.permissions {
        if !PERMISSIONS.contains(&p.as_str()) { return Err(format!("unknown permission '{p}' (allowed: net, fs, exec)")); }
    }
    Ok(Manifest { name: raw.name, version: raw.version, description: raw.description, entry, window, settings, permissions: raw.permissions })
}

/// Checks that the entry is a non-empty relative path staying in the folder.
fn check_entry(entry: &str) -> Result<(), String> {
    use std::path::{Component, Path};
    if entry.trim().is_empty() { return Err("'entry' must not be empty".into()); }
    let ok = Path::new(entry).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !ok { return Err(format!("'entry' must be a relative path inside the applet folder, got '{entry}'")); }
    Ok(())
}

/// Validates one window dimension and converts it to `i32`.
fn dimension(field: &str, value: i64) -> Result<i32, String> {
    if value <= 0 { return Err(format!("'window.{field}' must be greater than 0, got {value}")); }
    i32::try_from(value).map_err(|_| format!("'window.{field}' is too large: {value}"))
}

/// Builds the window spec, applying defaults and the minimum size rules.
fn build_window(raw: &RawWindow) -> Result<WindowSpec, String> {
    let width = raw.width.map_or(Ok(DEFAULT_WIDTH), |v| dimension("width", v))?;
    let height = raw.height.map_or(Ok(DEFAULT_HEIGHT), |v| dimension("height", v))?;
    let min_width = raw.min_width.map_or(Ok(DEFAULT_MIN_SIZE.min(width)), |v| dimension("minWidth", v))?;
    let min_height = raw.min_height.map_or(Ok(DEFAULT_MIN_SIZE.min(height)), |v| dimension("minHeight", v))?;
    if min_width > width { return Err(format!("'window.minWidth' ({min_width}) is larger than 'window.width' ({width})")); }
    if min_height > height { return Err(format!("'window.minHeight' ({min_height}) is larger than 'window.height' ({height})")); }
    Ok(WindowSpec { width, height, min_width, min_height, resizable: raw.resizable, transparent: raw.transparent })
}

/// Validates one setting: type, choices and the type of its default.
fn build_setting(s: RawSetting) -> Result<SettingDef, String> {
    let key = s.key;
    if key.trim().is_empty() { return Err("a setting has an empty 'key'".into()); }
    if s.label.trim().is_empty() { return Err(format!("setting '{key}' has an empty 'label'")); }
    let default = s.default.ok_or_else(|| format!("setting '{key}' has no 'default'"))?;
    let kind = match s.kind.as_str() {
        "string" => SettingKind::String,
        "number" => SettingKind::Number,
        "bool" => SettingKind::Bool,
        "choice" => match s.choices {
            Some(c) if !c.is_empty() => SettingKind::Choice(c),
            _ => return Err(format!("setting '{key}' is a 'choice' and needs a non-empty 'choices' list")),
        },
        other => return Err(format!("setting '{key}' has unknown type '{other}' (allowed: string, number, bool, choice)")),
    };
    check_default(&key, &kind, &default)?;
    Ok(SettingDef { key, kind, label: s.label, default })
}

/// Checks that the default value matches the setting type.
fn check_default(key: &str, kind: &SettingKind, default: &Value) -> Result<(), String> {
    let ok = match kind {
        SettingKind::String => default.is_string(),
        SettingKind::Number => default.is_number(),
        SettingKind::Bool => default.is_boolean(),
        SettingKind::Choice(choices) => default.as_str().is_some_and(|d| choices.iter().any(|c| c == d)),
    };
    if ok { return Ok(()); }
    let want = match kind {
        SettingKind::String => "a string".to_string(),
        SettingKind::Number => "a number".to_string(),
        SettingKind::Bool => "true or false".to_string(),
        SettingKind::Choice(c) => format!("one of {}", c.join(", ")),
    };
    Err(format!("setting '{key}': 'default' must be {want}, got {default}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_manifest_gets_the_defaults() {
        let m = parse(r#"{"name":"x"}"#).unwrap();
        assert_eq!(m.name, "x");
        assert_eq!(m.version, "");
        assert_eq!(m.description, "");
        assert_eq!(m.entry, "main.js");
        assert_eq!(m.window, WindowSpec { width: 300, height: 200, min_width: 50, min_height: 50, resizable: false, transparent: false });
        assert!(m.settings.is_empty());
        assert!(m.permissions.is_empty());
    }

    #[test]
    fn a_full_manifest_round_trips_every_field() {
        let m = parse(r#"{
            "name":"Clock","version":"1.2","description":"A clock","entry":"js/app.js","id":"ignored","future":1,
            "window":{"width":400,"height":250,"minWidth":100,"minHeight":80,"resizable":true,"transparent":true},
            "settings":[
              {"key":"title","type":"string","label":"Title","default":"hi"},
              {"key":"size","type":"number","label":"Size","default":2.5},
              {"key":"on","type":"bool","label":"On","default":true},
              {"key":"mode","type":"choice","label":"Mode","default":"b","choices":["a","b"]}
            ],
            "permissions":["net","fs","exec"]}"#).unwrap();
        assert_eq!((m.name.as_str(), m.version.as_str(), m.description.as_str(), m.entry.as_str()), ("Clock", "1.2", "A clock", "js/app.js"));
        assert_eq!(m.window, WindowSpec { width: 400, height: 250, min_width: 100, min_height: 80, resizable: true, transparent: true });
        assert_eq!(m.settings.len(), 4);
        assert_eq!(m.settings[0], SettingDef { key: "title".into(), kind: SettingKind::String, label: "Title".into(), default: Value::from("hi") });
        assert_eq!(m.settings[1].kind, SettingKind::Number);
        assert_eq!(m.settings[1].default, Value::from(2.5));
        assert_eq!(m.settings[2].kind, SettingKind::Bool);
        assert_eq!(m.settings[3].kind, SettingKind::Choice(vec!["a".into(), "b".into()]));
        assert_eq!(m.permissions, vec!["net", "fs", "exec"]);
    }

    #[test]
    fn a_choice_setting_needs_its_choices() {
        let missing = parse(r#"{"name":"x","settings":[{"key":"m","type":"choice","label":"M","default":"a"}]}"#).unwrap_err();
        assert!(missing.contains("'m'") && missing.contains("choices"), "{missing}");
        let empty = parse(r#"{"name":"x","settings":[{"key":"m","type":"choice","label":"M","default":"a","choices":[]}]}"#).unwrap_err();
        assert!(empty.contains("'m'") && empty.contains("choices"), "{empty}");
        let outside = parse(r#"{"name":"x","settings":[{"key":"m","type":"choice","label":"M","default":"z","choices":["a"]}]}"#).unwrap_err();
        assert!(outside.contains("'m'") && outside.contains("default"), "{outside}");
    }

    #[test]
    fn a_default_of_the_wrong_type_is_rejected() {
        for (kind, bad) in [("string", "1"), ("number", "\"1\""), ("bool", "\"true\"")] {
            let text = format!(r#"{{"name":"x","settings":[{{"key":"k","type":"{kind}","label":"K","default":{bad}}}]}}"#);
            let err = parse(&text).unwrap_err();
            assert!(err.contains("'k'") && err.contains("default"), "{kind}: {err}");
        }
        let none = parse(r#"{"name":"x","settings":[{"key":"k","type":"string","label":"K"}]}"#).unwrap_err();
        assert!(none.contains("'k'") && none.contains("default"), "{none}");
        let unknown = parse(r#"{"name":"x","settings":[{"key":"k","type":"color","label":"K","default":"r"}]}"#).unwrap_err();
        assert!(unknown.contains("color"), "{unknown}");
    }

    #[test]
    fn duplicate_setting_keys_are_rejected() {
        let err = parse(r#"{"name":"x","settings":[
            {"key":"k","type":"bool","label":"A","default":true},
            {"key":"k","type":"bool","label":"B","default":false}]}"#).unwrap_err();
        assert!(err.contains("duplicate") && err.contains("'k'"), "{err}");
    }

    #[test]
    fn an_entry_outside_the_folder_is_rejected() {
        for entry in ["../x.js", "/etc/x.js", "a/../../x.js", ""] {
            let err = parse(&format!(r#"{{"name":"x","entry":"{entry}"}}"#)).unwrap_err();
            assert!(err.contains("entry"), "{entry}: {err}");
        }
        assert!(parse(r#"{"name":"x","entry":"sub/a.js"}"#).is_ok());
    }

    #[test]
    fn an_empty_name_is_rejected() {
        assert!(parse(r#"{"name":""}"#).unwrap_err().contains("name"));
        assert!(parse(r#"{"name":"  "}"#).unwrap_err().contains("name"));
        assert!(parse(r#"{}"#).unwrap_err().contains("name"));
    }

    #[test]
    fn a_zero_size_window_is_rejected() {
        for w in [r#""width":0"#, r#""height":0"#, r#""width":-5"#, r#""minWidth":0"#, r#""minHeight":-1"#] {
            let err = parse(&format!(r#"{{"name":"x","window":{{{w}}}}}"#)).unwrap_err();
            assert!(err.contains("window."), "{w}: {err}");
        }
        let err = parse(r#"{"name":"x","window":{"width":100,"minWidth":200}}"#).unwrap_err();
        assert!(err.contains("minWidth"), "{err}");
    }

    #[test]
    fn unknown_permissions_are_rejected() {
        let err = parse(r#"{"name":"x","permissions":["net","netz"]}"#).unwrap_err();
        assert!(err.contains("netz"), "{err}");
    }

    #[test]
    fn bad_json_gives_an_error_with_the_position() {
        let err = parse("{\n  \"name\": \n").unwrap_err();
        assert!(err.contains("line 3") && err.contains("column"), "{err}");
    }
}
