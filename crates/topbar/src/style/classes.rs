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
/// The drawing area a press ripple is painted on.
pub const RIPPLE: &str = "ripple";
/// Wrapper holding a button's ripple, which takes the button's own shape.
pub const RIPPLE_CLIP: &str = "ripple-clip";
/// Set on the wrapper while this widget's popover is open.
pub const CHECKED: &str = "checked";

/// Set on a widget whose service has lost its connection.
pub const DISCONNECTED: &str = "disconnected";

/// Tint a widget's contents with the success colour.
///
/// The three of these are the panel's semantic states as a class rather than
/// as a widget: a `custom-*` script asking for `class: "warning"`, a headset
/// down to its last tenth, a metric over its threshold. `color` inherits in
/// GTK's CSS, so putting one on a wrapper tints the icons and labels inside it.
pub const STATE_SUCCESS: &str = "state-success";
/// The same, in the warning colour.
pub const STATE_WARNING: &str = "state-warning";
/// The same, in the urgent colour.
pub const STATE_URGENT: &str = "state-urgent";

/// The clock widget.
pub const CLOCK: &str = "clock";
/// The bell-with-slash beside the time while Do Not Disturb is on.
pub const CLOCK_DND: &str = "clock-dnd";

/// The workspaces widget.
pub const WORKSPACES: &str = "workspaces";
/// CSS name of the custom widget that draws the workspace indicators.
pub const WORKSPACE_STRIP: &str = "workspace-strip";

/// The weather widget.
pub const WEATHER: &str = "weather";
/// The condition icon in it.
pub const WEATHER_ICON: &str = "weather-icon";

/// The forecast component, in the control panel and in the popover.
pub const FORECAST: &str = "forecast";
/// Its header row: the title, the place, and the gear.
pub const FORECAST_HEADER: &str = "forecast-header";
/// The place the forecast is for.
pub const FORECAST_LOCATION: &str = "forecast-location";
/// The gear that opens the location dialog.
pub const FORECAST_CONFIGURE: &str = "forecast-configure";
/// The line of current conditions.
pub const FORECAST_CURRENT: &str = "forecast-current";
/// Its icon.
pub const FORECAST_CURRENT_ICON: &str = "forecast-current-icon";
/// The temperature, which is the biggest thing on the card.
pub const FORECAST_CURRENT_TEMP: &str = "forecast-current-temp";
/// The condition and the feels-like under it.
pub const FORECAST_CURRENT_CONDITION: &str = "forecast-current-condition";
/// The column of days.
pub const FORECAST_DAYS: &str = "forecast-days";
/// One day.
pub const FORECAST_ROW: &str = "forecast-row";
/// Its weekday.
pub const FORECAST_DAY: &str = "forecast-day";
/// Its condition icon.
pub const FORECAST_ICON: &str = "forecast-icon";
/// Its condition in words.
pub const FORECAST_CONDITION: &str = "forecast-condition";
/// Its high and low.
pub const FORECAST_TEMPS: &str = "forecast-temps";
/// Its chance of precipitation.
pub const FORECAST_PRECIPITATION: &str = "forecast-precipitation";
/// The dimmed row saying how old a stale reading is.
pub const FORECAST_STALE: &str = "forecast-stale";
/// The button that asks for a fresh one.
pub const FORECAST_RETRY: &str = "forecast-retry";

/// The crypto widget.
pub const CRYPTO: &str = "crypto";
/// One entry on the bar: its logo (or two) and its number.
pub const CRYPTO_ENTRY: &str = "crypto-entry";
/// An asset's logo, wherever it is drawn.
pub const CRYPTO_ICON: &str = "crypto-icon";
/// The denominator's logo on a pair, on the numerator's shoulder.
pub const CRYPTO_BADGE: &str = "crypto-badge";
/// An entry's number on the bar.
pub const CRYPTO_VALUE: &str = "crypto-value";

