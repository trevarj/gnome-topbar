//! Bar window implementation using GTK4 and layer-shell.

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{debug, info, warn};

use gnome_topbar_core::config::{WidgetEntry, WidgetOrGroup};
use gnome_topbar_core::{Config, ThemePalette};

use crate::sectioned_bar::SectionedBar;
use crate::services::config_manager::{ConfigManager, ThemeCallbackGuard};
use crate::styles::class;
use crate::widgets::{self, BarState, QuickSettingsConfig, WidgetConfig, WidgetFactory};

/// Create and configure the bar window with layer-shell.
///
/// The `state` parameter is used to store widget handles, keeping them alive
/// for the lifetime of the bar. The `output_id` is the monitor connector name
/// used for per-monitor widget filtering.
pub fn create_bar_window(
    app: &Application,
    config: &Config,
    monitor: &gtk4::gdk::Monitor,
    output_id: &str,
    state: &mut BarState,
) -> ApplicationWindow {
    // Window height determines the exclusive zone (via auto_exclusive_zone_enable).
    // - When bar is visible (opacity > 0): include padding on both sides.
    // - When bar is transparent (opacity = 0): include only screen-edge padding;
    //   CSS suppresses the center-side padding in islands mode.
    let bar_height = if config.bar.background_opacity > 0.0 {
        config.bar.size as i32 + 2 * config.bar.padding as i32
    } else {
        config.bar.size as i32 + config.bar.padding as i32
    };

    let window = ApplicationWindow::builder()
        .application(app)
        .title("gnome-topbar")
        .decorated(false)
        .resizable(false)
        .default_height(bar_height)
        .build();

    window.add_css_class(class::BAR_WINDOW);

    // Initialize layer-shell
    window.init_layer_shell();
    window.set_namespace(Some("gnome-topbar"));
    window.set_layer(Layer::Top);

    // Bind to specific monitor - this should handle width automatically
    window.set_monitor(Some(monitor));
    debug!("Bar bound to monitor: {:?}", monitor.connector());

    // Anchor to the configured edge, stretch horizontally
    let is_bottom = config.bar.is_bottom();
    window.set_anchor(Edge::Top, !is_bottom);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, is_bottom);

    // Reserve space (exclusive zone) so other windows don't overlap
    window.auto_exclusive_zone_enable();

    // Bar doesn't need keyboard input
    window.set_keyboard_mode(KeyboardMode::None);

    // Set margins from config (legacy behavior)
    // We keep window margins at 0 for left/right so the bar window
    // fills the monitor width; screen_margin is applied inside the
    // bar content instead.
    let margin = config.bar.screen_margin as i32;

    // Create the bar container using SectionedBar for proper left/center/right layout
    let bar_box = SectionedBar::new(
        config.bar.spacing as i32,
        config.bar.inset as i32,
        config.widgets.left_has_expander(),
        config.widgets.right_has_expander(),
    );
    bar_box.add_css_class(class::BAR);
    bar_box.set_hexpand(true);
    bar_box.set_vexpand(true);

    // Wrap bar_box in an outer container so we can inset the
    // visible bar from the anchored edge and sides while
    // keeping the window and exclusive zone full-width.
    let outer_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer_box.add_css_class(class::BAR_SHELL);
    outer_box.set_hexpand(true);
    outer_box.set_vexpand(true);

    // Spacer: empty area between bar content and screen edge.
    // For top bar, spacer goes above (pushes bar down from top edge).
    // For bottom bar, spacer goes below (pushes bar up from bottom edge).
    let spacer = if margin > 0 {
        let s = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        s.set_size_request(-1, margin);
        s.add_css_class(class::BAR_MARGIN_SPACER);
        Some(s)
    } else {
        None
    };

    if !is_bottom && let Some(ref spacer) = spacer {
        outer_box.append(spacer);
    }

    // Inner horizontal box adds left/right padding via CSS.
    let inner_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    inner_box.add_css_class(class::BAR_SHELL_INNER);
    inner_box.set_hexpand(true);
    inner_box.set_vexpand(false);
    inner_box.append(&bar_box);

    outer_box.append(&inner_box);

    if is_bottom && let Some(ref spacer) = spacer {
        outer_box.append(spacer);
    }

    crate::services::updates::UpdatesService::global().configure_from_config(config);

    // Find quick_settings config from widget entries to configure the window.
    // Get options from [widgets.quick_settings] if defined.
    let qs_config = config
        .widgets
        .get_options("quick_settings")
        .map(|opts| {
            let entry = WidgetEntry::with_options("quick_settings", opts);
            QuickSettingsConfig::from_entry(&entry)
        })
        .unwrap_or_default();

    // Create handle for this bar's Quick Settings window.
    // The window is created lazily on first open and kept alive for instant re-show.
    let qs_handle = crate::widgets::QuickSettingsWindowHandle::new(app.clone(), qs_config.clone());

    // Register QS handle with the popover registry for IPC control.
    crate::popover_registry::register(
        "quick_settings",
        Rc::new(qs_handle.clone()) as Rc<dyn crate::popover_registry::PopoverToggleable>,
    );

    // Create left section
    let left_section = create_section("left", config, state, &qs_handle, Some(output_id));
    bar_box.set_start_widget(Some(&left_section));

    // Create center section only if there are center widgets
    // Without a center widget, the layout manager uses linear allocation
    let has_center_content = !config.widgets.resolved_center().is_empty();
    if has_center_content {
        let center_section = create_center_section(config, state, &qs_handle, Some(output_id));
        bar_box.set_center_widget(Some(&center_section));
    }

    // Create right section
    let right_section = create_section("right", config, state, &qs_handle, Some(output_id));
    bar_box.set_end_widget(Some(&right_section));

    window.set_child(Some(&outer_box));

    // Set window width to the target monitor's width on map.
    // We capture the geometry now rather than using monitor_at_surface() later,
    // because the surface might not be on the correct monitor yet at map time.
    let target_geometry = monitor.geometry();
    let target_width = target_geometry.width();

    let is_island_mode = config.bar.background_opacity == 0.0;

    let bar_box_for_blur = bar_box.clone();
    window.connect_map(move |win| {
        win.set_default_size(target_width, bar_height);
        debug!(
            "Set window width to target monitor size: {}px",
            target_width
        );

        // Apply bar blur region on map (opaque/translucent bar path).
        // The islands path is handled by the layout allocate callback below.
        //
        // Island mode: allocation applies active blur regions. If blur was
        // disabled while unmapped, clean up the stale protocol object now that
        // the wl_surface is resolvable again.
        //
        // Opaque/translucent mode: apply blur on map.  The else-branch
        // removes any stale protocol object left from a previous map cycle
        // (blur enabled on last show, then disabled while bars were hidden).
        // `remove_blur_region` is idempotent (no-op when no effect exists).
        if is_island_mode {
            if !ConfigManager::global().blur_enabled()
                && let Some(blur) =
                    crate::services::background_effect::BackgroundEffectManager::global()
            {
                blur.remove_blur_region(win);
            }
        } else if ConfigManager::global().blur_enabled() {
            if let Some(blur) =
                crate::services::background_effect::BackgroundEffectManager::global()
            {
                blur.apply_bar_blur_region(win, &bar_box_for_blur);
            }
        } else if let Some(blur) =
            crate::services::background_effect::BackgroundEffectManager::global()
        {
            blur.remove_blur_region(win);
        }
    });

    // Install layout callback for island blur (transparent bar mode).
    // When bar.background_opacity == 0.0, we blur per-widget-island instead of
    // the whole surface. The callback fires after every layout pass so the blur
    // region stays in sync as widgets move or resize (tray changes, title width, etc).
    //
    // We also keep a shared clone of the island-apply closure so the theme-change
    // hot-reload handler can trigger an immediate re-apply when blur is toggled on.
    //
    // `prev_bounds` caches the last-applied island bounds to skip redundant
    // Wayland protocol traffic.  It is hoisted here (rather than inside the
    // closure) so the theme-change handler can clear it when blur is toggled off
    // — otherwise the stale cache would short-circuit the next apply.
    let prev_bounds = Rc::new(RefCell::new(Vec::<(i32, i32, i32, i32)>::new()));
    // Clone for the theme-change handler so it can invalidate the cache on any
    // theme change (the original `prev_bounds` is moved into the island closure).
    let prev_bounds_for_theme = Rc::clone(&prev_bounds);

    let island_apply: Option<Rc<dyn Fn()>> = if is_island_mode {
        let win_weak = window.downgrade();
        let bar_box_weak = bar_box.downgrade();
        let closure: Rc<dyn Fn()> = Rc::new(move || {
            if !ConfigManager::global().blur_enabled() {
                // Clean up any stale blur effect left from before blur was
                // disabled (e.g. ipc_hide -> blur-off -> ipc_show).
                // Only do this once: if prev_bounds is already empty we've
                // either already cleaned up or never had blur applied.
                if !prev_bounds.borrow().is_empty() {
                    prev_bounds.borrow_mut().clear();
                    // Defer the remove out of the GTK allocate pass: it calls
                    // wl_surface.commit() synchronously, and we'd rather not
                    // do that mid-layout.  Re-check guard inside idle in case
                    // a subsequent allocate flipped state back.
                    let win_weak_idle = win_weak.clone();
                    let prev_bounds_idle = Rc::clone(&prev_bounds);
                    gtk4::glib::idle_add_local_once(move || {
                        if !prev_bounds_idle.borrow().is_empty() {
                            return;
                        }
                        if ConfigManager::global().blur_enabled() {
                            return;
                        }
                        if let Some(win) = win_weak_idle.upgrade()
                            && let Some(blur) =
                                crate::services::background_effect::BackgroundEffectManager::global(
                                )
                        {
                            blur.remove_blur_region(&win);
                        }
                    });
                }
                return;
            }
            let Some(win) = win_weak.upgrade() else {
                return;
            };
            // Bar is mapped but opacity-hidden (e.g. hide_all during monitor
            // hotplug debounce).  Skip blur — it would be applied to an
            // invisible surface.  reconfigure_all() rebuilds bars and
            // connect_map re-applies blur when they are shown again.
            if win.opacity() <= 0.0 {
                return;
            }
            let Some(blur) = crate::services::background_effect::BackgroundEffectManager::global()
            else {
                return;
            };
            let Some(native) = win.native() else { return };
            let Some(bar_box) = bar_box_weak.upgrade() else {
                return;
            };
            let islands = collect_island_bounds(&bar_box, &native);
            // Skip redundant Wayland protocol traffic when bounds haven't changed.
            // The allocate callback fires on every layout pass (clock tick, tray
            // icon change, etc.) but most passes produce identical island bounds.
            if *prev_bounds.borrow() == islands {
                return;
            }
            *prev_bounds.borrow_mut() = islands.clone();
            if !islands.is_empty() {
                blur.apply_bar_island_blur_regions(&win, &islands);
            } else {
                // Defer the remove out of the GTK allocate pass: it calls
                // wl_surface.commit() synchronously, and we'd rather not
                // do that mid-layout.  Re-check inside idle so a fast
                // allocate-then-allocate sequence can't clear blur that
                // was just legitimately reapplied.
                let win_weak_idle = win_weak.clone();
                let prev_bounds_idle = Rc::clone(&prev_bounds);
                gtk4::glib::idle_add_local_once(move || {
                    if !prev_bounds_idle.borrow().is_empty() {
                        return;
                    }
                    if let Some(win) = win_weak_idle.upgrade()
                        && let Some(blur) =
                            crate::services::background_effect::BackgroundEffectManager::global()
                    {
                        blur.remove_blur_region(&win);
                    }
                });
            }
        });
        if let Some(lm) = bar_box
            .layout_manager()
            .and_downcast::<crate::sectioned_bar::CenterPriorityLayout>()
        {
            let closure_clone = Rc::clone(&closure);
            lm.set_on_allocate(move || closure_clone());
        }
        Some(closure)
    } else {
        None
    };

    // Hot-reload: re-apply or remove bar blur when the theme config changes
    // (e.g. user toggles `theme.blur` or changes `bar.border_radius`).
    //
    // Note: `background_opacity` changes trigger a structural rebuild
    // (config_structure_changed), so this callback only needs to handle
    // toggling blur on/off within the current mode (opaque or island).
    {
        let win_weak = window.downgrade();
        let bar_box_for_theme = bar_box.clone();
        let theme_cb_id = ConfigManager::global().on_theme_change(move || {
            let Some(win) = win_weak.upgrade() else {
                return;
            };
            if ConfigManager::global().blur_enabled() {
                // Invalidate the island-bounds cache so radius/theme changes
                // force a re-apply (the cache only tracks geometry, not radii).
                prev_bounds_for_theme.borrow_mut().clear();
                if let Some(apply) = &island_apply {
                    // Island mode: re-apply per-island regions immediately.
                    apply();
                } else if let Some(blur) =
                    crate::services::background_effect::BackgroundEffectManager::global()
                {
                    // Opaque/translucent mode: re-apply whole-bar region.
                    blur.apply_bar_blur_region(&win, &bar_box_for_theme);
                }
            } else if let Some(blur) =
                crate::services::background_effect::BackgroundEffectManager::global()
            {
                blur.remove_blur_region(&win);
            }
        });
        state.add_handle(Box::new(ThemeCallbackGuard(theme_cb_id)));
    }

    window.set_visible(true);

    info!(
        "Bar window created: size={}px, margin={}px, monitor={:?}, widgets={}",
        config.bar.size,
        config.bar.screen_margin,
        monitor.connector(),
        state.handle_count()
    );

    window
}

