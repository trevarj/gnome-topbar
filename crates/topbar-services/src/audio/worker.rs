//! The PulseAudio thread.
//!
//! `libpulse` wants a mainloop of its own and hands everything back through
//! callbacks, which is the one shape that cannot be expressed as a tokio task.
//! So it gets a real thread: a threaded mainloop on one side, two channels on
//! the other. Commands arrive on a `std::sync::mpsc` the thread blocks on;
//! snapshots leave on a `tokio::sync::mpsc` the owning task reads. Nothing in
//! this file knows what a widget is, and nothing outside it touches `libpulse`.
//!
//! The v1 plumbing this is adapted from is proven against real hardware, and
//! two of its lessons are kept verbatim: a sink reporting zero channels must
//! never be sent a volume (PulseAudio asserts and takes the process with it),
//! and every introspection callback runs with the mainloop already locked, so
//! locking it again from inside one deadlocks.
//!
//! What is *not* kept is v1's fetch graph — a dozen entry points, half of them
//! duplicated in an "inner" form for the locked case. Here a subscription event
//! refetches the one list it concerns, and the default device's volume is read
//! out of that list. One call per event, one code path per list.
//!
//! Losing the server is a normal state, not an error: the thread publishes
//! "unavailable", backs off, and connects again, so a `systemctl --user restart
//! pipewire` costs the panel a second of greyed-out controls and nothing else.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::{Facility, InterestMaskSet};
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::def::PortAvailable;
use pulse::mainloop::threaded::Mainloop;
use pulse::proplist::Proplist;
use pulse::volume::ChannelVolumes;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::audio::model::DeviceView;
use crate::audio::volume;

/// How the panel introduces itself to the sound server.
const APP_NAME: &str = "topbar";
/// The context name, which is what `pactl list clients` shows.
const CONTEXT_NAME: &str = "topbar-audio";
/// The application id, matching the panel's own.
const APP_ID: &str = "io.github.trevarj.topbar";

/// How long to wait for a fresh context to become ready.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the connect loop looks at the context state while waiting.
const CONNECT_POLL: Duration = Duration::from_millis(10);
/// The first wait after a failed connection.
const BACKOFF_START: Duration = Duration::from_millis(500);
/// The longest the thread ever waits between attempts.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A safety net behind the context state callback.
///
/// Losing the server is meant to arrive as a callback; this is how long the
/// command loop is willing to sit idle before checking for itself anyway. It
/// reads an in-process field, not the socket, so its cost is a wake-up.
const LIVENESS_CHECK: Duration = Duration::from_secs(2);

/// What the owning task asks the thread to do.
#[derive(Debug)]
pub(crate) enum Request {
    /// Set the default sink's volume to a percentage, already clamped.
    SetSinkVolume(u32),
    /// Mute or unmute the default sink.
    SetSinkMuted(bool),
    /// Flip the default sink's mute.
    ToggleSinkMuted,
    /// Set the default source's volume to a percentage, already clamped.
    SetSourceVolume(u32),
    /// Mute or unmute the default source.
    SetSourceMuted(bool),
    /// Flip the default source's mute.
    ToggleSourceMuted,
    /// Make a sink the default one.
    SetDefaultSink(String),
    /// Make a source the default one.
    SetDefaultSource(String),
    /// Read everything again.
    Refresh,
    /// The context changed state; find out whether it is still alive.
    CheckContext,
    /// Stop, disconnect, and let the thread end.
    Shutdown,
}

/// What the thread reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Report {
    /// A complete reading of the server's state.
    Snapshot(Box<Reading>),
    /// There is no server to read.
    Unavailable,
}

/// One complete reading, before the owning task attributes its changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Reading {
    pub(crate) sinks: Vec<DeviceView>,
    pub(crate) sources: Vec<DeviceView>,
    pub(crate) default_sink: Option<String>,
    pub(crate) default_source: Option<String>,
    pub(crate) sink_volume_pct: u32,
    pub(crate) sink_muted: bool,
    pub(crate) sink_controllable: bool,
    pub(crate) source_volume_pct: u32,
    pub(crate) source_muted: bool,
    pub(crate) source_controllable: bool,
    pub(crate) source_in_use: bool,
}

/// One device as the thread tracks it, which is more than a widget needs.
#[derive(Debug, Clone, Default)]
struct Device {
    view: DeviceView,
    index: u32,
    channels: u8,
    volume_pct: u32,
    muted: bool,
    controllable: bool,
}

