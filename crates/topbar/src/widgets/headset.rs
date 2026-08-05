//! The headset battery: an icon, a percentage, and mostly nothing at all.
//!
//! ```text
//!  🔋 45%
//! ```
//!
//! Invisible unless `headsetcontrol` reports a reading, which on most machines
//! most of the time it does not — the tool is not installed, or the headset is
//! switched off, or it is connected but asleep. See
//! [`topbar_services::headset`] for the three shapes that mean "nothing to
//! report" and why all three are normal rather than exceptional.
//!
//! The icon is the *battery* icon set, the same names the Quick Settings pill
//! draws the laptop's battery with. A headset battery is a battery; giving it
//! its own set of glyphs would be a second vocabulary for one concept.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Image, Label};
use topbar_core::config::HeadsetConfig;
use topbar_services::battery::icon;
use topbar_services::headset::model::{URGENT_PERCENT, WARNING_PERCENT};
use topbar_services::{BatteryStatus, HeadsetReading, HeadsetState};

use crate::bar::BarContext;
use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::shell::WidgetShell;
use crate::widgets::{ellipsize, install_click_commands, set_class};

/// What a failed click command is reported against.
const WIDGET_NAME: &str = "headset";

/// The headset battery widget.
pub struct HeadsetWidget {
    shell: WidgetShell,
    /// Holds the icon, the label and the tooltip the render closure touches.
    _inner: Rc<Inner>,
    /// Keeps them subscribed to the service.
    _binding: BindingGuard,
}

impl HeadsetWidget {
    /// Build the widget from `[widgets.headset]`.
    pub fn new(settings: &HeadsetConfig, context: &BarContext) -> Self {
        let shell = WidgetShell::new(classes::HEADSET);

        let image = Image::new();
        image.add_css_class(classes::HEADSET_ICON);
        shell.content().append(&image);

        let label = Label::new(None);
        shell.content().append(&label);

        let inner = Rc::new(Inner {
            wrapper: shell.root().clone(),
            image,
            label,
            tooltip: shell.set_tooltip(&settings.tooltip),
            max_chars: settings.max_chars.map(|max| max as usize),
            configured_tooltip: settings.tooltip.clone(),
        });

        let binding = bridge::bind_state(shell.root(), context.services.headset.state(), {
            let inner = Rc::downgrade(&inner);
            move |root: &gtk4::Box, state: &HeadsetState| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                match &state.reading {
                    Some(reading) => inner.render(reading),
                    // Zero width rather than an empty pill: a widget with
                    // nothing to say should not take up room saying it.
                    None => root.set_visible(false),
                }
            }
        });

        if settings.on_click.is_some()
            || settings.on_click_right.is_some()
            || settings.on_click_middle.is_some()
        {
            shell.make_interactive();
        }
        install_left_click(shell.root(), settings.on_click.as_deref());
        install_click_commands(
            shell.root(),
            WIDGET_NAME,
            settings.on_click_right.as_deref(),
            settings.on_click_middle.as_deref(),
        );

