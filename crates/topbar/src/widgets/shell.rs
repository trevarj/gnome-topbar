//! The shared widget shell.
//!
//! Every panel widget is built the same way, so hover, press, tooltips, and
//! the pill shape are implemented once here:
//!
//! ```text
//! .widget-wrapper          reserves the widget height, carries .clickable
//! └── .widget              the painted, rounded surface (clips to the pill)
//!     └── overlay
//!         ├── .widget-fill hover/press fill, opacity animated from Rust
//!         └── .content     the widget's own icons and labels
//! ```
//!
//! The fill is the overlay's *child* and the content an overlay on top of it,
//! so the translucent white never tints the text — it only lifts the surface
//! behind it.

use gtk4::prelude::*;
use gtk4::{Align, BaselinePosition, Orientation, Overlay};

use crate::anim::{Animation, AnimationParams, Easing, Ripple};
use crate::style::classes;
use crate::surfaces::tooltip::{self, TooltipHandle};

/// Hover fade-in duration, in milliseconds.
const FADE_IN_MS: u64 = 120;
/// Hover fade-out duration, in milliseconds. Slower out than in, GNOME style.
const FADE_OUT_MS: u64 = 200;

/// The common structure behind every panel widget.
pub struct WidgetShell {
    name: String,
    wrapper: gtk4::Box,
    content: gtk4::Box,
    fill: gtk4::Box,
    fade: Animation,
    ripple: Ripple,
}

impl WidgetShell {
    /// Build a shell for the widget called `name`.
    ///
    /// The name becomes a CSS class on the painted surface, with underscores
    /// turned into hyphens (`quick_settings` → `.quick-settings`).
    pub fn new(name: &str) -> Self {
        let wrapper = gtk4::Box::new(Orientation::Horizontal, 0);
        wrapper.add_css_class(classes::WIDGET_WRAPPER);
        wrapper.set_hexpand(false);

        let surface = gtk4::Box::new(Orientation::Horizontal, 0);
        surface.add_css_class(classes::WIDGET);
        surface.add_css_class(&name.replace('_', "-"));
        // Clip the fill and, later, the ripple to the rounded shape.
        surface.set_overflow(gtk4::Overflow::Hidden);
        surface.set_hexpand(true);
        surface.set_vexpand(true);

        let fill = gtk4::Box::new(Orientation::Horizontal, 0);
        fill.add_css_class(classes::WIDGET_FILL);
        fill.set_opacity(0.0);

        // The ripple is drawn inside the fill, which is the only box in the
        // shell that is exactly the painted pill: it has no padding of its own
        // (that lives on `.content`), so a circle in it reaches every edge, and
        // the surface above clips it to the rounded shape. Being inside the
        // fill also means it fades out with the hover it belongs to.
        let ripple = Ripple::new();
        fill.append(ripple.area());

        let content = gtk4::Box::new(Orientation::Horizontal, 0);
        content.add_css_class(classes::CONTENT);
        content.set_vexpand(true);
        content.set_valign(Align::Fill);
        // Baseline alignment shifts labels when icon and text fonts are mixed.
        content.set_baseline_position(BaselinePosition::Center);

        let overlay = Overlay::new();
        overlay.set_child(Some(&fill));
        overlay.add_overlay(&content);
        overlay.set_measure_overlay(&content, true);

        surface.append(&overlay);
        wrapper.append(&surface);

        let fade = Animation::new(&fill);
        Self {
            name: name.to_string(),
            wrapper,
            content,
            fill,
            fade,
            ripple,
        }
    }

    /// The widget to append to a bar section.
    pub fn root(&self) -> &gtk4::Box {
        &self.wrapper
    }

    /// The box a widget puts its own children in.
    pub fn content(&self) -> &gtk4::Box {
        &self.content
    }

