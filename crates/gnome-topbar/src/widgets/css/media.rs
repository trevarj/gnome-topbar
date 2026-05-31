//! Media control-panel CSS.

/// Return media CSS.
pub fn css(animations: bool) -> String {
    let slider_transition = if animations {
        "transition: transform 100ms ease-out;"
    } else {
        "transition: none;"
    };
    format!(
        r#"
/* ===== MEDIA CONTROL PANEL ===== */

/* Popover styling */
.media-popover.vp-surface-popover {{
    min-width: 340px;
}}

/* Popover header buttons row */
.media-popover-header {{
    margin-top: -12px;
    margin-right: -12px;
    margin-bottom: 2px;
}}

/* Override base popover icon button size for denser media layout */
.media-player-selector-btn {{
    min-width: 28px;
    min-height: 28px;
    margin-top: 0;
}}

/* Player selector menu - extends qs-row-menu-content */
.media-player-menu {{
    font-family: var(--font-family);
    font-size: var(--font-size);
}}

.media-player-menu * {{
    font-family: inherit;
    font-size: inherit;
}}

/* Player menu item - extends qs-row-menu-item */
.media-player-menu-item {{
    border: none;
    box-shadow: none;
}}

.media-player-menu-title {{
}}

.media-player-menu-subtitle {{
    font-size: var(--font-size-sm);
}}

/* Check icon in player menu - slightly larger for visibility */
.media-player-menu-check {{
    font-size: 1.15em;
}}

/* Album art in control-panel media view */
.media-art {{
    border-radius: var(--radius-widget);
    background: var(--color-card-overlay);
}}

.media-art-placeholder {{
    background: var(--color-card-overlay);
}}

.media-empty-icon {{
    font-size: 3em;
    color: var(--color-foreground-disabled);
}}

.media-track-title {{
    font-size: var(--font-size-lg);
    font-weight: 500;
}}

.media-artist,
.media-album {{
    font-size: var(--font-size-sm);
}}

/* Playback controls in popover */
.media-popover .media-controls {{
    padding: 0;
}}

.media-popover .media-control-btn {{
    background: transparent;
    border: none;
    box-shadow: none;
    min-width: 32px;
    min-height: 32px;
    padding: 0;
    border-radius: var(--radius-widget);
    color: var(--color-foreground-primary);
}}

.media-popover .media-control-btn .icon-root {{
    font-size: calc(var(--icon-size) * 1.25);
}}

.media-popover .media-control-btn:hover {{
    background: var(--color-card-overlay-hover);
}}

.media-popover .media-control-btn:disabled {{
    color: var(--color-foreground-disabled);
}}

/* Primary button (play/pause) - slightly larger with accent background */
.media-popover .media-control-btn.media-control-btn-primary {{
    min-width: 40px;
    min-height: 40px;
    background: var(--color-accent-primary);
    color: var(--color-accent-text, #fff);
}}

.media-popover .media-control-btn.media-control-btn-primary .icon-root {{
    font-size: calc(var(--icon-size) * 1.35);
}}

.media-popover .media-control-btn.media-control-btn-primary:hover {{
    background: var(--color-accent-hover-bg);
}}

/* Seek bar */
.media-seek {{
    margin-top: 4px;
}}

.media-seek-slider {{
    margin-left: -8px;
    margin-right: -8px;
}}

.media-seek-slider trough {{
    min-height: var(--slider-height);
    border-radius: var(--slider-radius);
    background-color: var(--color-slider-track);
}}

.media-seek-slider highlight {{
    background-image: image(var(--color-accent-slider, var(--color-accent-primary)));
    background-color: var(--color-accent-slider, var(--color-accent-primary));
    border: none;
    min-height: var(--slider-height);
    border-radius: var(--slider-radius);
}}

.media-seek-slider slider {{
    min-width: var(--slider-knob-size);
    min-height: var(--slider-knob-size);
    margin: -5px;
    padding: 0;
    background-color: var(--color-accent-primary);
    border-radius: var(--slider-knob-radius);
    border: none;
    box-shadow: none;
    {slider_transition}
}}

.media-seek-slider slider:active {{
    transform: scale(1.15);
}}

.media-time {{
    font-size: var(--font-size-xs);
    margin-top: -4px;
}}
"#
    )
}
