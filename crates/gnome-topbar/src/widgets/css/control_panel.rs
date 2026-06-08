//! Combined clock control panel CSS.

/// Return control panel CSS.
pub fn css() -> &'static str {
    r#"
/* ===== CONTROL PANEL ===== */

.control-panel .notification-header .vp-popover-title,
.control-panel .calendar-header .vp-popover-title {
    font-size: calc(var(--font-size-lg) * 1.15);
    font-weight: 700;
}

.control-panel-time {
    font-size: 2.15em;
    font-weight: 700;
    line-height: 1.05;
}

.control-panel-date {
    font-size: calc(var(--font-size-lg) * 1.15);
    font-weight: 700;
    color: var(--color-foreground-secondary);
}

.control-panel-weather {
    font-size: var(--font-size-base);
    color: var(--color-foreground-muted);
}

.control-panel-forecast {
    margin-top: 2px;
}

.control-panel-forecast-title {
    font-size: calc(var(--font-size-lg) * 1.15);
    font-weight: 700;
}

.control-panel-forecast-location {
    font-size: var(--font-size-base);
    font-weight: 500;
    color: var(--color-foreground-muted);
}

.control-panel-weather-config-button {
    min-width: 28px;
    min-height: 28px;
    padding: 4px;
    border-radius: var(--radius-sm);
    color: var(--color-foreground-muted);
}

.control-panel-weather-config-button:hover {
    color: var(--color-foreground);
    background-color: var(--color-card-overlay-hover);
}

.control-panel-forecast-current {
    font-size: var(--font-size-base);
    font-weight: 600;
    color: var(--color-foreground);
}

.control-panel-forecast-summary {
    font-size: var(--font-size-base);
    color: var(--color-foreground-muted);
}

.control-panel-forecast-days {
    margin-top: 2px;
}

.control-panel-forecast-row {
    min-height: 24px;
}

.control-panel-forecast-day,
.control-panel-forecast-condition,
.control-panel-forecast-temp,
.control-panel-forecast-precipitation {
    font-size: var(--font-size-base);
}

.control-panel-forecast-day,
.control-panel-forecast-temp {
    font-weight: 600;
    color: var(--color-foreground-secondary);
}

.control-panel-forecast-icon {
    font-size: var(--font-size-md);
    color: var(--color-foreground);
}

.control-panel-forecast-condition,
.control-panel-forecast-precipitation {
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
    font-size: var(--font-size-base);
    font-weight: 500;
    color: var(--color-foreground-secondary);
}

.control-panel-world-clock-time {
    font-size: var(--font-size-base);
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
