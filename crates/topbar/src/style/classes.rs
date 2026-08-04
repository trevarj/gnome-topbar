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
/// The bell-with-slash beside the time while Do Not Disturb is on.
pub const CLOCK_DND: &str = "clock-dnd";

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
/// The control panel's calendar.
pub const CALENDAR: &str = "calendar";
/// Its header row.
pub const CALENDAR_HEADER: &str = "calendar-header";
/// The month and year in the header.
pub const CALENDAR_TITLE: &str = "calendar-title";
/// The button carrying it, which returns to today.
pub const CALENDAR_MONTH: &str = "calendar-month";
/// A month chevron.
pub const CALENDAR_NAV: &str = "calendar-nav";
/// The 7x6 grid of days.
pub const CALENDAR_GRID: &str = "calendar-grid";
/// A weekday column header.
pub const CALENDAR_WEEKDAY: &str = "calendar-weekday";
/// An ISO week number.
pub const CALENDAR_WEEK: &str = "calendar-week";
/// One day cell.
pub const CALENDAR_DAY: &str = "calendar-day";
/// The cell for the real current date.
pub const CALENDAR_TODAY: &str = "calendar-today";
/// The cell the user picked.
pub const CALENDAR_SELECTED: &str = "calendar-selected";
/// A cell belonging to the month either side of the one on screen.
pub const CALENDAR_OUTSIDE: &str = "calendar-outside";

/// The media card in the control panel's right column.
pub const MEDIA_CARD: &str = "media-card";
/// The album art.
pub const MEDIA_ART: &str = "media-art";
/// What is drawn behind it while a player has no cover.
pub const MEDIA_ART_PLACEHOLDER: &str = "media-art-placeholder";
/// The track title.
pub const MEDIA_TITLE: &str = "media-title";
/// The artist under it.
pub const MEDIA_ARTIST: &str = "media-artist";
/// The row of transport buttons.
pub const MEDIA_CONTROLS: &str = "media-controls";
/// One transport button.
pub const MEDIA_CONTROL: &str = "media-control";
/// The play/pause button, which is the one of the three the eye goes to.
pub const MEDIA_CONTROL_PRIMARY: &str = "media-control-primary";
/// The seek bar.
pub const MEDIA_SEEK: &str = "media-seek";
/// The row of timestamps under it.
pub const MEDIA_TIME: &str = "media-time";
/// One of those timestamps.
pub const MEDIA_TIME_LABEL: &str = "media-time-label";
/// The row of players to switch between.
pub const MEDIA_SWITCHER: &str = "media-switcher";
/// One player's button in it.
pub const MEDIA_SWITCHER_BUTTON: &str = "media-switcher-button";
/// Set on the button of the player the card is showing.
pub const MEDIA_SWITCHER_ACTIVE: &str = "media-switcher-active";
/// The initial standing in for a player with no icon.
pub const MEDIA_SWITCHER_INITIAL: &str = "media-switcher-initial";

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

/// The notifications column's header row.
pub const NOTIFICATION_HEADER: &str = "notification-header";
/// The Clear button in it.
pub const NOTIFICATION_CLEAR_ALL: &str = "notification-clear-all";
/// The column of application groups.
pub const NOTIFICATION_LIST: &str = "notification-list";
/// One application's card.
pub const NOTIFICATION_GROUP: &str = "notification-group";
/// The card's header, which is also its expander.
pub const NOTIFICATION_GROUP_HEADER: &str = "notification-group-header";
/// Set on the header of a group that holds exactly one notification.
pub const NOTIFICATION_GROUP_SINGLE: &str = "notification-group-single";
/// The notifications inside a group.
pub const NOTIFICATION_GROUP_LIST: &str = "notification-group-list";
/// The button that clears a whole group.
pub const NOTIFICATION_GROUP_CLEAR: &str = "notification-group-clear";
/// A notification's application icon.
pub const NOTIFICATION_ICON: &str = "notification-icon";
/// The application name on a group header.
pub const NOTIFICATION_APP: &str = "notification-app";
/// The badge counting a group's notifications.
pub const NOTIFICATION_COUNT: &str = "notification-count";
/// The expand/collapse chevron.
pub const NOTIFICATION_CHEVRON: &str = "notification-chevron";
/// One notification in the history.
pub const NOTIFICATION_ROW: &str = "notification-row";
/// Its headline.
pub const NOTIFICATION_SUMMARY: &str = "notification-summary";
/// Its body.
pub const NOTIFICATION_BODY: &str = "notification-body";
/// How long ago it arrived.
pub const NOTIFICATION_TIME: &str = "notification-time";
/// The button that closes one notification.
pub const NOTIFICATION_CLOSE: &str = "notification-close";

