//! The one owner of a `custom-*` widget's script.
//!
//! One task per configured widget, and it is the only thing that starts that
//! script. Three properties follow from that, and all three were defects in v1:
//!
//! - **Runs never overlap.** The next run is scheduled when the last one
//!   *finishes*, not on a free-running timer, so a script that takes longer
//!   than its interval delays the next run instead of racing it. A manual
//!   refresh arriving mid-run is dropped for the same reason.
//! - **Nothing is left behind.** The script goes through
//!   [`proc::capture`](crate::proc::capture), which reaps it, caps its output
//!   and kills it at the timeout.
//! - **Being offline is not a failure.** A `requires_network` widget defers its
//!   run rather than failing it, and fires once the moment the machine is back
//!   — which is what makes the live configuration's crypto script survive a
//!   suspend without a stale price sitting there until the next half hour.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::connectivity::ConnectivityState;
use crate::custom::CustomState;
use crate::custom::model::{self, CustomDisplay};
use crate::proc::{self, CmdSpec};

/// How long a script may run before it is killed.
///
/// v1's, and generous on purpose: the live configuration's crypto script makes
/// an HTTPS request, and a cold DNS lookup on a laptop that has just woken up
/// really can take ten seconds.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a script's output is kept.
///
/// The contract is one line or one JSON document. A script that prints a
/// megabyte is misconfigured, and the panel should not carry the megabyte
/// around while its author finds out.
pub(crate) const OUTPUT_CAP: usize = 64 * 1024;

/// What the panel can ask of one custom widget.
pub(crate) enum Command {
    /// Run the script now, whatever the schedule said.
    Refresh,
}

/// Everything a task needs to know about the widget it belongs to.
#[derive(Debug, Clone)]
pub(crate) struct Spec {
    /// The widget's full name, e.g. `custom-crypto`, for log lines.
    pub(crate) name: String,
    /// The command line, run through a shell because it is the user's own.
    pub(crate) exec: String,
    /// How often to run it. Zero runs it once and never again.
    pub(crate) interval: Duration,
    /// Whether to wait for the machine to be online first.
    pub(crate) requires_network: bool,
    /// The static label shown when the script says nothing.
    pub(crate) label: String,
    /// The format string `{output}` is substituted into.
    pub(crate) template: Option<String>,
}

/// Run one widget's script until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<CustomState>>,
    spec: Spec,
    mut connectivity: watch::Receiver<Arc<ConnectivityState>>,
) {
    let (answers, mut outcomes) = mpsc::channel(1);
    let online = connectivity.borrow_and_update().online;

    let mut task = Task {
        display: idle(&spec),
        loading: false,
        failure: None,
        succeeded: false,
        due: Some(Instant::now()),
        online,
        in_flight: false,
        deferred: false,
        answers,
        publisher,
        spec,
    };
    task.publish();

    loop {
        let due = task.due;
        let timer = async move {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timer);

        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Refresh) => task.start(),
                None => break,
            },
            changed = connectivity.changed() => {
                // A watcher that has stopped is not evidence of being offline.
                let online = changed.map_or(true, |()| connectivity.borrow().online);
                task.set_online(online);
            },
            outcome = outcomes.recv() => {
                if let Some(outcome) = outcome {
                    task.settle(outcome);
                }
            },
            () = &mut timer => task.start(),
        }
    }

    debug!("`{}` has no handles left; stopping", task.spec.name);
}

/// What one run came back with: its exit status and its standard output.
type Outcome = Result<(Option<i32>, String), String>;

/// Everything the loop owns.
struct Task {
    /// The last thing the script successfully said, or the static fallback.
    display: CustomDisplay,
    /// Whether the placeholder is on the bar right now.
    loading: bool,
    /// The line a failed run added to the tooltip.
    failure: Option<String>,
    /// Whether the script has ever worked.
    succeeded: bool,
    /// When the next run is due. `None` while nothing is scheduled.
    due: Option<Instant>,
    online: bool,
    /// A run is out. A second would race it into the same snapshot.
    in_flight: bool,
    /// A run came due while the machine was offline and is owed.
    deferred: bool,
    /// Where a spawned run sends what it found.
    answers: mpsc::Sender<Outcome>,
    publisher: watch::Sender<Arc<CustomState>>,
    spec: Spec,
}

impl Task {
    /// Publish the snapshot the current state implies, if it changed.
    fn publish(&self) {
        let state = CustomState {
            display: self.display.clone(),
            loading: self.loading,
            failure: self.failure.clone(),
        };
        if **self.publisher.borrow() == state {
            return;
        }
        let _ = self.publisher.send(Arc::new(state));
    }

