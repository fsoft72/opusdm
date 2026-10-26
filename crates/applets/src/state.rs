//! Per-applet runtime state: enabled flag, window geometry and saved settings.

use crate::manifest::{SettingDef, SettingKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// What OpusDM remembers about one applet between runs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppletState {
    pub enabled: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub settings: Map<String, Value>,
}

/// Path of the state file of applet `id` under `base`.
fn state_path_in(base: &Path, id: &str) -> PathBuf {
    base.join(format!("{id}.json"))
}

/// Load the state of `id` from `base`; a missing or corrupt file gives the default.
pub fn load_in(base: &Path, id: &str) -> AppletState {
    opusdm_ipc::settings::load_json(&state_path_in(base, id), APP_TAG, "applet state")
}

/// Load the state of `id` from `base` before changing and saving it: a
/// missing file is the default state and a malformed one is backed up and
/// gives the default (as [`load_in`]), but a file that exists and cannot
/// be read is an `Err`, so the caller does not write defaults over it.
pub fn read_in(base: &Path, id: &str) -> Result<AppletState, String> {
    let path = state_path_in(base, id);
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppletState::default()),
        Err(e) => Err(format!("applet {id}: cannot read its state file {}: {e}", path.display())),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(state) => Ok(state),
            Err(_) => Ok(load_in(base, id)),
        },
    }
}

/// Save the state of `id` under `base` (atomic, creates the directory).
pub fn save_in(base: &Path, id: &str, state: &AppletState) -> std::io::Result<()> {
    opusdm_ipc::settings::save_json(&state_path_in(base, id), state)
}

/// Directory holding all the applet state files.
pub fn states_dir() -> PathBuf {
    opusdm_ipc::settings::state_dir().join("applets")
}

/// Path of the state file of applet `id`.
pub fn state_path(id: &str) -> PathBuf {
    state_path_in(&states_dir(), id)
}

/// Load the state of applet `id`; a new applet is disabled.
pub fn load(id: &str) -> AppletState {
    load_in(&states_dir(), id)
}

/// Save the state of applet `id` (atomic, creates the directory).
pub fn save(id: &str, state: &AppletState) -> std::io::Result<()> {
    save_in(&states_dir(), id, state)
}

/// The value of every declared setting: the saved one when its type matches
/// the declared kind, else the manifest default. Saved keys that are not
/// declared are dropped from the result (they stay in the file).
pub fn effective_settings(defs: &[SettingDef], state: &AppletState) -> Map<String, Value> {
    let mut out = Map::new();
    for def in defs {
        let saved = state
            .settings
            .get(&def.key)
            .filter(|v| _matches_kind(&def.kind, v));
        out.insert(def.key.clone(), saved.unwrap_or(&def.default).clone());
    }
    out
}

/// True when `value` has the JSON type that a setting of `kind` expects.
fn _matches_kind(kind: &SettingKind, value: &Value) -> bool {
    match kind {
        SettingKind::String => value.is_string(),
        SettingKind::Number => value.is_number(),
        SettingKind::Bool => value.is_boolean(),
        SettingKind::Choice(choices) => value
            .as_str()
            .is_some_and(|s| choices.iter().any(|c| c == s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(key: &str, kind: SettingKind, default: Value) -> SettingDef {
        SettingDef {
            key: key.into(),
            kind,
            label: key.into(),
            default,
        }
    }

    #[test]
    fn a_new_applet_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let st = load_in(dir.path(), "clock");
        assert!(!st.enabled);
        assert_eq!(st, AppletState::default());
    }

    #[test]
    fn state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppletState {
            enabled: true,
            x: Some(10),
            y: Some(-5),
            width: Some(320),
            height: Some(240),
            ..Default::default()
        };
        st.settings.insert("color".into(), json!("red"));
        save_in(dir.path(), "clock", &st).unwrap();
        assert!(state_path_in(dir.path(), "clock").exists());
        assert_eq!(load_in(dir.path(), "clock"), st);
    }

    #[test]
    fn a_corrupt_state_file_is_backed_up_and_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path_in(dir.path(), "clock");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_in(dir.path(), "clock"), AppletState::default());
        assert!(path.with_file_name("clock.json.bak").exists());
    }

    #[test]
    fn read_in_refuses_an_unreadable_file_but_not_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_in(dir.path(), "clock"), Ok(AppletState::default()));
        let st = AppletState { enabled: true, x: Some(3), ..Default::default() };
        save_in(dir.path(), "clock", &st).unwrap();
        assert_eq!(read_in(dir.path(), "clock"), Ok(st));
        std::fs::create_dir(state_path_in(dir.path(), "busy")).unwrap();
        let err = read_in(dir.path(), "busy").unwrap_err();
        assert!(err.contains("busy"), "{err}");
        std::fs::write(state_path_in(dir.path(), "bad"), "{ nope").unwrap();
        assert_eq!(read_in(dir.path(), "bad"), Ok(AppletState::default()), "malformed: backed up, default");
        assert!(dir.path().join("bad.json.bak").exists());
    }

    #[test]
    fn effective_settings_falls_back_on_the_default_for_a_wrong_type() {
        let defs = vec![
            def("name", SettingKind::String, json!("x")),
            def("size", SettingKind::Number, json!(3)),
            def("on", SettingKind::Bool, json!(true)),
            def(
                "mode",
                SettingKind::Choice(vec!["a".into(), "b".into()]),
                json!("a"),
            ),
            def("other", SettingKind::Number, json!(1.5)),
            def("ok", SettingKind::Number, json!(0)),
        ];
        let mut st = AppletState::default();
        st.settings.insert("name".into(), json!(5));
        st.settings.insert("size".into(), json!("big"));
        st.settings.insert("on".into(), json!("yes"));
        st.settings.insert("mode".into(), json!("zzz"));
        st.settings.insert("ok".into(), json!(2.5));
        st.settings.insert("stale".into(), json!(1));
        let eff = effective_settings(&defs, &st);
        assert_eq!(eff["name"], json!("x"));
        assert_eq!(eff["size"], json!(3));
        assert_eq!(eff["on"], json!(true));
        assert_eq!(eff["mode"], json!("a"));
        assert_eq!(eff["other"], json!(1.5));
        assert_eq!(eff["ok"], json!(2.5));
        assert!(!eff.contains_key("stale"));
        let mut good = AppletState::default();
        good.settings.insert("mode".into(), json!("b"));
        good.settings.insert("on".into(), json!(false));
        let eff = effective_settings(&defs, &good);
        assert_eq!(eff["mode"], json!("b"));
        assert_eq!(eff["on"], json!(false));
    }
}
