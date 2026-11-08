//! Entry point of `opusdm-dock`, the dock process started and supervised by
//! the hub (it also runs on its own, without a hub). See `standalone.rs`.

fn main() -> gtk4::glib::ExitCode {
    opusdm_dock::standalone::run()
}
