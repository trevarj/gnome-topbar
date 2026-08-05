//! The one owner of the audio thread.
//!
//! Commands come in from the panel, readings come back from the PulseAudio
//! thread, and this is where the two meet: a command is clamped against the
//! current state and its echo is recorded, a reading is diffed against the last
//! one and its changes attributed. Everything a widget ever sees leaves here as
//! one `Arc<AudioState>`.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, oneshot, watch};
use tracing::debug;

use crate::audio::model::AudioState;
use crate::audio::volume;
use crate::audio::worker::{self, Report};
use crate::change::{Change, ChangeSource, Echoes, Field};
use crate::error::SvcError;

/// One thing the panel can ask of the sound server.
#[derive(Debug)]
pub(crate) enum Action {
    /// Set the default sink to a percentage.
    SetSinkVolume(u32),
    /// Move the default sink's volume by a signed number of points.
    StepSinkVolume(i32),
    /// Mute or unmute the default sink.
    SetSinkMuted(bool),
    /// Flip the default sink's mute.
    ToggleSinkMuted,
    /// Set the default source to a percentage.
    SetSourceVolume(u32),
    /// Move the default source's volume by a signed number of points.
    StepSourceVolume(i32),
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
    /// Apply a changed `[audio] allow_overdrive`.
    ///
    /// The ceiling is policy, not hardware: it decides what a slider may ask
    /// for and what the OSD draws as full. A reload changes it under a running
    /// panel, and nothing about the sound server has to be re-read for it.
    SetAllowOverdrive(bool),
}

/// A command, with who sent it and where to answer.
#[derive(Debug)]
pub(crate) struct Command {
    pub(crate) action: Action,
    pub(crate) source: ChangeSource,
    pub(crate) reply: oneshot::Sender<Result<(), SvcError>>,
}

/// Follow the sound server until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<AudioState>>,
    allow_overdrive: bool,
) {
    let mut max_volume_pct = volume::max_percent(allow_overdrive);
    let (requests, request_queue) = std::sync::mpsc::channel();
    let (reports, mut report_queue) = mpsc::unbounded_channel();

    // The thread is given its own sender so the context state callback can
    // wake the command loop from inside libpulse.
    let inject = requests.clone();
    let thread = std::thread::Builder::new()
        .name("topbar-pulse".to_string())
        .spawn(move || worker::run(request_queue, inject, reports))
        .ok();
    if thread.is_none() {
        debug!("could not start the audio thread; audio stays unavailable");
    }

    let _ = publisher.send(Arc::new(AudioState {
        max_volume_pct,
        ..AudioState::default()
    }));

    let mut echoes = Echoes::new();

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                // The one action that is about policy rather than about the
                // sound server, so it is answered here rather than dispatched.
                if let Action::SetAllowOverdrive(allow) = command.action {
                    max_volume_pct = volume::max_percent(allow);
                    debug!("the volume ceiling is now {max_volume_pct}%");
                    publisher.send_if_modified(|current| {
                        if current.max_volume_pct == max_volume_pct {
                            return false;
                        }
                        let mut next = (**current).clone();
                        next.max_volume_pct = max_volume_pct;
                        // A ceiling that just dropped must not leave the
                        // published volume above it.
                        next.sink_volume_pct = next.sink_volume_pct.min(max_volume_pct);
                        next.source_volume_pct = next.source_volume_pct.min(max_volume_pct);
                        *current = Arc::new(next);
                        true
                    });
                    let _ = command.reply.send(Ok(()));
                    continue;
                }
                let answer = dispatch(&publisher, &mut echoes, &requests, command.action, command.source);
                let _ = command.reply.send(answer);
            }
            report = report_queue.recv() => {
                let Some(report) = report else { continue };
                let next = fold(&publisher.borrow(), report, &mut echoes, max_volume_pct);
                publisher.send_if_modified(|current| {
                    if **current == next {
                        false
                    } else {
                        *current = Arc::new(next);
                        true
                    }
                });
            }
        }
    }

    let _ = requests.send(worker::Request::Shutdown);
    if let Some(thread) = thread {
        let _ = thread.join();
    }
}

