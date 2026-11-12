//! `opusdm-keyring-watch`: checks the Secret Service now and then and shows a
//! tray icon plus a notification when it is missing or locked.

pub mod health;
pub mod app;
pub mod ask;
pub mod gnome;
pub mod notify;
pub mod probe;
pub mod selftest;
pub mod tray;
