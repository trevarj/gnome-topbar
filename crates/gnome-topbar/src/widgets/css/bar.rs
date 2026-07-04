//! Bar and workspace CSS.
//!
//! Note: This module requires config values for screen_margin and spacing,
//! so it returns a formatted String rather than a static str.

use super::{CONTENT_PADDING_X, WIDGET_BG_WITH_OPACITY};
use crate::widgets::workspaces::LONG_INDICATOR_HPAD;

/// Return bar CSS with config values interpolated.
///
/// Workspace indicator widths — resting and animated — are owned entirely by
/// Rust (see `widgets::workspaces::IndicatorWidth` and
/// `indicator_target_width`). The CSS deliberately declares no `min-width`:
/// GTK measures `max(css min-width, size_request)`, so a CSS minimum would
/// floor the Rust-animated value and snap the grow animation.
pub fn css(screen_margin: u32, spacing: u32) -> String {
    let widget_bg = WIDGET_BG_WITH_OPACITY;
    let long_hpad = LONG_INDICATOR_HPAD;
    let content_pad_x = CONTENT_PADDING_X;
    let content_pad_x_half = CONTENT_PADDING_X / 2;
    let content_pad_x_double = 2 * CONTENT_PADDING_X;
    format!(
        r#"
/* ===== BAR ===== */

/* Window must be transparent so bar background shows */
.bar-window {{
    background: transparent;
}}

/* Shell containers transparent */
.bar-shell,
.bar-shell-inner,
.bar-margin-spacer {{
    background: transparent;
}}

.bar-shell-inner {{
    padding-left: {screen_margin}px;
    padding-right: {screen_margin}px;
}}

/* Bar container - the visible bar */
sectioned-bar.bar {{
    min-height: var(--bar-height);
    padding-top: var(--bar-padding-y);
    padding-bottom: var(--bar-padding-y-bottom);
    background: var(--color-background-bar);
    border: var(--bar-outline-width) solid color-mix(in srgb, var(--bar-outline-color) var(--bar-outline-opacity), transparent);
    border-radius: var(--radius-bar);
    font-family: var(--font-family);
    font-size: var(--font-size);
    font-weight: 700;
    color: var(--color-foreground-primary);
}}

/* Widget wrapper — transparent so only .widget paints a background layer */
.widget-wrapper {{
    min-height: var(--widget-height);
    background: transparent;
}}

/* Widget — visual surface */
.widget {{
    background-color: {widget_bg};
    border: var(--widget-outline-width) solid color-mix(in srgb, var(--widget-outline-color) var(--widget-outline-opacity), transparent);
    border-radius: var(--radius-widget);
}}

/* Padding on .content (not the container) so the ripple overlay
   fills the entire widget background area edge-to-edge.
   Passive widgets (merge groups) have no .widget surface — target
   their .content directly via the third selector. */
.widget:not(.widget-group) .content,
.widget-group > .content > .widget-item .content,
.widget-item.passive > .content {{
    padding: var(--widget-padding-y) {content_pad_x}px;
}}

/* Widget groups — surface is transparent; each child paints its own base
   so translucent hover composites over the bar background, not on a
   stacked group base. Outer pill shape comes from position-aware radius
   on the first/last children below. */
.widget.widget-group {{
    padding: 0;
    background-color: transparent;
    background-image: none;
}}

/* Hover targets the wrapper but paints on the surface child */
.widget-wrapper.clickable:hover > .widget:not(.widget-group) {{
    background-color: var(--color-widget-hover-bg);
}}

/* Each grouped child paints its own background. Both adjacent painted
   siblings (.widget-merge-group and .widget-item) sit at the same DOM
   depth (direct children of .widget-group > .content) so their painted
   regions meet flush in the parent Box. The inner .widget stays
   transparent — painting the outer .widget-item ensures the painted
   surface fills the item's allocation, eliminating subpixel seams that
   appear when paint depth differs between siblings. */
.widget-group > .content > .widget-item {{
    background-color: {widget_bg};
}}
.widget-group > .content > .widget-item > .widget {{
    background-color: transparent;
    box-shadow: none;
    border: none;
}}

/* Halve the visible inter-item gap at every seam inside a group. Each
   item carries `pad_x` on both inner-content sides; without override,
   adjacent items show `2 * pad_x` between their icons (looks doubled vs
   the intra-merge gap, which is collapsed by the negative-margin overlap
   inside .merge-group-content + .merge-group-content's set_spacing).
   Halving both seam-facing sides to `pad_x / 2` yields a total seam gap
   of `pad_x`, matching MERGE_GROUP_SPACING for visual consistency.
   - .widget-item: padding lives on .widget > overlay > .content.
   - .widget-merge-group: the leftmost/rightmost passive item's .content
     carries the seam-facing padding (interior passive items are already
     overlapped by margin-left).
   :not(:first-child) → has a previous sibling → halve left side.
   :not(:last-child)  → has a following sibling → halve right side. */
.widget-group > .content > .widget-item:not(:first-child) > .widget > overlay > .content,
.widget-group > .content > .widget-merge-group:not(:first-child) > .merge-group-content > .widget-item:first-child > .content {{
    padding-left: {content_pad_x_half}px;
}}
.widget-group > .content > .widget-item:not(:last-child) > .widget > overlay > .content,
.widget-group > .content > .widget-merge-group:not(:last-child) > .merge-group-content > .widget-item:last-child > .content {{
    padding-right: {content_pad_x_half}px;
}}

/* Position-aware pill shape: outer corners rounded only on the leading
   and trailing children; interior edges square so adjacent children meet
   flush. Applies to both plain items and merge wrappers. */
.widget-group > .content > .widget-item,
.widget-group > .content > .widget-merge-group {{
    border-radius: 0;
}}
.widget-group > .content > :first-child {{
    border-top-left-radius: var(--radius-widget);
    border-bottom-left-radius: var(--radius-widget);
}}
.widget-group > .content > :last-child {{
    border-top-right-radius: var(--radius-widget);
    border-bottom-right-radius: var(--radius-widget);
}}

/* Inner .widget surface inside grouped items must match its parent
   .widget-item's position-aware radius — the inner .widget has
   Overflow::Hidden which clips the ripple, so without this override
   the ripple would clip to the standalone --radius-widget shape and
   not reach the painted square corners at interior seams. */
.widget-group > .content > .widget-item > .widget {{
    border-radius: 0;
}}
.widget-group > .content > .widget-item:first-child > .widget {{
    border-top-left-radius: var(--radius-widget);
    border-bottom-left-radius: var(--radius-widget);
}}
.widget-group > .content > .widget-item:last-child > .widget {{
    border-top-right-radius: var(--radius-widget);
    border-bottom-right-radius: var(--radius-widget);
}}

/* Grouped child hover — clears the cell base and paints a rounded hover
   pill on the inner surface. The spread shadow restores the cell's base
   color behind the rounded corners, clipped by the child allocation, so
   translucent hover values composite over the bar background exactly once
   without exposing wallpaper at mixed-group seams. */
.widget-group > .content > .widget-item.clickable:hover {{
    background-color: transparent;
}}
.widget-group > .content > .widget-item.clickable:hover > .widget {{
    background-color: var(--color-widget-hover-bg);
    border-radius: var(--radius-widget);
    box-shadow: 0 0 0 9999px {widget_bg};
}}

/* ===== MERGE GROUP ===== */

/* Merge group wrapper — acts as a single visual button for adjacent
   same-popover widgets. Paints its own base; the parent group surface
   is transparent. overflow:hidden is set in Rust (not a valid GTK4 CSS
   property). */
.widget-merge-group {{
    background-color: {widget_bg};
    border-radius: var(--radius-widget);
}}
.widget-merge-group > .merge-group-content {{
    box-shadow: none;
}}

/* Ripple clip box (added in build_merge_group). Establishes a rounded clip
   for the merge-group ripple so it matches the inner pill shape — the
   merge-group itself uses position-aware radius and would otherwise leak
   the ripple past rounded hover edges at mixed-group seams. */
.widget-merge-group-ripple-clip {{
    border-radius: var(--radius-widget);
    background: transparent;
}}

.widget-group > .content > .widget-merge-group.clickable:hover {{
    background-color: transparent;
}}
.widget-group > .content > .widget-merge-group.clickable:hover > .merge-group-content {{
    background-color: var(--color-widget-hover-bg);
    border-radius: var(--radius-widget);
    box-shadow: 0 0 0 9999px {widget_bg};
}}

/* Passive items in merge groups don't show their own hover */
.merge-group-content > .widget-item.passive:hover {{
    background-color: transparent;
}}

/* Pull non-first items left to overlap adjacent .content padding (2 × {content_pad_x}px) */
.merge-group-content > .widget-item:not(:first-child) {{
    margin-left: -{content_pad_x_double}px;
}}

/* Spacing between items inside widgets (icon→label, etc.).
   Restricted to inner .content elements: standalone widgets' .content sits
   inside .widget (which is NOT .widget-group), and grouped items' inner
   .content sits inside .widget-item.passive (merge groups) or one extra
   level deep under .widget-group's outer .content. The outer .content
   directly under .widget.widget-group must NOT match — its direct children
   are the painted siblings (.widget-merge-group, .widget-item) that need
   to meet flush with zero margin between them. */
.widget:not(.widget-group) > overlay > .content > *:not(:last-child),
.widget:not(.widget-group) > .content > *:not(:last-child),
.widget-group .content .content > *:not(:last-child) {{
    margin-right: var(--spacing-widget-gap);
}}

/* Quick Settings is one aggregate status button; keep its internal
   indicators tighter than general icon/label widget content. */
.quick-settings > overlay > .content > *:not(:last-child) {{
    margin-right: calc(var(--spacing-widget-gap) * 0.55);
}}

.quick-settings > overlay > .content {{
    padding-left: calc(var(--spacing-widget-edge) * 0.85);
    padding-right: calc(var(--spacing-widget-edge) * 0.85);
}}

.quick-settings .qs-privacy-indicator {{
    color: var(--color-state-urgent);
}}

/* The screen-sharing opacity pulse is driven from Rust (animation.rs) so it
   stays in sync with the frame clock and honors theme.animations. */

/* Section widget spacing via margins. */
.bar-section--left > *:not(:last-child),
.bar-section--right > *:not(:last-child) {{
    margin-right: {spacing}px;
}}

/* ===== WORKSPACE ===== */

.workspace-indicator {{
    padding: 0;
    min-height: 6px;
    border-radius: var(--radius-round);
    color: #71717a;
    background-color: #71717a;
    /* Width is owned entirely by Rust (set_size_request, both resting and
       per-frame animated); CSS must not declare min-width or GTK's
       max(css-min, size_request) floors the animated value. */
}}

/* Override ripple overlay fallback radius (overlay.vp-ripple-wrap uses --radius-widget) */
overlay.workspace-indicator {{
    border-radius: var(--radius-round);
}}

/* Workspace hover backgrounds use scoped tokens so active hover can differ from global accent hover. */
.workspace-indicator.clickable:hover {{
    background-color: #d4d4d8;
}}

.workspace-indicator.active.clickable:hover {{
    background-color: transparent;
}}

.workspace-indicator-content {{
    padding: 0;
}}

.workspace-indicator.active .workspace-indicator-content {{
    color: var(--widget-background-color);
}}

.workspace-indicator.active .workspace-indicator-progress {{
    color: var(--color-foreground-primary);
}}

.workspace-indicator.urgent.clickable:hover {{
    background-color: var(--color-workspace-indicator-urgent-hover-bg);
}}

.workspace-indicator-minimal {{
    background-color: #71717a;
    color: #71717a;
}}

.workspace-indicator.occupied {{
    background-color: #a1a1aa;
    color: #a1a1aa;
}}

.workspace-indicator.active {{
    /* Active pill is thicker than inactive dots to match GNOME. */
    min-height: 8px;
    color: var(--color-foreground-primary);
    background-color: transparent;
}}

.workspace-indicator.urgent {{
    color: var(--color-state-urgent);
    background-color: var(--color-state-urgent);
}}

.workspace-indicator-long {{
    padding: 0 {long_hpad}px;
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_css_has_no_min_width_transition() {
        // Width is Rust-owned now; the CSS must not drive a min-width transition.
        let css = css(0, 8);

        assert!(!css.contains("transition: min-width"));
        assert!(!css.contains("background-color 125ms"));
    }

    #[test]
    fn workspace_css_declares_no_min_width() {
        // Width is owned entirely by Rust via set_size_request; a CSS
        // min-width on the indicator would floor the animated value
        // (GTK measures max(css-min, size_request)) and snap the grow.
        let css = css(0, 8);

        assert!(!css.contains("min-width: "));
    }

    #[test]
    fn workspace_css_keeps_labels_contrasted_and_urgent_themed() {
        let css = css(0, 8);

        assert!(css.contains(".workspace-indicator.active .workspace-indicator-content {\n    color: var(--widget-background-color);\n}"));
        assert!(css.contains(".workspace-indicator.urgent {\n    color: var(--color-state-urgent);\n    background-color: var(--color-state-urgent);\n}"));
    }
}