/// The crypto popover, both views.
pub const CRYPTO_POPOVER: &str = "crypto-popover";
/// The header row of either view.
pub const CRYPTO_HEADER: &str = "crypto-header";
/// The gear that opens the settings view.
pub const CRYPTO_CONFIGURE: &str = "crypto-configure";
/// The button that comes back from it.
pub const CRYPTO_BACK: &str = "crypto-back";
/// The column of price rows.
pub const CRYPTO_LIST: &str = "crypto-list";
/// One price row.
pub const CRYPTO_ROW: &str = "crypto-row";
/// What the row's entry is called.
pub const CRYPTO_NAME: &str = "crypto-name";
/// Its price, written out.
pub const CRYPTO_ROW_VALUE: &str = "crypto-row-value";
/// The 24-hour change chip beside it.
pub const CRYPTO_CHANGE: &str = "crypto-change";
/// Set on a chip that went up.
pub const CRYPTO_CHANGE_UP: &str = "crypto-change-up";
/// Set on one that went down.
pub const CRYPTO_CHANGE_DOWN: &str = "crypto-change-down";
/// The dimmed line saying how old the prices are.
pub const CRYPTO_UPDATED: &str = "crypto-updated";

/// The settings view.
pub const CRYPTO_SETTINGS: &str = "crypto-settings";
/// One of its headings.
pub const CRYPTO_SECTION: &str = "crypto-section";
/// One of its rows.
pub const CRYPTO_SETTING_ROW: &str = "crypto-setting-row";
/// An up or down arrow on one.
pub const CRYPTO_REORDER: &str = "crypto-reorder";
/// The cross that takes a pair off the bar.
pub const CRYPTO_REMOVE: &str = "crypto-remove";
/// The row that builds a new pair.
pub const CRYPTO_ADD_PAIR: &str = "crypto-add-pair";
/// The slash between its two dropdowns.
pub const CRYPTO_PAIR_SLASH: &str = "crypto-pair-slash";

/// The location dialog's layer-shell window.
pub const LOCATION_WINDOW: &str = "location-window";
/// The dimmed surface behind it.
pub const LOCATION_BACKDROP: &str = "location-backdrop";
/// The dialog itself.
pub const LOCATION_DIALOG: &str = "location-dialog";
/// Its heading.
pub const LOCATION_TITLE: &str = "location-title";
/// The search entry.
pub const LOCATION_SEARCH: &str = "location-search";
/// The list of places the search found.
pub const LOCATION_RESULTS: &str = "location-results";
/// One of them.
pub const LOCATION_RESULT: &str = "location-result";
/// Set on the row the user picked.
pub const LOCATION_RESULT_SELECTED: &str = "location-result-selected";
/// The line explaining why nothing happened.
pub const LOCATION_ERROR: &str = "location-error";
/// The Advanced expander holding the coordinate entries.
pub const LOCATION_ADVANCED: &str = "location-advanced";
/// A coordinate entry.
pub const LOCATION_COORDINATE: &str = "location-coordinate";
/// The Cancel/Save row.
pub const LOCATION_ACTIONS: &str = "location-actions";
/// A dialog button.
pub const DIALOG_BUTTON: &str = "dialog-button";
/// The one that commits.
pub const DIALOG_BUTTON_PRIMARY: &str = "dialog-button-primary";

/// The keyboard-layout widget.
pub const KEYBOARD_LAYOUT: &str = "keyboard-layout";
/// The keyboard-layout widget's icon.
pub const KEYBOARD_LAYOUT_ICON: &str = "keyboard-layout-icon";

/// A `custom-*` widget's configured icon.
///
/// The widget itself wears a class made from its own name — `custom-crypto`
/// becomes `.custom-crypto` — so one script's indicator can be styled without
/// styling the rest. These two are what every one of them has.
pub const CUSTOM_ICON: &str = "custom-icon";
/// Its label, which is whatever the script printed.
pub const CUSTOM_LABEL: &str = "custom-label";

/// The headset battery widget.
pub const HEADSET: &str = "headset";
/// The battery icon inside it.
pub const HEADSET_ICON: &str = "headset-icon";

