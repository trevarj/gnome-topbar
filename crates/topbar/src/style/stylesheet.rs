//! The panel's one and only stylesheet.
//!
//! Everything the panel paints is styled by a single CSS string generated from
//! the configuration and installed as a single [`gtk4::CssProvider`] at
//! `APPLICATION` priority. There are no per-surface providers, no theme
//! variants, and no CSS `transition` rules: motion is Rust-driven (see
//! [`crate::anim`]) so it can be switched off wholesale and cannot leak.
//!
//! The generated sheet has two halves:
//!
//! - [`root_block`] — every value the configuration influences, emitted as CSS
//!   custom properties on `:root`. This is the part worth snapshotting.
//! - [`RULES`] — the selectors, which only ever reference those properties.

use std::cell::RefCell;

use gtk4::gdk;
use topbar_core::Config;
use topbar_core::theme::{Rgb, parse_hex_color};

/// Base color for elevated surfaces (tooltips now, popovers from M3).
const SURFACE_BASE: Rgb = Rgb::new(0x1e, 0x1e, 0x22);
/// Tooltips stay near-opaque: they must be readable before blur exists.
const SURFACE_OPACITY: f64 = 0.92;
/// Popovers are derived from pure black, which is what a translucent GNOME
/// menu over a bright window has to be to keep white text readable.
const POPOVER_BASE: Rgb = Rgb::new(0, 0, 0);
/// Alpha of the hairline border every elevated surface carries.
const SURFACE_BORDER_ALPHA: f64 = 0.08;

/// Corner radius of a popover surface, in pixels.
///
/// The popover's grow-in clips to a rounded rectangle drawn from Rust (see
/// [`crate::anim::ScaleBox`]), so this value has to exist outside the
/// stylesheet as well — the two would drift apart as separate constants.
pub const POPOVER_RADIUS: u32 = 16;
/// Fallback widget surface color when `widgets.background_color` is unset.
const WIDGET_BASE: Rgb = Rgb::new(0x1e, 0x1e, 0x22);
/// Panel foreground, GNOME Shell style: plain white on a black panel.
const FOREGROUND: Rgb = Rgb::new(0xff, 0xff, 0xff);

/// Horizontal padding inside a widget's content box, in pixels.
const WIDGET_PADDING_X: u32 = 10;

/// Fraction of the bar height taken by the gap above and below a widget.
const WIDGET_INSET_SCALE: f64 = 0.14;
/// Font size as a fraction of the widget height.
const FONT_SCALE: f64 = 0.6;
/// Symbolic icon size as a fraction of the bar height.
const ICON_SCALE: f64 = 0.5;
/// Spacing between a widget's own children as a fraction of the bar height.
const CONTENT_GAP_SCALE: f64 = 0.25;

/// Round up to the nearest even number so heights center on whole pixels.
fn round_to_even(value: u32) -> u32 {
    if value.is_multiple_of(2) {
        value
    } else {
        value + 1
    }
}

/// Height of a panel button: the bar height minus a symmetric inset.
pub fn widget_height(bar_size: u32) -> u32 {
    let inset = round_to_even((f64::from(bar_size) * WIDGET_INSET_SCALE) as u32);
    round_to_even(bar_size.saturating_sub(2 * inset))
}

/// Body font size for the bar, derived from the widget height.
pub fn font_size(bar_size: u32) -> u32 {
    round_to_even((f64::from(widget_height(bar_size)) * FONT_SCALE) as u32)
}

/// Total height the bar window occupies, including `bar.padding`.
///
/// An opaque bar pads both sides of its content; a transparent one only pads
/// the screen-edge side, keeping the exclusive zone tight.
pub fn window_height(config: &Config) -> i32 {
    let padding = config.bar.padding;
    let padding_total = if config.bar.background_opacity > 0.0 {
        2 * padding
    } else {
        padding
    };
    (config.bar.size + padding_total) as i32
}

