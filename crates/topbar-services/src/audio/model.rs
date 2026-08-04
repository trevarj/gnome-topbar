//! What the panel knows about the sound server.

use crate::change::Change;

/// One sink or source, as a widget needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct DeviceView {
    /// PulseAudio's own name, which is what `set_default_sink` takes.
    pub id: String,
    /// What to show the user.
    pub description: String,
    /// Whether this is the default device right now.
    pub is_default: bool,
    /// Whether the active port is plugged in, when the device says.
    ///
    /// `None` means the device has no jack detection, which is not the same as
    /// "unplugged" and must not be drawn as one.
    pub port_available: Option<bool>,
}

/// Everything the panel knows about audio.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct AudioState {
    /// Whether a sound server is answering at all.
    pub available: bool,
    /// Output devices.
    pub sinks: Vec<DeviceView>,
    /// Input devices, monitors excluded.
    pub sources: Vec<DeviceView>,
    /// The default sink's PulseAudio name.
    pub default_sink: Option<String>,
    /// The default source's PulseAudio name.
    pub default_source: Option<String>,
    /// The default sink's volume, where values above 100 are overdrive.
    pub sink_volume_pct: u32,
    /// Whether the default sink is muted.
    pub sink_muted: bool,
    /// Whether the default sink will accept a volume change.
    ///
    /// False when the sink reports no channels or an invalid volume structure,
    /// which happens on some stacks until something has played through it.
    /// Sending a volume to a sink in that state trips an assertion inside
    /// PulseAudio, so this is a guard rather than a nicety.
    pub sink_controllable: bool,
    /// The default source's volume.
    pub source_volume_pct: u32,
    /// Whether the default source is muted.
    pub source_muted: bool,
    /// Whether the default source will accept a volume change.
    pub source_controllable: bool,
    /// Whether anything is recording right now.
    ///
    /// Feeds the microphone privacy dot (M9). Corked and muted streams do not
    /// count: a video call on hold is not listening.
    pub source_in_use: bool,
    /// The highest volume the panel may ask for, per `audio.allow_overdrive`.
    pub max_volume_pct: u32,
    /// The last change to the sink's volume or mute, and who caused it.
    ///
    /// `None` until something actually moves — which is what keeps the burst
    /// of updates PulseAudio sends while it discovers devices from throwing an
    /// OSD at a user who has not touched anything.
    pub sink_change: Option<Change>,
    /// The same, for the source.
    pub source_change: Option<Change>,
}

impl AudioState {
    /// Whether the volume can be changed right now.
    pub fn can_set_sink_volume(&self) -> bool {
        self.available && self.sink_controllable && self.default_sink.is_some()
    }

    /// Whether the microphone volume can be changed right now.
    pub fn can_set_source_volume(&self) -> bool {
        self.available && self.source_controllable && self.default_source.is_some()
    }

    /// The default sink, if it is one of the sinks on the snapshot.
    pub fn default_sink_view(&self) -> Option<&DeviceView> {
        self.sinks.iter().find(|sink| sink.is_default)
    }

    /// The default source, likewise.
    pub fn default_source_view(&self) -> Option<&DeviceView> {
        self.sources.iter().find(|source| source.is_default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, is_default: bool) -> DeviceView {
        DeviceView {
            id: id.to_string(),
            description: id.to_string(),
            is_default,
            port_available: None,
        }
    }

    #[test]
    fn nothing_is_controllable_before_a_server_answers() {
        let state = AudioState::default();
        assert!(!state.available);
        assert!(!state.can_set_sink_volume());
        assert!(!state.can_set_source_volume());
        assert_eq!(state.sink_change, None);
    }

    #[test]
    fn a_sink_that_reports_no_channels_is_not_controllable() {
        let state = AudioState {
            available: true,
            default_sink: Some("alsa".into()),
            sink_controllable: false,
            ..AudioState::default()
        };
        assert!(!state.can_set_sink_volume());
    }

    #[test]
    fn the_default_device_is_found_by_its_flag() {
        let state = AudioState {
            sinks: vec![device("hdmi", false), device("analog", true)],
            sources: vec![device("mic", true)],
            ..AudioState::default()
        };
        assert_eq!(
            state.default_sink_view().map(|s| s.id.as_str()),
            Some("analog")
        );
        assert_eq!(
            state.default_source_view().map(|s| s.id.as_str()),
            Some("mic")
        );
    }
}
