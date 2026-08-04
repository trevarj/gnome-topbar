//! The capsule's progress bar.
//!
//! A widget of its own rather than a `gtk4::Scale` with the handle styled away.
//! Three reasons, in order of how much they cost to get wrong:
//!
//! 1. **The overdrive tint.** Above 100% the remainder of the fill is painted
//!    in the urgent colour, and no CSS a `Scale` understands can express "this
//!    part of the trough, in another colour".
//! 2. **The retarget.** An OSD already on screen slides its fill to the new
//!    value; a `Scale` animated through `set_value` would be a property
//!    animation on a widget that relayouts, and this is `queue_draw` alone.
//! 3. **The measured size never moves.** The capsule sits on a layer surface,
//!    and a layer surface that changes size mid-animation is a round trip to
//!    the compositor per frame — the same trap [`SlideBox`](crate::anim::SlideBox)
//!    documents.

use std::cell::{Cell, OnceCell};

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene, gsk};

use crate::anim::{Animation, AnimationParams, Easing};
use crate::style::classes;

/// How long the fill takes to slide to a new value.
pub const RETARGET_MS: u64 = 100;
/// Thickness of the bar, in pixels.
const THICKNESS: f32 = 8.0;
/// Alpha of the unfilled part, against the widget's own foreground colour.
const TRACK_ALPHA: f32 = 0.25;

/// Colours the bar paints itself with.
///
/// The foreground comes from CSS (the widget reads its own `color`); these two
/// cannot, for the same reason the workspace strip's cannot: GTK4 offers no
/// supported way to read a custom property from Rust, and duplicating the
/// palette in a constant would let the two drift.
#[derive(Debug, Clone, Copy)]
pub struct BarColors {
    /// The fill, up to 100%.
    pub accent: gdk::RGBA,
    /// The fill past 100%, which only overdrive can reach.
    pub urgent: gdk::RGBA,
}

impl Default for BarColors {
    fn default() -> Self {
        Self {
            accent: gdk::RGBA::WHITE,
            urgent: gdk::RGBA::RED,
        }
    }
}

mod imp {
    use super::*;

    pub struct OsdBar {
        /// Where the fill is drawn right now, `0.0..=1.0`.
        pub(super) fraction: Cell<f64>,
        /// Fraction at which the urgent tint takes over; `1.0` means never.
        pub(super) overdrive_at: Cell<f64>,
        pub(super) colors: Cell<BarColors>,
        pub(super) orientation: Cell<gtk4::Orientation>,
        /// Length along the bar's own axis, in pixels.
        pub(super) length: Cell<i32>,
        pub(super) slide: OnceCell<Animation>,
    }

    impl Default for OsdBar {
        fn default() -> Self {
            Self {
                fraction: Cell::new(0.0),
                overdrive_at: Cell::new(1.0),
                colors: Cell::new(BarColors::default()),
                orientation: Cell::new(gtk4::Orientation::Horizontal),
                length: Cell::new(148),
                slide: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OsdBar {
        const NAME: &'static str = "TopbarOsdBar";
        type Type = super::OsdBar;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name(classes::OSD_BAR);
        }
    }

    impl ObjectImpl for OsdBar {
        fn constructed(&self) {
            self.parent_constructed();
            let _ = self.slide.set(Animation::new(&*self.obj()));
        }
    }

    impl WidgetImpl for OsdBar {
        /// The same size whatever the fill is. See the module docs.
        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let along = self.orientation.get() == orientation;
            let size = if along {
                self.length.get()
            } else {
                THICKNESS as i32
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            if width <= 0.0 || height <= 0.0 {
                return;
            }

            let vertical = self.orientation.get() == gtk4::Orientation::Vertical;
            let thickness = if vertical { width } else { height };
            let radius = thickness / 2.0;
            let colors = self.colors.get();
            let foreground = widget.color();
            let track = gdk::RGBA::new(
                foreground.red(),
                foreground.green(),
                foreground.blue(),
                foreground.alpha() * TRACK_ALPHA,
            );

            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, radius));
            snapshot.append_color(&track, &bounds);

            let fraction = self.fraction.get().clamp(0.0, 1.0) as f32;
            let over = self.overdrive_at.get().clamp(0.0, 1.0) as f32;
            // Up to the overdrive point in the accent, past it in the urgent
            // colour. With no overdrive allowed the second segment is empty and
            // the branch costs one comparison.
            let normal = fraction.min(over);
            paint(
                snapshot,
                colors.accent,
                0.0,
                normal,
                width,
                height,
                vertical,
            );
            if fraction > over {
                paint(
                    snapshot,
                    colors.urgent,
                    over,
                    fraction,
                    width,
                    height,
                    vertical,
                );
            }
            snapshot.pop();
        }
    }

