//! One bar window: a layer-shell surface pinned to a monitor's top edge.

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use topbar_core::Config;
use topbar_services::Services;
use tracing::{debug, info};

use crate::anim;
use crate::bar::{BarContext, Section, SectionedBar};
use crate::fonts;
use crate::style::{self, classes};
use crate::widgets::{self, MountedWidget};

/// The layer-shell namespace the compositor sees. Keep it stable: niri rules
/// and other compositor configuration match on it.
const LAYER_NAMESPACE: &str = "gnome-topbar";

/// A bar on one monitor, with everything it needs to keep running.
pub struct BarWindow {
    window: ApplicationWindow,
    /// Mounted widgets, kept alive for as long as the bar exists.
    ///
    /// Anything a widget put on screen goes with it, including the popover
    /// host: the last handle to it lives in a widget's keep-alive box.
    _widgets: Vec<MountedWidget>,
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
            .title("gnome-topbar")
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
            bar.set_section(section, Some(&box_));
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
            window,
            _widgets: mounted,
        }
    }
}

impl Drop for BarWindow {
    fn drop(&mut self) {
        self.window.close();
    }
}

/// Build one section box and mount its widgets into it.
fn build_section(
    section: Section,
    names: &[String],
    config: &Config,
    context: &BarContext,
    mounted: &mut Vec<MountedWidget>,
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
        mounted.push(widget);
    }

    debug!(
        "{section:?} section: {} of {} widget(s) built",
        mounted.len() - before,
        names.len()
    );
    box_
}
