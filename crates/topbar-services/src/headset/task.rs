//! The one owner of the headset reading: a timer and one subprocess at a time.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, trace};

use crate::headset::{HeadsetState, model};
use crate::proc::{self, CmdSpec};

/// How long the tool may take before the poll is abandoned.
///
/// It talks to a HID device over USB; a reading takes milliseconds, and
/// anything approaching this means the dongle has stopped answering.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How much of its output is kept. The document is a few hundred bytes.
const OUTPUT_CAP: usize = 64 * 1024;

/// The arguments that ask for a live battery reading in JSON.
///
/// `-b` is load-bearing: without it the tool prints what the device *can* do
/// and no `battery` block at all, which the parser correctly reads as "nothing
/// to report" — a headset that would then never appear. It exits non-zero when
/// no device answers, and still prints usable JSON while doing so, which is why
/// the status is not checked here and the output is.
const ARGUMENTS: [&str; 3] = ["-b", "-o", "json"];

/// What the poll is: which tool, how often.
///
/// Sent again when `[widgets.headset]` changes under a running panel, which is
/// what makes a reloaded interval take effect without a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Poll {
    /// The tool to run.
    pub(crate) command: String,
    /// How long between readings.
    pub(crate) interval: Duration,
}

/// What the panel can ask of the poll between its own ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Poll this way from now on. A reload sends it.
    Configure(Poll),
    /// Read the headset now, whatever the schedule said. A resume sends it:
    /// the dongle may not even have been there when the machine went to sleep.
    Now,
}

/// Poll `headsetcontrol` until the last handle is dropped.
pub(crate) async fn run(
    publisher: watch::Sender<Arc<HeadsetState>>,
    poll: Poll,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
) {
    let mut poll = poll;
    let mut spec = command_line(&poll);
    let mut ticker = timer(poll.interval);

    loop {
        tokio::select! {
            asked = commands.recv() => {
                let Some(asked) = asked else {
                    // The service handle is gone, which only happens when the
                    // panel is on its way out.
                    break;
                };
                match asked {
                    Command::Configure(next) if next == poll => continue,
                    Command::Configure(next) => {
                        debug!(
                            "the headset is now read with `{}` every {:?}",
                            next.command, next.interval
                        );
                        poll = next;
                        spec = command_line(&poll);
                        ticker = timer(poll.interval);
                    }
                    // A fresh timer's first tick is immediate, so re-arming it
                    // *is* "read now" — and it re-aligns the schedule to the
                    // resume, which is where the user's attention is.
                    Command::Now => ticker = timer(poll.interval),
                }
            }
            _ = ticker.tick() => {
                if publisher.is_closed() {
                    break;
                }

                // Awaited rather than spawned, so two `headsetcontrol`
                // processes can never be talking to the same HID device at
                // once.
                let reading = match proc::capture(&spec).await {
                    Ok(captured) => model::parse(&captured.stdout),
                    Err(error) => {
                        // The usual reason is that the tool is not installed,
                        // which is the normal state of most machines rather
                        // than a fault.
                        trace!("the headset could not be read: {error}");
                        None
                    }
                };

                let state = HeadsetState { reading };
                if **publisher.borrow() == state {
                    continue;
                }
                debug!("the headset is now {:?}", state.reading);
                let _ = publisher.send(Arc::new(state));
            }
        }
    }

    debug!("the headset service has no subscribers left; stopping");
}

/// The command line one reading takes.
fn command_line(poll: &Poll) -> CmdSpec {
    let mut argv = vec![poll.command.clone()];
    argv.extend(ARGUMENTS.iter().map(|argument| (*argument).to_string()));
    CmdSpec {
        argv,
        timeout: TIMEOUT,
        max_output_bytes: OUTPUT_CAP,
    }
}

/// A timer that does not burst after a slow reading.
fn timer(interval: Duration) -> tokio::time::Interval {
    let mut ticker = tokio::time::interval(interval);
    // A poll that overran its interval must not be followed by a burst of
    // catch-up polls: the reading that matters is the current one.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_battery_flag_is_asked_for_explicitly() {
        // Without `-b` the tool reports capabilities and no reading, which is
        // indistinguishable from a headset that is switched off.
        assert_eq!(ARGUMENTS, ["-b", "-o", "json"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_fake_tool_on_the_path_is_read_and_published() {
        let script = std::env::temp_dir().join(format!("topbar-hs-{}.sh", std::process::id()));
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '{\"devices\":[{\"status\":\"success\",\"device\":\"Fake\",\"battery\":{\"status\":\"BATTERY_AVAILABLE\",\"level\":45}}]}'\n",
        )
        .expect("the script is written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("the script is executable");
        }

        let (publisher, state) = watch::channel(Arc::new(HeadsetState::default()));
        let (_commands, queue) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(run(
            publisher,
            Poll {
                command: script.display().to_string(),
                interval: Duration::from_millis(50),
            },
            queue,
        ));

        let mut state = state;
        let settled = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let current = state.borrow_and_update();
                    if current.reading.is_some() {
                        return current.clone();
                    }
                }
                state.changed().await.expect("the task is alive");
            }
        })
        .await
        .expect("a reading from the fake tool");

        let reading = settled.reading.as_ref().expect("a reading");
        assert_eq!(reading.percent, 45);
        assert_eq!(reading.name.as_deref(), Some("Fake"));

        task.abort();
        let _ = std::fs::remove_file(&script);
    }
}
