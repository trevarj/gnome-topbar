//! `custom-*` widgets: a script, an interval, and whatever it printed.
//!
//! ```text
//!   model.rs   the Waybar output contract and the display rules (pure)
//!   task.rs    one owner per widget: a timer, a subprocess, a snapshot
//! ```
//!
//! A `custom-*` widget is the escape hatch — the thing a panel needs so that
//! the one indicator nobody built in can still exist. The live configuration
//! uses it for a crypto-price script, and the contract it speaks is Waybar's,
//! because that is the contract every such script on the internet already
//! speaks: print a line, or print `{"text": …, "tooltip": …, "class": …}`.
//!
//! **The script is a service, not a widget.** It lives here rather than in the
//! GTK crate for the reason everything else does — a subprocess must not be
//! waited on from the main thread — and for one more: widgets are built per
//! monitor, so a script owned by a widget would run once per screen. One task
//! per configured widget, however many bars are drawing it.

pub mod model;
mod task;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use topbar_core::config::CustomWidgetConfig;
use tracing::debug;

pub use model::{CustomClass, CustomDisplay, CustomOutput};

use crate::connectivity::{Connectivity, ConnectivityState};
use task::{Command, Spec};

/// What stands in for a value that has not arrived yet.
///
/// An ellipsis rather than a spinner: v1 span a Cairo spinner here, which is an
/// animation running on a panel for something that finishes in under a second
/// and is invisible for the rest of the half hour.
pub const LOADING_TEXT: &str = "…";

/// Everything the panel knows about one `custom-*` widget.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomState {
    /// What to draw, once the template and the static fallback are applied.
    pub display: CustomDisplay,
    /// Whether a first run is out and there is nothing to show meanwhile.
    pub loading: bool,
    /// The line a failed run added to the tooltip, if the last one failed.
    pub failure: Option<String>,
}

impl CustomState {
    /// Whether the widget is on the bar at all.
    ///
    /// The placeholder counts: a widget showing `…` is a widget saying
    /// something, and taking it off the bar for the second the script runs
    /// would make the whole row shuffle.
    pub fn visible(&self) -> bool {
        self.loading || self.display.visible
    }

    /// The text on the bar.
    pub fn text(&self) -> &str {
        if self.loading {
            LOADING_TEXT
        } else {
            &self.display.text
        }
    }

    /// The tooltip, given the widget's configured one to fall back to.
    ///
    /// The script's own tooltip wins; a failure is appended rather than
    /// replacing anything, because "this is what it said, and it is not
    /// current" is two facts.
    pub fn tooltip(&self, configured: Option<&str>) -> Option<String> {
        let base = self
            .display
            .tooltip
            .as_deref()
            .or(configured)
            .filter(|text| !text.is_empty());
        match (base, self.failure.as_deref()) {
            (Some(base), Some(failure)) => Some(format!("{base}\n{failure}")),
            (Some(base), None) => Some(base.to_string()),
            (None, Some(failure)) => Some(failure.to_string()),
            (None, None) => None,
        }
    }
}

/// One custom widget's script, as a handle.
#[derive(Clone)]
pub struct CustomExec {
    state: watch::Receiver<Arc<CustomState>>,
    /// `None` for a widget with no `exec`: there is nothing to ask of it.
    commands: Option<mpsc::Sender<Command>>,
}

impl CustomExec {
    /// Subscribe to what the script last said.
    pub fn state(&self) -> watch::Receiver<Arc<CustomState>> {
        self.state.clone()
    }

    /// Run the script now.
    ///
    /// What a left click does after its `on_click` command has been started, so
    /// the label reflects whatever that command changed. Not fallible and not
    /// `#[must_use]`: a refresh that arrives while a run is out is dropped by
    /// design, and there is nothing a caller could do about it either way.
    pub async fn refresh(&self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Refresh).await;
        }
    }

    /// A widget with a script behind it.
    fn spawn(spec: Spec, connectivity: watch::Receiver<Arc<ConnectivityState>>) -> Self {
        let (commands, queue) = mpsc::channel(4);
        let (publisher, state) = watch::channel(Arc::new(CustomState::default()));
        tokio::spawn(task::run(queue, publisher, spec, connectivity));
        Self {
            state,
            commands: Some(commands),
        }
    }

    /// A widget with only an icon and a label: nothing runs, nothing changes.
    fn fixed(config: &CustomWidgetConfig) -> Self {
        let display = model::display("", &config.label, config.template.as_deref());
        let (_publisher, state) = watch::channel(Arc::new(CustomState {
            display,
            loading: false,
            failure: None,
        }));
        Self {
            state,
            commands: None,
        }
    }
}

/// Every configured `custom-*` widget, by name.
///
/// Cloning is cheap: it is a map of watch subscriptions.
#[derive(Clone, Default)]
pub struct CustomWidgets {
    widgets: BTreeMap<String, CustomExec>,
}