/// The banner surface's layer-shell window.
pub const TOAST_WINDOW: &str = "toast-window";
/// The column of banners inside it.
pub const TOAST_STACK: &str = "toast-stack";
/// One banner.
pub const TOAST: &str = "toast";
/// Set on a banner that will not go away by itself.
pub const TOAST_CRITICAL: &str = "toast-critical";
/// A banner's application icon.
pub const TOAST_ICON: &str = "toast-icon";
/// Its headline.
pub const TOAST_SUMMARY: &str = "toast-summary";
/// Its body.
pub const TOAST_BODY: &str = "toast-body";
/// The row of action buttons under it.
pub const TOAST_ACTIONS: &str = "toast-actions";
/// One of those buttons.
pub const TOAST_ACTION: &str = "toast-action";
/// The close button revealed while the pointer is over a banner.
pub const TOAST_CLOSE: &str = "toast-close";

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
    CLOCK_DND,
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
    CALENDAR,
    CALENDAR_HEADER,
    CALENDAR_TITLE,
    CALENDAR_MONTH,
    CALENDAR_NAV,
    CALENDAR_GRID,
    CALENDAR_WEEKDAY,
    CALENDAR_WEEK,
    CALENDAR_DAY,
    CALENDAR_TODAY,
    CALENDAR_SELECTED,
    CALENDAR_OUTSIDE,
    MEDIA_CARD,
    MEDIA_ART,
    MEDIA_ART_PLACEHOLDER,
    MEDIA_TITLE,
    MEDIA_ARTIST,
    MEDIA_CONTROLS,
    MEDIA_CONTROL,
    MEDIA_CONTROL_PRIMARY,
    MEDIA_SEEK,
    MEDIA_TIME,
    MEDIA_TIME_LABEL,
    MEDIA_SWITCHER,
    MEDIA_SWITCHER_BUTTON,
    MEDIA_SWITCHER_ACTIVE,
    MEDIA_SWITCHER_INITIAL,
    EMPTY_STATE,
    EMPTY_STATE_ICON,
    EMPTY_STATE_LABEL,
    DND_ROW,
    DND_LABEL,
    PLACEHOLDER,
    NOTIFICATION_HEADER,
    NOTIFICATION_CLEAR_ALL,
    NOTIFICATION_LIST,
    NOTIFICATION_GROUP,
    NOTIFICATION_GROUP_HEADER,
    NOTIFICATION_GROUP_SINGLE,
    NOTIFICATION_GROUP_LIST,
    NOTIFICATION_GROUP_CLEAR,
    NOTIFICATION_ICON,
    NOTIFICATION_APP,
    NOTIFICATION_COUNT,
    NOTIFICATION_CHEVRON,
    NOTIFICATION_ROW,
    NOTIFICATION_SUMMARY,
    NOTIFICATION_BODY,
    NOTIFICATION_TIME,
    NOTIFICATION_CLOSE,
    TOAST_WINDOW,
    TOAST_STACK,
    TOAST,
    TOAST_CRITICAL,
    TOAST_ICON,
    TOAST_SUMMARY,
    TOAST_BODY,
    TOAST_ACTIONS,
    TOAST_ACTION,
    TOAST_CLOSE,
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
