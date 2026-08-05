//! One bar window: a layer-shell surface pinned to a monitor's top edge.

use std::collections::BTreeSet;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use topbar_core::Config;
use topbar_services::Services;
use tracing::{debug, info, warn};

use crate::anim;
use crate::bar::{BarContext, Section, SectionClip, SectionedBar};
use crate::fonts;
use crate::style::{self, classes};
use crate::surfaces::osd::OsdSurface;
use crate::surfaces::toast::ToastSurface;
use crate::wayland::blur::{self, BlurAttachment};
use crate::widgets::{self, MountedWidget};

/// The layer-shell namespace the compositor sees. Keep it stable: niri rules
/// and other compositor configuration match on it.
const LAYER_NAMESPACE: &str = "topbar";

/// A widget in its place on the bar, and where that place is.
///
/// The section is kept so a hot reload can rebuild one widget without
/// rebuilding the bar around it: the new widget goes back exactly where the old
/// one was, between the same two neighbours.
struct Mounted {
    /// The configured name, e.g. `clock` or `custom-crypto`.
    name: String,
    /// Which section box it lives in.
    section: Section,
    /// The widget itself and everything keeping it running.
    widget: MountedWidget,
}

/// A bar on one monitor, with everything it needs to keep running.
pub struct BarWindow {
    window: ApplicationWindow,
    /// What a widget is allowed to know about this bar, including the popover
    /// host every widget on it shares. Kept so a rebuilt widget joins the same
    /// host rather than putting up a second pair of layer surfaces.
    context: BarContext,
    /// The section boxes, so one widget can be replaced inside one of them.
    sections: Vec<(Section, gtk4::Box)>,
    /// Mounted widgets, kept alive for as long as the bar exists.
    ///
    /// Anything a widget put on screen goes with it, including the popover
    /// host: the last handle to it lives in a widget's keep-alive box.
    widgets: Vec<Mounted>,
    /// This monitor's notification banners.
    ///
    /// Owned by the bar rather than by a widget: banners appear whether or not
    /// the user configured a clock, and they go away with the monitor.
    _toasts: std::rc::Rc<ToastSurface>,
    /// This monitor's volume/brightness capsule, when `[osd]` enables one.
    ///
    /// Owned here for the same reason, and `None` rather than hidden when the
    /// feature is switched off: an OSD nobody wants should not have a surface.
    osd: Option<std::rc::Rc<OsdSurface>>,
    /// The bar's blur region, removed when the bar goes.
    _blur: BlurAttachment,
}

impl BarWindow {
    /// Build and show the bar for `monitor`.
    pub fn build(
        app: &Application,
        config: &Config,
        monitor: &gdk::Monitor,
        connector: &str,
        services: &Services,
    ) -> Self {
        let height = style::window_height(config);
        let context = BarContext::new(connector, monitor, config, services);

        let window = ApplicationWindow::builder()
            .application(app)
            .title("topbar")
            .decorated(false)
            .resizable(false)
            .default_height(height)
            .build();
        window.add_css_class(classes::BAR_WINDOW);

        window.init_layer_shell();
        window.set_namespace(Some(LAYER_NAMESPACE));
        window.set_layer(Layer::Top);
        window.set_monitor(Some(monitor));

        // v2 is a top panel only: anchor to the top edge and stretch across.
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_anchor(Edge::Bottom, false);

        // Reserve the bar's height so windows tile below it.
        window.auto_exclusive_zone_enable();
        // The panel never takes keyboard focus; popovers request it per-open
        // from M3 and hand it straight back.
        window.set_keyboard_mode(KeyboardMode::None);

        let bar = SectionedBar::new(config.bar.spacing as i32, config.bar.inset as i32);
        bar.add_css_class(classes::BAR);
        bar.set_hexpand(true);
        bar.set_vexpand(true);

        // A shell box sits between the window and the painted bar so
        // `bar.screen_margin` can inset the bar while the window (and with it
        // the exclusive zone) still spans the whole monitor.
        let shell = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        shell.add_css_class(classes::BAR_SHELL);
        shell.set_hexpand(true);
        shell.set_vexpand(true);
        let margin = config.bar.screen_margin as i32;
        shell.set_margin_top(margin);
        shell.set_margin_start(margin);
        shell.set_margin_end(margin);
        shell.append(&bar);

        let mut mounted = Vec::new();
        let mut sections = Vec::new();
        for section in Section::ALL {
            let names = match section {
                Section::Left => &config.widgets.left,
                Section::Center => &config.widgets.center,
                Section::Right => &config.widgets.right,
            };
            // With no center section the layout falls back to a linear split,
            // which is what an empty center should look like.
            if section == Section::Center && names.is_empty() {
                continue;
            }
            let box_ = build_section(section, names, config, &context, &mut mounted);
            // The clip is what lets a section be allocated less than it needs
            // — fourteen tray icons on a narrow output — without asking GTK to
            // under-allocate anything, which it answers with a critical.
            let clip = SectionClip::new(section.clip_align(), &box_);
            bar.set_section(section, Some(&clip));
            sections.push((section, box_));
        }

        window.set_child(Some(&shell));

        // Layer-shell sizes the surface from the anchors, but GTK still wants
        // a sensible default before the first configure event.
        let width = monitor.geometry().width();
        window.set_default_size(width, height);

        fonts::apply_pango_rendering(config, &window);
        window.set_visible(true);
        anim::watchdog::install(&window);

        info!(
            "bar on {connector}: {width}x{height}, {} widget(s)",
            mounted.len()
        );

        Self {
            _blur: bar_blur(config, &window, &bar),
            window,
            sections,
            widgets: mounted,
            _toasts: ToastSurface::new(monitor, connector, config, services),
            osd: OsdSurface::new(monitor, connector, config, services),
            context,
        }
    }