/// Collect the surface-local bounds of every visible widget island in the bar.
///
/// Walks the children of each section in the `SectionedBar`, finds all
/// `.widget-wrapper` boxes that are visible, and returns their
/// `(x, y, width, height)` in surface-local logical coordinates via
/// `Widget::compute_bounds()`.
fn collect_island_bounds(
    bar_box: &SectionedBar,
    native: &gtk4::Native,
) -> Vec<(i32, i32, i32, i32)> {
    use crate::styles::class;
    let mut result = Vec::new();

    for section_name in &["left", "center", "right"] {
        let Some(section) = bar_box.section(section_name) else {
            continue;
        };
        if !section.is_visible() {
            continue;
        }
        let mut child = section.first_child();
        while let Some(widget) = child {
            if widget.is_visible()
                && widget.has_css_class(class::WIDGET_WRAPPER)
                && let Some(bounds) = widget.compute_bounds(native.upcast_ref::<gtk4::Widget>())
            {
                let x = bounds.x().round() as i32;
                let y = bounds.y().round() as i32;
                let w = bounds.width().round() as i32;
                let h = bounds.height().round() as i32;
                if w > 0 && h > 0 {
                    result.push((x, y, w, h));
                }
            }
            child = widget.next_sibling();
        }
    }

    result
}