/// The distribution logo.
pub const OS_LOGO: &str = "os-logo";
/// Its Nerd Font glyph — the one place in the panel a glyph stands in for an
/// icon, because no icon theme ships distribution logos as symbolics.
pub const OS_LOGO_GLYPH: &str = "os-logo-glyph";
/// The themed icon drawn instead on a distribution with no glyph.
pub const OS_LOGO_ICON: &str = "os-logo-icon";

/// The alert-only system monitor.
pub const SYSTEM_MONITOR: &str = "system-monitor";
/// One offending metric's icon inside it.
pub const SYSTEM_MONITOR_ICON: &str = "system-monitor-icon";
/// The compact reading beside it.
pub const SYSTEM_MONITOR_VALUE: &str = "system-monitor-value";
/// The popover it opens, which is the Quick Settings resource card again.
pub const SYSTEM_MONITOR_POPOVER: &str = "system-monitor-popover";

/// The system tray.
pub const TRAY: &str = "tray";
/// The row of item icons inside it.
pub const TRAY_ICONS: &str = "tray-icons";
/// One item's icon button, on the bar or in the overflow grid.
pub const TRAY_ITEM: &str = "tray-item";
/// The icon inside it.
pub const TRAY_ITEM_ICON: &str = "tray-item-icon";
/// The warning tint painted behind an item that wants attention.
pub const TRAY_ITEM_TINT: &str = "tray-item-tint";
/// Set on the button of an item that wants attention.
pub const TRAY_ITEM_SHOUTING: &str = "tray-item-shouting";
/// Set on a *themed* icon that wants attention, which may be recoloured.
pub const TRAY_ITEM_ATTENTION: &str = "tray-item-attention";
/// The chevron that opens the overflow popover.
pub const TRAY_OVERFLOW: &str = "tray-overflow";
/// That popover.
pub const TRAY_OVERFLOW_POPOVER: &str = "tray-overflow-popover";
/// The grid of icons in it.
pub const TRAY_OVERFLOW_GRID: &str = "tray-overflow-grid";

/// An application's own menu, as a popover.
pub const TRAY_MENU: &str = "tray-menu";
/// The column of rows in it.
pub const TRAY_MENU_LIST: &str = "tray-menu-list";
/// One row.
pub const TRAY_MENU_ROW: &str = "tray-menu-row";
/// A row's label.
pub const TRAY_MENU_LABEL: &str = "tray-menu-label";
/// A row's own icon.
pub const TRAY_MENU_ICON: &str = "tray-menu-icon";
/// The checkmark or radio column.
pub const TRAY_MENU_MARK: &str = "tray-menu-mark";
/// The chevron on a row that leads into a submenu.
pub const TRAY_MENU_CHEVRON: &str = "tray-menu-chevron";
/// The rule between groups of rows.
pub const TRAY_MENU_SEPARATOR: &str = "tray-menu-separator";
/// The row that leads back out of a submenu.
pub const TRAY_MENU_BACK: &str = "tray-menu-back";
/// What a menu with nothing in it says.
pub const TRAY_MENU_EMPTY: &str = "tray-menu-empty";

/// Set on a control the thing behind it has switched off.
pub const DISABLED: &str = "disabled";

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
///
/// Unused between milestones — M6 took the last placeholder out of the control
/// panel — but the rule and the constant stay, because every milestone that
/// reserves a card needs them again.
#[allow(dead_code)]
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

/// The volume/brightness capsule's layer-shell window.
pub const OSD_WINDOW: &str = "osd-window";
/// The capsule itself.
pub const OSD_CAPSULE: &str = "osd-capsule";
/// Its icon.
pub const OSD_ICON: &str = "osd-icon";
/// CSS name of the custom widget that draws the fill.
pub const OSD_BAR: &str = "osd-bar";
/// The line explaining an icon-only capsule.
pub const OSD_CAPTION: &str = "osd-caption";
/// The numeric percentage, shown only when `[osd] show_value` is on.
pub const OSD_VALUE: &str = "osd-value";

