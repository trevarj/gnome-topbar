//! Center-priority layout: the bar's three sections and how they share space.
//!
//! Ported from v1 `sectioned_bar.rs`. [`SectionedBar`] holds one child per
//! section and delegates placement to [`CenterPriorityLayout`], a
//! [`gtk4::LayoutManager`] subclass that:
//!
//! 1. anchors the center section to the bar's true center,
//! 2. hands each side section whatever space is left as its budget, and
//! 3. shrinks the sides — never the center — when space runs out.
//!
//! All the arithmetic lives in [`topbar_core::layout_math`], which is where
//! its tests live too; this module is the GTK shell around it.

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{LayoutManager, Orientation, Widget, glib};
use topbar_core::layout_math::{
    SectionSizes, compute_center_priority_allocation, compute_linear_allocation,
};

use crate::style::classes;

/// One of the bar's three widget sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Left-anchored section.
    Left,
    /// Center-anchored section; it keeps its natural width the longest.
    Center,
    /// Right-anchored section.
    Right,
}

impl Section {
    /// Every section, in painting order.
    pub const ALL: [Section; 3] = [Section::Left, Section::Center, Section::Right];

    /// The CSS class the bar puts on this section's box.
    pub fn css_class(self) -> &'static str {
        match self {
            Section::Left => classes::SECTION_LEFT,
            Section::Center => classes::SECTION_CENTER,
            Section::Right => classes::SECTION_RIGHT,
        }
    }
}

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(Default)]
    pub struct CenterPriorityLayout {
        pub spacing: Cell<i32>,
        pub edge_margin: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CenterPriorityLayout {
        const NAME: &'static str = "TopbarCenterPriorityLayout";
        type Type = super::CenterPriorityLayout;
        type ParentType = LayoutManager;
    }

    impl ObjectImpl for CenterPriorityLayout {}

    impl LayoutManagerImpl for CenterPriorityLayout {
        fn request_mode(&self, _widget: &Widget) -> gtk4::SizeRequestMode {
            gtk4::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &Widget,
            orientation: Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let Some(bar) = widget.downcast_ref::<super::SectionedBar>() else {
                return (0, 0, -1, -1);
            };
            let sections = bar.visible_sections();

            if orientation == Orientation::Vertical {
                let mut minimum = 0;
                let mut natural = 0;
                for child in sections.into_iter().flatten() {
                    let (min, nat, _, _) = child.measure(Orientation::Vertical, -1);
                    minimum = minimum.max(min);
                    natural = natural.max(nat);
                }
                return (minimum, natural, -1, -1);
            }

            let edge = self.edge_margin.get();
            let mut minimum = edge * 2;
            let mut natural = edge * 2;
            let mut count = 0;
            for child in sections.into_iter().flatten() {
                let (min, nat, _, _) = child.measure(Orientation::Horizontal, -1);
                minimum += min;
                natural += nat;
                count += 1;
            }

            let spacing_total = self.spacing.get() * (count - 1).max(0);
            (minimum + spacing_total, natural + spacing_total, -1, -1)
        }

        /// Place the three sections inside `width`.
        ///
        /// The center section is positioned first, at the true center of the
        /// interior (the allocation minus the edge margin on both sides). What
        /// remains on each side becomes that side's budget, and each side gets
        /// its natural width if it fits or shrinks toward its minimum if it
        /// does not. With no center section the sides share the interior
        /// linearly instead.
        fn allocate(&self, widget: &Widget, width: i32, height: i32, _baseline: i32) {
            let Some(bar) = widget.downcast_ref::<super::SectionedBar>() else {
                return;
            };

            let spacing = self.spacing.get();
            let edge = self.edge_margin.get();
            let interior = (width - 2 * edge).max(0);
            let [left, center, right] = bar.visible_sections();

            fn sizes(widget: Option<&Widget>) -> Option<SectionSizes> {
                widget.map(|widget| {
                    let (min, natural, _, _) = widget.measure(Orientation::Horizontal, -1);
                    SectionSizes { min, natural }
                })
            }

            let Some(center) = center else {
                let alloc = compute_linear_allocation(
                    interior,
                    spacing,
                    sizes(left.as_ref()),
                    sizes(right.as_ref()),
                );
                if let Some(left) = left {
                    allocate_at(&left, edge + alloc.left_x, alloc.left_width, height);
                }
                if let Some(right) = right {
                    allocate_at(&right, edge + alloc.right_x, alloc.right_width, height);
                }
                return;
            };

            let center_sizes = sizes(Some(&center)).expect("center section is present");
            let alloc = compute_center_priority_allocation(
                interior,
                spacing,
                sizes(left.as_ref()),
                false,
                center_sizes,
                sizes(right.as_ref()),
                false,
            );

            if let Some(left) = left {
                allocate_at(&left, edge + alloc.left_x, alloc.left_width, height);
            }
            allocate_at(&center, edge + alloc.center_x, alloc.center_width, height);
            if let Some(right) = right {
                allocate_at(&right, edge + alloc.right_x, alloc.right_width, height);
            }
        }
    }

    #[derive(Default)]
    pub struct SectionedBar {
        pub left: RefCell<Option<Widget>>,
        pub center: RefCell<Option<Widget>>,
        pub right: RefCell<Option<Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SectionedBar {
        const NAME: &'static str = "TopbarSectionedBar";
        type Type = super::SectionedBar;
        type ParentType = Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name(classes::SECTIONED_BAR);
        }
    }

    impl ObjectImpl for SectionedBar {
        fn dispose(&self) {
            for slot in [&self.left, &self.center, &self.right] {
                if let Some(child) = slot.borrow_mut().take() {
                    child.unparent();
                }
            }
        }
    }

    impl WidgetImpl for SectionedBar {}
}