/// Build a single widget or a group of widgets sharing one island.
///
/// Returns the number of widgets built (for counting purposes).
fn build_widget_or_group(
    item: &WidgetOrGroup,
    container: &gtk4::Box,
    state: &mut BarState,
    qs_handle: &crate::widgets::QuickSettingsWindowHandle,
    output_id: Option<&str>,
) -> usize {
    match item {
        WidgetOrGroup::Single(entry) => {
            // Single widget with its own island
            if let Some(built) = WidgetFactory::build(entry, Some(qs_handle), output_id) {
                container.append(&built.widget);
                state.add_handle(built.handle);
                1
            } else {
                0
            }
        }
        WidgetOrGroup::Group { group } => {
            if group.is_empty() {
                return 0;
            }

            // Create a shared island container for the group
            let island = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            island.add_css_class(class::WIDGET_WRAPPER);

            // Create inner content box (matching BaseWidget structure)
            let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            content.add_css_class(class::CONTENT);
            content.set_vexpand(true);
            content.set_valign(gtk4::Align::Fill);

            // Group surface — transparent in CSS. Direct children paint their
            // own backgrounds so hover colors composite once over the bar.
            let surface = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            surface.add_css_class(class::WIDGET);
            surface.add_css_class(class::WIDGET_GROUP);
            // First widget's per-widget style (outline_color, background_color)
            // applies to the group surface so the shared border uses the right color.
            if let Some(first) = group.first() {
                surface.add_css_class(&first.name.replace('_', "-"));
            }
            surface.set_overflow(gtk4::Overflow::Hidden);
            surface.set_hexpand(true);
            surface.set_vexpand(true);

            surface.append(&content);
            island.append(&surface);

            // Each child paints its own background; the group surface is transparent.
            let mut count = 0;
            for entry in group {
                if let Some(built) = WidgetFactory::build(entry, Some(qs_handle), output_id) {
                    // Strip the standalone wrapper class so the wrapper-hover
                    // rule doesn't fire — per-item hover is handled by a
                    // group-scoped rule that paints on the .widget-item.
                    built.widget.remove_css_class(class::WIDGET_WRAPPER);
                    built.widget.add_css_class(&entry.name.replace('_', "-"));
                    // Grouped hover uses a large box-shadow spread to refill
                    // the cell around the pill; this clips it to item bounds.
                    built.widget.set_overflow(gtk4::Overflow::Hidden);
                    content.append(&built.widget);
                    state.add_handle(built.handle);
                    count += 1;
                }
            }

            // Only append the island if we built at least one widget
            if count > 0 {
                container.append(&island);
                debug!("Created widget group with {} widget(s)", count);
            }

            count
        }
    }
}

