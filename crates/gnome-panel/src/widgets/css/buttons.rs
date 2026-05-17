//! Button CSS classes.

/// Return button CSS.
pub fn css() -> &'static str {
    r#"
/* ===== BUTTONS ===== */

/* Reset button - strips GTK chrome (background, border, shadow) */
button.vp-btn-reset,
button.vp-btn-compact {
    background: transparent;
    border: none;
    box-shadow: none;
    color: inherit;
    font-family: var(--font-family);
    font-size: var(--font-size);
    border-radius: var(--radius-widget);
}

/* Compact button - reset + zero padding/margin for icon-only buttons */
button.vp-btn-compact {
    padding: 0;
    margin: 0;
    min-width: var(--widget-height);
    min-height: var(--widget-height);
}

button.vp-btn-compact:hover {
    background: var(--color-card-overlay-hover);
}

button.vp-btn-accent {
    background: var(--color-accent-primary);
    color: var(--color-accent-text, #fff);
    border: none;
    box-shadow: none;
    border-radius: var(--radius-widget);
    min-height: var(--widget-height);
}

button.vp-btn-accent label {
    margin: 0 8px;
}

button.vp-btn-accent:hover {
    background: var(--color-accent-hover-bg);
}

button.vp-btn-card {
    background: var(--color-card-overlay);
    color: var(--color-foreground-primary);
    border: none;
    box-shadow: none;
    border-radius: var(--radius-widget);
    min-height: var(--widget-height);
}

button.vp-btn-card label {
    margin: 0 8px;
}

button.vp-btn-card:hover {
    background: var(--color-card-overlay-hover);
}

/* Link-style button - text only, no background */
button.vp-btn-link,
.vp-btn-link {
    background: transparent;
    border: none;
    box-shadow: none;
    color: var(--color-accent-primary);
    padding: 0;
    min-height: 0;
}

button.vp-btn-link:hover,
.vp-btn-link:hover {
    background: transparent;
    text-decoration: underline;
}

/* Ghost button - transparent with hover effect */
button.vp-btn-ghost {
    background: transparent;
    border: none;
    box-shadow: none;
    border-radius: var(--radius-widget);
    color: var(--color-foreground-primary);
    min-height: var(--widget-height);
}

button.vp-btn-ghost:hover {
    background: var(--color-card-overlay-hover);
}

/* Ripple buttons - zero padding so the Cairo ripple overlay fills the
   full hover/background area.  Individual button classes should use
   min-width / min-height to maintain their intended hit-target size. */
button.vp-has-ripple {
    padding: 0;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_css_standardizes_button_tokens() {
        let css = css();

        assert!(css.contains("font-family: var(--font-family);"));
        assert!(css.contains("font-size: var(--font-size);"));
        assert!(css.contains("border-radius: var(--radius-widget);"));
        assert!(css.contains("min-height: var(--widget-height);"));
        assert!(css.contains("button.vp-btn-compact:hover"));
    }
}
