//! A container that turns its child around its own centre.
//!
//! The expander chevrons: a section opening turns its arrow from pointing down
//! to pointing up, over the same 200ms the section itself takes, so the arrow
//! reads as the thing that opened it rather than as a second icon that swapped
//! in behind the user's back.
//!
//! Like [`ScaleBox`](super::ScaleBox) and [`SlideBox`](super::SlideBox), the
//! rotation happens at draw time only: the child is always measured and
//! allocated at full size, so an angle change costs a `queue_draw` and never a
//! relayout. Angles are set from Rust — there is no animation in here — which
//! keeps every duration in the panel in one place, next to the motion it
//! belongs to.

use std::cell::Cell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RotateBox {
        /// Clockwise angle in degrees.
        pub(super) angle: Cell<f32>,
        /// The single child.
        pub(super) child: glib::WeakRef<gtk4::Widget>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RotateBox {
        const NAME: &'static str = "TopbarRotateBox";
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
            self.child
                .upgrade()
                .map_or(gtk4::SizeRequestMode::ConstantSize, |child| {
                    child.request_mode()
                })
        }

        /// The child's own size, at every angle.
        ///
        /// A chevron is square and a quarter turn of a square is the same
        /// square; anything else would make a row change height as it turned.
        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
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
            let widget = self.obj();
            let angle = self.angle.get();
            if angle == 0.0 {
                widget.snapshot_child(&child, snapshot);
                return;
            }

            // Around the centre, not the origin: GTK rotates about the current
            // transform's origin, so the widget is moved under its own middle
            // first and put back afterwards.
            let centre = gtk4::graphene::Point::new(
                widget.width() as f32 / 2.0,
                widget.height() as f32 / 2.0,
            );
            snapshot.save();
            snapshot.translate(&centre);
            snapshot.rotate(angle);
            snapshot.translate(&gtk4::graphene::Point::new(-centre.x(), -centre.y()));
            widget.snapshot_child(&child, snapshot);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    /// A widget that draws its child turned by an angle.
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
    /// Create a box with nothing in it, at rest.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// The angle the child is currently drawn at, in degrees.
    ///
    /// Read by a rotation that is reversing, so a section closed halfway
    /// through opening turns back from where the arrow got to.
    pub fn angle(&self) -> f32 {
        self.imp().angle.get()
    }

    /// Draw the child turned `angle` degrees clockwise.
    pub fn set_angle(&self, angle: f32) {
        let imp = self.imp();
        if (imp.angle.get() - angle).abs() < f32::EPSILON {
            return;
        }
        imp.angle.set(angle);
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