/// Everything the thread has read, shared with the introspection callbacks.
#[derive(Debug, Default)]
struct State {
    default_sink: Option<String>,
    default_source: Option<String>,
    sinks: Vec<Device>,
    sources: Vec<Device>,
    /// Indexes of the non-monitor sources, so a recording client on a monitor
    /// (a screen recorder taking system audio) does not light the microphone
    /// dot.
    input_indexes: HashSet<u32>,
    /// Recording clients that are neither corked nor muted.
    recording: HashSet<u32>,
}

impl State {
    /// The default sink, by name.
    fn sink(&self) -> Option<&Device> {
        let name = self.default_sink.as_deref()?;
        self.sinks.iter().find(|device| device.view.id == name)
    }

    /// The default source, by name.
    fn source(&self) -> Option<&Device> {
        let name = self.default_source.as_deref()?;
        self.sources.iter().find(|device| device.view.id == name)
    }

    /// Build a reading of everything worth publishing.
    fn reading(&self) -> Reading {
        let sink = self.sink();
        let source = self.source();
        Reading {
            sinks: self.views(&self.sinks, self.default_sink.as_deref()),
            sources: self.views(&self.sources, self.default_source.as_deref()),
            default_sink: self.default_sink.clone(),
            default_source: self.default_source.clone(),
            sink_volume_pct: sink.map_or(0, |device| device.volume_pct),
            sink_muted: sink.is_some_and(|device| device.muted),
            sink_controllable: sink.is_some_and(|device| device.controllable),
            source_volume_pct: source.map_or(0, |device| device.volume_pct),
            source_muted: source.is_some_and(|device| device.muted),
            source_controllable: source.is_some_and(|device| device.controllable),
            source_in_use: !self.recording.is_empty(),
        }
    }

    /// The published view of a device list, with the default one marked.
    fn views(&self, devices: &[Device], default: Option<&str>) -> Vec<DeviceView> {
        devices
            .iter()
            .map(|device| DeviceView {
                is_default: default == Some(device.view.id.as_str()),
                ..device.view.clone()
            })
            .collect()
    }
}

/// Lock through a poisoned mutex rather than around it.
///
/// A panic inside an introspection callback must not take the audio service
/// down with it: the state it was holding is plain data, and the next reading
/// overwrites it wholesale.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run the PulseAudio thread until it is told to stop.
///
/// `inject` is the thread's own command sender, which the context state
/// callback uses to wake the command loop from inside `libpulse`.
pub(crate) fn run(
    commands: Receiver<Request>,
    inject: Sender<Request>,
    reports: UnboundedSender<Report>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        match session(&commands, &inject, &reports) {
            Outcome::Shutdown => break,
            Outcome::Lost => {
                let _ = reports.send(Report::Unavailable);
                info!("lost the sound server; reconnecting in {backoff:?}");
                // Draining the queue during the wait would drop commands the
                // panel is still entitled to have applied once we are back, so
                // the wait is a plain sleep and the queue keeps its order.
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
    debug!("the audio thread has stopped");
}

/// How one connection ended.
enum Outcome {
    /// The panel is shutting down.
    Shutdown,
    /// The server went away, or would not answer.
    Lost,
}

/// One connection to the sound server, from connect to disconnect.
fn session(
    commands: &Receiver<Request>,
    inject: &Sender<Request>,
    reports: &UnboundedSender<Report>,
) -> Outcome {
    let Some(mainloop) = Mainloop::new() else {
        warn!("could not create a PulseAudio mainloop");
        return Outcome::Lost;
    };

    let mut proplist = match Proplist::new() {
        Some(proplist) => proplist,
        None => {
            warn!("could not create a PulseAudio proplist");
            return Outcome::Lost;
        }
    };
    let _ = proplist.set_str(pulse::proplist::properties::APPLICATION_NAME, APP_NAME);
    let _ = proplist.set_str(pulse::proplist::properties::APPLICATION_ID, APP_ID);

    let Some(context) = Context::new_with_proplist(&mainloop, CONTEXT_NAME, &proplist) else {
        warn!("could not create a PulseAudio context");
        return Outcome::Lost;
    };

    // `Mainloop` and `Context` are neither Send nor Sync — they are meant to be
    // used from the thread that owns them, which is this one. The Arc<Mutex<_>>
    // is how the introspection callbacks reach them, and those run on the
    // mainloop's own thread; nothing here ever crosses to a tokio worker.
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "the callbacks share them, and only ever on this thread"
    )]
    let context = Arc::new(Mutex::new(context));
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "the callbacks share them, and only ever on this thread"
    )]
    let mainloop = Arc::new(Mutex::new(mainloop));
    let state = Arc::new(Mutex::new(State::default()));

    if lock(&mainloop).start().is_err() {
        warn!("could not start the PulseAudio mainloop");
        return Outcome::Lost;
    }

    let outcome = connected(commands, inject, reports, &mainloop, &context, &state);

    {
        let mut ml = lock(&mainloop);
        ml.lock();
        lock(&context).disconnect();
        ml.unlock();
        ml.stop();
    }
    outcome
}