impl CustomWidgets {
    /// Start a task for every configured widget that has a script.
    pub(crate) fn start(
        configured: &BTreeMap<String, CustomWidgetConfig>,
        connectivity: &Connectivity,
    ) -> Self {
        let mut widgets = BTreeMap::new();
        for (name, config) in configured {
            let exec = match config
                .exec
                .as_deref()
                .map(str::trim)
                .filter(|exec| !exec.is_empty())
            {
                Some(exec) => {
                    debug!("`{name}` runs `{exec}` every {}s", config.interval);
                    CustomExec::spawn(
                        Spec {
                            name: name.clone(),
                            exec: exec.to_string(),
                            interval: Duration::from_secs(config.interval),
                            requires_network: config.requires_network,
                            label: config.label.clone(),
                            template: config.template.clone(),
                        },
                        connectivity.state(),
                    )
                }
                None => CustomExec::fixed(config),
            };
            widgets.insert(name.clone(), exec);
        }
        Self { widgets }
    }

    /// The widget called `name`, if it was configured.
    pub fn get(&self, name: &str) -> Option<&CustomExec> {
        self.widgets.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(text: &str) -> CustomState {
        CustomState {
            display: CustomDisplay {
                text: text.to_string(),
                visible: true,
                ..CustomDisplay::default()
            },
            loading: false,
            failure: None,
        }
    }

    #[test]
    fn a_widget_with_nothing_to_say_is_not_on_the_bar() {
        assert!(!CustomState::default().visible());
    }

    #[test]
    fn the_placeholder_keeps_the_widget_on_the_bar() {
        let loading = CustomState {
            loading: true,
            ..CustomState::default()
        };
        assert!(loading.visible());
        assert_eq!(loading.text(), "…");
    }

    #[test]
    fn a_value_beats_the_placeholder() {
        let state = ready("BTC 103412");
        assert_eq!(state.text(), "BTC 103412");
        assert!(state.visible());
    }

    #[test]
    fn the_configured_tooltip_is_what_a_silent_script_leaves_behind() {
        let state = ready("x");
        assert_eq!(
            state.tooltip(Some("Crypto prices")).as_deref(),
            Some("Crypto prices")
        );
        assert_eq!(state.tooltip(None), None);
        assert_eq!(state.tooltip(Some("")), None);
    }

    #[test]
    fn the_scripts_own_tooltip_wins() {
        let mut state = ready("x");
        state.display.tooltip = Some("BTC 103,412".to_string());
        assert_eq!(
            state.tooltip(Some("Crypto prices")).as_deref(),
            Some("BTC 103,412")
        );
    }

    #[test]
    fn a_failure_is_added_to_the_tooltip_rather_than_replacing_it() {
        let mut state = ready("x");
        state.failure = Some("Last update failed (exit 1)".to_string());
        assert_eq!(
            state.tooltip(Some("Crypto prices")).as_deref(),
            Some("Crypto prices\nLast update failed (exit 1)")
        );
        // And it is still said when there was nothing else to say.
        assert_eq!(
            state.tooltip(None).as_deref(),
            Some("Last update failed (exit 1)")
        );
    }

    #[test]
    fn a_widget_with_no_script_shows_its_static_label_and_nothing_else() {
        let exec = CustomExec::fixed(&CustomWidgetConfig {
            label: "Power".to_string(),
            ..CustomWidgetConfig::default()
        });
        let state = exec.state().borrow().clone();
        assert_eq!(state.text(), "Power");
        assert!(state.visible());
        assert!(exec.commands.is_none(), "there is nothing to refresh");
    }

    #[test]
    fn an_icon_only_widget_draws_no_label_but_stays_on_the_bar() {
        // `icon` alone is a valid custom widget — config validation accepts
        // exec, label or icon — and the label half of it is simply empty.
        let exec = CustomExec::fixed(&CustomWidgetConfig::default());
        assert_eq!(exec.state().borrow().text(), "");
    }

    #[tokio::test]
    async fn only_widgets_with_a_script_get_a_task() {
        let mut configured = BTreeMap::new();
        configured.insert(
            "custom-quiet".to_string(),
            CustomWidgetConfig {
                label: "hi".to_string(),
                ..CustomWidgetConfig::default()
            },
        );
        configured.insert(
            "custom-blank".to_string(),
            CustomWidgetConfig {
                // Whitespace is not a command.
                exec: Some("   ".to_string()),
                label: "hi".to_string(),
                ..CustomWidgetConfig::default()
            },
        );

        let (_sender, receiver) = watch::channel(Arc::new(ConnectivityState::default()));
        let connectivity = Connectivity::from_receiver(receiver);
        let widgets = CustomWidgets::start(&configured, &connectivity);

        assert!(
            widgets
                .get("custom-quiet")
                .expect("configured")
                .commands
                .is_none()
        );
        assert!(
            widgets
                .get("custom-blank")
                .expect("configured")
                .commands
                .is_none()
        );
        assert!(widgets.get("custom-missing").is_none());
    }
}
