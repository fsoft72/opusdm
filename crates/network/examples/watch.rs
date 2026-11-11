//! Follows NetworkManager with the tray watcher and prints a line for every
//! message, with the seconds since the start (read only). Useful to check
//! that a change made by hand (for example with `nmcli`) reaches the tray.

use opusdm_network::{watch, WatchMsg};
use std::time::Instant;

fn main() {
    let start = Instant::now();
    let rx = watch::spawn();
    while let Ok(msg) = rx.recv_blocking() {
        let at = start.elapsed().as_secs_f32();
        match msg {
            WatchMsg::Backend(_) => println!("{at:7.2} backend"),
            WatchMsg::Unavailable(why) => println!("{at:7.2} unavailable: {why}"),
            WatchMsg::State(s) => {
                let active = s.active_ap().map(|a| a.ssid.clone()).unwrap_or_else(|| "-".to_string());
                println!("{at:7.2} state: eth {} wifi {} busy {} active {active} aps {}", s.eth_connected, s.wifi_enabled, s.wifi_busy, s.aps.len());
            }
        }
    }
}
