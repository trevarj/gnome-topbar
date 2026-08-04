//! What the panel knows about the backlight.

use crate::change::Change;

/// The backlight, as a widget needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct BrightnessState {
    /// Whether this machine has a backlight the panel can drive.
    pub available: bool,
    /// How bright it is, 0–100.
    pub percent: u32,
    /// The controller's name, e.g. `intel_backlight`.
    pub device: Option<String>,
    /// The last change, and who caused it.
    ///
    /// `None` until something moves, so the reading taken at start-up cannot
    /// throw an OSD at a user who has not touched anything.
    pub change: Option<Change>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_with_no_backlight_says_so() {
        let state = BrightnessState::default();
        assert!(!state.available);
        assert_eq!(state.device, None);
        assert_eq!(state.change, None);
    }
}
