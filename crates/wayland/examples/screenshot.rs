//! Save every output as a binary PPM file (`<name>.ppm` in the current
//! directory). A manual test tool:
//! `WAYLAND_DISPLAY=wayland-1 cargo run -p opusdm-wayland --example screenshot`.

use std::io::Write;

fn main() {
    let images = match opusdm_wayland::screencopy::capture_outputs() {
        Ok(images) => images,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    for image in images {
        let path = format!("{}.ppm", image.name);
        let mut file = std::fs::File::create(&path).expect("create the output file");
        write!(file, "P6\n{} {}\n255\n", image.width, image.height).expect("write the header");
        let rgb: Vec<u8> = image.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        file.write_all(&rgb).expect("write the pixels");
        println!("{path}: {}x{} at {},{}", image.width, image.height, image.x, image.y);
    }
}
