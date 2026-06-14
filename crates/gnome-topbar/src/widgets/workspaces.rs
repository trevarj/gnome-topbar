//! Workspaces widget — displays workspace indicators with animated transitions.
//!
//! Shows occupied/active workspaces as visual indicators (dots/pills/labels).
//! Clicking an indicator switches to that workspace.
//!
//! # Configuration
//!
//! - `label_type`: `"none"` (minimal dots, default), `"icons"` (●/○/◆ glyphs),
//!   `"name"` (workspace names; legacy alias: `"numbers"`), or `"index"`
//!   (meaningful numeric index, falling back to name when none exists).
//! - `separator`: string between indicators (non-minimal modes only).
//! - `animate`: `true` (default) enables the `WorkspaceContainer` custom widget
//!   for smooth transitions; `false` uses a plain GtkBox with no animation.
//! - `filter_by_output`: `true` (default) uses this bar output's per-output
//!   workspace state; `false` shows global/all-output workspace state, including
//!   each output's current workspace. In all-output mode, active styling still
//!   follows the compositor's globally focused workspace.
//!
//! # Architecture
//!
//! ## Two layout modes
//!
//! - **Animated** (`animate = true`): Indicators are children of a
//!   [`WorkspaceContainer`] custom widget. Each indicator's pixel width is owned
//!   frame-by-frame by Rust via an [`IndicatorWidth`] animator, which calls
//!   `set_size_request` per frame from the shared [`Animation`] helper. The
//!   container lays children out left-to-right; because Rust knows every child's
//!   exact width each frame, there is no rounding drift to hide.
//!
//! - **Non-animated** (`animate = false`): Indicators go directly in the content
//!   `GtkBox` and size themselves from the static CSS `min-width`. No
//!   `WorkspaceContainer`, no width animators.
//!
//! ## Width ownership
//!
//! There is one source of truth for indicator widths:
//! [`INDICATOR_INACTIVE_WIDTH_PX`] and [`INDICATOR_ACTIVE_WIDTH_PX`] (plus
//! [`LONG_INDICATOR_HPAD`] for named indicators). [`indicator_target_width`]
//! computes a pill's target from its active state and label content width; the
//! generated bar CSS interpolates the *same* constants into its `min-width`
//! rules so the animated and `animate = false` widths never diverge.
//!
//! ## Update paths
//!
//! [`classify_change`] picks the structural path; width animation (active grow /
//! inactive shrink) runs every update regardless via [`retarget_all_widths`].
//!
//! - **Switch** ([`StructuralChange::None`]): Same IDs/order. Surviving widgets
//!   keep their GTK identity; only classes/labels change and each indicator
//!   retargets its width. Rapid switches retarget smoothly from the current
//!   interpolated width (no jump) because [`Animation`] re-captures the start
//!   value on each `start()`.
//!
//! - **Removal** ([`StructuralChange::RemovalOnly`]): Departed indicators shrink
//!   to 0 px in place ([`IndicatorWidth::shrink_to_zero`]) and unparent on
//!   completion; survivors keep identity and in-flight animations.
//!
//! - **Recreate** ([`StructuralChange::Recreate`]): Additions (with or without
//!   removals) or a pure reorder. Full rebuild: new indicators seed at 0 px and
//!   grow in; survivors seed at their current animated width so they do not jump.
//!
//! ## Testing
//!
//! Pure logic ([`classify_change`], [`indicator_target_width`], `build_tooltip`)
//! is unit-tested. Layout behavior requires a GTK display server and is verified
//! manually.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::rc::Rc;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::gdk::BUTTON_PRIMARY;
use gtk4::glib;
use gtk4::pango::EllipsizeMode;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{
    Align, Box as GtkBox, DrawingArea, EventControllerScroll, EventControllerScrollFlags,
    GestureClick, Label, Overlay, Widget,
};
use tracing::{debug, trace, warn};

use crate::services::callbacks::CallbackId;
use crate::services::config_manager::ConfigManager;
use crate::services::tooltip::TooltipManager;
use crate::services::workspace::{Workspace, WorkspaceService, WorkspaceServiceSnapshot};
use crate::styles::{state, widget};
use crate::widgets::WidgetConfig;
use crate::widgets::animation::{Animation, AnimationParams};
use crate::widgets::base::BaseWidget;
use crate::widgets::ripple::{trigger_ripple_from_gesture, wrap_with_ripple};
use crate::widgets::warn_unknown_options;

#[derive(Debug)]
struct WorkspaceIndicatorProgressState {
    fraction: Cell<f64>,
    visible: Cell<bool>,
    area: glib::WeakRef<DrawingArea>,
    hide_timeout: RefCell<Option<glib::SourceId>>,
}

impl WorkspaceIndicatorProgressState {
    fn new() -> Self {
        Self {
            fraction: Cell::new(0.0),
            visible: Cell::new(false),
            area: glib::WeakRef::new(),
            hide_timeout: RefCell::new(None),
        }
    }

    fn request_redraw(&self) {
        if let Some(area) = self.area.upgrade() {
            area.queue_draw();
        }
    }

    fn cancel_hide_timeout(&self) {
        if let Some(id) = self.hide_timeout.borrow_mut().take() {
            id.remove();
        }
    }

    fn hide_after_transition(self: &Rc<Self>) {
        if self.hide_timeout.borrow().is_some() {
            return;
        }

        let state = Rc::downgrade(self);
        let source_id = glib::timeout_add_local_once(
            std::time::Duration::from_micros(INDICATOR_ANIM_DURATION_US as u64),
            move || {
                let Some(state) = state.upgrade() else {
                    return;
                };

                state.hide_timeout.borrow_mut().take();
                state.visible.set(false);
                state.fraction.set(0.0);
                state.request_redraw();
            },
        );
        self.hide_timeout.borrow_mut().replace(source_id);
    }
}

impl Drop for WorkspaceIndicatorProgressState {
    fn drop(&mut self) {
        if let Some(id) = self.hide_timeout.borrow_mut().take() {
            id.remove();
        }
    }
}

// ---------------------------------------------------------------------------
// WorkspaceContainer — a minimal left-to-right layout whose children's widths
// are owned frame-by-frame by Rust (see `IndicatorWidth`). Because Rust knows
// each child's exact pixel width every frame via `set_size_request`, the
// container can lay them out with simple left-to-right arithmetic — no CSS
// `min-width` transitions, no two-group split, and no convergence machinery.
// ---------------------------------------------------------------------------

