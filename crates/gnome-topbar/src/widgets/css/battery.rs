//! Quick Settings battery status CSS.

/// Return battery CSS.
pub fn css() -> &'static str {
    r#"
/* ===== QUICK SETTINGS BATTERY STATUS ===== */

/* Battery state classes - applied to the Quick Settings bar indicator. */
.battery-icon.battery-charging {
    color: var(--color-accent-primary);
}

.battery-icon.battery-plugged,
.battery-icon.battery-full {
    color: var(--color-foreground-secondary);
}

.battery-icon.battery-low {
    color: var(--color-state-urgent);
}
"#
}
