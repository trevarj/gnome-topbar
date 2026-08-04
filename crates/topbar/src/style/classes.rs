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

/// The popover host's layer-shell window.
pub const POPOVER_WINDOW: &str = "popover-window";
/// The box inside the popover window that reserves room for the drop shadow.
pub const POPOVER_WRAPPER: &str = "popover-wrapper";
/// The painted popover surface, i.e. a widget's popover content.
pub const POPOVER_SURFACE: &str = "popover-surface";
/// Set on popover content while its border is being drawn by the scale box.
pub const BORDERLESS: &str = "borderless";
/// The click-catcher's layer-shell window.
pub const CLICK_CATCHER_WINDOW: &str = "click-catcher-window";
/// The click-catcher's (transparent) child.
pub const CLICK_CATCHER: &str = "click-catcher";

/// The clock's control panel, GNOME's date menu.
pub const CONTROL_PANEL: &str = "control-panel";
/// One of the control panel's two columns.
pub const CONTROL_PANEL_COLUMN: &str = "control-panel-column";
/// The hairline between the two columns.
pub const CONTROL_PANEL_DIVIDER: &str = "control-panel-divider";
/// A raised block inside a control-panel column.
pub const CARD: &str = "control-panel-card";
/// A card's heading.
pub const CARD_TITLE: &str = "control-panel-title";
/// The large local time.
pub const CONTROL_PANEL_TIME: &str = "control-panel-time";
/// The full date under it.
pub const CONTROL_PANEL_DATE: &str = "control-panel-date";
/// One configured secondary time zone.
pub const WORLD_CLOCK_ROW: &str = "world-clock-row";
/// A world clock's configured label.
pub const WORLD_CLOCK_NAME: &str = "world-clock-name";
/// The dimmed weekday and date beside it.
pub const WORLD_CLOCK_ZONE: &str = "world-clock-zone";
/// A world clock's time.
pub const WORLD_CLOCK_TIME: &str = "world-clock-time";
/// A column with nothing in it, drawn as a designed state rather than a gap.
pub const EMPTY_STATE: &str = "empty-state";
/// The large dimmed icon in an empty state.
pub const EMPTY_STATE_ICON: &str = "empty-state-icon";
/// The line of text under it.
pub const EMPTY_STATE_LABEL: &str = "empty-state-label";
/// The Do Not Disturb row at the foot of the notifications column.
pub const DND_ROW: &str = "dnd-row";
/// Its label.
pub const DND_LABEL: &str = "dnd-label";
/// Text standing in for content a later milestone fills in.
pub const PLACEHOLDER: &str = "placeholder";

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
    POPOVER_WINDOW,
    POPOVER_WRAPPER,
    POPOVER_SURFACE,
    BORDERLESS,
    CLICK_CATCHER_WINDOW,
    CLICK_CATCHER,
    CONTROL_PANEL,
    CONTROL_PANEL_COLUMN,
    CONTROL_PANEL_DIVIDER,
    CARD,
    CARD_TITLE,
    CONTROL_PANEL_TIME,
    CONTROL_PANEL_DATE,
    WORLD_CLOCK_ROW,
    WORLD_CLOCK_NAME,
    WORLD_CLOCK_ZONE,
    WORLD_CLOCK_TIME,
    EMPTY_STATE,
    EMPTY_STATE_ICON,
    EMPTY_STATE_LABEL,
    DND_ROW,
    DND_LABEL,
    PLACEHOLDER,
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
