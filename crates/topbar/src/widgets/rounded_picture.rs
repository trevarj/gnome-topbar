//! A paintable drawn to fill a square, with rounded corners and a crossfade.
//!
//! Ported from v1 `widgets/rounded_picture.rs`, which used GSK's
//! `push_rounded_clip()` to round album art on the GPU rather than by touching
//! pixels. v2 adds the crossfade the media card needs, and it is drawn the
//! same way everything else in the panel is: two paintables and one number,
//! resolved in `snapshot()`. Changing that number costs a `queue_draw` and
//! nothing else — no relayout, no CSS resolution, no new render surfaces.

use std::cell::{Cell, RefCell};

use gtk4::gdk::Paintable;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{glib, graphene, gsk};

/// Draw `paintable` filling `width` x `height`, cropping the overflow.
///
/// Album art is square but not everything a player hands over is: a wide
/// cover is centred and its sides are cut off, which is what "fill" means
/// everywhere else in the desktop.
fn snapshot_filling(
    paintable: &Paintable,
    snapshot: &gtk4::Snapshot,
    width: f32,
    height: f32,
    opacity: f64,
) {
    let intrinsic_width = paintable.intrinsic_width();
    let intrinsic_height = paintable.intrinsic_height();

    snapshot.push_opacity(opacity);
    if intrinsic_width <= 0 || intrinsic_height <= 0 {
        paintable.snapshot(snapshot, f64::from(width), f64::from(height));
        snapshot.pop();
        return;
    }

    let scale = (width / intrinsic_width as f32).max(height / intrinsic_height as f32);
    let draw_width = intrinsic_width as f32 * scale;
    let draw_height = intrinsic_height as f32 * scale;

    snapshot.save();
    snapshot.translate(&graphene::Point::new(
        (width - draw_width) / 2.0,
        (height - draw_height) / 2.0,
    ));
    paintable.snapshot(snapshot, f64::from(draw_width), f64::from(draw_height));
    snapshot.restore();
    snapshot.pop();
}

mod imp {
    use super::*;

    pub struct RoundedPicture {
        /// What is being shown.
        pub(super) paintable: RefCell<Option<Paintable>>,
        /// What was being shown, while it fades out.
        pub(super) previous: RefCell<Option<Paintable>>,
        /// How far the crossfade has got: 1.0 is "only the new one".
        pub(super) fade: Cell<f64>,
        /// Corner radius, in pixels.
        pub(super) radius: Cell<f32>,
        /// The square the picture occupies.
        pub(super) pixel_size: Cell<i32>,
    }

    impl Default for RoundedPicture {
        fn default() -> Self {
            Self {
                paintable: RefCell::new(None),
                previous: RefCell::new(None),
                fade: Cell::new(1.0),
                radius: Cell::default(),
                pixel_size: Cell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoundedPicture {
        const NAME: &'static str = "TopbarRoundedPicture";
        type Type = super::RoundedPicture;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("rounded-picture");
        }
    }

    impl ObjectImpl for RoundedPicture {
        fn constructed(&self) {
            self.parent_constructed();
            // A picture of a fixed size never stretches to fill whatever it
            // was put inside.
            let obj = self.obj();
            obj.set_hexpand(false);
            obj.set_vexpand(false);
            obj.set_halign(gtk4::Align::Center);
            obj.set_valign(gtk4::Align::Center);
        }
    }

    impl WidgetImpl for RoundedPicture {
        fn measure(&self, _orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // Always the requested square, whether or not there is anything to
            // draw: art arriving must never change the size of the card.
            let size = self.pixel_size.get().max(0);
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            let width = widget.width() as f32;
            let height = widget.height() as f32;
            if width <= 0.0 || height <= 0.0 {
                return;
            }

            let current = self.paintable.borrow().clone();
            let previous = self.previous.borrow().clone();
            if current.is_none() && previous.is_none() {
                return;
            }

            let radius = self.radius.get().min(width / 2.0).min(height / 2.0);
            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            if radius > 0.0 {
                let corner = graphene::Size::new(radius, radius);
                snapshot.push_rounded_clip(&gsk::RoundedRect::new(
                    bounds, corner, corner, corner, corner,
                ));
            } else {
                snapshot.push_clip(&bounds);
            }

            let fade = self.fade.get().clamp(0.0, 1.0);
            if let Some(previous) = previous.filter(|_| fade < 1.0) {
                snapshot_filling(&previous, snapshot, width, height, 1.0 - fade);
            }
            if let Some(current) = current {
                snapshot_filling(&current, snapshot, width, height, fade);
            }

            snapshot.pop();
        }
    }
}

glib::wrapper! {
    /// A square, rounded picture that crossfades when its content changes.
    pub struct RoundedPicture(ObjectSubclass<imp::RoundedPicture>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl RoundedPicture {
    /// A picture `size` pixels square with a `radius` pixel corner.
    pub fn new(size: i32, radius: f32) -> Self {
        let picture: Self = glib::Object::new();
        picture.imp().pixel_size.set(size);
        picture.imp().radius.set(radius);
        picture.set_size_request(size, size);
        picture
    }

    /// Start a crossfade to `paintable`, driving it with [`Self::set_fade`].
    ///
    /// There is deliberately no instant setter: a run with motion switched off
    /// jumps straight to `set_fade(1.0)` (see [`crate::anim::Animation`]), so
    /// this is the instant path too.
    ///
    /// The outgoing picture is held until the fade finishes, which is the only
    /// thing this widget retains: one extra texture for 150ms.
    pub fn crossfade_to(&self, paintable: Option<&impl IsA<Paintable>>) {
        let paintable = paintable.map(|paintable| paintable.as_ref().clone());
        let outgoing = self.imp().paintable.replace(paintable);
        self.imp().previous.replace(outgoing);
        self.imp().fade.set(0.0);
        self.queue_draw();
    }

    /// Set how far a crossfade has got, `0.0..=1.0`.
    pub fn set_fade(&self, fade: f64) {
        if (self.imp().fade.get() - fade).abs() < f64::EPSILON {
            return;
        }
        self.imp().fade.set(fade);
        if fade >= 1.0 {
            // Nothing left to fade from; let the old texture go.
            self.imp().previous.replace(None);
        }
        self.queue_draw();
    }
}