fn create_section(
    position: &str,
    config: &Config,
    state: &mut BarState,
    qs_handle: &crate::widgets::QuickSettingsWindowHandle,
    output_id: Option<&str>,
) -> gtk4::Box {
    let section = gtk4::Box::new(
        gtk4::Orientation::Horizontal,
        0, // Spacing handled via CSS margins for consistent clipping.
    );
    // Clip overflowing content to prevent widgets from rendering beyond section bounds
    section.set_overflow(gtk4::Overflow::Hidden);
    let section_class = match position {
        "left" => class::BAR_SECTION_LEFT,
        "right" => class::BAR_SECTION_RIGHT,
        _ => class::BAR_SECTION_CENTER,
    };
    section.add_css_class(section_class);

    // Get the resolved widget entries for this position (with options applied, disabled filtered)
    let resolved = match position {
        "left" => config.widgets.resolved_left(),
        "right" => config.widgets.resolved_right(),
        _ => return section,
    };

    // Build widgets from resolved entries
    let mut widget_count = 0;
    for item in &resolved {
        widget_count += build_widget_or_group(item, &section, state, qs_handle, output_id);
    }

    debug!(
        "Created {} section with {} widget(s)",
        position, widget_count
    );
    section
}

/// Create the center section with widgets.
fn create_center_section(
    config: &Config,
    state: &mut BarState,
    qs_handle: &crate::widgets::QuickSettingsWindowHandle,
    output_id: Option<&str>,
) -> gtk4::Box {
    let section = gtk4::Box::new(gtk4::Orientation::Horizontal, config.bar.spacing as i32);
    section.add_css_class(class::BAR_SECTION_CENTER);

    let mut widget_count = 0;
    for item in &config.widgets.resolved_center() {
        widget_count += build_widget_or_group(item, &section, state, qs_handle, output_id);
    }

    debug!("Created center section with {} widget(s)", widget_count);
    section
}

