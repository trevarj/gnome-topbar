//! When to fetch next.
//!
//! Two rules and no clock, so both are testable:
//!
//! - **Success** schedules the configured interval with up to ±10% of jitter.
//!   Every panel on every machine would otherwise ask Open-Meteo for the
//!   weather at the same second past the half hour, having all been started by
//!   the same `spawn-at-startup` line.
//! - **Failure** schedules a backoff that starts at a minute and doubles up to
//!   the interval, so a service that is down is asked once a minute at first
//!   and then progressively left alone. The ladder is deliberately *not*
//!   jittered: it is short, it is exact, and a test can read it.
//!
//! Being offline is not a failure and does not touch either: the task simply
//! stops scheduling and fetches the moment connectivity comes back.

use std::time::Duration;

/// The first retry after a failure.
const BACKOFF_START: Duration = Duration::from_secs(60);
/// The most the steady-state interval may be moved, either way.
const JITTER: f64 = 0.10;

/// The refresh schedule for one weather service.
#[derive(Debug)]
pub struct Refresh {
    interval: Duration,
    /// The last backoff handed out, while the service is failing.
    backoff: Option<Duration>,
    jitter: Jitter,
}

impl Refresh {
    /// A schedule refreshing every `interval`.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            backoff: None,
            jitter: Jitter::from_clock(),
        }
    }

    /// How long until the next fetch, after one succeeded.
    pub fn succeeded(&mut self) -> Duration {
        self.backoff = None;
        self.jitter.spread(self.interval)
    }

    /// How long until the next attempt, after one failed.
    pub fn failed(&mut self) -> Duration {
        let next = match self.backoff {
            None => BACKOFF_START,
            Some(previous) => previous.saturating_mul(2),
        }
        // An interval shorter than the first backoff step (the config minimum
        // is 60s) must not make a failing service retried *slower* than a
        // working one.
        .min(self.interval.max(BACKOFF_START));
        self.backoff = Some(next);
        next
    }
}

/// A small deterministic spreader.
///
/// An xorshift rather than a `rand` dependency: the requirement is "not the
/// same second on every machine", not statistical quality.
#[derive(Debug)]
struct Jitter {
    state: u64,
}

impl Jitter {
    /// Seed from the wall clock, which differs per machine and per start.
    fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x2545_F491_4F6C_DD1D, |since| since.subsec_nanos().into());
        Self::seeded(nanos ^ u64::from(std::process::id()))
    }

    /// Seed explicitly, for the tests.
    fn seeded(seed: u64) -> Self {
        Self {
            // Zero is xorshift's fixed point; anything else will do.
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// The next value in the sequence.
    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    /// `base`, moved by up to [`JITTER`] in either direction.
    fn spread(&mut self, base: Duration) -> Duration {
        let span = base.as_secs_f64() * JITTER;
        // `next()` mapped onto -1.0..=1.0.
        let offset = (self.next() % 2_001) as f64 / 1_000.0 - 1.0;
        Duration::from_secs_f64((base.as_secs_f64() + span * offset).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(1800);

    #[test]
    fn a_successful_refresh_waits_the_interval_give_or_take_a_tenth() {
        let mut refresh = Refresh::new(INTERVAL);
        for _ in 0..1_000 {
            let wait = refresh.succeeded();
            assert!(
                wait >= Duration::from_secs(1620) && wait <= Duration::from_secs(1980),
                "{wait:?} is outside 1800s ±10%"
            );
        }
    }

    #[test]
    fn jitter_actually_spreads() {
        let mut jitter = Jitter::seeded(1);
        let first = jitter.spread(INTERVAL);
        let differs = (0..20).any(|_| jitter.spread(INTERVAL) != first);
        assert!(differs, "every interval came back identical");
    }

    #[test]
    fn a_zero_seed_still_produces_a_sequence() {
        let mut jitter = Jitter::seeded(0);
        assert_ne!(jitter.next(), 0);
    }

    #[test]
    fn failures_back_off_by_doubling_up_to_the_interval() {
        let mut refresh = Refresh::new(INTERVAL);
        assert_eq!(refresh.failed(), Duration::from_secs(60));
        assert_eq!(refresh.failed(), Duration::from_secs(120));
        assert_eq!(refresh.failed(), Duration::from_secs(240));
        assert_eq!(refresh.failed(), Duration::from_secs(480));
        assert_eq!(refresh.failed(), Duration::from_secs(960));
        // 1920 would be past the interval, so the ladder stops there.
        assert_eq!(refresh.failed(), INTERVAL);
        assert_eq!(refresh.failed(), INTERVAL);
    }

    #[test]
    fn a_success_clears_the_ladder() {
        let mut refresh = Refresh::new(INTERVAL);
        refresh.failed();
        refresh.failed();
        assert_eq!(refresh.failed(), Duration::from_secs(240));

        let _ = refresh.succeeded();
        assert_eq!(refresh.failed(), Duration::from_secs(60));
    }

    #[test]
    fn a_short_interval_never_makes_a_failing_service_slower_than_a_working_one() {
        // The config minimum is 60s, which is also the first backoff step.
        let mut refresh = Refresh::new(Duration::from_secs(60));
        assert_eq!(refresh.failed(), Duration::from_secs(60));
        assert_eq!(refresh.failed(), Duration::from_secs(60));
    }
}