        Self {
            shell,
            _inner: inner,
            _binding: binding,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// Everything the render closure touches.
struct Inner {
    wrapper: gtk4::Box,
    image: Image,
    label: Label,
    tooltip: TooltipHandle,
    /// `max_chars`, the width the reading is cut to.
    max_chars: Option<usize>,
    /// `tooltip`, the line the device's own details are added under.
    configured_tooltip: String,
}

impl Inner {
    /// Draw one reading.
    fn render(&self, reading: &HeadsetReading) {
        let name = battery_icon(reading.percent, reading.charging);
        if self.image.icon_name().as_deref() != Some(name.as_str()) {
            self.image.set_icon_name(Some(&name));
        }

        let text = ellipsize(&format!("{}%", reading.percent), self.max_chars);
        if self.label.text() != text {
            self.label.set_text(&text);
        }

        self.tooltip
            .set_text(&tooltip(&self.configured_tooltip, &reading.tooltip()));

        let level = tint(reading.percent, reading.charging);
        set_class(
            &self.wrapper,
            classes::STATE_URGENT,
            level == Some(Tint::Urgent),
        );
        set_class(
            &self.wrapper,
            classes::STATE_WARNING,
            level == Some(Tint::Warning),
        );

        self.wrapper.set_visible(true);
    }
}

/// The tint a reading wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tint {
    /// Low enough to want looking at.
    Warning,
    /// Low enough to be about to stop working.
    Urgent,
}

/// Which tint a charge deserves, if any.
///
/// A headset on its dock is not in trouble however low it reads, which is the
/// same rule the laptop battery's own low state follows.
fn tint(percent: u8, charging: bool) -> Option<Tint> {
    if charging {
        return None;
    }
    if percent <= URGENT_PERCENT {
        return Some(Tint::Urgent);
    }
    if percent <= WARNING_PERCENT {
        return Some(Tint::Warning);
    }
    None
}

/// The battery icon for a headset charge.
fn battery_icon(percent: u8, charging: bool) -> String {
    let status = if charging {
        BatteryStatus::Charging
    } else {
        BatteryStatus::Discharging
    };
    icon(Some(f64::from(percent)), status)
}

/// The configured line, with the device's own details under it.
fn tooltip(configured: &str, device: &str) -> String {
    if configured.trim().is_empty() {
        return device.to_string();
    }
    format!("{configured}\n{device}")
}

/// A left click runs the configured command and nothing else.
fn install_left_click(anchor: &gtk4::Box, command: Option<&str>) {
    let Some(command) = command.map(str::to_string) else {
        return;
    };
    let click = gtk4::GestureClick::new();
    click.set_button(gtk4::gdk::BUTTON_PRIMARY);
    click.connect_released(move |_, _, _, _| {
        let command = command.clone();
        bridge::act(
            crate::bridge::ActionScope::Toast {
                widget: WIDGET_NAME,
            },
            async move { topbar_services::proc::run(&command).await },
        );
    });
    anchor.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_follows_the_charge_in_tens() {
        assert_eq!(battery_icon(45, false), "battery-level-40-symbolic");
        assert_eq!(battery_icon(9, false), "battery-level-0-symbolic");
        assert_eq!(battery_icon(100, false), "battery-level-100-symbolic");
    }

    #[test]
    fn charging_has_its_own_icon_and_a_full_one_reads_as_charged() {
        assert_eq!(battery_icon(45, true), "battery-level-40-charging-symbolic");
        // Adwaita ships no `battery-level-100-charging-symbolic`; a headset
        // sitting full on its dock must not draw a missing-icon glyph.
        assert_eq!(
            battery_icon(100, true),
            "battery-level-100-charged-symbolic"
        );
    }

    #[test]
    fn a_low_headset_is_tinted_and_a_charging_one_is_not() {
        assert_eq!(tint(45, false), None);
        assert_eq!(tint(25, false), Some(Tint::Warning));
        assert_eq!(tint(11, false), Some(Tint::Warning));
        assert_eq!(tint(10, false), Some(Tint::Urgent));
        assert_eq!(tint(0, false), Some(Tint::Urgent));
        // On the dock, so it is filling up rather than running out.
        assert_eq!(tint(5, true), None);
    }

    #[test]
    fn the_configured_tooltip_keeps_its_place_above_the_device() {
        assert_eq!(
            tooltip("Headset battery", "Arctis Nova\n45% · discharging"),
            "Headset battery\nArctis Nova\n45% · discharging"
        );
        assert_eq!(tooltip("  ", "Arctis Nova\n45%"), "Arctis Nova\n45%");
    }

    #[test]
    fn the_reading_is_cut_to_the_configured_width() {
        // The live config asks for twelve characters, which "100%" is well
        // inside; the cut exists for a `max_chars` somebody sets to three.
        assert_eq!(ellipsize("100%", Some(12)), "100%");
        assert_eq!(ellipsize("100%", Some(3)), "10…");
    }
}