/// The Quick Settings bar button.
pub const QUICK_SETTINGS: &str = "quick-settings";
/// The row of status icons inside it.
pub const QS_INDICATOR: &str = "qs-indicator";
/// One of those icons.
pub const QS_ICON: &str = "qs-icon";
/// Set on the battery icon when it is low and nothing is charging it.
pub const QS_ICON_URGENT: &str = "qs-icon-urgent";
/// A status icon in the accent colour: something the user switched on.
pub const QS_ICON_ACCENT: &str = "qs-icon-accent";
/// The dot that says a microphone is live.
pub const QS_PRIVACY_DOT: &str = "qs-privacy-dot";

/// The Quick Settings panel's content.
pub const QS_PANEL: &str = "qs-panel";
/// The scroller inside it, which is what stops a tall panel overflowing.
pub const QS_SCROLL: &str = "qs-scroll";
/// The column of blocks inside the scroller.
pub const QS_CONTENT: &str = "qs-content";
/// One expandable section's slot, below the row that opens it.
pub const QS_SECTION: &str = "qs-section";

/// The header row: the battery pill, the lock button and the power button.
pub const QS_HEADER: &str = "qs-header";
/// The battery pill.
pub const QS_BATTERY_PILL: &str = "qs-battery-pill";
/// Its percentage.
pub const QS_BATTERY_PERCENT: &str = "qs-battery-percent";
/// A round icon button in the header.
pub const QS_ROUND_BUTTON: &str = "qs-round-button";

/// The block of sliders.
pub const QS_SLIDERS: &str = "qs-sliders";
/// One slider's row.
pub const QS_SLIDER_ROW: &str = "qs-slider-row";
/// The slider itself.
pub const QS_SLIDER: &str = "qs-slider";
/// The icon beside it, which is also the mute button where there is one.
pub const QS_SLIDER_ICON: &str = "qs-slider-icon";
/// The chevron that opens a device list.
pub const QS_CHOOSER: &str = "qs-chooser";
/// A list of output devices.
pub const QS_DEVICE_LIST: &str = "qs-device-list";
/// One device in it.
pub const QS_DEVICE_ROW: &str = "qs-device-row";
/// Its name.
pub const QS_DEVICE_NAME: &str = "qs-device-name";
/// The checkmark against the device in use.
pub const QS_DEVICE_MARK: &str = "qs-device-mark";
/// A paired Bluetooth device's battery level.
pub const QS_DEVICE_BATTERY: &str = "qs-device-battery";
/// The switch that connects or disconnects one.
pub const QS_DEVICE_SWITCH: &str = "qs-device-switch";

/// The grid of toggle pills.
pub const QS_GRID: &str = "qs-grid";
/// One row of two pills.
pub const QS_GRID_ROW: &str = "qs-grid-row";
/// One pill.
pub const QS_TOGGLE: &str = "qs-toggle";
/// Its icon.
pub const QS_TOGGLE_ICON: &str = "qs-toggle-icon";
/// Its label.
pub const QS_TOGGLE_LABEL: &str = "qs-toggle-label";
/// The dimmer line under it.
pub const QS_TOGGLE_SUBTITLE: &str = "qs-toggle-subtitle";
/// The chevron on an expandable pill.
pub const QS_TOGGLE_EXPAND: &str = "qs-toggle-expand";
/// One row of a radio list, e.g. a power profile.
pub const QS_RADIO_ROW: &str = "qs-radio-row";
/// Its mark.
pub const QS_RADIO_MARK: &str = "qs-radio-mark";

/// One network in the Wi-Fi list.
pub const QS_NETWORK_ROW: &str = "qs-network-row";
/// The padlock beside a network that wants a key.
pub const QS_NETWORK_BADGE: &str = "qs-network-badge";
/// A header above a list, with the scanning spinner in it.
pub const QS_LIST_HEADER: &str = "qs-list-header";
/// The password box that opens under a network row.
pub const QS_PASSWORD_ROW: &str = "qs-password-row";
/// The entry inside it.
pub const QS_PASSWORD_ENTRY: &str = "qs-password-entry";
/// A button inside it.
pub const QS_PASSWORD_BUTTON: &str = "qs-password-button";
/// The pairing box that opens under a Bluetooth device row.
pub const QS_PAIRING_ROW: &str = "qs-pairing-row";
/// The six-digit code inside it.
pub const QS_PAIRING_CODE: &str = "qs-pairing-code";
/// A full-width row that states something rather than doing anything.
pub const QS_STATUS_ROW: &str = "qs-status-row";
/// The switch on one VPN row.
pub const QS_VPN_ROW: &str = "qs-vpn-row";