/// Widget corner radius as a CSS length.
///
/// `widgets.border_radius` is a percentage of the bar height; at 50% or more a
/// panel button is a full pill.
fn widget_radius(config: &Config) -> String {
    let percent = config.widgets.border_radius;
    if percent >= 50 {
        return "9999px".to_string();
    }
    let radius = (config.bar.size * percent / 100).min(config.bar.size / 2);
    format!("{radius}px")
}

/// Render a color plus opacity as a CSS color value.
fn tinted(color: Rgb, opacity: f64) -> String {
    if opacity <= 0.0 {
        "transparent".to_string()
    } else if opacity >= 1.0 {
        color.to_hex()
    } else {
        color.to_rgba(opacity)
    }
}

/// The hairline border of an elevated surface, as GDK sees it.
///
/// `--color-surface-border` is generated from the same value. The popover
/// needs it in Rust because its border is drawn on the animating clip boundary
/// rather than by the child's CSS — see [`crate::anim::ScaleBox`].
pub fn surface_border() -> gdk::RGBA {
    let Rgb { r, g, b } = FOREGROUND;
    gdk::RGBA::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        SURFACE_BORDER_ALPHA as f32,
    )
}

/// Parse a configured hex color, falling back when it is not a hex value.
///
/// Config validation already rejects malformed colors, so the fallback only
/// covers a `Config` built by hand.
fn color_or(value: &str, fallback: Rgb) -> Rgb {
    parse_hex_color(value).unwrap_or(fallback)
}

/// Generate the complete stylesheet for a configuration.
pub fn generate(config: &Config) -> String {
    format!("{}{RULES}", root_block(config))
}

/// The configuration-derived half of the stylesheet.
fn root_block(config: &Config) -> String {
    let bar = &config.bar;
    let widgets = &config.widgets;
    let theme = &config.theme;

    let bar_padding_center = if bar.background_opacity > 0.0 {
        bar.padding
    } else {
        0
    };
    let widget_background = widgets
        .background_color
        .as_deref()
        .map_or(WIDGET_BASE, |color| color_or(color, WIDGET_BASE));

    format!(
        r#"/* Generated by gnome-topbar. Do not edit: regenerated on every start. */
:root {{
    /* Geometry */
    --bar-height: {bar_height}px;
    --bar-padding-top: {bar_padding_top}px;
    --bar-padding-bottom: {bar_padding_bottom}px;
    --widget-height: {widget_height}px;
    --widget-padding-x: {widget_padding_x}px;
    --widget-gap: {widget_gap}px;
    --spacing-widget: {spacing}px;
    --radius-bar: {radius_bar}px;
    --radius-widget: {radius_widget};
    --radius-surface: {radius_surface}px;
    --radius-popover: {radius_popover}px;
    --radius-card: 12px;

    /* Typography */
    --font-family: {font_family};
    --font-size: {font_size}px;
    --icon-size: {icon_size}px;

    /* Surfaces */
    --color-bar-background: {bar_background};
    --color-widget-background: {widget_background};
    --color-surface: {surface};
    --color-surface-border: {surface_border};
    --color-popover: {popover};
    --color-popover-shadow: {popover_shadow};
    --color-card: {card};

    /* Panel-button states */
    --color-widget-hover: {hover};
    --color-widget-pressed: {pressed};
    --color-widget-checked: {checked};

    /* Foreground */
    --color-foreground: {foreground};
    --color-foreground-muted: {foreground_muted};
    --color-foreground-disabled: {foreground_disabled};
    --color-accent: {accent};
    --color-state-success: {success};
    --color-state-warning: {warning};
    --color-state-urgent: {urgent};
}}
"#,
        bar_height = bar.size,
        bar_padding_top = bar.padding,
        bar_padding_bottom = bar_padding_center,
        widget_height = widget_height(bar.size),
        widget_padding_x = WIDGET_PADDING_X,
        widget_gap = ((f64::from(bar.size) * CONTENT_GAP_SCALE) as u32 / 2).max(4) + 5,
        spacing = bar.spacing,
        radius_bar = bar.border_radius,
        radius_widget = widget_radius(config),
        radius_surface = (bar.size / 3).max(6),
        radius_popover = POPOVER_RADIUS,
        font_family = theme.typography.font_family,
        font_size = font_size(bar.size),
        icon_size = round_to_even((f64::from(bar.size) * ICON_SCALE) as u32),
        bar_background = tinted(
            color_or(&bar.background_color, Rgb::new(0, 0, 0)),
            bar.background_opacity
        ),
        widget_background = tinted(widget_background, widgets.background_opacity),
        surface = SURFACE_BASE.to_rgba(SURFACE_OPACITY),
        surface_border = FOREGROUND.to_rgba(SURFACE_BORDER_ALPHA),
        // A popover with no opacity of its own follows the bar: an opaque
        // panel means opaque menus, which is what a user who turned
        // translucency off is asking for.
        popover = tinted(
            POPOVER_BASE,
            widgets
                .popover_background_opacity
                .unwrap_or(bar.background_opacity)
        ),
        popover_shadow = POPOVER_BASE.to_rgba(0.5),
        card = FOREGROUND.to_rgba(0.06),
        hover = FOREGROUND.to_rgba(0.1),
        pressed = FOREGROUND.to_rgba(0.15),
        checked = FOREGROUND.to_rgba(0.18),
        foreground = FOREGROUND.to_hex(),
        foreground_muted = FOREGROUND.to_rgba(0.6),
        foreground_disabled = FOREGROUND.to_rgba(0.4),
        accent = color_or(&theme.accent, Rgb::new(0x70, 0xb4, 0x9b)).to_hex(),
        success = color_or(&theme.states.success, Rgb::new(0x22, 0xc5, 0x5e)).to_hex(),
        warning = color_or(&theme.states.warning, Rgb::new(0xf5, 0x9e, 0x0b)).to_hex(),
        urgent = color_or(&theme.states.urgent, Rgb::new(0xef, 0x44, 0x44)).to_hex(),
    )
}