/// Validate a command, record its echo, and hand it to the thread.
fn dispatch(
    publisher: &watch::Sender<Arc<AudioState>>,
    echoes: &mut Echoes,
    requests: &std::sync::mpsc::Sender<worker::Request>,
    action: Action,
    source: ChangeSource,
) -> Result<(), SvcError> {
    let state = publisher.borrow().clone();
    if !state.available {
        return Err(SvcError::AudioUnavailable);
    }
    let now = Instant::now();

    let request = match action {
        Action::SetSinkVolume(percent) => {
            let target = sink_target(&state, percent)?;
            echoes.record(Field::SinkVolume, target, source, now);
            worker::Request::SetSinkVolume(target)
        }
        Action::StepSinkVolume(delta) => {
            let Some(target) = volume::step(state.sink_volume_pct, delta, state.max_volume_pct)
            else {
                // Already at the ceiling: not a failure, just nothing to do.
                return Ok(());
            };
            let target = sink_target(&state, target)?;
            echoes.record(Field::SinkVolume, target, source, now);
            worker::Request::SetSinkVolume(target)
        }
        Action::SetSinkMuted(muted) => {
            require_sink(&state)?;
            echoes.record(Field::SinkMute, u32::from(muted), source, now);
            worker::Request::SetSinkMuted(muted)
        }
        Action::ToggleSinkMuted => {
            require_sink(&state)?;
            echoes.record(Field::SinkMute, u32::from(!state.sink_muted), source, now);
            worker::Request::ToggleSinkMuted
        }
        Action::SetSourceVolume(percent) => {
            let target = source_target(&state, percent)?;
            echoes.record(Field::SourceVolume, target, source, now);
            worker::Request::SetSourceVolume(target)
        }
        Action::StepSourceVolume(delta) => {
            let Some(target) = volume::step(state.source_volume_pct, delta, state.max_volume_pct)
            else {
                return Ok(());
            };
            let target = source_target(&state, target)?;
            echoes.record(Field::SourceVolume, target, source, now);
            worker::Request::SetSourceVolume(target)
        }
        Action::SetSourceMuted(muted) => {
            require_source(&state)?;
            echoes.record(Field::SourceMute, u32::from(muted), source, now);
            worker::Request::SetSourceMuted(muted)
        }
        Action::ToggleSourceMuted => {
            require_source(&state)?;
            echoes.record(
                Field::SourceMute,
                u32::from(!state.source_muted),
                source,
                now,
            );
            worker::Request::ToggleSourceMuted
        }
        Action::SetDefaultSink(id) => {
            if !state.sinks.iter().any(|sink| sink.id == id) {
                return Err(SvcError::AudioDevice("output"));
            }
            worker::Request::SetDefaultSink(id)
        }
        Action::SetDefaultSource(id) => {
            if !state.sources.iter().any(|source| source.id == id) {
                return Err(SvcError::AudioDevice("input"));
            }
            worker::Request::SetDefaultSource(id)
        }
        Action::Refresh => worker::Request::Refresh,
        // Answered by the loop before it ever reaches here: the ceiling is
        // policy, and there is nothing to ask the sound server for.
        Action::SetAllowOverdrive(_) => return Ok(()),
    };

    requests
        .send(request)
        .map_err(|_| SvcError::ServiceStopped("audio"))
}

/// The percentage to send to a sink that can take one.
fn sink_target(state: &AudioState, percent: u32) -> Result<u32, SvcError> {
    if !state.can_set_sink_volume() {
        return Err(SvcError::AudioDevice("output"));
    }
    Ok(volume::clamp(percent, state.max_volume_pct))
}

/// The percentage to send to a source that can take one.
fn source_target(state: &AudioState, percent: u32) -> Result<u32, SvcError> {
    if !state.can_set_source_volume() {
        return Err(SvcError::AudioDevice("input"));
    }
    Ok(volume::clamp(percent, state.max_volume_pct))
}

/// A sink has to exist before it can be muted, but it need not be controllable
/// — muting a sink with no channels is harmless where setting its volume is
/// not.
fn require_sink(state: &AudioState) -> Result<(), SvcError> {
    if state.default_sink.is_some() {
        Ok(())
    } else {
        Err(SvcError::AudioDevice("output"))
    }
}

/// The same, for the microphone.
fn require_source(state: &AudioState) -> Result<(), SvcError> {
    if state.default_source.is_some() {
        Ok(())
    } else {
        Err(SvcError::AudioDevice("input"))
    }
}