    /// Paint the `from..to` slice of the bar in `color`.
    ///
    /// A vertical bar fills from the bottom, which is where "more" is.
    fn paint(
        snapshot: &gtk4::Snapshot,
        color: gdk::RGBA,
        from: f32,
        to: f32,
        width: f32,
        height: f32,
        vertical: bool,
    ) {
        if to <= from {
            return;
        }
        let rect = if vertical {
            graphene::Rect::new(0.0, height * (1.0 - to), width, height * (to - from))
        } else {
            graphene::Rect::new(width * from, 0.0, width * (to - from), height)
        };
        snapshot.append_color(&color, &rect);
    }
}

glib::wrapper! {
    /// The capsule's fill, drawn rather than laid out.
    pub struct OsdBar(ObjectSubclass<imp::OsdBar>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for OsdBar {
    fn default() -> Self {
        Self::new()
    }
}

impl OsdBar {
    /// An empty bar.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Lay the bar out along `orientation`, `length` pixels long.
    pub fn set_axis(&self, orientation: gtk4::Orientation, length: i32) {
        let imp = self.imp();
        imp.orientation.set(orientation);
        imp.length.set(length.max(1));
        self.queue_resize();
    }

    /// Set the colours the fill is painted in.
    pub fn set_colors(&self, colors: BarColors) {
        self.imp().colors.set(colors);
        self.queue_draw();
    }

    /// Show `value` out of `max`, sliding there from wherever the fill is.
    ///
    /// `overdrive_from` is the value past which the fill turns urgent — 100 in
    /// practice, and never reached at all unless `audio.allow_overdrive` put
    /// `max` above it.
    pub fn set_value(&self, value: u32, max: u32, overdrive_from: u32) {
        let max = f64::from(max.max(1));
        let target = (f64::from(value) / max).clamp(0.0, 1.0);
        self.imp()
            .overdrive_at
            .set((f64::from(overdrive_from) / max).clamp(0.0, 1.0));

        let start = self.imp().fraction.get();
        if (target - start).abs() < f64::EPSILON {
            self.queue_draw();
            return;
        }

        let this = self.clone();
        let Some(slide) = self.imp().slide.get() else {
            return;
        };
        slide.start(
            AnimationParams::new(RETARGET_MS).with_easing(Easing::EaseOutCubic),
            Box::new(move |progress| {
                this.imp().fraction.set(start + (target - start) * progress);
                this.queue_draw();
            }),
            None,
        );
    }

    /// Put the fill where it belongs with no motion at all.
    ///
    /// Used when the capsule is being raised from nothing: sliding up from
    /// zero on every appearance would read as a loading bar rather than as a
    /// value.
    pub fn jump_to(&self, value: u32, max: u32, overdrive_from: u32) {
        let imp = self.imp();
        if let Some(slide) = imp.slide.get() {
            slide.cancel();
        }
        let max = f64::from(max.max(1));
        imp.fraction.set((f64::from(value) / max).clamp(0.0, 1.0));
        imp.overdrive_at
            .set((f64::from(overdrive_from) / max).clamp(0.0, 1.0));
        self.queue_draw();
    }

    /// Where the fill is drawn right now, for the tests.
    #[cfg(test)]
    pub fn fraction(&self) -> f64 {
        self.imp().fraction.get()
    }

    /// Where the urgent tint starts, for the tests.
    #[cfg(test)]
    pub fn overdrive_at(&self) -> f64 {
        self.imp().overdrive_at.get()
    }
}