mod ws_container_imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct WorkspaceContainer {
        pub(super) children: RefCell<Vec<Widget>>,
        pub(super) gap: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WorkspaceContainer {
        const NAME: &'static str = "GnomePanelWorkspaceContainer";
        type Type = super::WorkspaceContainer;
        type ParentType = Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("workspace-container");
        }
    }

    impl ObjectImpl for WorkspaceContainer {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk4::Overflow::Hidden);
        }

        fn dispose(&self) {
            for child in self.children.borrow_mut().drain(..) {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for WorkspaceContainer {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = self.children.borrow();
            if children.is_empty() {
                return (0, 0, -1, -1);
            }
            if orientation == gtk4::Orientation::Horizontal {
                // Each child's width is owned by Rust via set_size_request, so
                // the sum of measured natural widths is exact — no rounding
                // drift to absorb.
                let w = sum_children_widths(&children, self.gap.get());
                (w, w, -1, -1)
            } else {
                let mut max_min = 0i32;
                let mut max_nat = 0i32;
                for child in children.iter() {
                    let (cmin, cnat, _, _) = child.measure(orientation, for_size);
                    max_min = max_min.max(cmin);
                    max_nat = max_nat.max(cnat);
                }
                (max_min, max_nat, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let children = self.children.borrow();
            let n = children.len();
            if n == 0 {
                return;
            }

            if n == 1 {
                let (_, cw, _, _) = children[0].measure(gtk4::Orientation::Horizontal, height);
                let x = (width - cw) / 2;
                let t = gtk4::gsk::Transform::new()
                    .translate(&gtk4::graphene::Point::new(x as f32, 0.0));
                children[0].allocate(cw, height, baseline, Some(t));
                return;
            }

            // Simple left-to-right layout. Rust owns each child's exact width,
            // so no two-group split is needed to hide transition rounding.
            let gap = self.gap.get();
            let mut x = 0i32;
            for child in children.iter() {
                let (_, cw, _, _) = child.measure(gtk4::Orientation::Horizontal, height);
                let t = gtk4::gsk::Transform::new()
                    .translate(&gtk4::graphene::Point::new(x as f32, 0.0));
                child.allocate(cw, height, baseline, Some(t));
                x += cw + gap;
            }
        }
    }
}

glib::wrapper! {
    pub struct WorkspaceContainer(ObjectSubclass<ws_container_imp::WorkspaceContainer>)
        @extends Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl WorkspaceContainer {
    fn new() -> Self {
        glib::Object::builder().build()
    }

    fn set_gap(&self, gap: i32) {
        self.imp().gap.set(gap);
    }

    fn add_child(&self, child: &Widget) {
        child.set_parent(self);
        self.imp().children.borrow_mut().push(child.clone());
    }

    fn clear_children(&self) {
        for child in self.imp().children.borrow_mut().drain(..) {
            child.unparent();
        }
    }

    /// Remove a specific child widget by reference. Order-preserving.
    fn remove_child(&self, child: &Widget) {
        let mut children = self.imp().children.borrow_mut();
        if let Some(pos) = children.iter().position(|c| c == child) {
            children.remove(pos);
            child.unparent();
        }
    }
}

/// Sum the current natural widths of `children` plus inter-child gaps.
///
/// Each child's width is owned by Rust (`IndicatorWidth::apply` calls
/// `set_size_request`), so measuring its natural width returns the exact
/// Rust-driven value.
///
/// Note: not unit-tested because `Widget::measure()` requires a GTK display
/// server. The gap arithmetic is trivially `(n-1) * gap`.
fn sum_children_widths(children: &[Widget], gap: i32) -> i32 {
    let n = children.len();
    if n == 0 {
        return 0;
    }
    let mut total = 0i32;
    for child in children {
        let (_, cw, _, _) = child.measure(gtk4::Orientation::Horizontal, -1);
        total += cw;
    }
    total + (n as i32 - 1) * gap
}

/// Per-indicator width animation: Rust owns this indicator's pixel width and
/// drives it toward a target over [`INDICATOR_ANIM_DURATION_MS`] using the
/// shared [`Animation`] helper.
///
/// Each frame applies `set_size_request(round(width), -1)` to the indicator and
/// `queue_resize()` on the container, so the container's
/// [`WorkspaceContainer::size_allocate`] sees the exact in-flight width. The
/// indicator and container are held weakly; if either is disposed the
/// underlying tick callback self-terminates.
///
/// Retargeting mid-flight is smooth: [`IndicatorWidth::retarget`] re-`start()`s
/// the animation from the current interpolated width, so rapid switches glide
/// from wherever the pill currently is rather than jumping.
struct IndicatorWidth {
    anim: Animation,
    indicator: glib::WeakRef<Widget>,
    container: glib::WeakRef<WorkspaceContainer>,
    /// Current interpolated width in pixels (f64 for smooth interpolation).
    current: Rc<Cell<f64>>,
    /// Last integer width actually pushed via `set_size_request`. Frames whose
    /// rounded width matches this are skipped, so we don't force a full-bar
    /// relayout for a visually-identical frame (common on the eased tail and
    /// for long/named pills whose sub-pixel width barely moves per frame).
    applied: Rc<Cell<i32>>,
    /// Current animation target in pixels.
    target: Cell<i32>,
}

impl IndicatorWidth {
    /// Create a width animator for `indicator` inside `container`, seeded at
    /// `initial` pixels (applied immediately, no animation).
    fn new(indicator: &Widget, container: &WorkspaceContainer, initial: i32) -> Self {
        indicator.set_size_request(initial, -1);
        let ind_weak = glib::WeakRef::new();
        ind_weak.set(Some(indicator));
        let container_weak = glib::WeakRef::new();
        container_weak.set(Some(container));
        Self {
            anim: Animation::new(indicator),
            indicator: ind_weak,
            container: container_weak,
            current: Rc::new(Cell::new(initial as f64)),
            applied: Rc::new(Cell::new(initial)),
            target: Cell::new(initial),
        }
    }

    /// Animate this indicator's width to `target` pixels.
    ///
    /// No-op when already targeting `target`. Otherwise starts a fresh linear
    /// run from the current interpolated width so reversals/retargets are
    /// smooth. When `theme.animations` is disabled, [`Animation::start`] jumps
    /// straight to the final width.
    fn retarget(&self, target: i32) {
        if !should_retarget_width(self.target.get(), target) {
            return;
        }
        self.target.set(target);

        let start = self.current.get();
        let end = target as f64;

        let current = Rc::clone(&self.current);
        let applied = Rc::clone(&self.applied);
        let indicator = self.indicator.clone();
        let container = self.container.clone();

        self.anim.start(
            AnimationParams::new(INDICATOR_ANIM_DURATION_MS),
            Box::new(move |eased| {
                let w = start + (end - start) * eased;
                current.set(w);
                let wi = w.round() as i32;
                if applied.get() == wi {
                    return;
                }
                applied.set(wi);
                if let Some(ind) = indicator.upgrade() {
                    ind.set_size_request(wi, -1);
                }
                if let Some(c) = container.upgrade() {
                    c.queue_resize();
                }
            }),
            None,
        );
    }

    /// Animate the width to 0 px, then run `after` once the shrink finishes
    /// (used by the removal path to unparent the indicator only after it has
    /// fully collapsed). Consumes `self`: the [`IndicatorWidth`] is kept alive
    /// (and its animation un-cancelled) by being moved into the done closure,
    /// then dropped after `after` runs. When `theme.animations` is disabled the
    /// shrink is instant and `after` fires synchronously.
    fn shrink_to_zero(self, after: impl FnOnce() + 'static) {
        self.target.set(0);
        let start = self.current.get();
        let current = Rc::clone(&self.current);
        let applied = Rc::clone(&self.applied);
        let indicator = self.indicator.clone();
        let container = self.container.clone();
        // Clone the animation handle so we can drive start() without borrowing
        // `self`, which is moved into the done closure to outlive the shrink.
        let anim = self.anim.clone();
        anim.start(
            AnimationParams::new(INDICATOR_ANIM_DURATION_MS),
            Box::new(move |eased| {
                let w = start * (1.0 - eased);
                current.set(w);
                let wi = w.round() as i32;
                if applied.get() == wi {
                    return;
                }
                applied.set(wi);
                if let Some(ind) = indicator.upgrade() {
                    ind.set_size_request(wi, -1);
                }
                if let Some(c) = container.upgrade() {
                    c.queue_resize();
                }
            }),
            Some(Box::new(move || {
                after();
                // `self` (the IndicatorWidth) drops here, after the animation
                // has already completed, so its Drop cancel() is a no-op.
                drop(self);
            })),
        );
    }

    /// Snap the width to `value` immediately, cancelling any animation.
    #[allow(dead_code)]
    fn snap(&self, value: i32) {
        self.anim.cancel();
        self.current.set(value as f64);
        self.applied.set(value);
        self.target.set(value);
        if let Some(ind) = self.indicator.upgrade() {
            ind.set_size_request(value, -1);
        }
        if let Some(c) = self.container.upgrade() {
            c.queue_resize();
        }
    }
}

fn should_retarget_width(current_target: i32, new_target: i32) -> bool {
    current_target != new_target
}

impl Drop for IndicatorWidth {
    fn drop(&mut self) {
        // Cancel the in-flight tick callback so it stops touching the (now
        // orphaned) indicator on the next frame.
        self.anim.cancel();
    }
}

/// Delta between active and inactive indicator widths.
const INDICATOR_WIDTH_DELTA: i32 = INDICATOR_ACTIVE_WIDTH_PX - INDICATOR_INACTIVE_WIDTH_PX;

/// Compute an indicator's target pixel width (the value Rust drives via
/// `set_size_request`), unifying short and long indicators.
///
/// - `is_active`: whether this indicator currently has the active class.
/// - `long_content_width`: for a long (named) indicator, the label's natural
///   content width in px (excludes padding). `None` for short/minimal
///   indicators.
///
/// For short indicators the width is simply [`INDICATOR_INACTIVE_WIDTH_PX`] or
/// [`INDICATOR_ACTIVE_WIDTH_PX`]. For long indicators the inactive width is the
/// label content width plus both horizontal paddings, floored at the short
/// inactive width; the active width adds [`INDICATOR_WIDTH_DELTA`] so the
/// active/inactive growth matches short indicators. This mirrors the measured
/// width because `set_size_request` is the widget's minimum and the long
/// indicator's natural content (label + `2 * LONG_INDICATOR_HPAD`) never exceeds
/// it once floored.
fn indicator_target_width(is_active: bool, long_content_width: Option<i32>) -> i32 {
    match long_content_width {
        None => {
            if is_active {
                INDICATOR_ACTIVE_WIDTH_PX
            } else {
                INDICATOR_INACTIVE_WIDTH_PX
            }
        }
        Some(content) => {
            let inactive = (content + 2 * LONG_INDICATOR_HPAD).max(INDICATOR_INACTIVE_WIDTH_PX);
            if is_active {
                inactive + INDICATOR_WIDTH_DELTA
            } else {
                inactive
            }
        }
    }
}

/// Label type for workspace indicators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelType {
    /// Show icon glyphs (●, ○, ◆).
    Icons,
    /// Show workspace labels/names.
    ///
    /// Historically configured as `label_type = "numbers"`; `"name"` is the
    /// preferred value for new configs.
    Name,
    /// Show a meaningful workspace index when available, otherwise the
    /// workspace name.
    Index,
    /// Minimal - no text, just CSS styling.
    None,
}

impl LabelType {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "icons" => LabelType::Icons,
            "name" | "numbers" => LabelType::Name,
            "index" => LabelType::Index,
            "none" => LabelType::None,
            other => {
                warn!(
                    "unknown workspace label_type {:?}, falling back to {:?}",
                    other, DEFAULT_LABEL_TYPE
                );
                DEFAULT_LABEL_TYPE
            }
        }
    }
}

