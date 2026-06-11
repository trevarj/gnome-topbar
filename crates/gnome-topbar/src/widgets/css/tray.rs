//! System tray CSS.

/// Return system tray CSS.
pub fn css(animations: bool) -> String {
    // The tray item's press cue (`transform: scale()` on :hover/:active)
    // formerly eased over 100ms via a CSS transition. All animation in this
    // crate is now frame-clock driven from Rust (see animation.rs), but the
    // scale applies to internal CSS sub-nodes of the tray item that have no
    // public widget handle (same situation as the removed slider cue — see
    // css/base.rs). Driving it per-frame from Rust would mean intercepting a
    // sub-node's transform, which is disproportionate for a 100ms press cue.
    // The transition is removed; the scale still applies, just instantly.
    // `animations` is no longer read here.
    let _ = animations;
    let tray_transition = "transition: none;";
    format!(
        r#"
/* ===== SYSTEM TRAY ===== */

/* Tray item hover - subtle scale up, press-back on active */
.tray-item {{
    {tray_transition}
}}
.tray-item:hover,
.tray-item.tray-item-menu-open {{
    transform: scale(1.15);
}}
.tray-item:active {{
    transform: scale(1.05);
}}

/* Ensure tray item images have no visual artifacts during updates */
.tray-item image,
.tray-item .icon-root,
.tray-item .icon-root image {{
    border: none;
    box-shadow: none;
    outline: none;
    background: transparent;
}}

.tray-menu {{
    padding: 6px;
    font-family: var(--font-family);
    font-size: var(--font-size);
}}

/* Row menu items - extends tray-menu-button pattern */
.qs-row-menu-item,
.tray-menu-button {{
    background: transparent;
    border: none;
    box-shadow: none;
    border-radius: var(--radius-widget);
}}

/* Padding moves to content margin inside the ripple overlay so the
   DrawingArea (and ripple animation) fills the button edge-to-edge.
   The `> overlay > *` path reaches the label/box through add_ripple()'s
   Overlay wrapper.  Exclude the ripple DrawingArea itself so it stays
   edge-to-edge inside the Overlay. */
.qs-row-menu-item > overlay > *:not(.vp-ripple-overlay),
.tray-menu-button > overlay > *:not(.vp-ripple-overlay) {{
    margin: 4px 8px;
}}

.qs-row-menu-item:hover,
.tray-menu-button:hover {{
    background: var(--color-card-overlay-hover);
}}

.tray-menu-button:disabled {{
    color: var(--color-foreground-disabled);
}}

.tray-menu-button:disabled:hover {{
    background: transparent;
}}
"#
    )
}
