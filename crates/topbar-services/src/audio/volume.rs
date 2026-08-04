//! Percentages in, PulseAudio volumes out — and the overdrive policy.
//!
//! PulseAudio's volume is an integer where [`Volume::NORMAL`] is 100% and
//! [`Volume::MAX`] is far above anything a user should be handed. Between them
//! sits `Volume::ui_max()`, the value PulseAudio itself recommends as the top
//! of a volume slider (about 153%), which is what `audio.allow_overdrive`
//! unlocks. With overdrive off — the live config's setting — the ceiling is a
//! plain 100%.
//!
//! Everything here is pure, so the clamping rules are a table rather than
//! something you have to run a sound server to check.

use libpulse_binding::volume::Volume;

/// The default step for `topbar volume inc`/`dec`, in percentage points.
pub const DEFAULT_STEP: u32 = 5;

/// A PulseAudio volume as a percentage of normal.
pub fn to_percent(volume: Volume) -> u32 {
    ((f64::from(volume.0) / f64::from(Volume::NORMAL.0)) * 100.0).round() as u32
}

/// A percentage as a PulseAudio volume, never above [`Volume::MAX`].
pub fn to_volume(percent: u32) -> Volume {
    let raw = (f64::from(Volume::NORMAL.0) * f64::from(percent) / 100.0).round();
    Volume(raw.min(f64::from(Volume::MAX.0)) as u32)
}

/// The percentage PulseAudio would actually store for `percent`.
///
/// Round-tripping through [`to_volume`] is not the identity — the conversion
/// is lossy at both ends — so this is what the panel records when it wants to
/// recognise its own change coming back.
pub fn representable(percent: u32) -> u32 {
    to_percent(to_volume(percent))
}

/// The top of a volume slider PulseAudio recommends, never below 100.
///
/// A backend reporting a UI maximum under normal volume would otherwise leave
/// the panel unable to ask for 100%.
pub fn ui_max_percent() -> u32 {
    to_percent(Volume::ui_max()).max(100)
}

/// The ceiling the panel is allowed to ask for.
pub fn max_percent(allow_overdrive: bool) -> u32 {
    if allow_overdrive {
        ui_max_percent()
    } else {
        100
    }
}

/// Clamp a requested percentage to what the backend can store and the policy
/// allows.
pub fn clamp(percent: u32, max_percent: u32) -> u32 {
    representable(percent).min(max_percent)
}

/// Where a relative change lands, or `None` if it should not be sent at all.
///
/// Two rules beyond the obvious clamp, both about a volume that is already
/// above the ceiling — something another application can easily do:
///
/// - turning it **up** does nothing, rather than pinning it to the ceiling,
///   because "louder" that makes it quieter is a worse answer than nothing;
/// - turning it **down** snaps to the ceiling first, so the first press brings
///   an out-of-range volume back into range instead of stepping down from
///   wherever it was.
pub fn step(current: u32, delta: i32, max_percent: u32) -> Option<u32> {
    match delta.signum() {
        1 => {
            if current >= max_percent {
                None
            } else {
                Some(
                    current
                        .saturating_add(delta.unsigned_abs())
                        .min(max_percent),
                )
            }
        }
        -1 => {
            if current > max_percent {
                Some(max_percent)
            } else {
                Some(current.saturating_sub(delta.unsigned_abs()))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hundred_percent_is_normal_volume() {
        assert_eq!(to_volume(100), Volume::NORMAL);
        assert_eq!(to_percent(Volume::NORMAL), 100);
    }

    #[test]
    fn nothing_can_ask_for_more_than_pulse_can_store() {
        assert_eq!(to_volume(u32::MAX), Volume::MAX);
    }

    #[test]
    fn representable_is_idempotent() {
        for percent in [0, 1, 50, 100, 153, u32::MAX] {
            let once = representable(percent);
            assert_eq!(representable(once), once, "{percent}");
        }
    }

    #[test]
    fn overdrive_is_the_only_way_past_a_hundred() {
        assert_eq!(max_percent(false), 100);
        let overdriven = max_percent(true);
        assert!(overdriven > 100, "{overdriven} is not overdrive");
        assert_eq!(overdriven, ui_max_percent());
    }

    #[test]
    fn the_recommended_maximum_is_within_what_pulse_can_store() {
        assert!(ui_max_percent() >= 100);
        assert!(ui_max_percent() <= representable(u32::MAX));
    }

    #[test]
    fn clamping_follows_the_policy() {
        assert_eq!(clamp(50, 100), 50);
        assert_eq!(clamp(100, 100), 100);
        assert_eq!(clamp(150, 100), 100);
        assert_eq!(clamp(u32::MAX, 100), 100);

        let overdriven = max_percent(true);
        assert_eq!(clamp(120, overdriven), 120);
        assert_eq!(clamp(overdriven + 10, overdriven), overdriven);
    }

    #[test]
    fn a_step_stays_inside_the_ceiling() {
        assert_eq!(step(50, 5, 100), Some(55));
        assert_eq!(step(98, 5, 100), Some(100));
        assert_eq!(step(100, 5, 100), None);
        assert_eq!(step(50, -5, 100), Some(45));
        assert_eq!(step(2, -5, 100), Some(0));
        assert_eq!(step(0, -5, 100), Some(0));
    }

    #[test]
    fn a_volume_above_the_ceiling_comes_back_before_it_steps_down() {
        assert_eq!(step(140, 5, 100), None, "louder is not an answer");
        assert_eq!(step(140, -5, 100), Some(100), "the first press snaps back");
        assert_eq!(step(100, -5, 100), Some(95));
    }

    #[test]
    fn a_zero_step_is_not_a_command() {
        assert_eq!(step(50, 0, 100), None);
    }

    #[test]
    fn the_default_step_is_the_one_the_docs_promise() {
        assert_eq!(DEFAULT_STEP, 5);
    }
}