/// The selectors. Every value here comes from a `:root` custom property.
const RULES: &str = r#"
/* ===== Bar ===== */

/* The layer-shell window and its shell box are invisible; only the bar paints. */
window.bar-window,
.bar-shell {
    background: transparent;
}

sectioned-bar.bar {
    min-height: var(--bar-height);
    padding-top: var(--bar-padding-top);
    padding-bottom: var(--bar-padding-bottom);
    background-color: var(--color-bar-background);
    border-radius: var(--radius-bar);
    color: var(--color-foreground);
    font-family: var(--font-family);
    font-size: var(--font-size);
    font-weight: 700;
}

.bar-section--left > *:not(:last-child),
.bar-section--center > *:not(:last-child),
.bar-section--right > *:not(:last-child) {
    margin-right: var(--spacing-widget);
}

/* ===== Panel buttons ===== */

/* The wrapper reserves height; the surface underneath does the painting. */
.widget-wrapper {
    min-height: var(--widget-height);
    background: transparent;
}

.widget {
    background-color: var(--color-widget-background);
    border-radius: var(--radius-widget);
}

/* Hover and press fill. It sits behind the content and its opacity is
   animated from Rust (see anim::Animation) — deliberately no CSS transition. */
.widget-fill {
    background-color: var(--color-widget-hover);
    border-radius: var(--radius-widget);
}

.widget-fill.pressed {
    background-color: var(--color-widget-pressed);
}

/* Exactly one widget is checked at a time: the one whose popover is open.
   It paints on the surface rather than on the fill, because the fill's opacity
   belongs to the hover animation and sits at zero while the pointer is away —
   a checked button has to stay lit whether or not it is hovered. Hover then
   layers on top, which is what GNOME does with an open menu under the mouse. */