/// Position a section at `x`, giving it the full bar height.
///
/// Baseline alignment is switched off (`-1`): with mixed icon and text fonts
/// it drags labels off-center.
fn allocate_at(child: &Widget, x: i32, width: i32, height: i32) {
    let transform = (x != 0)
        .then(|| gtk4::gsk::Transform::new().translate(&gtk4::graphene::Point::new(x as f32, 0.0)));
    child.allocate(width.max(0), height, -1, transform);
}

glib::wrapper! {
    /// Layout manager that gives the center section priority over the sides.
    pub struct CenterPriorityLayout(ObjectSubclass<imp::CenterPriorityLayout>)
        @extends LayoutManager;
}

impl CenterPriorityLayout {
    /// Create a layout with `spacing` between sections and `edge_margin` from
    /// the bar's left and right edges.
    pub fn new(spacing: i32, edge_margin: i32) -> Self {
        let layout: Self = glib::Object::builder().build();
        layout.imp().spacing.set(spacing);
        layout.imp().edge_margin.set(edge_margin);
        layout
    }
}

glib::wrapper! {
    /// The bar's content widget: three sections laid out center-first.
    pub struct SectionedBar(ObjectSubclass<imp::SectionedBar>)
        @extends Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl SectionedBar {
    /// Create a bar whose sections are `spacing` apart and `edge_margin` in
    /// from the bar's edges.
    pub fn new(spacing: i32, edge_margin: i32) -> Self {
        let bar: Self = glib::Object::builder().build();
        bar.set_layout_manager(Some(CenterPriorityLayout::new(spacing, edge_margin)));
        bar
    }

    /// The widget currently in `section`, if any.
    pub fn section(&self, section: Section) -> Option<Widget> {
        self.slot(section).borrow().clone()
    }

    /// Put `widget` in `section`, unparenting whatever was there.
    pub fn set_section(&self, section: Section, widget: Option<&impl IsA<Widget>>) {
        let slot = self.slot(section);
        if let Some(previous) = slot.borrow_mut().take() {
            previous.unparent();
        }
        if let Some(widget) = widget {
            let widget = widget.as_ref();
            widget.set_parent(self);
            *slot.borrow_mut() = Some(widget.clone());
        }
        self.queue_resize();
    }

    /// The three sections in order, with hidden ones reported as absent.
    fn visible_sections(&self) -> [Option<Widget>; 3] {
        Section::ALL.map(|section| self.section(section).filter(WidgetExt::is_visible))
    }

    fn slot(&self, section: Section) -> &std::cell::RefCell<Option<Widget>> {
        let imp = self.imp();
        match section {
            Section::Left => &imp.left,
            Section::Center => &imp.center,
            Section::Right => &imp.right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_map_to_distinct_css_classes() {
        let classes: Vec<&str> = Section::ALL.iter().map(|s| s.css_class()).collect();
        assert_eq!(classes.len(), 3);
        assert_ne!(classes[0], classes[1]);
        assert_ne!(classes[1], classes[2]);
        assert_ne!(classes[0], classes[2]);
    }
}