/// Turn a reading into the next published state, attributing what moved.
///
/// The `available` transition is deliberately *not* a change: PulseAudio sends
/// a flurry of updates as it discovers devices and settles on a default, and
/// treating those as user-visible events is what made v1 need a settle timer
/// to keep the OSD from appearing at login. Here there is nothing to time —
/// the first reading after a connection simply carries no change.
pub(crate) fn fold(
    previous: &AudioState,
    report: Report,
    echoes: &mut Echoes,
    max_volume_pct: u32,
) -> AudioState {
    let reading = match report {
        Report::Unavailable => {
            echoes.clear();
            return AudioState {
                max_volume_pct,
                ..AudioState::default()
            };
        }
        Report::Snapshot(reading) => *reading,
    };
    let now = Instant::now();
    let fresh = !previous.available;

    let sink_change = if fresh {
        None
    } else {
        change(
            echoes,
            now,
            [
                (
                    Field::SinkMute,
                    previous.sink_muted != reading.sink_muted,
                    u32::from(reading.sink_muted),
                ),
                (
                    Field::SinkVolume,
                    previous.sink_volume_pct != reading.sink_volume_pct,
                    reading.sink_volume_pct,
                ),
            ],
        )
    }
    .or(previous.sink_change);

    let source_change = if fresh {
        None
    } else {
        change(
            echoes,
            now,
            [
                (
                    Field::SourceMute,
                    previous.source_muted != reading.source_muted,
                    u32::from(reading.source_muted),
                ),
                (
                    Field::SourceVolume,
                    previous.source_volume_pct != reading.source_volume_pct,
                    reading.source_volume_pct,
                ),
            ],
        )
    }
    .or(previous.source_change);

    AudioState {
        available: true,
        sinks: reading.sinks,
        sources: reading.sources,
        default_sink: reading.default_sink,
        default_source: reading.default_source,
        sink_volume_pct: reading.sink_volume_pct,
        sink_muted: reading.sink_muted,
        sink_controllable: reading.sink_controllable,
        source_volume_pct: reading.source_volume_pct,
        source_muted: reading.source_muted,
        source_controllable: reading.source_controllable,
        source_in_use: reading.source_in_use,
        max_volume_pct,
        sink_change,
        source_change,
    }
}

