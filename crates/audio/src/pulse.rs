//! The PulseAudio client. One thread owns the threaded mainloop and the
//! context (neither is `Send`); every other thread talks to it through jobs.

use crate::backend::Backend;
use crate::diff::{diff, AudioEvent};
use crate::error::AudioError;
use crate::filter;
use crate::model::{AudioDevice, Kind};
use crate::volume;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect::{SinkInfo, SourceInfo};
use libpulse_binding::context::subscribe::InterestMaskSet;
use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::mainloop::threaded::Mainloop;
use libpulse_binding::volume::{ChannelVolumes, Volume};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Name the client shows to the sound server.
const APP_NAME: &str = "opusdm-panel";
/// How many times to look at the connection state while connecting.
const CONNECT_STEPS: u32 = 100;
/// Pause between two looks, in milliseconds (100 x 50 ms = 5 s).
const CONNECT_STEP_MS: u64 = 50;
/// How long one server call may take.
const CALL_TIMEOUT_SECS: u64 = 5;
/// Quiet time after a burst of server events before the list is read again.
const COALESCE_MS: u64 = 100;
/// Channel count used when a device was not listed yet.
const DEFAULT_CHANNELS: u8 = 2;

/// Work for the thread that owns libpulse.
type Job = Box<dyn FnOnce(&mut Pulse) + Send>;

/// What the owner thread receives.
enum Msg {
    /// A request from `PulseBackend`.
    Job(Job),
    /// The server says a sink, a source or the defaults changed.
    Tick,
    /// The connection state changed; check whether it is still alive.
    StateChanged,
}

/// One item of a list answer, sent from the libpulse callback.
enum Item {
    Device(AudioDevice, u8),
    End,
    Failed,
}

/// The error for a call that got no answer.
fn timeout_error() -> AudioError {
    AudioError::ConnectionFailed("the sound server did not answer".to_string())
}

/// The error for a lost connection.
fn gone_error() -> AudioError {
    AudioError::ConnectionFailed("the sound server went away".to_string())
}

/// The libpulse objects, only ever touched by the owner thread.
struct Pulse {
    // Field order matters: the context must drop before the mainloop.
    ctx: Context,
    ml: Mainloop,
    /// Channel count of every device seen by the last `list`.
    channels: HashMap<(Kind, u32), u8>,
}

impl Drop for Pulse {
    /// Disconnect and stop the mainloop thread before the fields drop.
    fn drop(&mut self) {
        self.ml.lock();
        self.ctx.disconnect();
        self.ml.unlock();
        self.ml.stop();
    }
}