    /// Whether this bar is on screen.
    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Build the widgets named in `names` again, in place.
    ///
    /// The one thing a hot reload does that a restart would otherwise be needed
    /// for: `[widgets.clock] format` changed, so the clock — and only the clock
    /// — is thrown away and built from the new configuration, between the same
    /// two neighbours it had. Dropping the old one is what releases its state
    /// subscriptions, its timers and its retained popover content; the popover
    /// host itself belongs to the bar and outlives them.
    ///
    /// Returns how many widgets were replaced.
    pub fn rebuild_widgets(&mut self, names: &BTreeSet<String>, config: &Config) -> usize {
        let mut rebuilt = 0;
        for index in 0..self.widgets.len() {
            if !names.contains(&self.widgets[index].name) {
                continue;
            }
            let (name, section) = {
                let mounted = &self.widgets[index];
                (mounted.name.clone(), mounted.section)
            };
            let Some((_, box_)) = self.sections.iter().find(|(kind, _)| *kind == section) else {
                continue;
            };
            let Some(replacement) = widgets::mount(&name, config, &self.context) else {
                // Nothing to put back. Leaving the old widget in place is the
                // safer of the two mistakes: a stale label beats a hole in the
                // bar, and validation has already refused unknown names.
                warn!("`{name}` could not be rebuilt; keeping the one on screen");
                continue;
            };

            let previous = self.widgets[index].widget.root.clone();
            let sibling = previous.prev_sibling();
            box_.remove(&previous);
            box_.insert_child_after(&replacement.root, sibling.as_ref());
            // Assigning over the old `MountedWidget` is what drops it, and the
            // order matters: the new one is on screen before the old one's
            // guards run, so nothing flickers through an empty slot.
            self.widgets[index].widget = replacement;
            rebuilt += 1;
        }
        if rebuilt > 0 {
            debug!("rebuilt {rebuilt} widget(s) on {}", self.context.connector);
        }
        rebuilt
    }

    /// Build this bar's OSD again from a changed `[osd]` section.
    ///
    /// The capsule's edge, its timeout and whether it exists at all are read
    /// when it is built, and it is not shown often enough for replacing it to
    /// be worth an in-place edit. Dropping the old surface unregisters it.
    pub fn reconfigure_osd(&mut self, config: &Config) {
        self.osd = None;
        self.osd = OsdSurface::new(
            &self.context.monitor,
            &self.context.connector,
            config,
            &self.context.services,
        );
    }

    /// Show or hide this bar.
    ///
    /// The window is hidden, not destroyed: the widgets keep their
    /// subscriptions and their timers, so a bar coming back is the bar that
    /// went away rather than a fresh one with a blank clock. The exclusive
    /// zone goes with the surface, so the desktop reclaims the strip.
    pub fn set_visible(&self, visible: bool) {
        if self.window.is_visible() == visible {
            return;
        }
        self.window.set_visible(visible);
    }
}

impl Drop for BarWindow {
    fn drop(&mut self) {
        self.window.close();
    }
}

/// Ask the compositor to blur what is behind the painted bar.
///
/// The region is the bar itself rather than the window, which is wider than the
/// bar whenever `bar.screen_margin` insets it and taller than nothing at all.
///
/// A fully transparent bar gets no region: there is no bar there to see the
/// blur through, and a blurred strip across the top of an empty desktop is a
/// defect rather than an effect. (v1 covered that case by blurring each widget
/// island separately; v2 has no islands mode, so it simply declines.) At any
/// other opacity the region goes on, exactly as v1 did — including at 1.0,
/// where the bar is opaque, nothing shows through, and the hint costs the
/// compositor one rectangle it can decide to ignore.
fn bar_blur(
    config: &Config,
    window: &ApplicationWindow,
    bar: &impl IsA<gtk4::Widget>,
) -> BlurAttachment {
    if config.bar.background_opacity <= 0.0 {
        debug!("bar blur skipped: the bar background is fully transparent");
        return BlurAttachment::inert();
    }
    let radius = config.bar.border_radius as i32;
    blur::attach(window, bar, move || radius)
}

/// Build one section box and mount its widgets into it.
fn build_section(
    section: Section,
    names: &[String],
    config: &Config,
    context: &BarContext,
    mounted: &mut Vec<Mounted>,
) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    box_.add_css_class(section.css_class());
    // Keep widgets inside their section when space runs short instead of
    // letting them paint over a neighbour.
    box_.set_overflow(gtk4::Overflow::Hidden);

    let before = mounted.len();
    for name in names {
        let Some(widget) = widgets::mount(name, config, context) else {
            continue;
        };
        box_.append(&widget.root);
        mounted.push(Mounted {
            name: name.clone(),
            section,
            widget,
        });
    }

    debug!(
        "{section:?} section: {} of {} widget(s) built",
        mounted.len() - before,
        names.len()
    );
    box_
}
