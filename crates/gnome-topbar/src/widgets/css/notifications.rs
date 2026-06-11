//! Notification widget CSS.

const NOTIFICATION_CARD_RADIUS: &str = "var(--radius-pill)";
const NOTIFICATION_ROW_PADDING: i32 = 6;
const NOTIFICATION_ACTION_GAP: i32 = 6;
const NOTIFICATION_CONTENT_INSET: i32 = 16;

/// Return notifications CSS.
pub fn css(animations: bool) -> String {
    // The dismiss fade-out (`.notification-row-dismissing { opacity: 0 }`) is now
    // frame-clock driven from Rust via the shared Animation helper (see
    // notifications_panel.rs `fade_out_dismissing`); the Revealer still handles
    // the height collapse. The former `transition: opacity {ms}ms ease` rule is
    // removed and `animations` is no longer read here.
    let _ = animations;
    let action_gap = NOTIFICATION_ACTION_GAP;
    let card_radius = NOTIFICATION_CARD_RADIUS;
    let content_inset = NOTIFICATION_CONTENT_INSET;
    let row_padding = NOTIFICATION_ROW_PADDING;

    format!(
        r#"
/* ===== NOTIFICATIONS ===== */
/* Shared styles for both popover rows and toasts */

/* Bell icon states */
.notification-icon.has-critical {{
    color: var(--color-state-warning);
}}

.notification-icon.backend-unavailable {{
    color: var(--color-foreground-disabled);
}}

/* Badge indicator dot */
.notification-badge {{
    margin-right: 0;
    margin-top: 0;
}}

.notification-badge-dot {{
    min-width: 4px;
    min-height: 4px;
    padding: 0;
    border-radius: var(--radius-round);
    background-color: var(--color-accent-primary);
}}

.clock-notifications {{
    margin-left: 8px;
}}

/* Shared icon styling (row + toast) */
.notification-row-icon,
.notification-toast-icon {{
    margin-top: 2px;
    min-width: 48px;
    min-height: 48px;
    border-radius: var(--radius-round);
}}

/* Shared typography (row + toast) */
.notification-app-name {{
    font-size: var(--font-size-sm);
    font-weight: 600;
    line-height: 1.15;
}}

.notification-toast-app {{
    font-size: calc(var(--font-size-md) * 1.05);
    font-weight: 600;
    color: var(--color-foreground-muted);
    line-height: 1.15;
}}

.notification-summary,
.notification-toast-summary {{
    font-size: calc(var(--font-size-md) * 1.1);
    font-weight: 600;
    color: var(--color-foreground-primary);
    line-height: 1.18;
}}

.notification-body,
.notification-toast-body {{
    font-size: calc(var(--font-size-md) * 1.1);
    color: var(--color-foreground-primary);
    line-height: 1.18;
    font-weight: 400;
    margin-top: 1px;
}}

/* Shared dismiss button styling (row + toast) */
.notification-dismiss-btn,
.notification-toast-dismiss {{
    min-width: 20px;
    min-height: 20px;
    padding: 0;
    opacity: 0.7;
    border-radius: var(--radius-round);
}}

.notification-dismiss-btn:hover,
.notification-toast-dismiss:hover {{
    opacity: 1;
    background: var(--color-card-overlay-hover);
}}

.notification-dismiss-btn {{
    margin-left: 4px;
    margin-top: -3px;
    margin-right: -3px;
}}

.notification-toast-dismiss {{
    margin-top: -3px;
    margin-right: -3px;
}}

/* Shared urgency styling (row + toast) */
.notification-row.notification-critical,
.notification-toast-critical {{
    border-left: 3px solid var(--color-state-warning);
}}

.notification-row.notification-critical {{
    background-color: var(--color-row-critical-background);
}}

.notification-toast-critical {{
    background-color: var(--color-toast-critical-background);
}}

.notification-row.notification-low {{
    opacity: 0.8;
}}

.notification-toast-low {{
    opacity: 0.9;
}}

/* === Control-panel notification list === */

/* Remove right padding from the surface so the overlay scrollbar sits at the
   panel edge instead of overlapping dismiss buttons. The header and list
   add their own right padding to keep content inset. */
.notification-popover {{
    padding-right: 0;
}}

.notification-header {{
    padding: 0 {content_inset}px 8px 0;
    margin: 0;
}}

.notification-header .notification-header-icon-btn {{
    margin-top: -4px;
}}

.notification-header-icon-btn.notification-mute-active {{
    background: var(--color-accent-primary);
    color: var(--color-accent-text, #fff);
}}

.notification-header-icon-btn.notification-mute-active:hover {{
    background: var(--color-accent-hover-bg);
}}

.notification-header-icon-btn.notification-mute-active .notification-header-icon,
.notification-header-icon-btn.notification-mute-active .vp-primary {{
    color: var(--color-accent-text, #fff);
}}

.notification-header-icon {{
    font-size: calc(var(--icon-size) * 1.15);
    margin-top: 1px;
    margin-left: 1px;
}}

.notification-clear-label {{
    font-size: var(--font-size-sm);
}}

.notification-list {{
    padding: 8px {content_inset}px 0 0;
}}

.notification-app-group {{
    padding: 0;
    border-radius: {card_radius};
}}

.notification-group-header {{
    padding: 0;
}}

button.notification-group-header,
button.notification-group-clear {{
    min-height: 0;
    min-width: 0;
    border-radius: {card_radius};
}}

button.notification-group-header {{
    padding: 6px;
}}

button.notification-group-clear {{
    margin: 4px 4px 4px 0;
    padding: 4px;
}}

button.notification-group-header:hover,
button.notification-group-clear:hover {{
    background: var(--color-card-overlay-hover);
}}

.notification-group-count {{
    font-size: var(--font-size-xs);
}}

.notification-group-list {{
    padding: 4px 6px 6px 6px;
}}

/* Empty state */
.notification-empty {{
    padding: 32px 16px;
}}

.notification-empty-label {{
    font-size: var(--font-size-sm);
}}

/* Notification row (spacing between rows handled by GtkBox) */
.notification-row {{
    padding: {row_padding}px;
    border-radius: {card_radius};
}}

/* Dismiss fade-out opacity is Rust-driven (see notifications_panel.rs); height
   collapse is handled by the Revealer. No CSS opacity transition here. */

.notification-timestamp {{
    font-size: var(--font-size-sm);
}}

/* Action buttons */
.notification-actions {{
    margin-top: {action_gap}px;
}}

button.notification-action-btn {{
    padding: 0;
    min-height: 0;
    min-width: 0;
    border-radius: var(--radius-widget);
    color: var(--color-accent-primary);
}}

button.notification-action-btn label {{
    font-size: var(--font-size-sm);
    padding: 2px 6px;
}}

/* === Toast-specific === */

window.notification-toast-wrapper,
.notification-toast-wrapper {{
    background: transparent;
}}

.notification-toast-container {{
    padding: 12px 14px;
    min-width: 300px;
}}

.notification-toast {{
    background-color: color-mix(in srgb, var(--color-background-popover) 58%, transparent);
}}

.notification-toast-actions {{
    margin-top: 10px;
    padding-top: 8px;
}}

button.notification-toast-action {{
    min-height: 0;
    border-radius: var(--radius-widget);
    color: var(--color-accent-primary);
}}

button.notification-toast-action label {{
    font-size: var(--font-size-sm);
    padding: 4px 8px;
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_css_styles_muted_header_button_as_active() {
        let css = css(true);

        assert!(css.contains(".notification-header-icon-btn.notification-mute-active"));
        assert!(css.contains("background: var(--color-accent-primary);"));
        assert!(css.contains("color: var(--color-accent-text, #fff);"));
        assert!(css.contains(".notification-header-icon-btn.notification-mute-active .vp-primary"));
    }
}
