//! A container that rotates its single child around its center in `snapshot()`.
//!
//! Used for the Quick Settings expander chevron and row-menu icon, whose
//! rotation on expand/collapse was previously a CSS `transform: rotate()` with a
//! `transition`. All animation in this crate is now frame-clock driven from Rust
//! (see [`crate::widgets::animation`]); `RotateBox` exposes a plain `angle`
//! property that the [`Animation`](crate::widgets::animation::Animation) helper
//! drives per-frame.
//!
//! The child is always allocated at full size — the rotation is purely visual,
//! applied via `Snapshot::rotate` around the widget's center. Unlike the
//! continuous scale transforms that caused GTK4 memory growth (see
//! [`crate::widgets::scale_box`]), this wraps a tiny static icon glyph and only
//! emits a transform node while the short rotation is in flight, so there is no
//! unbounded node accumulation. Only `queue_draw()` is called on angle changes —
//! never relayout or CSS resolution.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};

use crate::widgets::animation::{Animation, AnimationParams, Easing};

/// Duration of the chevron rotation, matching the former CSS
/// `transition: transform 225ms ease`.
const ROTATE_DURATION_MS: u64 = 225;

mod imp {
    use super::*;

    pub struct RotateBox {
        /// Current rotation angle in degrees (clockwise).
        pub(super) angle: Cell<f32>,
        /// The single child widget.
        pub(super) child: glib::WeakRef<gtk4::Widget>,
        /// Frame-clock animation driving `angle` on expand/collapse.
        pub(super) animation: RefCell<Option<Animation>>,
        /// Target angle when expanded (collapsed target is always 0).
        pub(super) expanded_angle: Cell<f32>,
    }

    impl Default for RotateBox {
        fn default() -> Self {
            Self {
                angle: Cell::new(0.0),
                child: glib::WeakRef::new(),
                animation: RefCell::new(None),
                expanded_angle: Cell::new(0.0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RotateBox {
        const NAME: &'static str = "GnomePanelRotateBox";
        type Type = super::RotateBox;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("rotate-box");
        }
    }

    impl ObjectImpl for RotateBox {
        fn dispose(&self) {
            if let Some(child) = self.child.upgrade() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for RotateBox {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            if let Some(child) = self.child.upgrade() {
                child.request_mode()
            } else {
                gtk4::SizeRequestMode::ConstantSize
            }
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(child) = self.child.upgrade() {
                child.measure(orientation, for_size)
            } else {
                (0, 0, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // Full allocation — rotation is purely visual via snapshot().
            if let Some(child) = self.child.upgrade() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let Some(child) = self.child.upgrade() else {
                return;
            };

            let widget = self.obj();
            let angle = self.angle.get();

            // Fast path: no rotation, no transform node.
            if angle.abs() < f32::EPSILON {
                widget.snapshot_child(&child, snapshot);
                return;
            }

            // Rotate around the widget center: translate to center, rotate,
            // translate back. The transform node is rebuilt each snapshot (not
            // retained), so nothing accumulates across frames.
            let cx = widget.width() as f32 / 2.0;
            let cy = widget.height() as f32 / 2.0;
            snapshot.translate(&gtk4::graphene::Point::new(cx, cy));
            snapshot.rotate(angle);
            snapshot.translate(&gtk4::graphene::Point::new(-cx, -cy));
            widget.snapshot_child(&child, snapshot);
        }
    }
}

glib::wrapper! {
    /// A container that rotates its single child around its center in
    /// `snapshot()`. The child always receives full allocation; rotation is
    /// purely visual and driven from Rust via [`RotateBox::set_angle`].
    pub struct RotateBox(ObjectSubclass<imp::RotateBox>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for RotateBox {
    fn default() -> Self {
        Self::new()
    }
}

impl RotateBox {
    /// Create a new `RotateBox` at angle 0.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Create a `RotateBox` that rotates between `0` and `expanded_angle`
    /// degrees when toggled via [`RotateBox::set_expanded`].
    pub fn with_expanded_angle(expanded_angle: f32) -> Self {
        let obj = Self::new();
        obj.imp().expanded_angle.set(expanded_angle);
        obj
    }

    /// Animate to the expanded (full angle) or collapsed (0°) state.
    ///
    /// Uses the shared frame-clock [`Animation`] helper, so it honors the
    /// global `theme.animations` toggle (when disabled it jumps straight to the
    /// final angle). Re-toggling mid-flight cleanly reverses from the current
    /// angle without glitching.
    pub fn set_expanded(&self, expanded: bool) {
        let target = if expanded {
            self.imp().expanded_angle.get()
        } else {
            0.0
        };

        let anim = self
            .imp()
            .animation
            .borrow_mut()
            .get_or_insert_with(|| Animation::new(self))
            .clone();

        let start = self.angle();
        let this = self.clone();
        anim.start(
            AnimationParams::new(ROTATE_DURATION_MS).with_easing(Easing::EaseOutCubic),
            Box::new(move |t| {
                let angle = start + (target - start) * t as f32;
                this.set_angle(angle);
            }),
            None,
        );
    }

    /// Current rotation angle in degrees (clockwise).
    pub fn angle(&self) -> f32 {
        self.imp().angle.get()
    }

    /// Set the rotation angle in degrees and queue a repaint.
    ///
    /// Only triggers `queue_draw()` (no relayout) when the angle changes.
    pub fn set_angle(&self, angle: f32) {
        let imp = self.imp();
        if (imp.angle.get() - angle).abs() < f32::EPSILON {
            return;
        }
        imp.angle.set(angle);
        self.queue_draw();
    }

    /// Set the single child widget, replacing any previous child.
    pub fn set_child(&self, child: &impl IsA<gtk4::Widget>) {
        let imp = self.imp();
        if let Some(old) = imp.child.upgrade() {
            old.unparent();
        }
        let widget = child.as_ref();
        widget.set_parent(self);
        imp.child.set(Some(widget));
    }
}
