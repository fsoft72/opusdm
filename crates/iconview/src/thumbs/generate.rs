//! Turn a source file into PNG thumbnail bytes. Images decode in-process with
//! gdk-pixbuf (thread-safe, no display needed); PDFs and videos shell out to
//! optional tools with a hard timeout. Every path returns PNG bytes or an
//! error; the caller falls back to the glyph on error.

use crate::thumbs::ThumbKind;
use gtk4::gdk_pixbuf;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Longest a generator subprocess may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Which optional tools were found on `$PATH`.
pub struct ToolReport {
    pub pdf: bool,
    pub video: bool,
}

#[derive(Clone, Copy)]
enum PdfTool {
    Pdftoppm,
    Pdftocairo,
}

#[derive(Clone, Copy)]
enum VideoTool {
    FfmpegThumbnailer,
    Ffmpeg,
}

struct Tools {
    pdf: Option<PdfTool>,
    video: Option<VideoTool>,
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| Tools {
        pdf: if in_path("pdftoppm") {
            Some(PdfTool::Pdftoppm)
        } else if in_path("pdftocairo") {
            Some(PdfTool::Pdftocairo)
        } else {
            None
        },
        video: if in_path("ffmpegthumbnailer") {
            Some(VideoTool::FfmpegThumbnailer)
        } else if in_path("ffmpeg") {
            Some(VideoTool::Ffmpeg)
        } else {
            None
        },
    })
}

/// Is `name` an executable file on `$PATH`?
fn in_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let cand = dir.join(name);
        std::fs::metadata(&cand).map(|m| m.is_file()).unwrap_or(false)
    })
}

/// Diagnostics: which optional tools are present.
pub fn tools_available() -> ToolReport {
    let t = tools();
    ToolReport { pdf: t.pdf.is_some(), video: t.video.is_some() }
}

/// PNG thumbnail bytes for `path`, at most `px` on the long edge.
pub fn generate(path: &Path, kind: ThumbKind, px: i32) -> io::Result<Vec<u8>> {
    match kind {
        ThumbKind::Image => generate_image(path, px),
        ThumbKind::Pdf => generate_pdf(path, px),
        ThumbKind::Video => generate_video(path, px),
    }
}

fn generate_image(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let pb = gdk_pixbuf::Pixbuf::from_file_at_scale(path, px, px, true)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let bytes = pb
        .save_to_bufferv("png", &[])
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes)
}

fn generate_pdf(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let tool = tools().pdf.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no pdf tool"))?;
    let out = match tool {
        PdfTool::Pdftoppm => run_with_timeout(
            Command::new("pdftoppm")
                .args(["-png", "-singlefile", "-scale-to"])
                .arg(px.to_string())
                .arg(path),
            TIMEOUT,
        )?,
        PdfTool::Pdftocairo => run_with_timeout(
            Command::new("pdftocairo")
                .args(["-png", "-singlefile", "-scale-to"])
                .arg(px.to_string())
                .arg(path)
                .arg("-"),
            TIMEOUT,
        )?,
    };
    non_empty_png(out)
}

fn generate_video(path: &Path, px: i32) -> io::Result<Vec<u8>> {
    let tool =
        tools().video.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no video tool"))?;
    let out = match tool {
        VideoTool::FfmpegThumbnailer => run_with_timeout(
            Command::new("ffmpegthumbnailer")
                .arg("-i")
                .arg(path)
                .args(["-o", "-", "-c", "png", "-s"])
                .arg(px.to_string()),
            TIMEOUT,
        )?,
        // No `-ss`: seeking past the end of a very short clip yields no frame.
        // The first frame is always a keyframe and good enough for an icon.
        VideoTool::Ffmpeg => run_with_timeout(
            Command::new("ffmpeg")
                .args(["-y", "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-f", "image2", "-c:v", "png", "-vf"])
                .arg(format!("scale={px}:-1"))
                .arg("-"),
            TIMEOUT,
        )?,
    };
    non_empty_png(out)
}

fn non_empty_png(bytes: Vec<u8>) -> io::Result<Vec<u8>> {
    if bytes.len() > 8 && bytes[1..4] == *b"PNG" {
        Ok(bytes)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "generator produced no PNG"))
    }
}

