//! Small shared animation helpers for widget-local tick animations.

/// Frame interval for lightweight timeout-driven animations (~60fps).
pub(crate) const FRAME_INTERVAL_MS: u32 = 16;

/// Cubic ease-out for short UI motion that should start quickly and settle gently.
pub(crate) fn ease_out_cubic(progress: f64) -> f64 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

/// Convert a duration into at least one fixed-rate animation step.
pub(crate) fn animation_steps(duration_ms: i32, frame_interval_ms: u32) -> i32 {
    (duration_ms / frame_interval_ms as i32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_reaches_expected_bounds() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn animation_steps_never_returns_zero() {
        assert_eq!(animation_steps(0, FRAME_INTERVAL_MS), 1);
        assert_eq!(animation_steps(15, FRAME_INTERVAL_MS), 1);
        assert_eq!(animation_steps(150, FRAME_INTERVAL_MS), 9);
    }
}