.widget-wrapper.checked .widget {
    background-color: var(--color-widget-checked);
}

/* Passive widgets never light up, whatever the pointer does. */
.widget-wrapper:not(.clickable) .widget-fill {
    background-color: transparent;
}

.content {
    padding: 0 var(--widget-padding-x);
}

.content > *:not(:last-child) {
    margin-right: var(--widget-gap);
}

/* A widget whose service has dropped keeps its last state on screen, dimmed:
   an empty panel would read as "no workspaces", which is a different claim. */
.widget-wrapper.disconnected {
    opacity: 0.5;
}

/* ===== Clock ===== */

/* Tabular figures keep the width steady as the digits change. */
.clock .content {
    font-feature-settings: "tnum";
}

/* ===== Workspaces ===== */

/* The strip paints its own indicators in snapshot(); the only thing it takes
   from CSS is the foreground color they are derived from. Their sizes are
   Rust constants (see widgets/workspaces/model.rs) so the drawn geometry and
   the measured width can never disagree. */
workspace-strip {
    color: var(--color-foreground);
}

/* Dots are their own visual rhythm; the widget's own padding is enough. */
.workspaces .content > * {
    margin: 0;
}

/* ===== Keyboard layout ===== */

/* A two-letter code has to read as a unit rather than as running text. */
.keyboard-layout label {
    font-weight: 700;
}

.keyboard-layout-icon {
    -gtk-icon-size: var(--icon-size);
}

/* ===== Popovers ===== */

/* Both layer surfaces are invisible: the popover paints, the catcher does not
   paint at all — it exists only to turn a click into a dismissal. */
window.popover-window,
window.click-catcher-window,
.popover-wrapper,
.click-catcher {
    background: transparent;
}

.popover-surface {
    background-color: var(--color-popover);
    border: 1px solid var(--color-surface-border);
    border-radius: var(--radius-popover);
    box-shadow: 0 2px 6px var(--color-popover-shadow);
    color: var(--color-foreground);
    font-family: var(--font-family);
    font-size: var(--font-size);
    font-weight: 400;
    padding: 16px;
}

/* While a popover grows in, its outline is drawn on the animating clip (see
   anim::ScaleBox) because this border sits outside it until the last frame. */
.popover-surface.borderless {
    border-color: transparent;
}

/* ===== Control panel ===== */

/* The columns are fixed width so nothing reflows as content arrives; the
   widths live in Rust beside the layout that depends on them. */
.control-panel-column {
    background: transparent;
}

.control-panel-divider {
    background-color: var(--color-surface-border);
}

.control-panel-card {
    background-color: var(--color-card);
    border-radius: var(--radius-card);
    padding: 12px 14px;
}

.control-panel-title {
    font-size: 1.05em;
    font-weight: 700;
}

.control-panel-time {
    font-size: 2.4em;
    font-weight: 700;
    font-feature-settings: "tnum";
}

.control-panel-date {
    font-size: 1.05em;
    font-weight: 700;
    color: var(--color-foreground-muted);
}

.world-clock-row {
    min-height: 22px;
}

.world-clock-name {
    font-weight: 500;
}

.world-clock-zone {
    color: var(--color-foreground-muted);
}

.world-clock-time {
    font-weight: 600;
    font-feature-settings: "tnum";
}

/* An empty column is a designed state, not a hole: a large dimmed icon and a
   line of text, centred, exactly as GNOME's own notification list does it. */
.empty-state-icon {
    -gtk-icon-size: 48px;
    opacity: 0.4;
}

.empty-state-label {
    font-size: 1.05em;
    font-weight: 500;
    color: var(--color-foreground-muted);
}

.dnd-row {
    min-height: 32px;
    margin-top: 6px;
}

.dnd-label {
    font-weight: 500;
}

/* Reserved space for a later milestone's content. */
.placeholder {
    color: var(--color-foreground-disabled);
}