/// Load and apply CSS styling to the application.
pub fn load_css(config: &Config) {
    let provider = gtk4::CssProvider::new();

    // Use cached palettes from ConfigManager
    let palette = ConfigManager::global().palette();
    let popover_palette = ConfigManager::global().popover_palette();
    let css = generate_css(config, &palette, popover_palette.as_ref());

    // Debug: print theme configuration
    debug!("Generated theme CSS:");
    debug!(
        "  mode = {} (is_gtk_mode={})",
        config.theme.mode, palette.is_gtk_mode
    );
    debug!("  accent_source = {:?}", palette.accent_source);
    debug!("  accent_primary = {}", palette.accent_primary);
    debug!("  state_warning = {}", palette.state_warning);
    debug!("  state_urgent = {}", palette.state_urgent);
    debug!("  state_success = {}", palette.state_success);

    provider.load_from_string(&css);

    // Apply to default display with USER priority to override GTK themes
    if let Some(display) = gtk4::gdk::Display::default() {
        // Remove the old theme CSS provider first to ensure clean reload
        // (without this, removed config values would leave stale CSS rules)
        THEME_CSS_PROVIDER.with(|cell| {
            if let Some(old_provider) = cell.borrow_mut().take() {
                gtk4::style_context_remove_provider_for_display(&display, &old_provider);
            }
        });

        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );

        // Store the new provider so we can remove it on next reload
        THEME_CSS_PROVIDER.with(|cell| {
            *cell.borrow_mut() = Some(provider);
        });

        debug!(
            "CSS loaded and applied (dark_mode={})",
            palette.is_dark_mode
        );
    } else {
        warn!("No default display available, CSS styling not applied");
    }
}

// Thread-local storage for the theme CSS provider so we can replace it on reload
thread_local! {
    static THEME_CSS_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

/// Generate CSS string from configuration and theme palette.
fn generate_css(
    config: &Config,
    palette: &ThemePalette,
    popover_palette: Option<&ThemePalette>,
) -> String {
    // Get CSS variables from theme palette
    let css_vars = palette.css_vars_block();

    // Per-widget CSS overrides (background_color, etc. from [widgets.xxx] sections)
    let per_widget_css = ThemePalette::generate_per_widget_css(config);

    // Popover polarity overrides (scoped under .vp-surface-popover)
    let popover_css = popover_palette
        .map(|p| p.css_popover_vars_block())
        .unwrap_or_default();

    // Utility CSS shared across widgets and surfaces
    let utility_css = widgets::css::utility_css(config);

    // Widget-specific CSS
    let widget_css = widgets::css::widget_css(config);

    format!(
        "{}\n{}\n{}\n{}\n{}",
        css_vars, per_widget_css, popover_css, utility_css, widget_css
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generate_css_uses_default_theme_tokens() {
        let config = Config::from_default_toml().expect("default config parses");
        let palette = ThemePalette::from_config(&config, None, None);
        let popover_palette = ThemePalette::popover_palette(&config, None, None);
        let css = generate_css(&config, &palette, popover_palette.as_ref());

        assert!(css.contains(":root"));
        assert!(css.contains(".bar"));
        assert!(css.contains("window.quick-settings-window"));
        assert!(css.contains(".media-popover"));
    }
}