fn workspace_label_text(label_type: LabelType, workspace: &Workspace) -> String {
    match label_type {
        LabelType::Icons => ICON_EMPTY.to_string(),
        LabelType::Name => workspace.name.clone(),
        LabelType::Index => {
            if workspace.idx >= 0 {
                workspace.idx.to_string()
            } else {
                workspace.name.clone()
            }
        }
        LabelType::None => String::new(),
    }
}

const DEFAULT_LABEL_TYPE: LabelType = LabelType::None;
const DEFAULT_SEPARATOR: &str = "";

/// Inactive (short) indicator width in pixels.
///
/// Single source of truth for the inactive pill width. The generated bar CSS
/// interpolates this same constant into `.workspace-indicator { min-width }`
/// (see [`crate::widgets::css::bar::css`]) so the Rust-animated width and the
/// `animate = false` CSS-static width agree exactly. The `.workspace-indicator`
/// rule has `padding: 0` and no border, so this value equals the final measured
/// width.
pub(crate) const INDICATOR_INACTIVE_WIDTH_PX: i32 = 6;

/// Active (long) indicator width in pixels.
///
/// Single source of truth for the active pill width; interpolated into
/// `.workspace-indicator.active { min-width }` in the generated CSS.
pub(crate) const INDICATOR_ACTIVE_WIDTH_PX: i32 = 28;

/// Horizontal padding per side (px) for long (named) indicators.
///
/// `.workspace-indicator-long { padding: 0 LONG_INDICATOR_HPAD }`. Rust adds
/// `2 * LONG_INDICATOR_HPAD` to a label's natural content width when computing a
/// long indicator's target so the Rust-owned width matches the final measured
/// (padding-inclusive) width.
pub(crate) const LONG_INDICATOR_HPAD: i32 = 6;

/// Configuration for the workspaces widget.
#[derive(Debug, Clone)]
pub struct WorkspacesConfig {
    /// How to display workspace labels.
    pub label_type: LabelType,
    /// Separator string between workspace indicators.
    pub separator: String,
    /// Whether to animate circle↔pill transitions.
    /// `None` = not explicitly set by user (inherits from global `theme.animations`).
    pub animate: Option<bool>,
    /// Whether to use this bar output's per-output workspace state.
    ///
    /// When `true`, active styling reflects the workspace active on this bar's
    /// output. When `false`, the widget shows global/all-output workspace state,
    /// including each output's current workspace, but active styling still
    /// reflects the compositor's globally focused workspace.
    pub filter_by_output: bool,
    /// Whether to show compositor-reported workspaces without windows.
    pub show_unoccupied: bool,
}

impl WidgetConfig for WorkspacesConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options(
            "workspaces",
            entry,
            &[
                "label_type",
                "separator",
                "animate",
                "filter_by_output",
                "show_unoccupied",
            ],
        );

        let label_type = entry
            .options
            .get("label_type")
            .and_then(|v| v.as_str())
            .map(LabelType::from_str)
            .unwrap_or(DEFAULT_LABEL_TYPE);

        let separator = entry
            .options
            .get("separator")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_SEPARATOR)
            .to_string();

        let animate = entry.options.get("animate").and_then(|v| v.as_bool());
        let defaults = Self::default();
        let filter_by_output = entry
            .options
            .get("filter_by_output")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.filter_by_output);
        let show_unoccupied = entry
            .options
            .get("show_unoccupied")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.show_unoccupied);

        Self {
            label_type,
            separator,
            animate,
            filter_by_output,
            show_unoccupied,
        }
    }
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            label_type: DEFAULT_LABEL_TYPE,
            separator: DEFAULT_SEPARATOR.to_string(),
            animate: None,
            filter_by_output: true,
            show_unoccupied: false,
        }
    }
}

/// Workspaces widget that displays workspace indicators.
pub struct WorkspacesWidget {
    /// Shared base widget container.
    base: BaseWidget,
    /// Callback ID for WorkspaceService, used to disconnect on drop.
    workspace_callback_id: CallbackId,
}