/// Attribute every field that moved, and report the most notable one.
///
/// Each moved field consumes its own echo — a stale record left behind would
/// go on to claim somebody else's change — but only one becomes the published
/// [`Change`]. Mute comes first in the list because a mute and a volume landing
/// together is a mute as far as the OSD is concerned: it draws a different
/// icon, and drawing the volume instead would hide the fact.
fn change(echoes: &mut Echoes, now: Instant, fields: [(Field, bool, u32); 2]) -> Option<Change> {
    let mut result = None;
    for (field, moved, value) in fields {
        if !moved {
            continue;
        }
        let attributed = echoes.attribute(field, value, now);
        result = result.or(Some(attributed));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::DeviceView;
    use crate::audio::worker::Reading;

    fn reading(volume: u32, muted: bool) -> Report {
        Report::Snapshot(Box::new(Reading {
            sinks: vec![DeviceView {
                id: "analog".into(),
                description: "Analog".into(),
                is_default: true,
                port_available: None,
            }],
            default_sink: Some("analog".into()),
            sink_volume_pct: volume,
            sink_muted: muted,
            sink_controllable: true,
            ..Reading::default()
        }))
    }

    fn connected(volume: u32) -> AudioState {
        let mut echoes = Echoes::new();
        fold(
            &AudioState::default(),
            reading(volume, false),
            &mut echoes,
            100,
        )
    }

    #[test]
    fn the_first_reading_after_a_connection_is_not_a_change() {
        let mut echoes = Echoes::new();
        let state = fold(&AudioState::default(), reading(40, false), &mut echoes, 100);
        assert!(state.available);
        assert_eq!(state.sink_volume_pct, 40);
        assert_eq!(state.sink_change, None, "logging in must not raise the OSD");
    }

    #[test]
    fn a_change_nobody_claimed_is_external() {
        let previous = connected(40);
        let mut echoes = Echoes::new();
        let state = fold(&previous, reading(70, false), &mut echoes, 100);
        assert_eq!(
            state.sink_change.map(|change| change.source),
            Some(ChangeSource::External)
        );
    }

    #[test]
    fn a_change_the_panel_asked_for_carries_its_source() {
        let previous = connected(40);
        let mut echoes = Echoes::new();
        echoes.record(Field::SinkVolume, 70, ChangeSource::Ui, Instant::now());
        let state = fold(&previous, reading(70, false), &mut echoes, 100);
        assert_eq!(
            state.sink_change.map(|change| change.source),
            Some(ChangeSource::Ui)
        );
        assert!(!ChangeSource::Ui.shows_osd());
    }

    #[test]
    fn a_reading_that_moved_nothing_keeps_the_last_change() {
        let previous = connected(40);
        let mut echoes = Echoes::new();
        let moved = fold(&previous, reading(70, false), &mut echoes, 100);
        let serial = moved.sink_change.expect("a change").serial;

        let still = fold(&moved, reading(70, false), &mut echoes, 100);
        assert_eq!(still.sink_change.expect("carried forward").serial, serial);
    }

    #[test]
    fn a_mute_outranks_a_volume_that_moved_with_it() {
        let previous = connected(40);
        let mut echoes = Echoes::new();
        echoes.record(Field::SinkMute, 1, ChangeSource::Cli, Instant::now());
        let state = fold(&previous, reading(70, true), &mut echoes, 100);
        assert_eq!(
            state.sink_change.map(|change| change.source),
            Some(ChangeSource::Cli),
            "the mute's echo is the one that decided it"
        );
    }

    #[test]
    fn losing_the_server_clears_everything() {
        let previous = connected(40);
        let mut echoes = Echoes::new();
        echoes.record(Field::SinkVolume, 70, ChangeSource::Ui, Instant::now());

        let gone = fold(&previous, Report::Unavailable, &mut echoes, 100);
        assert!(!gone.available);
        assert_eq!(gone.max_volume_pct, 100);
        assert_eq!(gone.sink_change, None);

        // Coming back is not a change either, and the orphaned echo is gone.
        let back = fold(&gone, reading(70, false), &mut echoes, 100);
        assert_eq!(back.sink_change, None);
    }

    #[test]
    fn a_command_needs_a_server_to_answer_it() {
        let (publisher, _receiver) = watch::channel(Arc::new(AudioState::default()));
        let (requests, _queue) = std::sync::mpsc::channel();
        let mut echoes = Echoes::new();
        let error = dispatch(
            &publisher,
            &mut echoes,
            &requests,
            Action::SetSinkVolume(40),
            ChangeSource::Cli,
        )
        .expect_err("no server, no volume");
        assert!(matches!(error, SvcError::AudioUnavailable));
    }

    #[test]
    fn a_command_needs_a_controllable_sink() {
        let (publisher, _receiver) = watch::channel(Arc::new(AudioState {
            available: true,
            max_volume_pct: 100,
            ..AudioState::default()
        }));
        let (requests, _queue) = std::sync::mpsc::channel();
        let mut echoes = Echoes::new();
        let error = dispatch(
            &publisher,
            &mut echoes,
            &requests,
            Action::SetSinkVolume(40),
            ChangeSource::Cli,
        )
        .expect_err("no sink, no volume");
        assert!(matches!(error, SvcError::AudioDevice("output")));
    }

    #[test]
    fn a_command_is_clamped_and_its_echo_recorded() {
        let (publisher, _receiver) = watch::channel(Arc::new(connected(40)));
        let (requests, queue) = std::sync::mpsc::channel();
        let mut echoes = Echoes::new();

        dispatch(
            &publisher,
            &mut echoes,
            &requests,
            Action::SetSinkVolume(400),
            ChangeSource::Ui,
        )
        .expect("a clamped command still goes through");

        match queue.try_recv().expect("the thread was asked") {
            worker::Request::SetSinkVolume(percent) => assert_eq!(percent, 100),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            echoes
                .attribute(Field::SinkVolume, 100, Instant::now())
                .source,
            ChangeSource::Ui
        );
    }

    #[test]
    fn a_step_at_the_ceiling_is_a_no_op_rather_than_a_failure() {
        let (publisher, _receiver) = watch::channel(Arc::new(connected(100)));
        let (requests, queue) = std::sync::mpsc::channel();
        let mut echoes = Echoes::new();

        dispatch(
            &publisher,
            &mut echoes,
            &requests,
            Action::StepSinkVolume(5),
            ChangeSource::Cli,
        )
        .expect("already loud is not an error");
        assert!(queue.try_recv().is_err(), "nothing was sent");
    }

    #[test]
    fn a_default_device_has_to_be_one_the_server_reported() {
        let (publisher, _receiver) = watch::channel(Arc::new(connected(40)));
        let (requests, _queue) = std::sync::mpsc::channel();
        let mut echoes = Echoes::new();

        assert!(matches!(
            dispatch(
                &publisher,
                &mut echoes,
                &requests,
                Action::SetDefaultSink("nonsense".into()),
                ChangeSource::Ui,
            ),
            Err(SvcError::AudioDevice("output"))
        ));
        assert!(
            dispatch(
                &publisher,
                &mut echoes,
                &requests,
                Action::SetDefaultSink("analog".into()),
                ChangeSource::Ui,
            )
            .is_ok()
        );
    }
}