impl Pulse {
    /// Create the mainloop and the context and wait until the server is ready.
    /// `events` receives a `StateChanged` whenever the connection state moves.
    fn connect(events: &Sender<Msg>) -> Result<Self, AudioError> {
        let fail = |why: &str| AudioError::ConnectionFailed(why.to_string());
        let mut ml = Mainloop::new().ok_or_else(|| fail("cannot create the mainloop"))?;
        let mut ctx = Context::new(&ml, APP_NAME).ok_or_else(|| fail("cannot create the context"))?;
        let state_tx = events.clone();
        ctx.set_state_callback(Some(Box::new(move || {
            let _ = state_tx.send(Msg::StateChanged);
        })));
        ml.lock();
        let started = ctx
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|e| fail(&e.to_string().unwrap_or_default()))
            .and_then(|_| ml.start().map_err(|e| fail(&e.to_string().unwrap_or_default())));
        ml.unlock();
        started?;
        let mut pulse = Self { ctx, ml, channels: HashMap::new() };
        pulse.wait_ready()?;
        Ok(pulse)
    }

    /// The connection state, read under the mainloop lock.
    fn state(&mut self) -> State {
        self.ml.lock();
        let state = self.ctx.get_state();
        self.ml.unlock();
        state
    }

    /// Poll the connection state until it is ready, failed or out of time.
    fn wait_ready(&mut self) -> Result<(), AudioError> {
        for _ in 0..CONNECT_STEPS {
            match self.state() {
                State::Ready => return Ok(()),
                State::Failed | State::Terminated => return Err(gone_error()),
                _ => std::thread::sleep(Duration::from_millis(CONNECT_STEP_MS)),
            }
        }
        Err(timeout_error())
    }

    /// Fail when the connection is no longer usable.
    fn check_alive(&mut self) -> Result<(), AudioError> {
        match self.state() {
            State::Failed | State::Terminated => Err(gone_error()),
            _ => Ok(()),
        }
    }

    /// Ask for events about sinks, sources and the defaults; each one is a `Tick`.
    fn subscribe(&mut self, events: Sender<Msg>) {
        self.ml.lock();
        self.ctx.set_subscribe_callback(Some(Box::new(move |_, _, _| {
            let _ = events.send(Msg::Tick);
        })));
        self.ctx.subscribe(InterestMaskSet::SINK | InterestMaskSet::SOURCE | InterestMaskSet::SERVER, |_| {});
        self.ml.unlock();
    }

    /// The names of the default sink and the default source.
    fn defaults(&mut self) -> Result<(Option<String>, Option<String>), AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_server_info(move |s| {
            let _ = tx.send((
                s.default_sink_name.as_ref().map(|n| n.to_string()),
                s.default_source_name.as_ref().map(|n| n.to_string()),
            ));
        });
        self.ml.unlock();
        rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)).map_err(|_| timeout_error())
    }

    /// Every sink, with its channel count.
    fn list_sinks(&mut self, default: Option<String>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_sink_info_list(move |r| {
            let item = match r {
                ListResult::Item(i) => Item::Device(sink_device(i, default.as_deref()), i.volume.len()),
                ListResult::End => Item::End,
                ListResult::Error => Item::Failed,
            };
            let _ = tx.send(item);
        });
        self.ml.unlock();
        collect(&rx)
    }

    /// Every source that is not a monitor, with its channel count.
    fn list_sources(&mut self, default: Option<String>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_source_info_list(move |r| {
            let item = match r {
                ListResult::Item(i) => match source_device(i, default.as_deref()) {
                    Some(d) => Item::Device(d, i.volume.len()),
                    None => return,
                },
                ListResult::End => Item::End,
                ListResult::Error => Item::Failed,
            };
            let _ = tx.send(item);
        });
        self.ml.unlock();
        collect(&rx)
    }

    /// Outputs first, then inputs. Remembers each device's channel count.
    fn list(&mut self) -> Result<Vec<AudioDevice>, AudioError> {
        let (sink_default, source_default) = self.defaults()?;
        let mut all = self.list_sinks(sink_default)?;
        all.extend(self.list_sources(source_default)?);
        self.channels = all.iter().map(|(d, c)| (d.key(), *c)).collect();
        Ok(all.into_iter().map(|(d, _)| d).collect())
    }

    /// Set one volume on every channel of a device.
    fn set_volume(&mut self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        let channels = self.channels.get(&(kind, index)).copied().unwrap_or(DEFAULT_CHANNELS);
        let mut cv = ChannelVolumes::default();
        cv.set(channels, Volume(volume::to_raw(percent)));
        let (tx, rx) = mpsc::channel();
        let done = Box::new(move |ok: bool| {
            let _ = tx.send(ok);
        });
        self.ml.lock();
        let mut intro = self.ctx.introspect();
        let _ = match kind {
            Kind::Output => intro.set_sink_volume_by_index(index, &cv, Some(done)),
            Kind::Input => intro.set_source_volume_by_index(index, &cv, Some(done)),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }

    /// Mute or unmute a device.
    fn set_mute(&mut self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        let (tx, rx) = mpsc::channel();
        let done = Box::new(move |ok: bool| {
            let _ = tx.send(ok);
        });
        self.ml.lock();
        let mut intro = self.ctx.introspect();
        let _ = match kind {
            Kind::Output => intro.set_sink_mute_by_index(index, muted, Some(done)),
            Kind::Input => intro.set_source_mute_by_index(index, muted, Some(done)),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }

    /// Make the device called `name` the default of its kind.
    fn set_default(&mut self, kind: Kind, name: &str) -> Result<(), AudioError> {
        let (tx, rx) = mpsc::channel();
        let done = move |ok: bool| {
            let _ = tx.send(ok);
        };
        self.ml.lock();
        let _ = match kind {
            Kind::Output => self.ctx.set_default_sink(name, done),
            Kind::Input => self.ctx.set_default_source(name, done),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }
}

/// A sink as the panel model.
fn sink_device(i: &SinkInfo, default: Option<&str>) -> AudioDevice {
    let name = i.name.as_deref().unwrap_or("").to_string();
    let is_default = default == Some(name.as_str());
    AudioDevice {
        index: i.index,
        description: i.description.as_deref().unwrap_or("").to_string(),
        name,
        kind: Kind::Output,
        volume_percent: volume::from_raw(i.volume.avg().0),
        muted: i.mute,
        is_default,
    }
}

/// A source as the panel model; `None` for a monitor.
fn source_device(i: &SourceInfo, default: Option<&str>) -> Option<AudioDevice> {
    let name = i.name.as_deref().unwrap_or("").to_string();
    if !filter::is_input(&name) {
        return None;
    }
    let is_default = default == Some(name.as_str());
    Some(AudioDevice {
        index: i.index,
        description: i.description.as_deref().unwrap_or("").to_string(),
        name,
        kind: Kind::Input,
        volume_percent: volume::from_raw(i.volume.avg().0),
        muted: i.mute,
        is_default,
    })
}

/// Read list items until the end marker.
fn collect(rx: &Receiver<Item>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
    let mut out = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)) {
            Ok(Item::Device(d, channels)) => out.push((d, channels)),
            Ok(Item::End) => return Ok(out),
            Ok(Item::Failed) => return Err(AudioError::Rejected("the device list failed".to_string())),
            Err(_) => return Err(timeout_error()),
        }
    }
}