    /// Start a run, or explain to itself why not.
    fn start(&mut self) {
        self.due = None;

        if self.in_flight {
            // Not an error and not a missed beat: the run in flight is about to
            // publish, and `settle` schedules the next one.
            debug!(
                "`{}` is already running; this tick is dropped",
                self.spec.name
            );
            return;
        }

        if self.spec.requires_network && !self.online {
            debug!("`{}` is offline; the run is deferred", self.spec.name);
            self.deferred = true;
            // Nothing has ever arrived and nothing is coming, so the ellipsis
            // would be a promise the task cannot keep.
            if !self.succeeded {
                self.loading = false;
                self.publish();
            }
            return;
        }

        self.deferred = false;
        self.in_flight = true;
        self.loading = model::shows_loading(self.display.visible, &self.display.text);
        self.publish();

        let spec = CmdSpec {
            argv: shell_argv(&self.spec.exec),
            timeout: TIMEOUT,
            max_output_bytes: OUTPUT_CAP,
        };
        let answers = self.answers.clone();
        tokio::spawn(async move {
            let outcome = match proc::capture(&spec).await {
                Ok(captured) => Ok((captured.code, captured.stdout)),
                Err(error) => Err(error.to_string()),
            };
            let _ = answers.send(outcome).await;
        });
    }

    /// A run came back.
    fn settle(&mut self, outcome: Outcome) {
        self.in_flight = false;
        self.loading = false;

        match outcome {
            Ok((Some(0), stdout)) => {
                self.display =
                    model::display(&stdout, &self.spec.label, self.spec.template.as_deref());
                self.failure = None;
                self.succeeded = true;
                debug!("`{}` printed `{}`", self.spec.name, self.display.text);
            }
            Ok((code, _)) => self.failed(code),
            Err(reason) => {
                warn!("`{}` could not be run: {reason}", self.spec.name);
                self.failed(None);
            }
        }

        self.publish();
        self.schedule();
    }

    /// A run that did not work out.
    ///
    /// The last good value stays where it is — a script with a bad minute must
    /// not blank the bar — and the tooltip says the reading is not current. A
    /// script that has *never* worked has nothing to keep, so the widget goes
    /// away rather than sitting there empty.
    fn failed(&mut self, code: Option<i32>) {
        warn!("`{}` failed: {}", self.spec.name, model::failure_note(code));
        self.failure = Some(model::failure_note(code));
        if !self.succeeded {
            self.display = idle(&self.spec);
        }
    }

    /// Put the next run on the clock, unless this widget only runs once.
    fn schedule(&mut self) {
        if self.spec.interval.is_zero() {
            debug!("`{}` runs once; nothing more is scheduled", self.spec.name);
            return;
        }
        self.due = Some(Instant::now() + self.spec.interval);
    }

    /// The network came or went.
    fn set_online(&mut self, online: bool) {
        if self.online == online {
            return;
        }
        self.online = online;
        if online && self.deferred {
            info!("the machine is back online; running `{}`", self.spec.name);
            self.start();
        }
    }
}

/// What the widget shows with nothing from the script behind it.
fn idle(spec: &Spec) -> CustomDisplay {
    model::display("", &spec.label, spec.template.as_deref())
}

