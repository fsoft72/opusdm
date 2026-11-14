//! Engine tests on a generated WAV file (no display, no sound card). One test
//! function runs every scenario in turn: they share the default main context,
//! which only one thread can own. Skipped when GStreamer cannot start.

use opusdm_media::{Engine, EngineOptions, Event, MediaError, PlayState, VideoSink};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

const RATE: u32 = 8000;
const WAIT: Duration = Duration::from_secs(8);

/// Write `secs` seconds of 8-bit mono silence as a WAV file.
fn write_wav(path: &Path, secs: u32) {
    let data = vec![128u8; (RATE * secs) as usize];
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes()); // byte rate
    out.extend_from_slice(&1u16.to_le_bytes()); // block align
    out.extend_from_slice(&8u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

/// Write `secs` seconds of silence as a WAV file with a `LIST INFO` chunk
/// holding the `(id, text)` tags (`INAM` title, `IART` artist).
fn write_tagged_wav(path: &Path, secs: u32, tags: &[(&[u8; 4], &str)]) {
    write_wav(path, secs);
    let mut info = b"INFO".to_vec();
    for (id, text) in tags {
        let mut value = text.as_bytes().to_vec();
        value.push(0);
        if value.len() % 2 == 1 { value.push(0); }
        info.extend_from_slice(*id);
        info.extend_from_slice(&(value.len() as u32).to_le_bytes());
        info.extend_from_slice(&value);
    }
    let mut out = std::fs::read(path).unwrap();
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&(info.len() as u32).to_le_bytes());
    out.extend_from_slice(&info);
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    std::fs::write(path, out).unwrap();
}

/// An engine plus the events it has produced.
struct Rig {
    engine: Engine,
    events: Rc<RefCell<Vec<Event>>>,
}

fn rig(looping: bool) -> Rig {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let options = EngineOptions { audio: true, looping, video_sink: VideoSink::Discard };
    let engine = Engine::new(options, move |e| sink.borrow_mut().push(e)).unwrap();
    Rig { engine, events }
}

impl Rig {
    /// Run the main loop until an event matches, or fail after `WAIT`.
    fn wait_for(&self, what: &str, pred: impl Fn(&Event) -> bool) {
        let ctx = gstreamer::glib::MainContext::default();
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.events.borrow().iter().any(&pred) { return; }
            ctx.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timeout waiting for {what}; events: {:?}", self.events.borrow());
    }

    fn clear(&self) {
        self.events.borrow_mut().clear();
    }

    fn count(&self, pred: impl Fn(&Event) -> bool) -> usize {
        self.events.borrow().iter().filter(|e| pred(e)).count()
    }
}

#[test]
fn engine_scenarios() {
    if gstreamer::init().is_err() {
        eprintln!("skipped: GStreamer is not available");
        return;
    }
    let ctx = gstreamer::glib::MainContext::default();
    let _owner = ctx.acquire().expect("main context");
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("two seconds.wav");
    write_wav(&wav, 2);
    let wav = wav.to_str().unwrap();

    // Open prerolls paused and reports the length.
    let r = rig(false);
    r.engine.open(wav).unwrap();
    r.wait_for("paused state", |e| *e == Event::State(PlayState::Paused));
    r.wait_for("duration", |e| matches!(e, Event::Duration(d) if (1_900_000_000..=2_100_000_000).contains(d)));
    assert!(r.engine.duration().is_some());

    // Seek lands near the target and reports it.
    r.clear();
    r.engine.seek(1_000_000_000, true).unwrap();
    r.wait_for("position after seek", |e| matches!(e, Event::Position(p) if (900_000_000..=1_100_000_000).contains(p)));

    // Play runs to the end, then rewinds and pauses.
    r.clear();
    r.engine.set_volume(0.5);
    r.engine.set_muted(true);
    r.engine.play().unwrap();
    r.wait_for("playing", |e| *e == Event::State(PlayState::Playing));
    r.wait_for("end of stream", |e| *e == Event::Eos);
    r.wait_for("paused after the end", |e| *e == Event::State(PlayState::Paused));
    r.engine.pause().unwrap();
    drop(r);

    // Looping keeps playing past the end: a second pass produces a second Eos.
    let r = rig(true);
    r.engine.open(wav).unwrap();
    r.engine.play().unwrap();
    r.wait_for("first end", |e| *e == Event::Eos);
    r.wait_for("second end", |_| r.count(|e| *e == Event::Eos) >= 2);
    assert_eq!(r.count(|e| *e == Event::State(PlayState::Paused)), 1, "only the preroll pause: looping never pauses");
    drop(r);

    // A second file on the same engine starts with empty tags: the artist of
    // the first one must not stick to it (the Sidecar reuses one engine).
    let first = dir.path().join("first.wav");
    let second = dir.path().join("second.wav");
    write_tagged_wav(&first, 1, &[(b"INAM", "First"), (b"IART", "Artist A")]);
    write_tagged_wav(&second, 1, &[(b"INAM", "Second")]);
    let r = rig(false);
    r.engine.open(first.to_str().unwrap()).unwrap();
    r.wait_for("tags of the first file", |e| matches!(e, Event::Tags(t) if t.title.as_deref() == Some("First") && t.artist.as_deref() == Some("Artist A")));
    r.clear();
    r.engine.open(second.to_str().unwrap()).unwrap();
    r.wait_for("tags of the second file", |e| matches!(e, Event::Tags(t) if t.title.as_deref() == Some("Second")));
    let stale = r.count(|e| matches!(e, Event::Tags(t) if t.artist.is_some() || t.title.as_deref() == Some("First")));
    assert_eq!(stale, 0, "tags of the first file reached the second: {:?}", r.events.borrow());
    drop(r);

    // A file that is not media is reported as an error event.
    let bad = dir.path().join("bad.wav");
    std::fs::write(&bad, b"this is not audio at all").unwrap();
    let r = rig(false);
    r.engine.open(bad.to_str().unwrap()).unwrap();
    r.wait_for("error event", |e| matches!(e, Event::Error(_)));
    drop(r);

    // A missing file too.
    let r = rig(false);
    r.engine.open(dir.path().join("nope.wav").to_str().unwrap()).unwrap();
    r.wait_for("error for a missing file", |e| matches!(e, Event::Error(MediaError::Pipeline { .. } | MediaError::Unsupported(_))));
}
