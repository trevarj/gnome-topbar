//! Typed CSS class names.
//!
//! Every selector the generated stylesheet writes has a constant here, and
//! every widget adds classes through these constants — never a string literal.
//! The test at the bottom asserts each constant actually appears in the
//! generated sheet, so a renamed class cannot silently stop matching.

/// The bar's layer-shell `ApplicationWindow`.
pub const BAR_WINDOW: &str = "bar-window";
/// The transparent box between the window and the painted bar.
pub const BAR_SHELL: &str = "bar-shell";
/// The painted bar itself (a [`SectionedBar`](crate::bar::SectionedBar)).
pub const BAR: &str = "bar";
/// CSS name of the `SectionedBar` widget class.
pub const SECTIONED_BAR: &str = "sectioned-bar";
/// Left widget section.
pub const SECTION_LEFT: &str = "bar-section--left";
/// Center widget section.
pub const SECTION_CENTER: &str = "bar-section--center";
/// Right widget section.
pub const SECTION_RIGHT: &str = "bar-section--right";

/// Outer box of a panel widget; carries no paint of its own.
pub const WIDGET_WRAPPER: &str = "widget-wrapper";
/// The painted, rounded widget surface.
pub const WIDGET: &str = "widget";
/// The hover/press fill painted behind widget content.
///
/// Its opacity is animated from Rust; the stylesheet only picks the color.
pub const WIDGET_FILL: &str = "widget-fill";
/// Inner box that holds a widget's icons and labels.
pub const CONTENT: &str = "content";
/// Marks a widget that reacts to clicks (pointer cursor, hover fill).
pub const CLICKABLE: &str = "clickable";
/// Set on the fill while a primary press is held.
pub const PRESSED: &str = "pressed";
/// Set on the wrapper while this widget's popover is open.
///
/// The popover framework (M3) is what toggles it; the rule exists now so
/// widgets are fully styled the day they gain a popover.
#[allow(dead_code)]
pub const CHECKED: &str = "checked";

/// Set on a widget whose service has lost its connection.
pub const DISCONNECTED: &str = "disconnected";

/// The clock widget.
pub const CLOCK: &str = "clock";

/// The workspaces widget.
pub const WORKSPACES: &str = "workspaces";
/// CSS name of the custom widget that draws the workspace indicators.
pub const WORKSPACE_STRIP: &str = "workspace-strip";

/// The keyboard-layout widget.
pub const KEYBOARD_LAYOUT: &str = "keyboard-layout";
/// The keyboard-layout widget's icon.
pub const KEYBOARD_LAYOUT_ICON: &str = "keyboard-layout-icon";

/// The shared tooltip's layer-shell window.
pub const TOOLTIP_WINDOW: &str = "tooltip-window";
/// The tooltip's painted surface.
pub const TOOLTIP_SURFACE: &str = "tooltip-surface";
/// The tooltip's text.
pub const TOOLTIP_LABEL: &str = "tooltip-label";

/// Every class name above, for the coverage test.
#[cfg(test)]
pub const ALL: &[&str] = &[
    BAR_WINDOW,
    BAR_SHELL,
    BAR,
    SECTIONED_BAR,
    SECTION_LEFT,
    SECTION_CENTER,
    SECTION_RIGHT,
    WIDGET_WRAPPER,
    WIDGET,
    WIDGET_FILL,
    CONTENT,
    CLICKABLE,
    PRESSED,
    CHECKED,
    DISCONNECTED,
    CLOCK,
    WORKSPACES,
    WORKSPACE_STRIP,
    KEYBOARD_LAYOUT,
    KEYBOARD_LAYOUT_ICON,
    TOOLTIP_WINDOW,
    TOOLTIP_SURFACE,
    TOOLTIP_LABEL,
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn class_names_are_unique() {
        let unique: BTreeSet<&&str> = ALL.iter().collect();
        assert_eq!(unique.len(), ALL.len(), "duplicate class name in ALL");
    }

    #[test]
    fn class_names_are_css_identifiers() {
        for class in ALL {
            assert!(!class.is_empty());
            assert!(
                class
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{class} is not a lowercase CSS identifier"
            );
        }
    }
}
