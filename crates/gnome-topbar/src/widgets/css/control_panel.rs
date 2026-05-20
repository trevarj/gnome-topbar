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

.control-panel-time-weather-with-world-clocks {
    margin-bottom: 10px;
}

.control-panel-world-clocks {
    margin-top: 8px;
    padding-top: 2px;
}

.control-panel-world-clock-label {
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-foreground-secondary);
}

.control-panel-world-clock-time {
    font-size: var(--font-size-md);
    font-weight: 600;
    color: var(--color-foreground);
}

.control-panel-module-separator {
    min-height: 1px;
    margin-top: 2px;
    margin-bottom: 2px;
    background-color: var(--color-foreground-muted);
    opacity: 0.36;
}

.control-panel-column-separator {
    min-width: 1px;
    background-color: var(--color-foreground-muted);
    opacity: 0.36;
}

.control-panel-notifications {
    min-height: 0;
}

.control-panel-notifications .notification-scroll {
    min-height: 0;
}
"#
}