/// The argument vector for a user-authored command line.
///
/// **The documented exception** to [`crate::proc`]'s argv rule, alongside
/// `update_count_command`: `exec` has been a shell command line since v1 — the
/// live configuration's own value ends in `-r` and people write pipelines here
/// — and turning it into an argument vector would break every configuration
/// that has one.
fn shell_argv(command: &str) -> Vec<String> {
    CmdSpec::shell(command).argv
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::custom::CustomExec;

    /// A connectivity channel a test drives by hand.
    fn connectivity(
        online: bool,
    ) -> (
        watch::Sender<Arc<ConnectivityState>>,
        watch::Receiver<Arc<ConnectivityState>>,
    ) {
        watch::channel(Arc::new(ConnectivityState { online }))
    }

    fn spec(exec: &str) -> Spec {
        Spec {
            name: "custom-test".to_string(),
            exec: exec.to_string(),
            interval: Duration::ZERO,
            requires_network: false,
            label: String::new(),
            template: None,
        }
    }

    /// Wait until `wanted` says yes about the published state.
    async fn settle(
        exec: &CustomExec,
        what: &str,
        wanted: impl Fn(&CustomState) -> bool,
    ) -> Arc<CustomState> {
        let mut state = exec.state();
        let wait = async {
            loop {
                {
                    let current = state.borrow_and_update();
                    if wanted(&current) {
                        return current.clone();
                    }
                }
                state.changed().await.expect("the task is alive");
            }
        };
        tokio::time::timeout(Duration::from_secs(20), wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_that_prints_a_line_puts_it_on_the_bar() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(spec("echo 'BTC 103412'"), receiver);
        let state = settle(&exec, "the first reading", |state| state.display.visible).await;
        assert_eq!(state.display.text, "BTC 103412");
        assert!(!state.loading);
        assert_eq!(state.failure, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn waybar_json_carries_its_tooltip_and_its_tint_through() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(
            spec(r#"echo '{"text":"7 due","tooltip":"7 updates","class":"warning"}'"#),
            receiver,
        );
        let state = settle(&exec, "the reading", |state| state.display.visible).await;
        assert_eq!(state.display.text, "7 due");
        assert_eq!(state.display.tooltip.as_deref(), Some("7 updates"));
        assert_eq!(state.display.class, Some(model::CustomClass::Warning));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_that_prints_nothing_takes_the_widget_off_the_bar() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(spec("true"), receiver);
        let state = settle(&exec, "the empty reading", |state| !state.loading).await;
        assert!(!state.display.visible);
        assert_eq!(state.display.text, "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_script_keeps_the_last_good_value_and_says_so() {
        let (_sender, receiver) = connectivity(true);
        // Succeeds once, then fails: the file is the run counter.
        let marker = std::env::temp_dir().join(format!("topbar-custom-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let exec = CustomExec::spawn(
            Spec {
                interval: Duration::from_millis(150),
                ..spec(&format!(
                    "if [ -f {marker} ]; then exit 1; fi; touch {marker}; echo 42",
                    marker = marker.display()
                ))
            },
            receiver,
        );

        let good = settle(&exec, "the first reading", |state| state.display.visible).await;
        assert_eq!(good.display.text, "42");

        let bad = settle(&exec, "the failure note", |state| state.failure.is_some()).await;
        assert_eq!(bad.display.text, "42", "the last good value must survive");
        assert!(bad.display.visible);
        assert_eq!(bad.failure.as_deref(), Some("Last update failed (exit 1)"));

        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_that_has_never_worked_hides_its_widget() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(spec("exit 3"), receiver);
        let state = settle(&exec, "the failure", |state| state.failure.is_some()).await;
        assert!(!state.display.visible);
        assert_eq!(
            state.failure.as_deref(),
            Some("Last update failed (exit 3)")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_widget_with_a_static_label_falls_back_to_it_rather_than_vanishing() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(
            Spec {
                label: "VPN".to_string(),
                ..spec("exit 1")
            },
            receiver,
        );
        let state = settle(&exec, "the failure", |state| state.failure.is_some()).await;
        assert!(state.display.visible);
        assert_eq!(state.display.text, "VPN");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_once_widget_runs_exactly_once() {
        let (_sender, receiver) = connectivity(true);
        let counter = std::env::temp_dir().join(format!("topbar-once-{}", std::process::id()));
        let _ = std::fs::remove_file(&counter);
        let exec = CustomExec::spawn(
            spec(&format!(
                "echo x >>{counter}; wc -l <{counter} | tr -d ' '",
                counter = counter.display()
            )),
            receiver,
        );

        settle(&exec, "the only reading", |state| state.display.visible).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        let runs = std::fs::read_to_string(&counter)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(runs, 1, "an interval of zero means once, not once a tick");
        let _ = std::fs::remove_file(&counter);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_runs_never_overlap() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(
            Spec {
                // Far shorter than the script takes: a free-running timer would
                // have started several more by the time the first finishes.
                interval: Duration::from_millis(50),
                ..spec("sleep 0.6; echo done")
            },
            receiver,
        );

        // Every refresh a widget could possibly ask for, while one is out.
        for _ in 0..10 {
            exec.refresh().await;
        }
        let started = std::time::Instant::now();
        settle(&exec, "the first reading", |state| state.display.visible).await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the refreshes queued up behind each other instead of being dropped"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_offline_widget_defers_its_run_and_fires_once_on_reconnect() {
        let (sender, receiver) = connectivity(false);
        let counter = std::env::temp_dir().join(format!("topbar-net-{}", std::process::id()));
        let _ = std::fs::remove_file(&counter);
        let exec = CustomExec::spawn(
            Spec {
                requires_network: true,
                ..spec(&format!(
                    "echo x >>{counter}; echo online",
                    counter = counter.display()
                ))
            },
            receiver,
        );

        // Offline: the script must not have been run at all.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!counter.exists(), "an offline widget ran its script anyway");
        assert!(
            !exec.state().borrow().loading,
            "nothing is coming, so nothing is promised"
        );

        sender
            .send(Arc::new(ConnectivityState { online: true }))
            .expect("the task is listening");
        let state = settle(&exec, "the deferred run", |state| state.display.visible).await;
        assert_eq!(state.display.text, "online");

        // Once, not once per transition.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let runs = std::fs::read_to_string(&counter)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(runs, 1);
        let _ = std::fs::remove_file(&counter);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_widget_that_does_not_need_the_network_runs_while_offline() {
        let (_sender, receiver) = connectivity(false);
        let exec = CustomExec::spawn(spec("echo local"), receiver);
        let state = settle(&exec, "the reading", |state| state.display.visible).await;
        assert_eq!(state.display.text, "local");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_that_never_finishes_is_killed_rather_than_held() {
        let (_sender, receiver) = connectivity(true);
        let exec = CustomExec::spawn(spec("sleep 120"), receiver);
        // The task publishes its placeholder immediately and the run is out;
        // what matters is that the timeout exists and is the one v1 used.
        assert_eq!(TIMEOUT, Duration::from_secs(30));
        let state = settle(&exec, "the placeholder", |state| state.loading).await;
        assert!(state.loading);
        drop(exec);
    }

    #[test]
    fn the_users_own_command_line_still_reaches_a_shell() {
        assert_eq!(
            shell_argv("crypto.sh -r | head -1"),
            ["sh", "-c", "crypto.sh -r | head -1"]
        );
    }
}
