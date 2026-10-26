//! Library face of opusdm-hub so integration tests can drive the server
//! layer without the GTK toolbar.

pub mod activation_follow;
pub mod applet_host;
pub mod bg_sequence;
pub mod channels;
pub mod children;
pub mod compositor;
pub mod config_cache;
pub mod desktop_diff;
pub mod desktop_files;
pub mod file_manager;
pub mod folder_handler;
pub mod fullscreen;
pub mod io_worker;
pub mod desktop_settings;
pub mod functions;
pub mod jobs;
pub mod launch_focus;
pub mod leftout;
pub mod roles;
pub mod server;
pub mod remote;
pub mod startup_apps;
pub mod startup_config;
pub mod tray_start;
pub mod tray_start_wl;
pub mod settings;
pub mod snap;
pub mod global_keys;
pub mod global_keys_wl;
pub mod script_events;
pub mod scripting;
