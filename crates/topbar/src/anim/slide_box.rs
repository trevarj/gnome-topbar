//! A container that reveals its child by sliding it down out of the edge.
//!
//! A toast arrives from behind the bar and the banners under it move down to
//! make room; when one goes, they close the gap. Both halves of that are the
//! same thing: a box whose *measured* height is a fraction of its child's, with
//! the child pinned to the bottom of what is left, so the visible part grows
//! downwards while everything below it is pushed by the ordinary layout.
//!
//! The child is always allocated its full natural height — nothing inside it
//! reflows mid-animation, so a wrapped body never re-wraps as the banner grows
//! — and `overflow: hidden` clips the part that has not arrived yet.
//!
//! Unlike [`ScaleBox`](super::ScaleBox) this does queue a resize per frame, and
//! it has to: making room for a banner *is* a layout change. The cost is
//! bounded by there being at most three of them, on a surface of their own.

use std::cell::Cell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    pub struct SlideBox {
        /// How much of the child is on screen, `0.0..=1.0`.
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
        const NAME: &'static str = "GnomeTopbarSlideBox";
        type Type = super::SlideBox;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("slide-box");
        }
    }

    impl ObjectImpl for SlideBox {
        fn constructed(&self) {
            self.parent_constructed();
            // The whole illusion is the clip: without it the child simply
            // draws over the banner above.
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
            gtk4::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk4::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.upgrade() else {
                return (0, 0, -1, -1);
            };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            if orientation == gtk4::Orientation::Horizontal {
                return (min, natural, -1, -1);
            }
            // Baselines are dropped on purpose: a baseline halfway through a
            // slide would drag the banners either side of it around.
            let revealed = self.revealed(natural);
            (revealed.min(min), revealed, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.child.upgrade() else {
                return;
            };
            let (_, natural, _, _) = child.measure(gtk4::Orientation::Vertical, width);
            // Pin the child to the bottom edge: as `height` grows from 0 the
            // child slides down into view rather than being squashed.
            let offset = (height - natural) as f32;
            let transform =
                gtk4::gsk::Transform::new().translate(&gtk4::graphene::Point::new(0.0, offset));
            child.allocate(width, natural, -1, Some(transform));
        }
    }

    impl SlideBox {
        /// How much of `natural` is on screen right now.
        fn revealed(&self, natural: i32) -> i32 {
            (f64::from(natural) * self.reveal.get()).round() as i32
        }
    }
}

glib::wrapper! {
    /// A one-child container that slides its child in from the top edge.
    ///
    /// See the module docs for why this reallocates and [`ScaleBox`] does not.
    ///
    /// [`ScaleBox`]: super::ScaleBox
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

    /// Set how much of the child is on screen, clamped to `0.0..=1.0`.
    pub fn set_reveal(&self, reveal: f64) {
        let imp = self.imp();
        let reveal = reveal.clamp(0.0, 1.0);
        if (imp.reveal.get() - reveal).abs() < f64::EPSILON {
            return;
        }
        imp.reveal.set(reveal);
        // A resize, not a redraw: the point of this widget is that the
        // banners below it move.
        self.queue_resize();
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