/* ===== Tooltip ===== */

window.tooltip-window {
    background: transparent;
}

.tooltip-surface {
    background-color: var(--color-surface);
    border: 1px solid var(--color-surface-border);
    border-radius: var(--radius-surface);
    padding: 6px 10px;
}

.tooltip-label {
    font-family: var(--font-family);
    font-size: var(--font-size);
    color: var(--color-foreground);
}
"#;

thread_local! {
    /// The provider currently installed on the default display.
    static PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

/// Install `css` on `display`, replacing whatever was installed before.
///
/// The new provider is added before the old one is removed, so a reload never
/// leaves a frame with unstyled widgets.
pub fn apply(display: &gdk::Display, css: &str) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(css);

    gtk4::style_context_add_provider_for_display(
        display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let previous = PROVIDER.with(|cell| cell.borrow_mut().replace(provider));
    if let Some(previous) = previous {
        gtk4::style_context_remove_provider_for_display(display, &previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::classes;

    /// The live configuration this project is written for. The fixture is
    /// shared with `topbar-core`'s drop-in compatibility contract test.
    const LIVE_CONFIG: &str = include_str!("../../../topbar-core/tests/fixtures/live-config.toml");

    fn live_config() -> Config {
        Config::parse(LIVE_CONFIG)
            .expect("the live config fixture must parse")
            .0
    }

    /// Snapshot of every configuration-derived value in the stylesheet.
    ///
    /// Update it deliberately: a diff here is a visible change to the panel.
    const LIVE_ROOT_BLOCK: &str = "\
/* Generated by gnome-topbar. Do not edit: regenerated on every start. */
:root {
    /* Geometry */
    --bar-height: 36px;
    --bar-padding-top: 0px;
    --bar-padding-bottom: 0px;
    --widget-height: 24px;
    --widget-padding-x: 10px;
    --widget-gap: 9px;
    --spacing-widget: 2px;
    --radius-bar: 0px;
    --radius-widget: 9999px;
    --radius-surface: 12px;
    --radius-popover: 16px;
    --radius-card: 12px;

    /* Typography */
    --font-family: NotoSans, Iosevka SS12, Symbols Nerd Font Mono, Symbols Nerd Font;
    --font-size: 14px;
    --icon-size: 18px;

    /* Surfaces */
    --color-bar-background: #000000;
    --color-widget-background: transparent;
    --color-surface: rgba(30, 30, 34, 0.92);
    --color-surface-border: rgba(255, 255, 255, 0.08);
    --color-popover: rgba(0, 0, 0, 0.76);
    --color-popover-shadow: rgba(0, 0, 0, 0.5);
    --color-card: rgba(255, 255, 255, 0.06);

    /* Panel-button states */
    --color-widget-hover: rgba(255, 255, 255, 0.1);
    --color-widget-pressed: rgba(255, 255, 255, 0.15);
    --color-widget-checked: rgba(255, 255, 255, 0.18);

    /* Foreground */
    --color-foreground: #ffffff;
    --color-foreground-muted: rgba(255, 255, 255, 0.6);
    --color-foreground-disabled: rgba(255, 255, 255, 0.4);
    --color-accent: #70b49b;
    --color-state-success: #22c55e;
    --color-state-warning: #f59e0b;
    --color-state-urgent: #ef4444;
}
";

    #[test]
    fn live_config_root_block_matches_snapshot() {
        assert_eq!(root_block(&live_config()), LIVE_ROOT_BLOCK);
    }

    #[test]
    fn every_class_constant_is_styled() {
        let css = generate(&live_config());
        for class in classes::ALL {
            assert!(
                css.contains(class),
                "class `{class}` never appears in the generated stylesheet"
            );
        }
    }

    #[test]
    fn stylesheet_declares_no_css_transitions() {
        // All motion is frame-clock driven from Rust so `theme.animations`
        // and `gtk-enable-animations` can switch it off completely.
        let css = generate(&live_config());
        assert!(!css.contains("transition:"), "{css}");
        assert!(!css.contains("animation:"), "{css}");
    }

    #[test]
    fn hover_states_follow_the_ux_spec() {
        let css = generate(&live_config());
        assert!(css.contains("--color-widget-hover: rgba(255, 255, 255, 0.1);"));
        assert!(css.contains("--color-widget-pressed: rgba(255, 255, 255, 0.15);"));
        assert!(css.contains("--color-widget-checked: rgba(255, 255, 255, 0.18);"));
    }

    #[test]
    fn every_custom_property_is_defined_before_use() {
        let css = generate(&live_config());
        let defined: Vec<&str> = css
            .lines()
            .filter_map(|line| line.trim().strip_prefix("--"))
            .filter_map(|line| line.split(':').next())
            .collect();

        for (_, rest) in css.match_indices("var(--").map(|(i, _)| css.split_at(i)) {
            let name = rest
                .trim_start_matches("var(--")
                .split(')')
                .next()
                .expect("var() reference must close");
            assert!(defined.contains(&name), "var(--{name}) is never defined");
        }
    }

    #[test]
    fn transparent_bar_keeps_the_exclusive_zone_tight() {
        let mut config = Config::default();
        config.bar.padding = 4;
        assert_eq!(window_height(&config), 36 + 8);

        config.bar.background_opacity = 0.0;
        assert_eq!(window_height(&config), 36 + 4);
    }

    #[test]
    fn widget_radius_becomes_a_pill_at_fifty_percent() {
        let mut config = Config::default();
        assert_eq!(widget_radius(&config), "9999px");

        config.widgets.border_radius = 25;
        assert_eq!(widget_radius(&config), "9px");

        config.widgets.border_radius = 0;
        assert_eq!(widget_radius(&config), "0px");
    }

    #[test]
    fn metrics_scale_with_the_bar_height() {
        assert_eq!(widget_height(36), 24);
        assert_eq!(font_size(36), 14);
        assert!(widget_height(48) > widget_height(36));
        assert!(font_size(48) > font_size(36));
    }

    #[test]
    fn popover_opacity_falls_back_to_the_bar() {
        // The live config asks for 0.76; an unset value follows the bar, so
        // turning panel translucency off turns it off for menus too.
        let mut config = Config::default();
        assert_eq!(config.widgets.popover_background_opacity, None);
        assert!(root_block(&config).contains("--color-popover: #000000;"));

        config.bar.background_opacity = 0.5;
        assert!(root_block(&config).contains("--color-popover: rgba(0, 0, 0, 0.5);"));

        config.widgets.popover_background_opacity = Some(0.76);
        assert!(root_block(&config).contains("--color-popover: rgba(0, 0, 0, 0.76);"));
    }

    #[test]
    fn the_animated_outline_matches_the_css_border() {
        // The popover draws its border twice — in CSS at rest, from Rust while
        // it grows — and the two must be the same color.
        let border = surface_border();
        assert!((f64::from(border.alpha()) - SURFACE_BORDER_ALPHA).abs() < 1e-6);
        assert_eq!(border.red(), f32::from(FOREGROUND.r) / 255.0);
        assert!(root_block(&Config::default()).contains(&format!(
            "--color-surface-border: {};",
            FOREGROUND.to_rgba(SURFACE_BORDER_ALPHA)
        )));
    }

    #[test]
    fn bar_background_honors_opacity() {
        let mut config = Config::default();
        assert!(root_block(&config).contains("--color-bar-background: #000000;"));

        config.bar.background_opacity = 0.5;
        assert!(root_block(&config).contains("--color-bar-background: rgba(0, 0, 0, 0.5);"));

        config.bar.background_opacity = 0.0;
        assert!(root_block(&config).contains("--color-bar-background: transparent;"));
    }
}
