//! Combined clock control panel CSS.

/// Return control panel CSS.
pub fn css() -> &'static str {
    r#"
/* ===== CONTROL PANEL ===== */

.control-panel .notification-header .vp-popover-title,
.control-panel .calendar-header .vp-popover-title {
    font-size: var(--font-size-base);
    font-weight: 600;
}

.control-panel-time {
    font-size: 2.15em;
    font-weight: 700;
    line-height: 1.05;
}

.control-panel-date {
    font-size: var(--font-size-md);
    font-weight: 500;
    color: var(--color-foreground-secondary);
}

.control-panel-weather {
    font-size: var(--font-size-sm);
    color: var(--color-foreground-muted);
}
"#
}