impl WorkspacesWidget {
    /// Create a new workspaces widget with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Widget configuration (label type, separator).
    /// * `output_id` - Optional output/monitor name. When set and
    ///   `filter_by_output = true`, the widget will:
    ///   - Only show Niri workspaces belonging to this output.
    pub fn new(config: WorkspacesConfig, output_id: Option<String>) -> Self {
        let base = BaseWidget::new(&[widget::WORKSPACES]);

        let label_type = config.label_type;
        let filter_by_output = config.filter_by_output;
        let show_unoccupied = config.show_unoccupied;
        // Per-widget animate flag takes precedence when explicitly set.
        // Falls back to the global theme.animations setting.
        let animate = config
            .animate
            .unwrap_or_else(|| ConfigManager::global().animations_enabled());

        // Cache theme sizes at construction time. These values are derived
        // from bar.size/bar.padding, and any change to those triggers a full
        // bar rebuild (config_structure_changed → reconfigure_all), which
        // destroys and recreates this widget with fresh values.
        let sizes = ConfigManager::global().theme_sizes();
        let content_gap = sizes.widget_content_gap;

        // Animated mode uses WorkspaceContainer; otherwise indicators go in the GtkBox.
        let ws_container: Option<WorkspaceContainer> = if animate {
            let container = WorkspaceContainer::new();
            container.set_gap(content_gap as i32);
            base.content().append(&container);
            Some(container)
        } else {
            None
        };

        let content_box = base.content().clone();

        let workspace_labels: Rc<RefCell<HashMap<i32, Widget>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let progress_states: Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>> =
            Rc::new(RefCell::new(HashMap::new()));
        // Per-indicator width animators (animated/minimal mode only). Parallel
        // to `workspace_labels`; entry presence mirrors a live indicator.
        let workspace_widths: Rc<RefCell<HashMap<i32, IndicatorWidth>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let current_ids = Rc::new(RefCell::new(Vec::new()));
        let separator = config.separator;

        let output_id_debug = output_id.clone();
        let scroll_output_id = output_id.clone();
        let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(move |_, _dx, dy| {
            if dy == 0.0 {
                return glib::Propagation::Proceed;
            }

            let snapshot = WorkspaceService::global().snapshot();
            if let Some(next_id) = workspace_id_for_scroll(
                &snapshot,
                show_unoccupied,
                if filter_by_output {
                    scroll_output_id.as_deref()
                } else {
                    None
                },
                dy,
            ) {
                TooltipManager::global().cancel_and_hide();
                WorkspaceService::global().switch_workspace(next_id);
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        });
        base.widget().add_controller(scroll);

        let workspace_callback_id = WorkspaceService::global().connect(move |snapshot| {
            update_indicators(
                &content_box,
                ws_container.as_ref(),
                &workspace_labels,
                &progress_states,
                &workspace_widths,
                &current_ids,
                label_type,
                &separator,
                snapshot,
                show_unoccupied,
                if filter_by_output {
                    output_id.as_deref()
                } else {
                    None
                },
            );
        });

        debug!(
            "WorkspacesWidget created (output_id: {:?})",
            output_id_debug
        );
        Self {
            base,
            workspace_callback_id,
        }
    }

    /// Get the root GTK widget for embedding in the bar.
    pub fn widget(&self) -> &GtkBox {
        self.base.widget()
    }
}

impl Drop for WorkspacesWidget {
    fn drop(&mut self) {
        WorkspaceService::global().disconnect(self.workspace_callback_id);
    }
}

/// Icon glyphs for workspace indicators.
const ICON_OCCUPIED: &str = "●";
const ICON_EMPTY: &str = "○";
const ICON_ACTIVE: &str = "◆";

/// Duration of the workspace indicator width animation (grow-in, switch
/// resize, removal) in milliseconds. Rust owns the width frame-by-frame; this
/// is the single source of truth for the pill animation feel.
const INDICATOR_ANIM_DURATION_MS: u64 = 225;

/// Same duration in microseconds, used by `glib::timeout` and tick math (e.g.
/// the progress-track hide delay that outlasts a pill's shrink).
const INDICATOR_ANIM_DURATION_US: i64 = (INDICATOR_ANIM_DURATION_MS as i64) * 1_000;

/// Clear all workspace indicator widgets from the container.
fn clear_indicators(
    container: &GtkBox,
    ws_container: Option<&WorkspaceContainer>,
    labels: &Rc<RefCell<HashMap<i32, Widget>>>,
    progress_states: &Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>>,
    widths: &Rc<RefCell<HashMap<i32, IndicatorWidth>>>,
    ids: &Rc<RefCell<Vec<i32>>>,
) {
    if let Some(wsc) = ws_container {
        wsc.clear_children();
    } else {
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
    }
    labels.borrow_mut().clear();
    progress_states.borrow_mut().clear();
    // Dropping the IndicatorWidth values cancels their in-flight animations.
    widths.borrow_mut().clear();
    ids.borrow_mut().clear();
}

/// Create a single workspace indicator widget.
///
/// Returns an `Overlay` wrapping the inner dot/label with a ripple effect.
/// State and sizing CSS classes (`.active`, `.workspace-indicator-long`, …) go
/// on the overlay — it is the widget [`IndicatorWidth`] drives via
/// `set_size_request` and the container measures and lays out.
fn create_single_indicator(
    label_type: LabelType,
    workspace: &Workspace,
    progress_states: &Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>>,
) -> Widget {
    let workspace_id = workspace.id;
    let mut is_long = false;

    let content = GtkBox::new(gtk4::Orientation::Horizontal, 0);
    content.add_css_class(widget::WORKSPACE_INDICATOR_CONTENT);

    if label_type != LabelType::None {
        let label_text = workspace_label_text(label_type, workspace);
        is_long = label_text.chars().count() > 2;
        let label = Label::new(Some(&label_text));

        // Optical centering: glyphs ●/○/◆ appear left-heavy at 0.5;
        // 0.55 nudges them to look visually centered in the pill.
        label.set_xalign(0.55);
        label.set_ellipsize(EllipsizeMode::End);
        label.set_single_line_mode(true);
        content.append(&label);
    }

    let progress_state = Rc::new(WorkspaceIndicatorProgressState::new());

    let progress_area = DrawingArea::new();
    progress_area.add_css_class(widget::WORKSPACE_INDICATOR_PROGRESS);
    progress_area.set_halign(Align::Fill);
    progress_area.set_valign(Align::Fill);
    progress_area.set_vexpand(true);
    progress_area.set_hexpand(true);
    progress_area.set_can_target(false);

    let progress_overlay = Overlay::new();
    progress_overlay.set_child(Some(&progress_area));

    if label_type != LabelType::None {
        progress_overlay.add_overlay(&content);
    }

    let state_for_area = Rc::clone(&progress_state);
    // Cache resolved colors keyed on the area's CSS `color`. The active pill is
    // redrawn every frame while its width animates, but the resolved colors are
    // constant for the life of the theme, so resolving them (palette lookup,
    // RGBA parse, style-context color resolution) once per color value instead
    // of once per frame keeps the animation's per-frame work minimal. The key
    // changes — and the cache refreshes — when the theme/CSS changes.
    let cached_colors: Cell<Option<(gtk4::gdk::RGBA, (gtk4::gdk::RGBA, gtk4::gdk::RGBA))>> =
        Cell::new(None);
    progress_area.set_draw_func(move |area, cr, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        if !state_for_area.visible.get() {
            return;
        }

        let radius = (width.min(height) / 2.0).max(0.0);
        let color_key = area.color();
        let (track_color, fill_color) = match cached_colors.get() {
            Some((key, colors)) if key == color_key => colors,
            _ => {
                let colors = workspace_progress_colors(area);
                cached_colors.set(Some((color_key, colors)));
                colors
            }
        };

        let fraction = state_for_area.fraction.get().clamp(0.0, 1.0);
        let fill_width = (width * fraction).round().clamp(0.0, width);

        let _ = cr.save();
        draw_rounded_rect(cr, 0.0, 0.0, width, height, radius);
        cr.clip();

        if fill_width <= 0.0 {
            set_cairo_source_rgba(cr, track_color);
            cr.paint().unwrap_or(());
            let _ = cr.restore();
            return;
        }

        if fill_width >= width {
            set_cairo_source_rgba(cr, track_color);
            cr.paint().unwrap_or(());
            let _ = cr.restore();
            return;
        }

        // Paint the fill as the base layer, then punch the track out of it.
        // This keeps the left rounded fill edge from antialiasing over the
        // white active track while still giving the inner fill edge a rounded
        // cap against the remaining track.
        set_cairo_source_rgba(cr, fill_color);
        cr.paint().unwrap_or(());

        let fill_radius = radius.min(fill_width / 2.0);
        set_cairo_source_rgba(cr, track_color);
        cr.set_fill_rule(gtk4::cairo::FillRule::EvenOdd);
        draw_rounded_rect(cr, 0.0, 0.0, width, height, radius);
        draw_rounded_rect(cr, 0.0, 0.0, fill_width, height, fill_radius);
        let _ = cr.fill();
        cr.set_fill_rule(gtk4::cairo::FillRule::Winding);

        let _ = cr.restore();
    });

    let (overlay, ripple_handle) = wrap_with_ripple(&progress_overlay);

    progress_state.area.set(Some(&progress_area));
    progress_states
        .borrow_mut()
        .insert(workspace_id, Rc::clone(&progress_state));

    // Sizing, state, and visual classes go on the overlay — it is the
    // widget that WorkspaceContainer measures and lays out.
    overlay.add_css_class(widget::WORKSPACE_INDICATOR);
    overlay.add_css_class(state::CLICKABLE);
    if label_type == LabelType::None {
        overlay.add_css_class(widget::WORKSPACE_INDICATOR_MINIMAL);
    }
    if is_long {
        overlay.add_css_class(widget::WORKSPACE_INDICATOR_LONG);
    }
    overlay.set_valign(Align::Center);

    let gesture = GestureClick::new();
    gesture.set_button(BUTTON_PRIMARY);
    gesture.connect_pressed({
        let rh = ripple_handle;
        move |gesture, _n_press, x, y| {
            trigger_ripple_from_gesture(gesture, x, y, &rh);
        }
    });
    gesture.connect_released(move |gesture, _n_press, _x, _y| {
        if gesture.current_button() != BUTTON_PRIMARY {
            return;
        }
        TooltipManager::global().cancel_and_hide();
        debug!("Switching to workspace {}", workspace_id);
        WorkspaceService::global().switch_workspace(workspace_id);
    });
    overlay.add_controller(gesture);

    overlay.upcast()
}

fn draw_rounded_rect(
    cr: &gtk4::cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    if radius <= 0.0 {
        cr.rectangle(x, y, width, height);
        return;
    }

    let x0 = x;
    let y0 = y;
    let x1 = x + width;
    let y1 = y + height;

    cr.new_path();
    cr.move_to(x0 + radius, y0);
    cr.arc(x1 - radius, y0 + radius, radius, -PI / 2.0, 0.0);
    cr.arc(x1 - radius, y1 - radius, radius, 0.0, PI / 2.0);
    cr.arc(x0 + radius, y1 - radius, radius, PI / 2.0, PI);
    cr.arc(x0 + radius, y0 + radius, radius, PI, 3.0 * PI / 2.0);
    cr.close_path();
}