/// A card inside the panel, e.g. battery health.
pub const QS_CARD: &str = "qs-card";
/// Its heading.
pub const QS_CARD_TITLE: &str = "qs-card-title";
/// A line of detail in it.
pub const QS_CARD_LINE: &str = "qs-card-line";
/// The row of charge-limit buttons.
pub const QS_LIMIT_ROW: &str = "qs-limit-row";
/// One of them.
pub const QS_LIMIT_BUTTON: &str = "qs-limit-button";
/// The pending-updates card.
pub const QS_UPDATES: &str = "qs-updates";
/// The resource-overview card.
pub const QS_RESOURCES: &str = "qs-resources";
/// One metered row inside it: a caption, a bar and a reading.
pub const QS_METER_ROW: &str = "qs-meter-row";
/// The bar itself.
pub const QS_METER: &str = "qs-meter";
/// Set on a bar whose reading has crossed its threshold.
pub const QS_METER_WARNING: &str = "qs-meter-warning";
/// The number beside it.
pub const QS_METER_VALUE: &str = "qs-meter-value";
/// A dimmed explanation under a control that cannot be used.
pub const QS_HINT: &str = "qs-hint";

/// One power action's row.
pub const QS_POWER_ROW: &str = "qs-power-row";
/// The accent fill that grows across it while it is held.
pub const QS_POWER_FILL: &str = "qs-power-fill";
/// Set on a row that is being held down.
pub const CONFIRMING: &str = "confirming";