/// Wait for the success flag of a set call.
fn wait_ack(rx: &Receiver<bool>) -> Result<(), AudioError> {
    match rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AudioError::Rejected("the server said no".to_string())),
        Err(_) => Err(timeout_error()),
    }
}

/// The handle other threads use: every call becomes a job for the owner thread.
pub struct PulseBackend {
    tx: Mutex<Sender<Msg>>,
}

impl PulseBackend {
    /// Run `f` on the owner thread and wait for its answer.
    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Pulse) -> Result<T, AudioError> + Send + 'static,
    ) -> Result<T, AudioError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: Job = Box::new(move |p| {
            let _ = reply_tx.send(f(p));
        });
        self.tx.lock().unwrap().send(Msg::Job(job)).map_err(|_| gone_error())?;
        // Longer than one server call: `list` makes three of them.
        reply_rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS * 3)).map_err(|_| timeout_error())?
    }
}

impl Backend for PulseBackend {
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError> {
        self.call(|p| p.list())
    }

    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        self.call(move |p| p.set_volume(kind, index, percent))
    }

    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        self.call(move |p| p.set_mute(kind, index, muted))
    }

    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError> {
        let name = name.to_string();
        self.call(move |p| p.set_default(kind, &name))
    }
}

/// A connection to the sound server, owned by the thread that created it.
pub struct Session {
    pulse: Pulse,
    rx: Receiver<Msg>,
    backend: Arc<PulseBackend>,
}

impl Session {
    /// Connect and subscribe to server events.
    pub fn connect() -> Result<Self, AudioError> {
        let (tx, rx) = mpsc::channel();
        let mut pulse = Pulse::connect(&tx)?;
        pulse.subscribe(tx.clone());
        Ok(Self { pulse, rx, backend: Arc::new(PulseBackend { tx: Mutex::new(tx) }) })
    }

    /// The handle for other threads; its jobs run while `follow` is running.
    pub fn backend(&self) -> Arc<PulseBackend> {
        self.backend.clone()
    }

    /// Read the devices now.
    pub fn list(&mut self) -> Result<Vec<AudioDevice>, AudioError> {
        self.pulse.list()
    }

    /// Serve jobs and report changes against `known` until `on_event` returns
    /// false (`Ok`) or the connection is lost (`Err`).
    pub fn follow(
        &mut self,
        mut known: Vec<AudioDevice>,
        mut on_event: impl FnMut(AudioEvent) -> bool,
    ) -> Result<(), AudioError> {
        loop {
            let msg = self.rx.recv().map_err(|_| gone_error())?;
            let mut changed = self.dispatch(msg)?;
            // A burst of events (one volume drag) is one re-read.
            while changed {
                match self.rx.recv_timeout(Duration::from_millis(COALESCE_MS)) {
                    Ok(next) => changed |= self.dispatch(next)?,
                    Err(_) => break,
                }
            }
            if !changed {
                continue;
            }
            let now = self.pulse.list()?;
            for event in diff(&known, &now) {
                if !on_event(event) {
                    return Ok(());
                }
            }
            known = now;
        }
    }

    /// Handle one message; true when it was a server event.
    fn dispatch(&mut self, msg: Msg) -> Result<bool, AudioError> {
        match msg {
            Msg::Job(job) => {
                job(&mut self.pulse);
                Ok(false)
            }
            Msg::StateChanged => self.pulse.check_alive().map(|_| false),
            Msg::Tick => Ok(true),
        }
    }
}