/// Connect, subscribe, and serve commands until the connection ends.
fn connected(
    commands: &Receiver<Request>,
    inject: &Sender<Request>,
    reports: &UnboundedSender<Report>,
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
) -> Outcome {
    {
        let mut ml = lock(mainloop);
        ml.lock();
        // The state callback is installed *before* connecting so the first
        // transition — including a straight-to-Failed on a missing socket — is
        // seen rather than raced.
        //
        // It does exactly one thing: post a message. It must not look at the
        // context, because `connect` invokes it synchronously, on this thread,
        // while this very function holds the context's mutex — and a second
        // lock of a `std::sync::Mutex` from the thread already holding it is a
        // deadlock, not a re-entry. That mistake cost the audio service its
        // whole first connection, silently, with no log line to show for it.
        let inject_for_cb = inject.clone();
        let mut ctx = lock(context);
        ctx.set_state_callback(Some(Box::new(move || {
            let _ = inject_for_cb.send(Request::CheckContext);
        })));
        if ctx.connect(None, ContextFlagSet::NOFLAGS, None).is_err() {
            drop(ctx);
            ml.unlock();
            debug!("no PulseAudio server to connect to");
            return Outcome::Lost;
        }
        drop(ctx);
        ml.unlock();
    }

    if !wait_until_ready(mainloop, context) {
        return Outcome::Lost;
    }
    info!("connected to the sound server");

    subscribe(mainloop, context, state, reports);
    refresh_all(mainloop, context, state, reports);

    serve(commands, reports, mainloop, context, state)
}

/// Block until the context is ready, or give up.
fn wait_until_ready(mainloop: &Arc<Mutex<Mainloop>>, context: &Arc<Mutex<Context>>) -> bool {
    let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        let current = {
            let mut ml = lock(mainloop);
            ml.lock();
            let state = lock(context).get_state();
            ml.unlock();
            state
        };
        match current {
            ContextState::Ready => return true,
            ContextState::Failed | ContextState::Terminated => return false,
            _ if std::time::Instant::now() >= deadline => {
                warn!("the sound server did not become ready within {CONNECT_TIMEOUT:?}");
                return false;
            }
            _ => std::thread::sleep(CONNECT_POLL),
        }
    }
}

/// Ask for the four kinds of event the panel reacts to.
fn subscribe(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
) {
    let mut ml = lock(mainloop);
    ml.lock();

    let context_for_cb = Arc::clone(context);
    let state_for_cb = Arc::clone(state);
    let reports_for_cb = reports.clone();

    let mut ctx = lock(context);
    ctx.set_subscribe_callback(Some(Box::new(move |facility, operation, _index| {
        let (Some(facility), Some(_)) = (facility, operation) else {
            return;
        };
        // Already inside the mainloop lock: every fetch below is the "locked"
        // form and must not take it again.
        match facility {
            Facility::Sink => fetch_sinks(&context_for_cb, &state_for_cb, &reports_for_cb),
            Facility::Source => fetch_sources(&context_for_cb, &state_for_cb, &reports_for_cb),
            Facility::SourceOutput => {
                fetch_recording(&context_for_cb, &state_for_cb, &reports_for_cb);
            }
            // The default device changed, which every list has an opinion on.
            Facility::Server => {
                fetch_server(&context_for_cb, &state_for_cb, &reports_for_cb, true);
            }
            _ => {}
        }
    })));

    ctx.subscribe(
        InterestMaskSet::SINK
            | InterestMaskSet::SOURCE
            | InterestMaskSet::SOURCE_OUTPUT
            | InterestMaskSet::SERVER,
        |_| {},
    );
    drop(ctx);
    ml.unlock();
}

