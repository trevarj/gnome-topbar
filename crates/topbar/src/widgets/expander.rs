//! Expandable sections, one open at a time.
//!
//! The panel lives on a layer surface that sizes itself to its content, and a
//! layer surface's size is negotiated with the compositor: every change is a
//! configure and a round trip. A container whose *measured* height moved every
//! frame would therefore ask for sixty configures a second, and what actually
//! happens then is that the animation stalls part-way and leaves the surface
//! stuck at a size nobody asked for. The panel's banners hit exactly this in
//! M4 (see [`SlideBox`](crate::anim::SlideBox)).
//!
//! So the height changes **once** per toggle — the section is shown or hidden,
//! the compositor is told once — and what animates is where the content is
//! *painted* inside the slot it already has. Opening looks like the rows
//! sliding down out of the row above them, which is what GNOME's own
//! expanders look like, and it costs one `queue_draw` a frame.
//!
//! Only one section is open at a time. Opening a second closes the first
//! outright rather than animating it shut: two surface heights changing in one
//! frame is the one case where the compositor can be asked for a size that
//! matches neither.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk4::prelude::*;

use crate::anim::{Animation, AnimationParams, Easing, SlideBox, motion_enabled};
use crate::style::classes;

/// How long a section takes to slide open.
pub const REVEAL_MS: u64 = 200;
/// How long a row that comes and goes on its own takes.
///
/// Shorter than a section the user opened: the microphone slider appears
/// because something started recording, and a control arriving under the
/// pointer should be quick about it.
pub const ROW_REVEAL_MS: u64 = 150;

/// One expandable section: a slot, and whether it is open.
pub struct Section {
    slot: SlideBox,
    anim: Animation,
    expanded: Cell<bool>,
    duration_ms: u64,
}

impl Section {
    /// Wrap `content` in a section, closed.
    pub fn new(content: &impl IsA<gtk4::Widget>) -> Rc<Self> {
        Self::with_duration(content, REVEAL_MS)
    }

    /// The same, at a duration of the caller's choosing.
    pub fn with_duration(content: &impl IsA<gtk4::Widget>, duration_ms: u64) -> Rc<Self> {
        let slot = SlideBox::new();
        slot.add_css_class(classes::SECTION);
        slot.set_child(content);
        slot.set_reveal(0.0);
        // Hidden rather than zero-height: an invisible child is not measured,
        // which is what keeps a closed section out of the panel's height.
        slot.set_visible(false);

        Rc::new(Self {
            anim: Animation::new(&slot),
            slot,
            expanded: Cell::new(false),
            duration_ms,
        })
    }

    /// The widget to put in the panel, under the row that opens it.
    pub fn root(&self) -> &SlideBox {
        &self.slot
    }

    /// Whether it is open.
    pub fn is_expanded(&self) -> bool {
        self.expanded.get()
    }

    /// Open or close it, sliding.
    pub fn set_expanded(self: &Rc<Self>, expanded: bool) {
        if self.expanded.get() == expanded {
            return;
        }
        self.expanded.set(expanded);

        if !motion_enabled() {
            self.anim.cancel();
            self.slot.set_reveal(if expanded { 1.0 } else { 0.0 });
            self.slot.set_visible(expanded);
            return;
        }

        if expanded {
            // The height arrives first, in one configure; the content then
            // slides down into the space that is already there.
            self.slot.set_visible(true);
            self.animate(1.0);
        } else {
            self.animate(0.0);
        }
    }

    /// Close it with no animation at all.
    ///
    /// What happens to the section that was open when another is opened: it
    /// gives its height back in the same frame the new one takes it, so the
    /// compositor sees one configure rather than two fighting.
    pub fn collapse_now(&self) {
        if !self.expanded.get() {
            return;
        }
        self.expanded.set(false);
        self.anim.cancel();
        self.slot.set_reveal(0.0);
        self.slot.set_visible(false);
    }

    /// Run the reveal toward `target`.
    fn animate(self: &Rc<Self>, target: f64) {
        let from = self.slot.reveal();
        let distance = (target - from).abs();
        if distance <= f64::EPSILON {
            return;
        }
        let duration = (self.duration_ms as f64 * distance).round() as u64;

        let easing = if target > from {
            Easing::EaseOutCubic
        } else {
            Easing::EaseInCubic
        };

        let on_frame = {
            let slot = self.slot.clone();
            move |progress: f64| slot.set_reveal(from + (target - from) * progress)
        };
        let on_done = {
            let section = Rc::downgrade(self);
            move || {
                if let Some(section) = section.upgrade() {
                    section.slot.set_reveal(target);
                    // The height goes back only once the content has finished
                    // leaving, so the panel does not jump ahead of the slide.
                    section.slot.set_visible(target > 0.0);
                }
            }
        };

        self.anim.start(
            AnimationParams::new(duration).with_easing(easing),
            Box::new(on_frame),
            Some(Box::new(on_done)),
        );
    }
}

/// Every section in one panel, enforcing that at most one is open.
#[derive(Default)]
pub struct Accordion {
    sections: std::cell::RefCell<Vec<Weak<Section>>>,
}

impl Accordion {
    /// An accordion with nothing in it.
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    /// Add a section to the group.
    pub fn add(&self, section: &Rc<Section>) {
        self.sections.borrow_mut().push(Rc::downgrade(section));
    }

    /// Flip `section`, closing whatever else was open.
    pub fn toggle(&self, section: &Rc<Section>) {
        if section.is_expanded() {
            section.set_expanded(false);
            return;
        }
        self.collapse_others(section);
        section.set_expanded(true);
    }

    /// Close everything.
    ///
    /// Runs when the panel closes, so reopening it does not show whichever
    /// card the user happened to leave open a day ago.
    pub fn collapse_all(&self) {
        for section in self.live() {
            section.collapse_now();
        }
    }

    /// Close everything except `keep`.
    fn collapse_others(&self, keep: &Rc<Section>) {
        for section in self.live() {
            if !Rc::ptr_eq(&section, keep) {
                section.collapse_now();
            }
        }
    }

    /// Whichever sections still exist.
    fn live(&self) -> Vec<Rc<Section>> {
        let mut sections = self.sections.borrow_mut();
        sections.retain(|section| section.strong_count() > 0);
        sections.iter().filter_map(Weak::upgrade).collect()
    }
}
