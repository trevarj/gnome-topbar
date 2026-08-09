//! `advanced.pango_font_rendering`.
//!
//! GTK sets font sizes from CSS with `pango_font_description_set_absolute_size`,
//! which bypasses Pango's DPI-aware hinting. On layer-shell surfaces that can
//! clip tall glyphs at some sizes — the reason v1 grew this workaround
//! (`services/surfaces.rs`). Turning the flag on re-states each label's font
//! as a Pango attribute using `set_size` (points), which does go through the
//! DPI-aware path.
//!
//! The size is read back from the label's own Pango context, so whatever CSS
//! resolved — including relative sizes — is preserved.

use gtk4::pango::{AttrFontDesc, AttrList, FontDescription};
use gtk4::prelude::*;
use topbar_core::Config;
use tracing::debug;

/// Apply the workaround to every label under `root`, if it is enabled.
pub fn apply_pango_rendering(config: &Config, root: &impl IsA<gtk4::Widget>) {
    let Some(rendering) = FontRendering::from_config(config) else {
        return;
    };
    debug!(
        "applying Pango font attributes (family `{}`, {}px fallback)",
        rendering.family, rendering.fallback_px
    );
    render_tree(&rendering, root);
}

/// What [`render_tree`] needs, captured once from the config so a popover host
/// can re-apply the workaround on every open without holding the whole config.
///
/// `None` when `advanced.pango_font_rendering` is off, which is how the gate
/// that used to live in [`apply_pango_rendering`] survives the split.
#[derive(Clone)]
pub struct FontRendering {
    family: String,
    fallback_px: u32,
}

impl FontRendering {
    /// The font settings the workaround restates, or `None` when it is off.
    pub fn from_config(config: &Config) -> Option<Self> {
        config.advanced.pango_font_rendering.then(|| Self {
            family: config.theme.typography.font_family.clone(),
            fallback_px: crate::style::font_size(config.bar.size),
        })
    }
}

/// Re-state every label under `root` in points.
///
/// The popover surfaces call this on every open rather than once at build:
/// their content is rebuilt on [`PopoverContent::refresh`](crate::surfaces::popovers::PopoverContent::refresh),
/// so the rows that arrive on an open would miss a walk taken when the panel
/// was first built.
pub fn render_tree(rendering: &FontRendering, root: &impl IsA<gtk4::Widget>) {
    apply_to_tree(root.as_ref(), &rendering.family, rendering.fallback_px);
}

fn apply_to_tree(widget: &gtk4::Widget, family: &str, fallback_px: u32) {
    if let Some(label) = widget.downcast_ref::<gtk4::Label>() {
        let size_px = css_font_size(label).unwrap_or(fallback_px);
        label.set_attributes(Some(&font_attributes(family, size_px)));
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        apply_to_tree(&widget, family, fallback_px);
        child = widget.next_sibling();
    }
}

/// Build the attribute list that restates a font in points.
fn font_attributes(family: &str, size_px: u32) -> AttrList {
    let mut description = FontDescription::new();
    description.set_family(family);
    description.set_size(points_from_pixels(size_px));

    let attributes = AttrList::new();
    attributes.insert(AttrFontDesc::new(&description));
    attributes
}

/// Convert a CSS pixel size to Pango units at the standard 96 DPI.
fn points_from_pixels(size_px: u32) -> i32 {
    let points = (f64::from(size_px) * 72.0 / 96.0).round() as i32;
    points.max(1) * gtk4::pango::SCALE
}

/// The font size CSS resolved for `label`, in pixels.
fn css_font_size(label: &gtk4::Label) -> Option<u32> {
    let description = label.pango_context().font_description()?;
    let size = description.size();
    if size <= 0 {
        return None;
    }

    let size = f64::from(size) / f64::from(gtk4::pango::SCALE);
    let pixels = if description.is_size_absolute() {
        size
    } else {
        size * 96.0 / 72.0
    };
    Some((pixels.round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_convert_to_pango_points() {
        // 14 CSS pixels is 10.5pt at 96 DPI, rounded to 11pt.
        assert_eq!(points_from_pixels(14), 11 * gtk4::pango::SCALE);
        assert_eq!(points_from_pixels(16), 12 * gtk4::pango::SCALE);
    }

    #[test]
    fn tiny_sizes_never_round_to_zero() {
        assert_eq!(points_from_pixels(0), gtk4::pango::SCALE);
        assert_eq!(points_from_pixels(1), gtk4::pango::SCALE);
    }

    #[test]
    fn font_rendering_is_off_when_the_flag_is_off() {
        let mut config = Config::default();
        config.advanced.pango_font_rendering = false;
        assert!(FontRendering::from_config(&config).is_none());
    }

    #[test]
    fn font_rendering_captures_the_family_and_fallback_when_on() {
        let mut config = Config::default();
        config.advanced.pango_font_rendering = true;
        config.theme.typography.font_family = "Test Sans".to_string();
        config.bar.size = 36;

        let rendering = FontRendering::from_config(&config).expect("the flag is on");
        assert_eq!(rendering.family, "Test Sans");
        // 36px bar -> 24px widget height -> 14px body font, the same fallback
        // the bar itself applies.
        assert_eq!(rendering.fallback_px, crate::style::font_size(36));
    }
}