fn set_cairo_source_rgba(cr: &gtk4::cairo::Context, color: gtk4::gdk::RGBA) {
    cr.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

fn workspace_progress_colors(area: &DrawingArea) -> (gtk4::gdk::RGBA, gtk4::gdk::RGBA) {
    let palette = ConfigManager::global().palette();
    let fallback = area.color();
    let track = resolve_workspace_progress_color(area, &palette.foreground_primary, fallback);
    let fill = gtk4::gdk::RGBA::parse("#71717a").unwrap_or(track);
    (track, fill)
}

fn resolve_workspace_progress_color(
    area: &DrawingArea,
    color: &str,
    fallback: gtk4::gdk::RGBA,
) -> gtk4::gdk::RGBA {
    gtk4::gdk::RGBA::parse(color).unwrap_or_else(|_| {
        if let Some(name) = color.strip_prefix('@') {
            // GTK4 has no non-deprecated replacement for resolving runtime
            // theme color tokens from a widget style context.
            #[allow(deprecated)]
            if let Some(rgba) = area.style_context().lookup_color(name) {
                return rgba;
            }
        }

        fallback
    })
}

fn workspace_indicator_label(indicator: &Widget) -> Option<Label> {
    let overlay = indicator.downcast_ref::<Overlay>()?;
    let content_overlay = overlay.child()?.downcast::<Overlay>().ok()?;

    let mut child = content_overlay.first_child();
    while let Some(child_widget) = child {
        if child_widget.has_css_class(widget::WORKSPACE_INDICATOR_CONTENT) {
            let content = child_widget.downcast::<GtkBox>().ok()?;
            let mut content_child = content.first_child();

            while let Some(node) = content_child {
                if let Ok(label) = node.clone().downcast::<Label>() {
                    return Some(label);
                }

                content_child = node.next_sibling();
            }

            break;
        }

        child = child_widget.next_sibling();
    }

    None
}

fn workspace_indicator_progress(
    progress_states: &Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>>,
    workspace: &Workspace,
) {
    let Some(state) = progress_states.borrow().get(&workspace.id).cloned() else {
        return;
    };

    if !workspace.active {
        // The CSS min-width transition still shrinks the old active pill.
        // Keep its Rust-drawn surface alive until that transition finishes so
        // switching workspaces does not snap to the inactive background first.
        if state.visible.get() {
            state.hide_after_transition();
        }
        state.request_redraw();
        return;
    }

    let fraction = workspace
        .active_window_progress
        .map(|progress| progress.clamp(0.0, 1.0))
        .unwrap_or(0.0);

    // Rust draws the active pill surface even when there is no progress. That
    // keeps the progress fill from compositing over a separate CSS background.
    state.cancel_hide_timeout();
    state.visible.set(true);
    state.fraction.set(fraction);
    state.request_redraw();
}

/// Create workspace indicator widgets for the given workspaces.
///
/// In animated mode (`ws_container` present), each indicator also gets an
/// [`IndicatorWidth`] seeded from `seeds` (keyed by workspace id), defaulting to
/// 0 px for any id absent from `seeds`. Newly-appearing indicators are seeded at
/// 0 so the caller's later `retarget` grows them in; survivors are seeded at
/// their pre-recreate width so they do not jump. The caller is responsible for
/// retargeting every indicator to its final width after labels/classes are set.
#[allow(clippy::too_many_arguments)]
fn create_indicators(
    container: &GtkBox,
    ws_container: Option<&WorkspaceContainer>,
    labels_cell: &Rc<RefCell<HashMap<i32, Widget>>>,
    progress_states: &Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>>,
    widths_cell: &Rc<RefCell<HashMap<i32, IndicatorWidth>>>,
    ids_cell: &Rc<RefCell<Vec<i32>>>,
    label_type: LabelType,
    separator: &str,
    workspaces: &[Workspace],
    seeds: &HashMap<i32, i32>,
) {
    clear_indicators(
        container,
        ws_container,
        labels_cell,
        progress_states,
        widths_cell,
        ids_cell,
    );

    let mut labels = labels_cell.borrow_mut();
    let mut widths = widths_cell.borrow_mut();
    let mut ids = ids_cell.borrow_mut();

    for (i, workspace) in workspaces.iter().enumerate() {
        let indicator = create_single_indicator(label_type, workspace, progress_states);

        labels.insert(workspace.id, indicator.clone());
        if let Some(wsc) = ws_container {
            wsc.add_child(&indicator);
            let seed = seeds.get(&workspace.id).copied().unwrap_or(0);
            widths.insert(workspace.id, IndicatorWidth::new(&indicator, wsc, seed));
        } else {
            container.append(&indicator);
            // Non-animated mode has no IndicatorWidth animator and the CSS
            // declares no min-width, so set the resting width explicitly.
            // The styling pass refreshes it once state classes are final.
            let target = indicator_target_width(workspace.active, long_content_width(&indicator));
            indicator.set_size_request(target, -1);
        }
        ids.push(workspace.id);

        if ws_container.is_none() && i < workspaces.len() - 1 && !separator.is_empty() {
            let sep = Label::new(Some(separator));
            sep.set_valign(Align::Center);
            sep.add_css_class(widget::WORKSPACE_SEPARATOR);
            container.append(&sep);
        }
    }
}

fn set_label_text_if_changed(label: &Label, text: &str) {
    if label.text().as_str() != text {
        label.set_text(text);
    }
}

/// Measure a long indicator's label content width (px, excludes padding).
///
/// Returns `None` for a short/minimal indicator (no `.workspace-indicator-long`
/// class), in which case [`indicator_target_width`] uses the fixed short width.
fn long_content_width(indicator: &Widget) -> Option<i32> {
    if !indicator.has_css_class(widget::WORKSPACE_INDICATOR_LONG) {
        return None;
    }
    let label = workspace_indicator_label(indicator)?;
    let (_, nat, _, _) = label.measure(gtk4::Orientation::Horizontal, -1);
    Some(nat)
}

/// Compute and apply the target width for every live indicator.
///
/// Reads each indicator's active class and (for long indicators) label content
/// width, computes the target via [`indicator_target_width`], and retargets its
/// [`IndicatorWidth`] animator. Must run after CSS state classes and label text
/// are updated so the measurements are current.
fn retarget_all_widths(labels: &HashMap<i32, Widget>, widths: &HashMap<i32, IndicatorWidth>) {
    for (id, indicator) in labels {
        let Some(width) = widths.get(id) else {
            continue;
        };
        let is_active = indicator.has_css_class(widget::ACTIVE);
        let target = indicator_target_width(is_active, long_content_width(indicator));
        width.retarget(target);
    }
}

// ---------------------------------------------------------------------------
// StructuralChange — classifies how the visible workspace set changed.
// ---------------------------------------------------------------------------

/// How the visible workspace set changed, from the perspective of structural
/// (add/remove/reorder) layout work. Width animations (active/inactive resize)
/// are driven per-indicator regardless and do not need a variant here.
#[derive(Debug, PartialEq)]
enum StructuralChange {
    /// Same IDs in the same order — only state (active/occupied/urgent) and/or
    /// labels may have changed. No add/remove/reorder. Covers the common
    /// workspace-switch case; widths retarget in place, identity preserved.
    None,
    /// IDs were removed but none added (minimal/animated mode only). Departed
    /// indicators shrink to 0 px in place, then unparent — surviving widgets
    /// keep their GTK identity and in-flight animations.
    RemovalOnly,
    /// Any change involving additions, or a pure reorder (minimal/animated mode
    /// only). Full recreate: new indicators seed at 0 px and grow in; survivors
    /// seed at their pre-recreate width so they do not jump.
    Recreate,
}

/// Pure classification of the structural change for the animated path.
///
/// Inputs:
/// - `ids_changed`: whether `new_ids != old_ids` (order-sensitive)
/// - `has_additions`: whether any ID in `new_ids` is not in `old_ids`
/// - `has_removals`: whether any ID in `old_ids` is not in `new_ids`
/// - `is_animated`: whether a [`WorkspaceContainer`] is present (animated mode)
///
/// This function has no side effects — the caller uses the result to decide
/// which layout path to take.
fn classify_change(
    ids_changed: bool,
    has_additions: bool,
    has_removals: bool,
    is_animated: bool,
) -> StructuralChange {
    if !ids_changed || !is_animated {
        // Unchanged, or non-animated (handled by a plain recreate elsewhere).
        return StructuralChange::None;
    }

    if has_removals && !has_additions {
        // Pure removal(s), same relative order of survivors.
        StructuralChange::RemovalOnly
    } else {
        // Additions (with or without removals) or a pure reorder.
        StructuralChange::Recreate
    }
}

fn collect_display_ids(
    workspaces: &[Workspace],
    active_workspaces: &HashSet<i32>,
    snapshot: &WorkspaceServiceSnapshot,
    show_unoccupied: bool,
    include_all_output_active: bool,
) -> HashSet<i32> {
    let mut display_ids: HashSet<i32> = workspaces
        .iter()
        // `window_count.is_some()` filters out synthetic placeholders and only
        // includes empty workspaces explicitly reported by the compositor.
        .filter(|ws| ws.occupied || (show_unoccupied && ws.window_count.is_some()))
        .map(|ws| ws.id)
        .collect();

    // Include active workspaces (supports multi-tag view).
    display_ids.extend(active_workspaces.iter());

    if include_all_output_active {
        // In all-output mode, show each output's current workspace even when it
        // is empty. Styling still uses `active_workspaces`, so only the
        // compositor's globally focused workspace gets the active class.
        for per_output in snapshot.per_output.values() {
            display_ids.extend(per_output.active_workspace.iter());
        }
    }

    display_ids
}

fn display_workspaces_for_snapshot(
    snapshot: &WorkspaceServiceSnapshot,
    show_unoccupied: bool,
    output_id: Option<&str>,
) -> Vec<Workspace> {
    let (workspaces, active_workspaces): (&[Workspace], &HashSet<i32>) =
        if let Some(output) = output_id {
            if let Some(per_output) = snapshot.per_output.get(output) {
                (&per_output.workspaces, &per_output.active_workspace)
            } else {
                (&snapshot.workspaces, &snapshot.active_workspace)
            }
        } else {
            (&snapshot.workspaces, &snapshot.active_workspace)
        };

    let display_ids = collect_display_ids(
        workspaces,
        active_workspaces,
        snapshot,
        show_unoccupied,
        output_id.is_none(),
    );

    workspaces
        .iter()
        .filter(|ws| display_ids.contains(&ws.id))
        .cloned()
        .collect()
}

fn workspace_id_for_scroll(
    snapshot: &WorkspaceServiceSnapshot,
    show_unoccupied: bool,
    output_id: Option<&str>,
    dy: f64,
) -> Option<i32> {
    let display_workspaces = display_workspaces_for_snapshot(snapshot, show_unoccupied, output_id);
    if display_workspaces.len() < 2 {
        return None;
    }

    let active_idx = display_workspaces.iter().position(|ws| ws.active)?;
    let next_idx = if dy > 0.0 {
        active_idx.checked_add(1)?
    } else {
        active_idx.checked_sub(1)?
    };

    display_workspaces.get(next_idx).map(|ws| ws.id)
}

/// Update workspace indicators based on the current snapshot.
///
/// When `output_id` is provided (i.e. `filter_by_output = true`):
/// - Uses per-output workspace data if available.
/// - Only shows Niri workspaces belonging to this output.
///
/// When `output_id` is not provided (i.e. `filter_by_output = false`), uses
/// global/all-output workspace data and also displays each output's current
/// workspace. Active styling still follows the compositor's globally focused
/// workspace.
#[allow(clippy::too_many_arguments)]
fn update_indicators(
    container: &GtkBox,
    ws_container: Option<&WorkspaceContainer>,
    labels_cell: &Rc<RefCell<HashMap<i32, Widget>>>,
    progress_states: &Rc<RefCell<HashMap<i32, Rc<WorkspaceIndicatorProgressState>>>>,
    widths_cell: &Rc<RefCell<HashMap<i32, IndicatorWidth>>>,
    ids_cell: &Rc<RefCell<Vec<i32>>>,
    label_type: LabelType,
    separator: &str,
    snapshot: &WorkspaceServiceSnapshot,
    show_unoccupied: bool,
    output_id: Option<&str>,
) {
    let (workspaces, active_workspaces, source): (&[Workspace], &HashSet<i32>, &str) = if let Some(
        output,
    ) =
        output_id
    {
        if let Some(per_output) = snapshot.per_output.get(output) {
            (
                &per_output.workspaces,
                &per_output.active_workspace,
                "per_output",
            )
        } else {
            debug!(
                "workspace widget: output_id={:?} not found in per_output keys={:?}, using global",
                output,
                snapshot.per_output.keys().collect::<Vec<_>>()
            );
            (
                &snapshot.workspaces,
                &snapshot.active_workspace,
                "global_fallback",
            )
        }
    } else {
        (&snapshot.workspaces, &snapshot.active_workspace, "global")
    };

    trace!(
        "workspace widget: source={}, output_id={:?}, active_workspaces={:?}",
        source, output_id, active_workspaces
    );

    let display_ids = collect_display_ids(
        workspaces,
        active_workspaces,
        snapshot,
        show_unoccupied,
        output_id.is_none(),
    );

    trace!(
        "workspace widget: occupied_ids={:?}, adding active={:?}",
        display_ids, active_workspaces
    );

    let display_workspaces: Vec<_> = workspaces
        .iter()
        .filter(|ws| display_ids.contains(&ws.id))
        .cloned()
        .collect();

    trace!(
        "workspace widget: display_ids={:?}, display_workspaces={:?}",
        display_ids,
        display_workspaces
            .iter()
            .map(|ws| (ws.id, ws.active, ws.occupied))
            .collect::<Vec<_>>()
    );

    if display_workspaces.is_empty() {
        let current_ids = ids_cell.borrow();
        if !current_ids.is_empty() {
            drop(current_ids);
            clear_indicators(
                container,
                ws_container,
                labels_cell,
                progress_states,
                widths_cell,
                ids_cell,
            );
        }
        return;
    }

    let new_ids: Vec<i32> = display_workspaces.iter().map(|ws| ws.id).collect();

    let ids_changed = new_ids != *ids_cell.borrow();

    let old_ids: HashSet<i32> = ids_cell.borrow().iter().copied().collect();
    let new_ids_set: HashSet<i32> = new_ids.iter().copied().collect();
    let has_additions = ids_changed && new_ids_set.iter().any(|id| !old_ids.contains(id));
    let has_removals = ids_changed && old_ids.iter().any(|id| !new_ids_set.contains(id));

    let change_type = classify_change(
        ids_changed,
        has_additions,
        has_removals,
        ws_container.is_some(),
    );

    // `ids_changed && !is_animated` is a structural change with no animation
    // path — recreate the plain GtkBox children below. Tracked separately from
    // `StructuralChange` (which only describes the animated container).
    let non_animated_recreate = ids_changed && ws_container.is_none();

    match change_type {
        StructuralChange::None => {
            if non_animated_recreate {
                // Non-animated mode — plain full recreate, no width animators.
                create_indicators(
                    container,
                    None,
                    labels_cell,
                    progress_states,
                    widths_cell,
                    ids_cell,
                    label_type,
                    separator,
                    &display_workspaces,
                    &HashMap::new(),
                );
            }
            // Otherwise: same IDs/order — survivors keep identity; the styling
            // tail updates classes/labels and `retarget_all_widths` animates.
        }
        StructuralChange::RemovalOnly => {
            // Departed indicators shrink to 0 px in place, then unparent.
            // Surviving widgets keep their identity (and any in-flight width
            // animation toward a new active/inactive target).
            if let Some(wsc) = ws_container {
                let removed: Vec<i32> = ids_cell
                    .borrow()
                    .iter()
                    .copied()
                    .filter(|id| !new_ids_set.contains(id))
                    .collect();

                for id in removed {
                    // Detach the indicator from the bookkeeping maps now so the
                    // styling tail and width retargeting ignore it, but keep its
                    // widget parented and its IndicatorWidth alive in a local so
                    // the shrink animation can run to completion.
                    let indicator = labels_cell.borrow_mut().remove(&id);
                    progress_states.borrow_mut().remove(&id);
                    let width = widths_cell.borrow_mut().remove(&id);

                    if let (Some(indicator), Some(width)) = (indicator, width) {
                        let wsc = wsc.clone();
                        width.shrink_to_zero(move || {
                            wsc.remove_child(&indicator);
                        });
                    }
                }

                ids_cell.borrow_mut().retain(|id| new_ids_set.contains(id));
            }
        }
        StructuralChange::Recreate => {
            // Additions (with or without removals) or a pure reorder. Full
            // recreate: new indicators seed at 0 px (grow in); survivors seed at
            // their current animated width so they do not jump.
            if let Some(wsc) = ws_container {
                let mut seeds: HashMap<i32, i32> = HashMap::new();
                for (&id, width) in widths_cell.borrow().iter() {
                    if new_ids_set.contains(&id) {
                        seeds.insert(id, width.current.get().round() as i32);
                    }
                }
                create_indicators(
                    container,
                    Some(wsc),
                    labels_cell,
                    progress_states,
                    widths_cell,
                    ids_cell,
                    label_type,
                    separator,
                    &display_workspaces,
                    &seeds,
                );
            }
        }
    }

    // ── Shared styling tail — update classes and labels before retargeting. ──
    let labels = labels_cell.borrow();
    for workspace in &display_workspaces {
        let Some(indicator) = labels.get(&workspace.id) else {
            continue;
        };

        // Only toggle classes that changed — remove+re-add of the same
        // class causes GTK's style system to return stale measure() values.
        let target_class: Option<&str> = if workspace.active {
            Some(widget::ACTIVE)
        } else if workspace.urgent {
            Some(state::URGENT)
        } else if workspace.occupied {
            Some(state::OCCUPIED)
        } else {
            None
        };

        // State classes are mutually exclusive — only toggle what changed.
        for &cls in &[widget::ACTIVE, state::OCCUPIED, state::URGENT] {
            if Some(cls) == target_class {
                if !indicator.has_css_class(cls) {
                    indicator.add_css_class(cls);
                }
            } else if indicator.has_css_class(cls) {
                indicator.remove_css_class(cls);
            }
        }

        // Update icon/name/index label.
        // The indicator is an Overlay wrapping the content overlay and progress track.
        if let Some(label) = workspace_indicator_label(indicator) {
            match label_type {
                LabelType::Icons => {
                    let text = if workspace.active {
                        ICON_ACTIVE
                    } else if workspace.occupied {
                        ICON_OCCUPIED
                    } else {
                        ICON_EMPTY
                    };
                    set_label_text_if_changed(&label, text);
                }
                LabelType::Name | LabelType::Index => {
                    let label_text = workspace_label_text(label_type, workspace);
                    set_label_text_if_changed(&label, &label_text);
                    let now_long = label_text.chars().count() > 2;
                    let was_long = indicator.has_css_class(widget::WORKSPACE_INDICATOR_LONG);
                    if now_long != was_long {
                        if now_long {
                            indicator.add_css_class(widget::WORKSPACE_INDICATOR_LONG);
                        } else {
                            indicator.remove_css_class(widget::WORKSPACE_INDICATOR_LONG);
                        }
                    }
                }
                LabelType::None => unreachable!(),
            }
        }

        workspace_indicator_progress(progress_states, workspace);

        let tooltip_text = build_tooltip(workspace);
        TooltipManager::global().set_styled_tooltip(indicator, &tooltip_text);
    }

    // ── Drive each indicator's width to its target (active/inactive/long). ──
    // Runs after classes and labels are set so active state and long content
    // width are current. Rust owns the width frame-by-frame from here.
    if ws_container.is_some() {
        retarget_all_widths(&labels, &widths_cell.borrow());
    } else {
        // Non-animated mode: apply resting widths directly. There are no
        // animators and the CSS declares no min-width to fall back on.
        for indicator in labels.values() {
            let is_active = indicator.has_css_class(widget::ACTIVE);
            let target = indicator_target_width(is_active, long_content_width(indicator));
            indicator.set_size_request(target, -1);
        }
    }
    drop(labels);
}

/// Build tooltip text for a workspace.
fn build_tooltip(workspace: &Workspace) -> String {
    let mut parts = Vec::new();

    // Negative indexes mean the compositor has no meaningful numeric label, so
    // show the name without an index prefix.
    let idx_str = workspace.idx.to_string();
    if workspace.idx < 0 {
        parts.push(format!("Workspace {}", workspace.name));
    } else if workspace.name != idx_str {
        parts.push(format!("Workspace {}: {}", workspace.idx, workspace.name));
    } else {
        parts.push(format!("Workspace {}", workspace.name));
    }

    if workspace.active {
        parts.push("Active".to_string());
    } else if workspace.urgent {
        parts.push("Urgent".to_string());
    }

    if let Some(count) = workspace.window_count {
        let windows_str = if count == 1 { "window" } else { "windows" };
        parts.push(format!("{} {}", count, windows_str));
    } else if workspace.occupied {
        parts.push("Has windows".to_string());
    } else {
        parts.push("Empty".to_string());
    }

    parts.join(" • ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::workspace::PerOutputWorkspaces;
    use std::collections::{HashMap, HashSet};
    use toml::Value;

    fn make_widget_entry(name: &str, options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: name.to_string(),
            options,
        }
    }

    #[test]
    fn test_workspace_config_default() {
        let entry = make_widget_entry("workspaces", HashMap::new());
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.label_type, LabelType::None);
        assert_eq!(config.separator, "");
        assert!(config.filter_by_output);
        assert!(!config.show_unoccupied);
    }

    #[test]
    fn test_workspace_config_numbers() {
        let mut options = HashMap::new();
        options.insert(
            "label_type".to_string(),
            Value::String("numbers".to_string()),
        );
        options.insert("separator".to_string(), Value::String("|".to_string()));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.label_type, LabelType::Name);
        assert_eq!(config.separator, "|");
    }

    #[test]
    fn test_workspace_config_name() {
        let mut options = HashMap::new();
        options.insert("label_type".to_string(), Value::String("name".to_string()));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.label_type, LabelType::Name);
    }

    #[test]
    fn test_workspace_config_index() {
        let mut options = HashMap::new();
        options.insert("label_type".to_string(), Value::String("index".to_string()));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.label_type, LabelType::Index);
    }

    #[test]
    fn test_workspace_config_none() {
        let mut options = HashMap::new();
        options.insert("label_type".to_string(), Value::String("none".to_string()));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.label_type, LabelType::None);
    }

    #[test]
    fn test_label_type_from_str() {
        assert_eq!(LabelType::from_str("icons"), LabelType::Icons);
        assert_eq!(LabelType::from_str("ICONS"), LabelType::Icons);
        assert_eq!(LabelType::from_str("name"), LabelType::Name);
        assert_eq!(LabelType::from_str("numbers"), LabelType::Name);
        assert_eq!(LabelType::from_str("index"), LabelType::Index);
        assert_eq!(LabelType::from_str("none"), LabelType::None);
        assert_eq!(LabelType::from_str("unknown"), DEFAULT_LABEL_TYPE);
    }

    #[test]
    fn test_workspace_label_text_name_and_index() {
        let named = make_workspace(4, "Discord", false, true, false, Some(1));
        assert_eq!(workspace_label_text(LabelType::Name, &named), "Discord");
        assert_eq!(workspace_label_text(LabelType::Index, &named), "4");

        let named_without_index = make_workspace(0, "web", false, false, false, Some(0));
        assert_eq!(
            workspace_label_text(LabelType::Index, &named_without_index),
            "0"
        );

        let named_without_index = Workspace {
            idx: -1,
            ..named_without_index
        };
        assert_eq!(
            workspace_label_text(LabelType::Index, &named_without_index),
            "web"
        );
    }

    #[test]
    fn test_workspace_config_animate_default() {
        let entry = make_widget_entry("workspaces", HashMap::new());
        let config = WorkspacesConfig::from_entry(&entry);
        assert!(config.animate.is_none());
    }

    #[test]
    fn test_workspace_config_animate_disabled() {
        let mut options = HashMap::new();
        options.insert("animate".to_string(), Value::Boolean(false));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert_eq!(config.animate, Some(false));
    }

    #[test]
    fn test_workspace_config_filter_by_output_disabled() {
        let mut options = HashMap::new();
        options.insert("filter_by_output".to_string(), Value::Boolean(false));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert!(!config.filter_by_output);
    }

    #[test]
    fn test_workspace_config_show_unoccupied_enabled() {
        let mut options = HashMap::new();
        options.insert("show_unoccupied".to_string(), Value::Boolean(true));
        let entry = make_widget_entry("workspaces", options);
        let config = WorkspacesConfig::from_entry(&entry);
        assert!(config.show_unoccupied);
    }

    #[test]
    fn test_global_display_includes_each_outputs_current_workspace() {
        let active_workspaces = HashSet::from([2]);
        let workspaces = vec![
            make_workspace(2, "2", true, false, false, None),
            make_workspace(4, "4", false, true, false, Some(1)),
            make_workspace(8, "8", false, false, false, None),
        ];
        let snapshot = WorkspaceServiceSnapshot {
            active_workspace: active_workspaces.clone(),
            occupied_workspaces: HashSet::from([4]),
            window_counts: HashMap::from([(4, 1)]),
            workspaces: workspaces.clone(),
            per_output: HashMap::from([
                (
                    "eDP-1".to_string(),
                    PerOutputWorkspaces {
                        active_workspace: HashSet::from([2]),
                        workspaces: vec![],
                    },
                ),
                (
                    "HDMI-A-1".to_string(),
                    PerOutputWorkspaces {
                        active_workspace: HashSet::from([8]),
                        workspaces: vec![],
                    },
                ),
            ]),
        };

        let display_ids =
            collect_display_ids(&workspaces, &active_workspaces, &snapshot, false, true);

        assert!(display_ids.contains(&2));
        assert!(display_ids.contains(&4));
        assert!(display_ids.contains(&8));
        assert_eq!(active_workspaces, HashSet::from([2]));
        assert!(!workspaces.iter().find(|ws| ws.id == 8).unwrap().active);
    }

    #[test]
    fn test_show_unoccupied_includes_reported_empty_workspaces() {
        let active_workspaces = HashSet::from([1]);
        let workspaces = vec![
            make_workspace(1, "1", true, false, false, Some(0)),
            make_workspace(2, "2", false, true, false, Some(1)),
            make_workspace(3, "3", false, false, false, Some(0)),
            make_workspace(4, "4", false, false, false, None),
        ];
        let snapshot = WorkspaceServiceSnapshot {
            active_workspace: active_workspaces.clone(),
            occupied_workspaces: HashSet::from([2]),
            window_counts: HashMap::from([(1, 0), (2, 1), (3, 0)]),
            workspaces: workspaces.clone(),
            per_output: HashMap::new(),
        };

        let default_ids =
            collect_display_ids(&workspaces, &active_workspaces, &snapshot, false, false);
        assert_eq!(default_ids, HashSet::from([1, 2]));

        let show_unoccupied_ids =
            collect_display_ids(&workspaces, &active_workspaces, &snapshot, true, false);
        assert_eq!(show_unoccupied_ids, HashSet::from([1, 2, 3]));
        assert!(!show_unoccupied_ids.contains(&4));
    }

    #[test]
    fn test_workspace_scroll_moves_to_next_visible_workspace() {
        let snapshot = WorkspaceServiceSnapshot {
            active_workspace: HashSet::from([1]),
            occupied_workspaces: HashSet::from([2, 3]),
            window_counts: HashMap::from([(1, 0), (2, 1), (3, 1)]),
            workspaces: vec![
                make_workspace(1, "1", true, false, false, Some(0)),
                make_workspace(2, "2", false, true, false, Some(1)),
                make_workspace(3, "3", false, true, false, Some(1)),
            ],
            per_output: HashMap::new(),
        };

        assert_eq!(
            workspace_id_for_scroll(&snapshot, false, None, 1.0),
            Some(2)
        );
        assert_eq!(workspace_id_for_scroll(&snapshot, false, None, -1.0), None);
    }

    #[test]
    fn test_workspace_scroll_moves_to_previous_visible_workspace() {
        let snapshot = WorkspaceServiceSnapshot {
            active_workspace: HashSet::from([3]),
            occupied_workspaces: HashSet::from([1, 2]),
            window_counts: HashMap::from([(1, 1), (2, 1), (3, 0)]),
            workspaces: vec![
                make_workspace(1, "1", false, true, false, Some(1)),
                make_workspace(2, "2", false, true, false, Some(1)),
                make_workspace(3, "3", true, false, false, Some(0)),
            ],
            per_output: HashMap::new(),
        };

        assert_eq!(
            workspace_id_for_scroll(&snapshot, false, None, -1.0),
            Some(2)
        );
        assert_eq!(workspace_id_for_scroll(&snapshot, false, None, 1.0), None);
    }

    #[test]
    fn test_workspace_scroll_uses_per_output_workspace_state() {
        let mut output_1_ws1 = make_workspace(1, "1", true, false, false, Some(0));
        output_1_ws1.output = Some("eDP-1".to_string());
        let mut output_1_ws2 = make_workspace(2, "2", false, true, false, Some(1));
        output_1_ws2.output = Some("eDP-1".to_string());
        let mut output_2_ws3 = make_workspace(3, "3", false, true, false, Some(1));
        output_2_ws3.output = Some("HDMI-A-1".to_string());

        let snapshot = WorkspaceServiceSnapshot {
            active_workspace: HashSet::from([1]),
            occupied_workspaces: HashSet::from([2, 3]),
            window_counts: HashMap::from([(1, 0), (2, 1), (3, 1)]),
            workspaces: vec![
                output_1_ws1.clone(),
                output_1_ws2.clone(),
                output_2_ws3.clone(),
            ],
            per_output: HashMap::from([
                (
                    "eDP-1".to_string(),
                    PerOutputWorkspaces {
                        active_workspace: HashSet::from([1]),
                        workspaces: vec![output_1_ws1, output_1_ws2],
                    },
                ),
                (
                    "HDMI-A-1".to_string(),
                    PerOutputWorkspaces {
                        active_workspace: HashSet::from([3]),
                        workspaces: vec![output_2_ws3],
                    },
                ),
            ]),
        };

        assert_eq!(
            workspace_id_for_scroll(&snapshot, false, Some("eDP-1"), 1.0),
            Some(2)
        );
        assert_eq!(
            workspace_id_for_scroll(&snapshot, false, Some("HDMI-A-1"), 1.0),
            None
        );
    }

    // -- build_tooltip tests --

    fn make_workspace(
        id: i32,
        name: &str,
        active: bool,
        occupied: bool,
        urgent: bool,
        window_count: Option<u32>,
    ) -> Workspace {
        Workspace {
            id,
            idx: id,
            name: name.to_string(),
            active,
            occupied,
            urgent,
            active_window_progress: None,
            window_count,
            output: None,
        }
    }

    #[test]
    fn test_build_tooltip_active_with_windows() {
        let ws = make_workspace(1, "1", true, true, false, Some(3));
        assert_eq!(build_tooltip(&ws), "Workspace 1 • Active • 3 windows");
    }

    #[test]
    fn test_build_tooltip_active_single_window() {
        let ws = make_workspace(2, "2", true, true, false, Some(1));
        assert_eq!(build_tooltip(&ws), "Workspace 2 • Active • 1 window");
    }

    #[test]
    fn test_build_tooltip_inactive_empty() {
        let ws = make_workspace(3, "3", false, false, false, None);
        assert_eq!(build_tooltip(&ws), "Workspace 3 • Empty");
    }

    #[test]
    fn test_build_tooltip_occupied_no_count() {
        let ws = make_workspace(4, "4", false, true, false, None);
        assert_eq!(build_tooltip(&ws), "Workspace 4 • Has windows");
    }

    #[test]
    fn test_build_tooltip_urgent() {
        let ws = make_workspace(5, "5", false, true, true, Some(2));
        assert_eq!(build_tooltip(&ws), "Workspace 5 • Urgent • 2 windows");
    }

    #[test]
    fn test_build_tooltip_custom_name() {
        let ws = make_workspace(1, "browser", true, true, false, Some(5));
        assert_eq!(
            build_tooltip(&ws),
            "Workspace 1: browser • Active • 5 windows"
        );
    }

    #[test]
    fn test_build_tooltip_named_workspace_hides_negative_id() {
        let ws = make_workspace(-1337, "web", true, true, false, Some(2));
        assert_eq!(build_tooltip(&ws), "Workspace web • Active • 2 windows");
    }

    // -- classify_change tests --

    #[test]
    fn test_classify_no_change() {
        // IDs didn't change at all → no structural work.
        assert_eq!(
            classify_change(false, false, false, true),
            StructuralChange::None
        );
    }

    #[test]
    fn test_classify_no_change_non_animated() {
        // Non-animated mode with unchanged IDs → None.
        assert_eq!(
            classify_change(false, false, false, false),
            StructuralChange::None
        );
    }

    #[test]
    fn test_classify_non_animated_returns_none() {
        // Non-animated mode is always None even when IDs changed: the caller
        // handles its plain recreate separately (no width animators).
        assert_eq!(
            classify_change(true, true, true, false),
            StructuralChange::None
        );
    }

    #[test]
    fn test_classify_removal_only() {
        // IDs changed, removals but no additions → surgical shrink.
        // e.g., [1,2,3] → [1,2].
        assert_eq!(
            classify_change(true, false, true, true),
            StructuralChange::RemovalOnly
        );
    }

    #[test]
    fn test_classify_reorder_recreates() {
        // Same IDs, different order: no adds/removes → Recreate path.
        // e.g., [1,6,2,15] → [1,2,6,15].
        assert_eq!(
            classify_change(true, false, false, true),
            StructuralChange::Recreate
        );
    }

    #[test]
    fn test_classify_swap_recreates() {
        // Same count, different IDs (add + remove) → Recreate.
        // e.g., [1,2,3] → [1,2,4].
        assert_eq!(
            classify_change(true, true, true, true),
            StructuralChange::Recreate
        );
    }

    #[test]
    fn test_classify_addition_recreates() {
        // Pure additions → Recreate. e.g., [1,2] → [1,2,3].
        assert_eq!(
            classify_change(true, true, false, true),
            StructuralChange::Recreate
        );
    }

    #[test]
    fn test_classify_initial_population_recreates() {
        // First workspaces appear (all additions) → Recreate.
        assert_eq!(
            classify_change(true, true, false, true),
            StructuralChange::Recreate
        );
    }

    #[test]
    fn test_classify_add_and_remove_recreates() {
        // Simultaneous add + remove (e.g. [1,2,3] → [1,4]) → Recreate,
        // not RemovalOnly, because there are additions.
        assert_eq!(
            classify_change(true, true, true, true),
            StructuralChange::Recreate
        );
    }

    // -- width retarget tests --

    #[test]
    fn test_width_retarget_skips_identical_target() {
        // Repeated workspace snapshots may update progress metadata while the
        // active/inactive pill widths stay unchanged.
        assert!(!should_retarget_width(28, 28));
    }

    #[test]
    fn test_width_retarget_runs_for_new_target() {
        assert!(should_retarget_width(16, 28));
        assert!(should_retarget_width(28, 16));
    }

    // -- indicator_target_width tests --

    #[test]
    fn test_indicator_target_width_short() {
        assert_eq!(
            indicator_target_width(false, None),
            INDICATOR_INACTIVE_WIDTH_PX
        );
        assert_eq!(
            indicator_target_width(true, None),
            INDICATOR_ACTIVE_WIDTH_PX
        );
    }

    #[test]
    fn test_indicator_target_width_active_inactive_delta_matches_short() {
        // The active/inactive growth of a long indicator equals that of a short
        // indicator, so the pill "feels" consistent regardless of label width.
        let inactive = indicator_target_width(false, Some(40));
        let active = indicator_target_width(true, Some(40));
        assert_eq!(active - inactive, INDICATOR_WIDTH_DELTA);
    }

    #[test]
    fn test_indicator_target_width_long_includes_padding() {
        // Long inactive width = content + 2 * hpad (when above the short floor).
        assert_eq!(
            indicator_target_width(false, Some(40)),
            40 + 2 * LONG_INDICATOR_HPAD
        );
    }

    #[test]
    fn test_indicator_target_width_long_never_below_short() {
        // Even an empty-content long indicator (padding only) is never narrower
        // than the short inactive width, so a pill never collapses below the
        // minimal dot size.
        assert!(indicator_target_width(false, Some(0)) >= INDICATOR_INACTIVE_WIDTH_PX);
    }

    #[test]
    fn test_indicator_target_width_monotonic_in_content() {
        // Wider labels never produce a narrower target.
        let mut prev = indicator_target_width(false, Some(0));
        for content in [1, 5, 10, 50, 100] {
            let cur = indicator_target_width(false, Some(content));
            assert!(cur >= prev, "target not monotonic at content={content}");
            prev = cur;
        }
    }
}
