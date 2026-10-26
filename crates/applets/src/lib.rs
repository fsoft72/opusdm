//! OpusDM applets: small JavaScript desktop tools, one folder each under
//! `~/.config/opusdm/applets/<id>/` with an `applet.json` manifest and a
//! `main.js` entry script.

pub mod canvas;
pub mod discovery;
pub mod instance;
pub mod manifest;
pub mod protocol;
pub mod runtime;
pub mod server;
pub mod state;
pub mod ui_tree;
pub mod widgets;
