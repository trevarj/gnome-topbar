//! A container that slides its child in from its own top edge.
//!
//! A banner arrives from behind the bar: it is drawn part-way out of its slot
//! and slides down into it. Like [`ScaleBox`](super::ScaleBox) — and for the
//! same reason — that is done entirely at draw time. The child is always
//! *measured* and *allocated* at full size, so the slot a banner occupies is
//! its final size from the first frame; only where the child is painted inside
//! that slot changes, and changing it costs a `queue_draw` and nothing else.
//!
//! This matters more here than anywhere else in the panel. The banners live on
//! a layer-shell surface that sizes itself to its content, so a container whose
//! *measured* height changed every frame would ask the compositor to
//! reconfigure the surface every frame — and a configure is a round trip. In
//! practice the animation then stalls part-way and leaves a banner permanently
//! half-drawn, which is exactly what the first version of this widget did.

use std::cell::Cell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    pub struct SlideBox {
        /// How much of the child is in its slot, `0.0..=1.0`.
        pub(super) reveal: Cell<f64>,
        /// The single child.
        pub(super) child: glib::WeakRef<gtk4::Widget>,
    }

    impl Default for SlideBox {
        fn default() -> Self {
            Self {
                reveal: Cell::new(1.0),
                child: glib::WeakRef::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SlideBox {
        const NAME: &'static str = "TopbarSlideBox";
        type Type = super::SlideBox;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("slide-box");
        }
    }

    impl ObjectImpl for SlideBox {
        fn constructed(&self) {
            self.parent_constructed();
            // The whole illusion is the clip: without it a child drawn above
            // its slot simply paints over the banner and the bar above it.
            self.obj().set_overflow(gtk4::Overflow::Hidden);
        }

        fn dispose(&self) {
            if let Some(child) = self.child.upgrade() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SlideBox {
        fn request_mode(&self) -> gtk4::SizeRequestMode {
            self.child
                .upgrade()
                .map_or(gtk4::SizeRequestMode::ConstantSize, |child| {
                    child.request_mode()
                })
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // Always the child's own size, whatever the reveal: the slot is
            // its final size from the first frame. See the module docs.
            match self.child.upgrade() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.upgrade() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let Some(child) = self.child.upgrade() else {
                return;
            };
            let reveal = self.reveal.get();
            if reveal <= 0.0 {
                return;
            }

            let widget = self.obj();
            if reveal >= 1.0 {
                widget.snapshot_child(&child, snapshot);
                return;
            }

            // Sitting `(1 - reveal)` of its own height above the slot, clipped
            // to it: the banner appears to come down out of the bar.
            let offset = -(1.0 - reveal) as f32 * widget.height() as f32;
            snapshot.save();
            snapshot.translate(&gtk4::graphene::Point::new(0.0, offset));
            widget.snapshot_child(&child, snapshot);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    /// A one-child container that slides its child down into its own slot.
    ///
    /// The child keeps its full allocation at every reveal; only where it is
    /// painted changes. See the module docs for why that is not negotiable on
    /// a layer surface.
    pub struct SlideBox(ObjectSubclass<imp::SlideBox>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for SlideBox {
    fn default() -> Self {
        Self::new()
    }
}

impl SlideBox {
    /// Create a box with nothing in it, fully revealed.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Set how much of the child is in its slot, clamped to `0.0..=1.0`.
    pub fn set_reveal(&self, reveal: f64) {
        let imp = self.imp();
        let reveal = reveal.clamp(0.0, 1.0);
        if (imp.reveal.get() - reveal).abs() < f64::EPSILON {
            return;
        }
        imp.reveal.set(reveal);
        self.queue_draw();
    }

    /// Parent `child`, replacing whatever was there.
    pub fn set_child(&self, child: &impl IsA<gtk4::Widget>) {
        let imp = self.imp();
        if let Some(previous) = imp.child.upgrade() {
            previous.unparent();
        }
        let child = child.as_ref();
        child.set_parent(self);
        imp.child.set(Some(child));
        self.queue_resize();
    }
}
