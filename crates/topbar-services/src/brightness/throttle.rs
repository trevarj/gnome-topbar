//! One brightness call in flight at a time, and always the latest value.
//!
//! Dragging a brightness slider produces a value per frame. v1 sent each one
//! with a blocking D-Bus call on the GTK main thread, so a drag was sixty
//! synchronous round trips a second and the slider stuttered under its own
//! feedback. The fix is not a debounce — a debounce makes the screen lag the
//! finger — but a coalescing gate: send the first value immediately, hold the
//! newest of everything that arrives while that call is outstanding, and send
//! exactly that one when it lands.
//!
//! The result is a call rate that adapts to how fast logind is answering, with
//! the final value always sent, which is the only one the user checks.

/// The gate. Pure: no clock, no I/O, one bool and one `Option`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Throttle {
    in_flight: bool,
    pending: Option<u32>,
}

impl Throttle {
    /// An idle gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask for `percent`. Returns the value to send now, if any.
    ///
    /// A value arriving while a call is outstanding replaces whatever was
    /// waiting: nothing is queued, because nobody wants to watch a backlog of
    /// brightness values replay.
    pub fn request(&mut self, percent: u32) -> Option<u32> {
        if self.in_flight {
            self.pending = Some(percent);
            return None;
        }
        self.in_flight = true;
        Some(percent)
    }

    /// The outstanding call landed. Returns the next value to send, if any.
    pub fn finished(&mut self) -> Option<u32> {
        match self.pending.take() {
            Some(percent) => {
                // Still in flight: the call this returns is the new one.
                Some(percent)
            }
            None => {
                self.in_flight = false;
                None
            }
        }
    }

    /// Whether a call is outstanding.
    #[cfg(test)]
    pub fn is_busy(&self) -> bool {
        self.in_flight
    }

    /// Forget the outstanding call: the device went away.
    #[cfg(test)]
    pub fn reset(&mut self) {
        self.in_flight = false;
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a whole drag through the gate, counting the calls it made.
    fn drive(values: &[u32]) -> Vec<u32> {
        let mut throttle = Throttle::new();
        let mut sent = Vec::new();
        for &value in values {
            if let Some(percent) = throttle.request(value) {
                sent.push(percent);
            }
        }
        // Every outstanding call eventually lands.
        while let Some(percent) = throttle.finished() {
            sent.push(percent);
        }
        sent
    }

    #[test]
    fn the_first_value_goes_straight_out() {
        let mut throttle = Throttle::new();
        assert_eq!(throttle.request(50), Some(50));
        assert!(throttle.is_busy());
    }

    #[test]
    fn a_quiet_gate_returns_to_idle() {
        let mut throttle = Throttle::new();
        throttle.request(50);
        assert_eq!(throttle.finished(), None);
        assert!(!throttle.is_busy());
        assert_eq!(throttle.request(60), Some(60), "and takes the next one");
    }

    #[test]
    fn only_the_newest_value_waits() {
        let mut throttle = Throttle::new();
        assert_eq!(throttle.request(10), Some(10));
        assert_eq!(throttle.request(20), None);
        assert_eq!(throttle.request(30), None);
        assert_eq!(throttle.request(40), None);
        assert_eq!(
            throttle.finished(),
            Some(40),
            "the ones between are dropped"
        );
        assert_eq!(throttle.finished(), None);
    }

    #[test]
    fn a_drag_costs_two_calls_however_long_it_is() {
        // The first value and the last one; everything between is coalesced.
        let drag: Vec<u32> = (1..=60).collect();
        assert_eq!(drive(&drag), vec![1, 60]);
    }

    #[test]
    fn the_final_value_is_never_the_one_that_is_dropped() {
        for length in 1..40u32 {
            let drag: Vec<u32> = (1..=length).collect();
            let sent = drive(&drag);
            assert_eq!(
                sent.last().copied(),
                Some(length),
                "a {length}-step drag ended on the wrong value"
            );
        }
    }

    #[test]
    fn a_reset_forgets_a_call_nothing_is_going_to_answer() {
        let mut throttle = Throttle::new();
        throttle.request(10);
        throttle.request(20);
        throttle.reset();
        assert!(!throttle.is_busy());
        assert_eq!(throttle.finished(), None);
        assert_eq!(throttle.request(30), Some(30));
    }

    #[test]
    fn an_idle_gate_that_lands_anyway_stays_idle() {
        // A stray completion — a call that failed and was already reset — must
        // not make the gate believe it has work.
        let mut throttle = Throttle::new();
        assert_eq!(throttle.finished(), None);
        assert_eq!(throttle, Throttle::new());
    }
}