    /// Turn the widget into a panel button: pointer cursor and hover states.
    ///
    /// Passive widgets skip this and stay visually inert, which is what the
    /// `.widget-wrapper:not(.clickable)` rule enforces in CSS.
    pub fn make_interactive(&self) {
        self.wrapper.add_css_class(classes::CLICKABLE);
        self.wrapper.set_cursor_from_name(Some("pointer"));
        self.install_ripple_smoke_action();

        let motion = gtk4::EventControllerMotion::new();
        motion.connect_enter({
            let fill = self.fill.clone();
            let fade = self.fade.clone();
            move |_, _, _| fade_to(&fill, &fade, 1.0, FADE_IN_MS)
        });
        motion.connect_leave({
            let fill = self.fill.clone();
            let fade = self.fade.clone();
            move |_| fade_to(&fill, &fade, 0.0, FADE_OUT_MS)
        });
        self.wrapper.add_controller(motion);

        // Press feedback is a color swap, not a fade: it has to read as
        // immediate. The ripple expanding from the pointer is what gives the
        // press its direction, and it finishes even if the button is let go
        // first — a circle cut off mid-flight reads as a dropped frame.
        let click = gtk4::GestureClick::new();
        click.set_button(gtk4::gdk::BUTTON_PRIMARY);
        click.connect_pressed({
            let fill = self.fill.clone();
            let ripple = self.ripple.clone();
            move |gesture, _, x, y| {
                fill.add_css_class(classes::PRESSED);
                ripple.start_from(gesture, x, y);
            }
        });
        click.connect_released({
            let fill = self.fill.clone();
            move |_, _, _, _| fill.remove_css_class(classes::PRESSED)
        });
        click.connect_cancel({
            let fill = self.fill.clone();
            move |_, _| fill.remove_css_class(classes::PRESSED)
        });
        self.wrapper.add_controller(click);
    }

    /// Give the widget a tooltip, returning a handle that can update its text.
    pub fn set_tooltip(&self, text: &str) -> TooltipHandle {
        tooltip::attach(&self.wrapper, text)
    }

    /// Let a smoke run photograph this widget's hover and press states.
    ///
    /// `topbar popover show clock-hover` lights the hover fill;
    /// `clock-ripple` does the same and adds the frame a press would have
    /// produced part-way through. The difference between those two frames is
    /// the ripple and nothing else, which is the only way to see it: there is
    /// no synthetic pointer in the nested session to press with, and a ripple
    /// that moved would never survive the helper's wait for a still frame.
    #[cfg(debug_assertions)]
    fn install_ripple_smoke_action(&self) {
        use crate::surfaces::popovers::register_smoke_action;

        let fill = self.fill.clone();
        register_smoke_action(&format!("{}-hover", self.name), move || {
            fill.set_opacity(1.0);
        });

        let ripple = self.ripple.clone();
        let fill = self.fill.clone();
        register_smoke_action(&format!("{}-ripple", self.name), move || {
            // Under the hover fill, which is where a press puts it: the fill is
            // at zero opacity with the pointer away, and a ripple inside an
            // invisible box is an invisible ripple.
            fill.set_opacity(1.0);
            ripple.paint(0.35);
        });
    }

    /// Nothing to install in a packaged build.
    #[cfg(not(debug_assertions))]
    fn install_ripple_smoke_action(&self) {}
}

/// Animate the fill toward `target` opacity.
///
/// The duration is scaled by how far there is to travel, so reversing a fade
/// mid-flight takes as long as the distance left rather than restarting the
/// full run.
fn fade_to(fill: &gtk4::Box, fade: &Animation, target: f64, full_ms: u64) {
    let start = fill.opacity();
    let distance = (target - start).abs();
    if distance <= f64::EPSILON {
        fade.cancel();
        fill.set_opacity(target);
        return;
    }

    let duration = (full_ms as f64 * distance).round() as u64;
    let fill = fill.clone();
    fade.start(
        AnimationParams::new(duration).with_easing(Easing::EaseOutCubic),
        Box::new(move |progress| fill.set_opacity(start + (target - start) * progress)),
        None,
    );
}
