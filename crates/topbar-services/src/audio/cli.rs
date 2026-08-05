//! The volume path that does not need a panel.
//!
//! `topbar volume …` is what a media key is bound to, and a media key has to
//! work when the panel has crashed, when it has not started yet, and when the
//! configuration file it would have read is broken. So the command talks to
//! PulseAudio itself — a standard (not threaded) mainloop, iterated by hand,
//! nothing shared with the running panel — and only *then* tries to tell a
//! panel about it so an OSD can appear. The second half is best effort; the
//! first half is the command.
//!
//! This is v1's `AudioCli` with the same shape and the same waits, because the
//! shape is what makes it safe: every operation is bounded by a timeout, and
//! the process exits rather than lingering on a server that stopped answering.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::proplist::Proplist;
use pulse::volume::ChannelVolumes;

use crate::audio::volume;

/// How long any one operation may take before the command gives up.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long to sleep between mainloop iterations while connecting.
const CONNECT_POLL: Duration = Duration::from_millis(5);

/// What went wrong, in words a terminal can print.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// There is no sound server on this machine right now.
    #[error("could not connect to PulseAudio (is PulseAudio/pipewire-pulse running?)")]
    NoServer,
    /// The server is there but has no default output.
    #[error("no default sink found (is anything configured as the output?)")]
    NoSink,
    /// The sink exists but will not accept a volume yet.
    ///
    /// Kept separate because the panel answers it differently: this is the one
    /// case that raises the "no output device" OSD rather than a plain error.
    #[error("the audio device is not ready (try playing something through it first)")]
    NotReady,
    /// The server stopped answering mid-operation.
    #[error("the sound server stopped responding")]
    Stopped,
}

/// A short-lived, synchronous connection to the sound server.
pub struct AudioCli {
    mainloop: Mainloop,
    context: Context,
    volume_pct: u32,
    muted: bool,
    sink_index: Option<u32>,
    channels: u8,
    controllable: bool,
    max_percent: u32,
}

impl AudioCli {
    /// Connect and read the default sink, or explain why not.
    ///
    /// `allow_overdrive` is read straight from the config file by the caller —
    /// see `Config::read_audio_allow_overdrive`, which tolerates a config too
    /// broken for the panel to start on.
    pub fn connect(allow_overdrive: bool) -> Result<Self, CliError> {
        let mut mainloop = Mainloop::new().ok_or(CliError::NoServer)?;
        let mut proplist = Proplist::new().ok_or(CliError::NoServer)?;
        let _ = proplist.set_str(pulse::proplist::properties::APPLICATION_NAME, "topbar");
        let mut context = Context::new_with_proplist(&mainloop, "topbar-cli", &proplist)
            .ok_or(CliError::NoServer)?;

        context
            .connect(None, ContextFlagSet::NOFLAGS, None)
            .map_err(|_| CliError::NoServer)?;

        let deadline = Instant::now() + TIMEOUT;
        loop {
            match mainloop.iterate(false) {
                IterateResult::Success(_) => {}
                IterateResult::Quit(_) | IterateResult::Err(_) => return Err(CliError::NoServer),
            }
            match context.get_state() {
                ContextState::Ready => break,
                ContextState::Failed | ContextState::Terminated => return Err(CliError::NoServer),
                _ if Instant::now() >= deadline => return Err(CliError::NoServer),
                _ => std::thread::sleep(CONNECT_POLL),
            }
        }

        let mut cli = Self {
            mainloop,
            context,
            volume_pct: 0,
            muted: false,
            sink_index: None,
            channels: 0,
            controllable: false,
            max_percent: volume::max_percent(allow_overdrive),
        };
        cli.read()?;
        Ok(cli)
    }

    /// The default sink's volume, as a percentage.
    pub fn volume(&self) -> u32 {
        self.volume_pct
    }

    /// Whether the default sink is muted.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// The ceiling this process is allowed to ask for.
    pub fn max_percent(&self) -> u32 {
        self.max_percent
    }

    /// Set the volume, returning the value that was actually applied.
    pub fn set_volume(&mut self, percent: u32) -> Result<u32, CliError> {
        let index = self.sink_index.ok_or(CliError::NoSink)?;
        if !self.controllable || self.channels == 0 {
            return Err(CliError::NotReady);
        }

        let percent = volume::clamp(percent, self.max_percent);
        let value = volume::to_volume(percent);
        let mut volumes = ChannelVolumes::default();
        volumes.set(self.channels, value);

        let operation = self
            .context
            .introspect()
            .set_sink_volume_by_index(index, &volumes, None);
        self.wait(&operation)?;
        self.volume_pct = percent;
        Ok(percent)
    }

    /// Move the volume by a signed number of points; see [`volume::step`].
    pub fn step_volume(&mut self, delta: i32) -> Result<u32, CliError> {
        match volume::step(self.volume_pct, delta, self.max_percent) {
            Some(target) => self.set_volume(target),
            None => Ok(self.volume_pct),
        }
    }

