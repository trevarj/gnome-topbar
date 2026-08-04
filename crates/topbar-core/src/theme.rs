//! Theme primitives.
//!
//! M0 ships only the color types and hex parsing that config validation needs.
//! The full palette + stylesheet generator lands with the bar shell (M1).

/// An opaque 8-bit-per-channel RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Construct a color from its channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Render as a lowercase `#rrggbb` string.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Render as a CSS `rgba()` expression with the given alpha.
    pub fn to_rgba(self, alpha: f64) -> String {
        format!("rgba({}, {}, {}, {})", self.r, self.g, self.b, alpha)
    }

    /// WCAG relative luminance in the 0.0..=1.0 range.
    pub fn relative_luminance(self) -> f64 {
        fn channel(value: u8) -> f64 {
            let v = f64::from(value) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }
}

/// Parse a `#rgb` or `#rrggbb` hex color (the leading `#` is optional).
///
/// Returns `None` for anything else, which is what config validation uses to
/// reject bad color values.
///
/// # Example
/// ```
/// use topbar_core::theme::{Rgb, parse_hex_color};
///
/// assert_eq!(parse_hex_color("#fff"), Some(Rgb::new(255, 255, 255)));
/// assert_eq!(parse_hex_color("#70B49B"), Some(Rgb::new(0x70, 0xB4, 0x9B)));
/// assert_eq!(parse_hex_color("nope"), None);
/// ```
pub fn parse_hex_color(color: &str) -> Option<Rgb> {
    let color = color.trim().trim_start_matches('#');

    // Expand shorthand (e.g. "fff" -> "ffffff").
    let color = if color.len() == 3 {
        color.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        color.to_string()
    };

    if color.len() != 6 || !color.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let r = u8::from_str_radix(&color[0..2], 16).ok()?;
    let g = u8::from_str_radix(&color[2..4], 16).ok()?;
    let b = u8::from_str_radix(&color[4..6], 16).ok()?;

    Some(Rgb::new(r, g, b))
}

/// Whether a string is an acceptable hex color for config validation.
pub fn is_valid_hex_color(color: &str) -> bool {
    color.starts_with('#') && parse_hex_color(color).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shorthand_and_full_hex() {
        assert_eq!(parse_hex_color("#000"), Some(Rgb::new(0, 0, 0)));
        assert_eq!(parse_hex_color("000000"), Some(Rgb::new(0, 0, 0)));
        assert_eq!(
            parse_hex_color("  #70b49b  "),
            Some(Rgb::new(112, 180, 155))
        );
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
        assert_eq!(parse_hex_color("rgb(1,2,3)"), None);
    }

    #[test]
    fn hex_color_validation_requires_hash() {
        assert!(is_valid_hex_color("#3584e4"));
        assert!(!is_valid_hex_color("3584e4"));
        assert!(!is_valid_hex_color("accent"));
    }

    #[test]
    fn renders_css_forms() {
        let color = Rgb::new(0x70, 0xB4, 0x9B);
        assert_eq!(color.to_hex(), "#70b49b");
        assert_eq!(color.to_rgba(0.5), "rgba(112, 180, 155, 0.5)");
    }

    #[test]
    fn luminance_orders_black_below_white() {
        assert!(
            Rgb::new(0, 0, 0).relative_luminance() < Rgb::new(255, 255, 255).relative_luminance()
        );
    }
}