/// Serve commands until the server goes away or the panel stops.
fn serve(
    commands: &Receiver<Request>,
    reports: &UnboundedSender<Report>,
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
) -> Outcome {
    loop {
        let request = match commands.recv_timeout(LIVENESS_CHECK) {
            Ok(request) => request,
            // Nothing to do: check the server is still there anyway. The state
            // callback is meant to have told us, and this is the net under it.
            Err(RecvTimeoutError::Timeout) => Request::CheckContext,
            // Every handle has been dropped, which only happens at shutdown.
            Err(RecvTimeoutError::Disconnected) => return Outcome::Shutdown,
        };

        match request {
            Request::Shutdown => return Outcome::Shutdown,
            Request::CheckContext => {
                if !is_ready(mainloop, context) {
                    return Outcome::Lost;
                }
            }
            Request::Refresh => refresh_all(mainloop, context, state, reports),
            Request::SetSinkVolume(percent) => {
                set_sink_volume(mainloop, context, state, reports, percent);
            }
            Request::SetSinkMuted(muted) => {
                set_sink_muted(mainloop, context, state, reports, muted)
            }
            Request::ToggleSinkMuted => {
                let muted = lock(state).sink().is_some_and(|device| device.muted);
                set_sink_muted(mainloop, context, state, reports, !muted);
            }
            Request::SetSourceVolume(percent) => {
                set_source_volume(mainloop, context, state, reports, percent);
            }
            Request::SetSourceMuted(muted) => {
                set_source_muted(mainloop, context, state, reports, muted);
            }
            Request::ToggleSourceMuted => {
                let muted = lock(state).source().is_some_and(|device| device.muted);
                set_source_muted(mainloop, context, state, reports, !muted);
            }
            Request::SetDefaultSink(name) => {
                let mut ml = lock(mainloop);
                ml.lock();
                lock(context).set_default_sink(&name, |_| {});
                ml.unlock();
                // A Server event follows and refreshes everything.
            }
            Request::SetDefaultSource(name) => {
                let mut ml = lock(mainloop);
                ml.lock();
                lock(context).set_default_source(&name, |_| {});
                ml.unlock();
            }
        }
    }
}