    /// Mute or unmute.
    pub fn set_muted(&mut self, muted: bool) -> Result<(), CliError> {
        let index = self.sink_index.ok_or(CliError::NoSink)?;
        let operation = self
            .context
            .introspect()
            .set_sink_mute_by_index(index, muted, None);
        self.wait(&operation)?;
        self.muted = muted;
        Ok(())
    }

    /// Read the default sink's name, then its state.
    fn read(&mut self) -> Result<(), CliError> {
        let name = self.default_sink_name()?;
        self.read_sink(&name)
    }

    /// Ask the server which sink is the default one.
    fn default_sink_name(&mut self) -> Result<String, CliError> {
        let found: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let done = Arc::new(Mutex::new(false));

        let found_for_cb = Arc::clone(&found);
        let done_for_cb = Arc::clone(&done);
        self.context.introspect().get_server_info(move |info| {
            if let Some(name) = info.default_sink_name.as_ref() {
                *guard(&found_for_cb) = Some(name.to_string());
            }
            *guard(&done_for_cb) = true;
        });

        self.pump(&done)?;
        let name = guard(&found).clone();
        name.ok_or(CliError::NoSink)
    }

    /// Read one sink's volume, mute, index and controllability.
    fn read_sink(&mut self, name: &str) -> Result<(), CliError> {
        type Sink = (u32, bool, u32, u8, bool);
        let found: Arc<Mutex<Option<Sink>>> = Arc::new(Mutex::new(None));
        let done = Arc::new(Mutex::new(false));

        let found_for_cb = Arc::clone(&found);
        let done_for_cb = Arc::clone(&done);
        self.context
            .introspect()
            .get_sink_info_by_name(name, move |result| match result {
                ListResult::Item(info) => {
                    let channels = info.volume.len();
                    *guard(&found_for_cb) = Some((
                        volume::to_percent(info.volume.avg()),
                        info.mute,
                        info.index,
                        channels,
                        channels > 0
                            && info.volume.is_valid()
                            && info.channel_map.is_valid()
                            && info.sample_spec.is_valid(),
                    ));
                }
                ListResult::End | ListResult::Error => *guard(&done_for_cb) = true,
            });

        self.pump(&done)?;
        let Some((percent, muted, index, channels, controllable)) = *guard(&found) else {
            return Err(CliError::NoSink);
        };
        self.volume_pct = percent;
        self.muted = muted;
        self.sink_index = Some(index);
        self.channels = channels;
        self.controllable = controllable;
        Ok(())
    }

    /// Iterate the mainloop until `done`, or until the deadline.
    ///
    /// Never a blocking iterate: `iterate(true)` polls with no timeout, so a
    /// sound server that dies without waking the socket — a sandbox teardown
    /// SIGKILLing the whole run, say — left the deadline check unreachable and
    /// the process asleep forever. Two hung `topbar volume` CLIs from exactly
    /// that were found parked days into the v2 work. The non-blocking spin at
    /// [`CONNECT_POLL`] costs nothing measurable for a bounded five seconds.
    fn pump(&mut self, done: &Arc<Mutex<bool>>) -> Result<(), CliError> {
        let deadline = Instant::now() + TIMEOUT;
        while !*guard(done) {
            match self.mainloop.iterate(false) {
                IterateResult::Success(_) => {}
                IterateResult::Quit(_) | IterateResult::Err(_) => return Err(CliError::Stopped),
            }
            if Instant::now() >= deadline {
                return Err(CliError::Stopped);
            }
            std::thread::sleep(CONNECT_POLL);
        }
        Ok(())
    }

    /// Iterate the mainloop until an operation leaves the running state.
    ///
    /// Non-blocking for the same reason as [`Self::pump`].
    fn wait(
        &mut self,
        operation: &pulse::operation::Operation<dyn FnMut(bool)>,
    ) -> Result<(), CliError> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.mainloop.iterate(false) {
                IterateResult::Success(_) => {}
                IterateResult::Quit(_) | IterateResult::Err(_) => return Err(CliError::Stopped),
            }
            if operation.get_state() != pulse::operation::State::Running {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CliError::Stopped);
            }
            std::thread::sleep(CONNECT_POLL);
        }
    }
}

impl Drop for AudioCli {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

/// Lock through poisoning: these mutexes only ever hold plain data.
fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_says_what_to_do_about_it() {
        for error in [
            CliError::NoServer,
            CliError::NoSink,
            CliError::NotReady,
            CliError::Stopped,
        ] {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(
                message.chars().next().is_some_and(char::is_lowercase),
                "`{message}` reads badly after `Error: `"
            );
        }
    }

    #[test]
    fn a_device_that_is_not_ready_is_its_own_failure() {
        // The panel answers this one with an OSD rather than a message, so it
        // must stay distinguishable from every other failure.
        assert!(matches!(CliError::NotReady, CliError::NotReady));
        assert_ne!(CliError::NotReady.to_string(), CliError::NoSink.to_string());
    }
}