/// A failure reported under the control that caused it.
pub const INLINE_ERROR: &str = "inline-error";

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
    RIPPLE,
    RIPPLE_CLIP,
    CHECKED,
    DISCONNECTED,
    STATE_SUCCESS,
    STATE_WARNING,
    STATE_URGENT,
    CLOCK,
    CLOCK_DND,
    WORKSPACES,
    WORKSPACE_STRIP,
    WEATHER,
    WEATHER_ICON,
    FORECAST,
    FORECAST_HEADER,
    FORECAST_LOCATION,
    FORECAST_CONFIGURE,
    FORECAST_CURRENT,
    FORECAST_CURRENT_ICON,
    FORECAST_CURRENT_TEMP,
    FORECAST_CURRENT_CONDITION,
    FORECAST_DAYS,
    FORECAST_ROW,
    FORECAST_DAY,
    FORECAST_ICON,
    FORECAST_CONDITION,
    FORECAST_TEMPS,
    FORECAST_PRECIPITATION,
    FORECAST_STALE,
    FORECAST_RETRY,
    CRYPTO,
    CRYPTO_ENTRY,
    CRYPTO_ICON,
    CRYPTO_BADGE,
    CRYPTO_VALUE,
    CRYPTO_POPOVER,
    CRYPTO_HEADER,
    CRYPTO_CONFIGURE,
    CRYPTO_BACK,
    CRYPTO_LIST,
    CRYPTO_ROW,
    CRYPTO_NAME,
    CRYPTO_ROW_VALUE,
    CRYPTO_CHANGE,
    CRYPTO_CHANGE_UP,
    CRYPTO_CHANGE_DOWN,
    CRYPTO_UPDATED,
    CRYPTO_SETTINGS,
    CRYPTO_SECTION,
    CRYPTO_SETTING_ROW,
    CRYPTO_REORDER,
    CRYPTO_REMOVE,
    CRYPTO_ADD_PAIR,
    CRYPTO_PAIR_SLASH,
    LOCATION_WINDOW,
    LOCATION_BACKDROP,
    LOCATION_DIALOG,
    LOCATION_TITLE,
    LOCATION_SEARCH,
    LOCATION_RESULTS,
    LOCATION_RESULT,
    LOCATION_RESULT_SELECTED,
    LOCATION_ERROR,
    LOCATION_ADVANCED,
    LOCATION_COORDINATE,
    LOCATION_ACTIONS,
    DIALOG_BUTTON,
    DIALOG_BUTTON_PRIMARY,
    KEYBOARD_LAYOUT,
    KEYBOARD_LAYOUT_ICON,
    CUSTOM_ICON,
    CUSTOM_LABEL,
    HEADSET,
    HEADSET_ICON,
    OS_LOGO,
    OS_LOGO_GLYPH,
    OS_LOGO_ICON,
    SYSTEM_MONITOR,
    SYSTEM_MONITOR_ICON,
    SYSTEM_MONITOR_VALUE,
    SYSTEM_MONITOR_POPOVER,
    TRAY,
    TRAY_ICONS,
    TRAY_ITEM,
    TRAY_ITEM_ICON,
    TRAY_ITEM_TINT,
    TRAY_ITEM_SHOUTING,
    TRAY_ITEM_ATTENTION,
    TRAY_OVERFLOW,
    TRAY_OVERFLOW_POPOVER,
    TRAY_OVERFLOW_GRID,
    TRAY_MENU,
    TRAY_MENU_LIST,
    TRAY_MENU_ROW,
    TRAY_MENU_LABEL,
    TRAY_MENU_ICON,
    TRAY_MENU_MARK,
    TRAY_MENU_CHEVRON,
    TRAY_MENU_SEPARATOR,
    TRAY_MENU_BACK,
    TRAY_MENU_EMPTY,
    DISABLED,
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
    OSD_WINDOW,
    OSD_CAPSULE,
    OSD_ICON,
    OSD_BAR,
    OSD_CAPTION,
    OSD_VALUE,
    QUICK_SETTINGS,
    QS_INDICATOR,
    QS_ICON,
    QS_ICON_URGENT,
    QS_ICON_ACCENT,
    QS_PRIVACY_DOT,
    QS_PANEL,
    QS_SCROLL,
    QS_CONTENT,
    QS_SECTION,
    QS_HEADER,
    QS_BATTERY_PILL,
    QS_BATTERY_PERCENT,
    QS_ROUND_BUTTON,
    QS_SLIDERS,
    QS_SLIDER_ROW,
    QS_SLIDER,
    QS_SLIDER_ICON,
    QS_CHOOSER,
    QS_DEVICE_LIST,
    QS_DEVICE_ROW,
    QS_DEVICE_NAME,
    QS_DEVICE_MARK,
    QS_DEVICE_BATTERY,
    QS_DEVICE_SWITCH,
    QS_GRID,
    QS_GRID_ROW,
    QS_TOGGLE,
    QS_TOGGLE_ICON,
    QS_TOGGLE_LABEL,
    QS_TOGGLE_SUBTITLE,
    QS_TOGGLE_EXPAND,
    QS_RADIO_ROW,
    QS_RADIO_MARK,
    QS_NETWORK_ROW,
    QS_NETWORK_BADGE,
    QS_LIST_HEADER,
    QS_PASSWORD_ROW,
    QS_PASSWORD_ENTRY,
    QS_PASSWORD_BUTTON,
    QS_PAIRING_ROW,
    QS_PAIRING_CODE,
    QS_STATUS_ROW,
    QS_VPN_ROW,
    QS_CARD,
    QS_CARD_TITLE,
    QS_CARD_LINE,
    QS_LIMIT_ROW,
    QS_LIMIT_BUTTON,
    QS_UPDATES,
    QS_RESOURCES,
    QS_METER_ROW,
    QS_METER,
    QS_METER_WARNING,
    QS_METER_VALUE,
    QS_HINT,
    QS_POWER_ROW,
    QS_POWER_FILL,
    CONFIRMING,
    INLINE_ERROR,
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