/// Whether the context is still ready.
fn is_ready(mainloop: &Arc<Mutex<Mainloop>>, context: &Arc<Mutex<Context>>) -> bool {
    let mut ml = lock(mainloop);
    ml.lock();
    let state = lock(context).get_state();
    ml.unlock();
    state == ContextState::Ready
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Read everything, from outside a callback.
fn refresh_all(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
) {
    let mut ml = lock(mainloop);
    ml.lock();
    fetch_server(context, state, reports, true);
    ml.unlock();
}

/// Read the default device names, and optionally everything they refer to.
///
/// Must be called with the mainloop locked.
fn fetch_server(
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
    cascade: bool,
) {
    let introspect = lock(context).introspect();
    let context = Arc::clone(context);
    let state = Arc::clone(state);
    let reports = reports.clone();

    introspect.get_server_info(move |info| {
        {
            let mut current = lock(&state);
            current.default_sink = info.default_sink_name.as_ref().map(ToString::to_string);
            current.default_source = info.default_source_name.as_ref().map(ToString::to_string);
        }
        if cascade {
            fetch_sinks(&context, &state, &reports);
            fetch_sources(&context, &state, &reports);
            fetch_recording(&context, &state, &reports);
        } else {
            publish(&state, &reports);
        }
    });
}

/// Read every sink. Must be called with the mainloop locked.
fn fetch_sinks(
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
) {
    let introspect = lock(context).introspect();
    let state = Arc::clone(state);
    let reports = reports.clone();
    let collected = Arc::new(Mutex::new(Vec::new()));

    introspect.get_sink_info_list(move |result| match result {
        ListResult::Item(info) => {
            let name = info
                .name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let channels = info.volume.len();
            lock(&collected).push(Device {
                view: DeviceView {
                    description: info
                        .description
                        .as_ref()
                        .map_or_else(|| name.clone(), ToString::to_string),
                    id: name,
                    is_default: false,
                    port_available: port_available(info.active_port.as_ref().map(|p| p.available)),
                },
                index: info.index,
                channels,
                volume_pct: volume::to_percent(info.volume.avg()),
                muted: info.mute,
                controllable: channels > 0
                    && info.volume.is_valid()
                    && info.channel_map.is_valid()
                    && info.sample_spec.is_valid(),
            });
        }
        ListResult::End => {
            lock(&state).sinks = std::mem::take(&mut *lock(&collected));
            publish(&state, &reports);
        }
        ListResult::Error => warn!("the sound server would not list its sinks"),
    });
}

/// Read every non-monitor source. Must be called with the mainloop locked.
fn fetch_sources(
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
) {
    let introspect = lock(context).introspect();
    let state = Arc::clone(state);
    let reports = reports.clone();
    let collected = Arc::new(Mutex::new(Vec::new()));
    let indexes = Arc::new(Mutex::new(HashSet::new()));

    introspect.get_source_info_list(move |result| match result {
        ListResult::Item(info) => {
            // A monitor mirrors a sink; it is not something anybody speaks into.
            if info.monitor_of_sink.is_some() {
                return;
            }
            lock(&indexes).insert(info.index);
            let name = info
                .name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let channels = info.volume.len();
            lock(&collected).push(Device {
                view: DeviceView {
                    description: info
                        .description
                        .as_ref()
                        .map_or_else(|| name.clone(), ToString::to_string),
                    id: name,
                    is_default: false,
                    port_available: port_available(info.active_port.as_ref().map(|p| p.available)),
                },
                index: info.index,
                channels,
                volume_pct: volume::to_percent(info.volume.avg()),
                muted: info.mute,
                controllable: channels > 0
                    && info.volume.is_valid()
                    && info.channel_map.is_valid()
                    && info.sample_spec.is_valid(),
            });
        }
        ListResult::End => {
            let mut current = lock(&state);
            current.sources = std::mem::take(&mut *lock(&collected));
            current.input_indexes = std::mem::take(&mut *lock(&indexes));
            drop(current);
            publish(&state, &reports);
        }
        ListResult::Error => warn!("the sound server would not list its sources"),
    });
}

/// Read who is recording. Must be called with the mainloop locked.
fn fetch_recording(
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
) {
    let introspect = lock(context).introspect();
    let state = Arc::clone(state);
    let reports = reports.clone();
    let active = Arc::new(Mutex::new(HashSet::new()));

    introspect.get_source_output_info_list(move |result| match result {
        ListResult::Item(info) => {
            // Corked or muted: the stream exists but nothing is being heard
            // through it, and the privacy dot is about being heard.
            let listening = lock(&state).input_indexes.contains(&info.source);
            if listening && !info.corked && !info.mute {
                lock(&active).insert(info.index);
            }
        }
        ListResult::End => {
            lock(&state).recording = std::mem::take(&mut *lock(&active));
            publish(&state, &reports);
        }
        ListResult::Error => warn!("the sound server would not list its recording clients"),
    });
}

/// Translate PulseAudio's three-valued jack detection.
///
/// "Unknown" means the device cannot tell, which reads as available: a sink
/// drawn as unplugged because it has no jack sensing would be a lie.
fn port_available(available: Option<PortAvailable>) -> Option<bool> {
    available.map(|available| !matches!(available, PortAvailable::No))
}

/// Send the current reading to the owning task.
fn publish(state: &Arc<Mutex<State>>, reports: &UnboundedSender<Report>) {
    let reading = lock(state).reading();
    let _ = reports.send(Report::Snapshot(Box::new(reading)));
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Apply a volume to the default sink, if it will take one.
fn set_sink_volume(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
    percent: u32,
) {
    let Some((index, channels)) = lock(state)
        .sink()
        .filter(|device| device.controllable && device.channels > 0)
        .map(|device| (device.index, device.channels))
    else {
        debug!("no controllable sink to set the volume on");
        return;
    };

    let value = volume::to_volume(percent);
    let mut volumes = ChannelVolumes::default();
    volumes.set(channels, value);

    let mut ml = lock(mainloop);
    ml.lock();
    lock(context)
        .introspect()
        .set_sink_volume_by_index(index, &volumes, None);
    ml.unlock();
    drop(ml);

    // Answer the panel now rather than a round trip later: the slider under
    // the user's finger must not lag the sound.
    let applied = volume::to_percent(value);
    {
        let mut current = lock(state);
        if let Some(name) = current.default_sink.clone()
            && let Some(device) = current.sinks.iter_mut().find(|d| d.view.id == name)
        {
            device.volume_pct = applied;
        }
    }
    publish(state, reports);
}

/// Mute or unmute the default sink.
fn set_sink_muted(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
    muted: bool,
) {
    let Some(index) = lock(state).sink().map(|device| device.index) else {
        debug!("no sink to mute");
        return;
    };

    let mut ml = lock(mainloop);
    ml.lock();
    lock(context)
        .introspect()
        .set_sink_mute_by_index(index, muted, None);
    ml.unlock();
    drop(ml);

    {
        let mut current = lock(state);
        if let Some(name) = current.default_sink.clone()
            && let Some(device) = current.sinks.iter_mut().find(|d| d.view.id == name)
        {
            device.muted = muted;
        }
    }
    publish(state, reports);
}

/// Apply a volume to the default source, if it will take one.
fn set_source_volume(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
    percent: u32,
) {
    let Some((index, channels)) = lock(state)
        .source()
        .filter(|device| device.controllable && device.channels > 0)
        .map(|device| (device.index, device.channels))
    else {
        debug!("no controllable source to set the volume on");
        return;
    };

    let value = volume::to_volume(percent);
    let mut volumes = ChannelVolumes::default();
    volumes.set(channels, value);

    let mut ml = lock(mainloop);
    ml.lock();
    lock(context)
        .introspect()
        .set_source_volume_by_index(index, &volumes, None);
    ml.unlock();
    drop(ml);

    let applied = volume::to_percent(value);
    {
        let mut current = lock(state);
        if let Some(name) = current.default_source.clone()
            && let Some(device) = current.sources.iter_mut().find(|d| d.view.id == name)
        {
            device.volume_pct = applied;
        }
    }
    publish(state, reports);
}

/// Mute or unmute the default source.
fn set_source_muted(
    mainloop: &Arc<Mutex<Mainloop>>,
    context: &Arc<Mutex<Context>>,
    state: &Arc<Mutex<State>>,
    reports: &UnboundedSender<Report>,
    muted: bool,
) {
    let Some(index) = lock(state).source().map(|device| device.index) else {
        debug!("no source to mute");
        return;
    };

    let mut ml = lock(mainloop);
    ml.lock();
    lock(context)
        .introspect()
        .set_source_mute_by_index(index, muted, None);
    ml.unlock();
    drop(ml);

    {
        let mut current = lock(state);
        if let Some(name) = current.default_source.clone()
            && let Some(device) = current.sources.iter_mut().find(|d| d.view.id == name)
        {
            device.muted = muted;
        }
    }
    publish(state, reports);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, volume_pct: u32, muted: bool) -> Device {
        Device {
            view: DeviceView {
                id: id.to_string(),
                description: format!("{id} device"),
                is_default: false,
                port_available: None,
            },
            index: 0,
            channels: 2,
            volume_pct,
            muted,
            controllable: true,
        }
    }

    #[test]
    fn a_reading_takes_its_numbers_from_the_default_device() {
        let state = State {
            default_sink: Some("analog".into()),
            sinks: vec![device("hdmi", 10, true), device("analog", 40, false)],
            ..State::default()
        };
        let reading = state.reading();
        assert_eq!(reading.sink_volume_pct, 40);
        assert!(!reading.sink_muted);
        assert!(reading.sink_controllable);
        assert_eq!(reading.sinks.len(), 2);
        assert!(reading.sinks.iter().filter(|s| s.is_default).count() == 1);
        assert!(
            reading
                .sinks
                .iter()
                .any(|s| s.id == "analog" && s.is_default)
        );
    }

    #[test]
    fn a_default_that_is_not_in_the_list_reads_as_uncontrollable() {
        let state = State {
            default_sink: Some("gone".into()),
            sinks: vec![device("analog", 40, false)],
            ..State::default()
        };
        let reading = state.reading();
        assert_eq!(reading.sink_volume_pct, 0);
        assert!(!reading.sink_controllable);
    }

    #[test]
    fn recording_clients_make_the_source_in_use() {
        let mut state = State::default();
        assert!(!state.reading().source_in_use);
        state.recording.insert(7);
        assert!(state.reading().source_in_use);
    }

    #[test]
    fn jack_detection_only_says_unplugged_when_it_knows() {
        assert_eq!(port_available(None), None);
        assert_eq!(port_available(Some(PortAvailable::Unknown)), Some(true));
        assert_eq!(port_available(Some(PortAvailable::Yes)), Some(true));
        assert_eq!(port_available(Some(PortAvailable::No)), Some(false));
    }

    #[test]
    fn a_poisoned_lock_still_hands_the_state_over() {
        let mutex = Arc::new(Mutex::new(State::default()));
        let poisoner = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("fresh mutex");
            panic!("poison it");
        })
        .join();
        assert!(mutex.is_poisoned());
        assert!(lock(&mutex).sinks.is_empty());
    }
}