/// Run `cmd` capturing stdout, killing it after `timeout`.
///
/// Stdout is drained on a helper thread while the child runs: a PNG larger
/// than the pipe buffer (64 KiB) would otherwise block the child on write
/// until the timeout killed it.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err(io::Error::other("generator stdout was not captured"));
    };
    let reader = std::thread::Builder::new().name("thumb-stdout".into()).spawn(move || {
        let mut out = Vec::new();
        io::Read::read_to_end(&mut stdout, &mut out).map(|_| out)
    })?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            // Killing closes the pipe, so the reader thread ends too.
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "generator timed out"));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let out = reader.join().map_err(|_| io::Error::other("generator stdout reader panicked"))??;
    if !status.success() {
        return Err(io::Error::other("generator exited non-zero"));
    }
    Ok(out)
}

impl ThumbKind {
    /// The thumbnail kind for a lowercase file extension, or `None` when the
    /// extension is not previewable.
    pub fn from_ext(ext: Option<&str>) -> Option<ThumbKind> {
        let ext = ext?;
        const IMAGE: &[&str] =
            &["png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "ico", "svg"];
        const VIDEO: &[&str] =
            &["mp4", "mkv", "avi", "mov", "webm", "m4v", "mpg", "mpeg", "wmv", "flv"];
        if ext == "pdf" {
            return Some(ThumbKind::Pdf);
        }
        if IMAGE.contains(&ext) {
            return Some(ThumbKind::Image);
        }
        if VIDEO.contains(&ext) {
            return Some(ThumbKind::Video);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_ext_classifies() {
        assert_eq!(ThumbKind::from_ext(Some("png")), Some(ThumbKind::Image));
        assert_eq!(ThumbKind::from_ext(Some("pdf")), Some(ThumbKind::Pdf));
        assert_eq!(ThumbKind::from_ext(Some("mkv")), Some(ThumbKind::Video));
        assert_eq!(ThumbKind::from_ext(Some("rs")), None);
        assert_eq!(ThumbKind::from_ext(None), None);
    }

    #[test]
    fn image_generation_produces_a_png() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("in.png");
        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 2, 2).unwrap();
        pb.savev(&src, "png", &[]).unwrap();

        let out = generate(&src, ThumbKind::Image, 64).unwrap();
        assert!(out.len() > 8 && &out[1..4] == b"PNG");
    }

    #[test]
    fn missing_tools_error_rather_than_panic() {
        let td = tempfile::tempdir().unwrap();
        let fake_pdf = td.path().join("x.pdf");
        std::fs::write(&fake_pdf, b"%PDF-1.4\n").unwrap();
        let _ = generate(&fake_pdf, ThumbKind::Pdf, 128);
    }

    #[test]
    fn output_larger_than_the_pipe_buffer_is_read_while_running() {
        // 256 KiB is well past the 64 KiB pipe buffer: a child writing it
        // blocks until someone reads, so reading only after exit times out.
        const BYTES: usize = 256 * 1024;
        let out = run_with_timeout(
            Command::new("head").args(["-c", &BYTES.to_string(), "/dev/zero"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(out.len(), BYTES);
    }

    #[test]
    fn a_hung_child_is_killed_after_the_timeout() {
        let start = Instant::now();
        let err = run_with_timeout(Command::new("sleep").arg("30"), Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    /// Real PDF and video generation. Ignored by default because it needs
    /// `pdftoppm`/`pdftocairo` and `ffmpegthumbnailer`/`ffmpeg` on `$PATH`.
    /// Run with `cargo test -p opusdm-iconview -- --ignored real_generators`.
    #[test]
    #[ignore]
    fn real_generators_produce_pngs() {
        let td = tempfile::tempdir().unwrap();

        if tools_available().pdf {
            // A 1-page PDF made from a solid-colour image via ImageMagick.
            let img = td.path().join("seed.png");
            let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 8, 8).unwrap();
            pb.savev(&img, "png", &[]).unwrap();
            let pdf = td.path().join("doc.pdf");
            let ok = std::process::Command::new("convert")
                .arg(&img)
                .arg(&pdf)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                let out = generate(&pdf, ThumbKind::Pdf, 128).unwrap();
                assert!(out.len() > 8 && &out[1..4] == b"PNG");
            }
        }

        if tools_available().video {
            let mp4 = td.path().join("clip.mp4");
            let ok = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "color=c=red:s=64x64:d=1", "-pix_fmt", "yuv420p"])
                .arg(&mp4)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                let out = generate(&mp4, ThumbKind::Video, 128).unwrap();
                assert!(out.len() > 8 && &out[1..4] == b"PNG");
            }
        }
    }
}
