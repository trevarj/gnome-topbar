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

/// Poll `command` every `interval` until the last handle is dropped.
pub(crate) async fn run(
    publisher: watch::Sender<Arc<HeadsetState>>,
    command: String,
    interval: Duration,
) {
    let mut argv = vec![command];
    argv.extend(ARGUMENTS.iter().map(|argument| (*argument).to_string()));
    let spec = CmdSpec {
        argv,
        timeout: TIMEOUT,
        max_output_bytes: OUTPUT_CAP,
    };

    let mut ticker = tokio::time::interval(interval);
    // A poll that overran its interval must not be followed by a burst of
    // catch-up polls: the reading that matters is the current one.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if publisher.is_closed() {
            break;
        }

        // Awaited rather than spawned, so two `headsetcontrol` processes can
        // never be talking to the same HID device at once.
        let reading = match proc::capture(&spec).await {
            Ok(captured) => model::parse(&captured.stdout),
            Err(error) => {
                // The usual reason is that the tool is not installed, which is
                // the normal state of most machines rather than a fault.
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

    debug!("the headset service has no subscribers left; stopping");
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
        let task = tokio::spawn(run(
            publisher,
            script.display().to_string(),
            Duration::from_millis(50),
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
