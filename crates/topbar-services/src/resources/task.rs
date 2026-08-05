//! The sampler: read three files, publish a snapshot, sleep.
//!
//! The one thing here that is not obvious is what happens across a **suspend**.
//! `/proc/stat`'s counters keep the machine's own idea of time, so a sample
//! taken before a laptop was shut and one taken after it was opened are hours
//! apart in jiffies and about a second apart in the user's experience. The
//! delta between them is arithmetically valid and completely meaningless — it
//! reads as whatever the machine averaged overnight.
//!
//! So the sampler discards a delta that spans more wall-clock time than it
//! should have. That is cheaper and more reliable than subscribing to logind's
//! resume signal for it, and it covers the other case that produces the same
//! symptom: a task starved for a few seconds under load.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::debug;

use super::model::{
    CpuSample, ResourceState, cpu_usage_percent, measure, parse_cpu_sample, parse_memory,
    parse_mountinfo,
};

/// How much later than scheduled a sample may arrive and still be trusted.
///
/// Twice the interval plus a second: enough slack for a loaded machine, not
/// nearly enough for a suspend.
fn stale_after(interval: Duration) -> Duration {
    interval * 2 + Duration::from_secs(1)
}

/// What a pair of samples says about the CPU, if anything.
///
/// Pure, and separated from the sampler for one reason: on an idle machine two
/// readings taken microseconds apart differ by zero jiffies, so a test that
/// sampled `/proc` twice and asserted a number would pass or fail depending on
/// what else the machine happened to be doing.
fn cpu_reading(
    previous: Option<(CpuSample, Instant)>,
    current: Option<CpuSample>,
    now: Instant,
    interval: Duration,
) -> Option<u8> {
    let (earlier, taken) = previous?;
    let current = current?;
    if now.duration_since(taken) > stale_after(interval) {
        // A delta across a suspend is arithmetically valid and completely
        // meaningless. Skip a reading rather than draw one.
        debug!("resources: discarding a stale CPU delta");
        return None;
    }
    cpu_usage_percent(earlier, current)
}

/// What the sampler can be asked to do between readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Sample this often from now on.
    Interval(Duration),
    /// Throw the previous CPU sample away.
    ///
    /// CPU usage is a delta between two readings of `/proc/stat` and a wall
    /// clock. After a suspend the two disagree by however long the machine was
    /// asleep, and the percentage that comes out of them is meaningless —
    /// usually a spike, occasionally a negative that clamps to zero. The next
    /// reading has to start a fresh pair.
    Discard,
}

/// Sample until every subscriber is gone.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<ResourceState>>,
    mut interval: Duration,
) {
    let mut previous: Option<(CpuSample, Instant)> = None;

    loop {
        let state = sample(&mut previous, interval);
        publisher.send_if_modified(|current| {
            if **current == state {
                false
            } else {
                *current = Arc::new(state);
                true
            }
        });

        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            wanted = commands.recv() => match wanted {
                Some(Command::Interval(wanted)) => {
                    debug!("resources: sampling every {wanted:?}");
                    interval = wanted;
                }
                Some(Command::Discard) => {
                    debug!("resources: dropping the sample the machine slept through");
                    previous = None;
                }
                // Only the handle is gone, not the subscribers: keep sampling.
                None => tokio::time::sleep(interval).await,
            },
            () = publisher.closed() => break,
        }
    }

    debug!("the resources service has no subscribers left; stopping");
}

/// One reading of the machine.
fn sample(previous: &mut Option<(CpuSample, Instant)>, interval: Duration) -> ResourceState {
    let now = Instant::now();
    let current = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|stat| parse_cpu_sample(&stat));

    let cpu_pct = cpu_reading(*previous, current, now, interval);
    if let Some(current) = current {
        *previous = Some((current, now));
    }

    let memory = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|meminfo| parse_memory(&meminfo))
        .unwrap_or_default();

    let mut disks = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|mountinfo| parse_mountinfo(&mountinfo))
        .unwrap_or_default();
    // A filesystem `statvfs` will not answer for — an automount that has gone
    // away, a permission the user does not have — is dropped rather than shown
    // as a zero-byte disk.
    disks.retain_mut(measure);

    ResourceState {
        cpu_pct,
        memory,
        disks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_that_arrived_on_time_is_trusted_and_one_from_yesterday_is_not() {
        let interval = Duration::from_secs(5);
        assert_eq!(stale_after(interval), Duration::from_secs(11));
        assert!(Duration::from_secs(5) <= stale_after(interval));
        assert!(
            Duration::from_secs(8) <= stale_after(interval),
            "a loaded machine gets slack"
        );
        assert!(
            Duration::from_secs(3600) > stale_after(interval),
            "a laptop that was shut overnight does not"
        );
    }

    /// Two samples five seconds apart, half of it busy.
    fn pair() -> (CpuSample, CpuSample) {
        let earlier = parse_cpu_sample("cpu  100 0 0 100 0\n").expect("a sample");
        let later = parse_cpu_sample("cpu  150 0 0 150 0\n").expect("a sample");
        (earlier, later)
    }

    const INTERVAL: Duration = Duration::from_secs(5);

    #[test]
    fn the_first_sample_of_a_session_produces_no_cpu_reading() {
        let (_, later) = pair();
        assert_eq!(
            cpu_reading(None, Some(later), Instant::now(), INTERVAL),
            None,
            "there is nothing to subtract from"
        );

        // But it is kept, so the next one has something to subtract from.
        let mut previous = None;
        sample(&mut previous, INTERVAL);
        assert!(previous.is_some());
    }

    #[test]
    fn a_pair_taken_on_time_produces_a_reading() {
        let (earlier, later) = pair();
        let now = Instant::now();
        assert_eq!(
            cpu_reading(Some((earlier, now - INTERVAL)), Some(later), now, INTERVAL),
            Some(50)
        );
    }

    #[test]
    fn a_delta_across_a_suspend_is_thrown_away() {
        let (earlier, later) = pair();
        let now = Instant::now();
        // An hour, which is what shutting a laptop lid does to a sample: the
        // jiffies are real and the number they produce means nothing.
        let overnight = now - Duration::from_secs(3600);
        assert_eq!(
            cpu_reading(Some((earlier, overnight)), Some(later), now, INTERVAL),
            None,
            "a reading averaged over the night is not a reading"
        );
    }

    #[test]
    fn a_machine_that_was_merely_busy_still_gets_its_reading() {
        // Twice the interval plus a second is slack for a starved task, not
        // for a suspend — the two look identical to `/proc` and only one of
        // them should cost a reading.
        let (earlier, later) = pair();
        let now = Instant::now();
        let late = now - Duration::from_secs(10);
        assert_eq!(
            cpu_reading(Some((earlier, late)), Some(later), now, INTERVAL),
            Some(50)
        );
    }

    #[test]
    fn a_proc_stat_that_could_not_be_read_produces_nothing() {
        let (earlier, _) = pair();
        assert_eq!(
            cpu_reading(
                Some((earlier, Instant::now())),
                None,
                Instant::now(),
                INTERVAL
            ),
            None
        );
    }

    #[test]
    fn a_sample_of_this_machine_reads_all_three_files() {
        let mut previous = None;
        let state = sample(&mut previous, Duration::from_secs(5));
        assert!(state.memory.total_kib > 0, "every machine has memory");
        assert!(!state.disks.is_empty(), "and a root filesystem to measure");
        assert!(state.disks.iter().all(|disk| disk.total > 0));
    }
}
