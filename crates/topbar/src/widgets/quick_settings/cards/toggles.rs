//! The toggle grid: two columns of pills, some of which expand.
//!
//! ```text
//! ┌──────────────────┬──────────────────┐
//! │ ☕ Caffeine      │ ⚡ Balanced   ⌄ │
//! └──────────────────┴──────────────────┘
//!   ○ Power Saver                          ← the expanded section, full width
//!   ● Balanced
//!   ○ Performance
//! ```
//!
//! The grid is built from a list, not from a hand-written layout: M9b adds
//! Wi-Fi and VPN and M9c adds Bluetooth, and each of them is one entry in
//! [`model::GRID_ORDER`] plus a [`Pill`]. The wrapping, the ordering and the
//! short last row are [`model::grid_rows`]'s problem and are tested there.
//!
//! An expandable pill's section is appended after the *row* it is in rather
//! than inside the grid, so it spans the panel: a list of radio rows squeezed
//! into half the width would be unreadable.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Image, Label, Orientation};
use topbar_services::{InhibitorState, PowerProfilesState, Services};

use crate::bridge::{self, BindingGuard};
use crate::style::{classes, icons};
use crate::surfaces::inline::{self, names};
use crate::widgets::quick_settings::expander::{Accordion, Section};
use crate::widgets::quick_settings::model::{self, Toggle};
use crate::widgets::quick_settings::{attempt, set_icon, set_text};

/// Gap between the two columns, and between rows.
const GAP: i32 = 8;
/// What the Power Mode pill shows before the daemon has answered.
const POWER_MODE_ICON: &str = "power-profile-balanced-symbolic";

/// One pill: an icon, a label, a subtitle, and optionally a chevron.
struct Pill {
    root: gtk4::Box,
    button: Button,
    icon: Image,
    label: Label,
    subtitle: Label,
    expand: Option<Button>,
}

impl Pill {
    /// Build a pill. `expandable` gives it a chevron.
    fn new(icon_name: &str, title: &str, expandable: bool) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Horizontal, 0);
        root.set_hexpand(true);

        let button = Button::new();
        button.add_css_class(classes::QS_TOGGLE);
        button.set_hexpand(true);

        let content = gtk4::Box::new(Orientation::Horizontal, 0);
        content.set_valign(Align::Center);

        let icon = Image::from_icon_name(icon_name);
        icon.add_css_class(classes::QS_TOGGLE_ICON);
        content.append(&icon);

        let text = gtk4::Box::new(Orientation::Vertical, 0);
        text.set_valign(Align::Center);
        text.set_hexpand(true);

        let label = Label::new(Some(title));
        label.add_css_class(classes::QS_TOGGLE_LABEL);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        text.append(&label);

        let subtitle = Label::new(None);
        subtitle.add_css_class(classes::QS_TOGGLE_SUBTITLE);
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        subtitle.set_visible(false);
        text.append(&subtitle);

        content.append(&text);
        button.set_child(Some(&content));
        root.append(&button);

        let expand = expandable.then(|| {
            let expand = Button::new();
            expand.add_css_class(classes::QS_TOGGLE_EXPAND);
            expand.set_child(Some(&Image::from_icon_name(icons::EXPAND)));
            expand.set_valign(Align::Center);
            content.append(&expand);
            expand
        });

        Rc::new(Self {
            root,
            button,
            icon,
            label,
            subtitle,
            expand,
        })
    }

    /// Wear the accent fill, or take it off.
    fn set_checked(&self, checked: bool) {
        if checked {
            self.button.add_css_class(classes::CHECKED);
        } else {
            self.button.remove_css_class(classes::CHECKED);
        }
    }

    /// Change the pill's title.
    fn set_title(&self, title: &str) {
        set_text(&self.label, title);
    }

    /// Put a second line under the title, or take it away.
    fn set_subtitle(&self, text: Option<&str>) {
        match text {
            Some(text) if !text.is_empty() => {
                set_text(&self.subtitle, text);
                self.subtitle.set_visible(true);
            }
            _ => self.subtitle.set_visible(false),
        }
    }

    /// Point the chevron the way the section is going.
    fn set_expanded(&self, expanded: bool) {
        let Some(expand) = &self.expand else {
            return;
        };
        if let Some(image) = expand.child().and_downcast::<Image>() {
            set_icon(
                &image,
                if expanded {
                    "pan-up-symbolic"
                } else {
                    icons::EXPAND
                },
            );
        }
    }
}

/// The grid, and everything keeping it alive.
pub struct Toggles {
    root: gtk4::Box,
    caffeine: Option<Rc<Pill>>,
    power_mode: Option<Rc<Pill>>,
    power_mode_section: Option<Rc<Section>>,
    power_mode_list: gtk4::Box,
    /// The profiles the list was last built from, so it is not rebuilt on
    /// every percentage change the daemon happens to publish alongside.
    built_profiles: RefCell<Vec<String>>,
    /// Each radio row's checkmark, beside the profile it belongs to.
    profile_marks: RefCell<Vec<(String, Image)>>,
    services: Services,
    _slots: Vec<inline::InlineSlot>,
    bindings: RefCell<Vec<BindingGuard>>,
}

impl Toggles {
    /// Build the grid.
    ///
    /// `show_caffeine` is `[widgets.quick_settings] idle_inhibitor`; Power
    /// Mode has no switch of its own because it is hidden exactly when the
    /// daemon is absent, which is a fact rather than a preference.
    pub fn new(services: &Services, accordion: &Rc<Accordion>, show_caffeine: bool) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, GAP);
        root.add_css_class(classes::QS_GRID);

        let caffeine = show_caffeine
            .then(|| Pill::new(icons::first_available(icons::CAFFEINE), "Caffeine", false));
        // The icon is replaced from state on the first render; the balanced
        // one stands in until the daemon has said which profile is in force.
        let power_mode = Some(Pill::new(POWER_MODE_ICON, "Power Mode", true));

        let power_mode_list = gtk4::Box::new(Orientation::Vertical, 2);
        power_mode_list.add_css_class(classes::QS_DEVICE_LIST);
        let power_mode_section = Section::new(&power_mode_list);
        accordion.add(&power_mode_section);

        let (caffeine_error, caffeine_slot) = inline::slot(names::CAFFEINE);
        let (power_error, power_slot) = inline::slot(names::POWER_MODE);

        let present: Vec<Toggle> = [
            caffeine.as_ref().map(|_| Toggle::Caffeine),
            Some(Toggle::PowerMode),
        ]
        .into_iter()
        .flatten()
        .collect();

        for row in model::grid_rows(&present) {
            let line = gtk4::Box::new(Orientation::Horizontal, GAP);
            line.add_css_class(classes::QS_GRID_ROW);
            line.set_homogeneous(true);

            let mut carries_power_mode = false;
            for toggle in &row {
                match toggle {
                    Toggle::Caffeine => {
                        if let Some(pill) = &caffeine {
                            line.append(&pill.root);
                        }
                    }
                    Toggle::PowerMode => {
                        if let Some(pill) = &power_mode {
                            line.append(&pill.root);
                            carries_power_mode = true;
                        }
                    }
                    // M9b and M9c: the ordering already has a place for them.
                    Toggle::WiFi | Toggle::Bluetooth | Toggle::Vpn => {}
                }
            }
            // A lone pill on the last row keeps its column width rather than
            // stretching across the panel and looking like a different control.
            for _ in row.len()..model::COLUMNS {
                let filler = gtk4::Box::new(Orientation::Horizontal, 0);
                filler.set_hexpand(true);
                line.append(&filler);
            }

            root.append(&line);
            if carries_power_mode {
                root.append(&power_error);
                root.append(power_mode_section.root());
            }
        }
        if caffeine.is_some() {
            root.append(&caffeine_error);
        }

        let toggles = Rc::new(Self {
            root,
            caffeine,
            power_mode,
            power_mode_section: Some(power_mode_section),
            power_mode_list,
            built_profiles: RefCell::new(Vec::new()),
            profile_marks: RefCell::new(Vec::new()),
            services: services.clone(),
            _slots: vec![caffeine_slot, power_slot],
            bindings: RefCell::new(Vec::new()),
        });

        Self::wire(&toggles, accordion);
        toggles
    }

    /// The widget to put in the panel.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    pub fn refresh(&self) {
        self.render_inhibitor(&self.services.inhibitor.current());
        self.render_profiles(&self.services.power_profiles.current());
    }

    /// Open the Power Mode radio list, without a pointer.
    ///
    /// The smoke hook's way in: there is no synthetic input in the nested
    /// session, so an expanded toggle could not otherwise be photographed.
    #[cfg(debug_assertions)]
    pub fn expand_power_mode(&self) {
        if let Some(section) = &self.power_mode_section {
            section.set_expanded(true);
            self.sync_chevrons();
        }
    }

    /// Put the chevron back when the section is closed from outside.
    pub fn sync_chevrons(&self) {
        if let (Some(pill), Some(section)) = (&self.power_mode, &self.power_mode_section) {
            pill.set_expanded(section.is_expanded());
        }
    }

    /// Connect every handler and subscription.
    fn wire(toggles: &Rc<Self>, accordion: &Rc<Accordion>) {
        if let Some(pill) = &toggles.caffeine {
            pill.button.connect_clicked({
                let inhibitor = toggles.services.inhibitor.handle().clone();
                move |_| {
                    let inhibitor = inhibitor.clone();
                    attempt(names::CAFFEINE, async move { inhibitor.toggle().await });
                }
            });
        }

        if let (Some(pill), Some(section)) = (&toggles.power_mode, &toggles.power_mode_section) {
            // The pill's body and its chevron both expand it: there is nothing
            // else a click on Power Mode could sensibly mean, and a pill whose
            // left half did nothing would read as broken.
            for button in [Some(&pill.button), pill.expand.as_ref()]
                .into_iter()
                .flatten()
            {
                button.connect_clicked({
                    let toggles = Rc::downgrade(toggles);
                    let accordion = Rc::clone(accordion);
                    let section = Rc::clone(section);
                    move |_| {
                        accordion.toggle(&section);
                        if let Some(toggles) = toggles.upgrade() {
                            toggles.sync_chevrons();
                        }
                    }
                });
            }
        }

        let inhibitor_binding =
            bridge::bind_state(&toggles.root, toggles.services.inhibitor.state(), {
                let toggles = Rc::downgrade(toggles);
                move |_: &gtk4::Box, state: &InhibitorState| {
                    if let Some(toggles) = toggles.upgrade() {
                        toggles.render_inhibitor(state);
                    }
                }
            });
        let profiles_binding =
            bridge::bind_state(&toggles.root, toggles.services.power_profiles.state(), {
                let toggles = Rc::downgrade(toggles);
                move |_: &gtk4::Box, state: &PowerProfilesState| {
                    if let Some(toggles) = toggles.upgrade() {
                        toggles.render_profiles(state);
                    }
                }
            });

        toggles
            .bindings
            .borrow_mut()
            .extend([inhibitor_binding, profiles_binding]);
    }

    /// Draw the idle inhibitor.
    fn render_inhibitor(&self, state: &InhibitorState) {
        let Some(pill) = &self.caffeine else {
            return;
        };
        // No logind means no lock to hold; the pill goes rather than sitting
        // there insensitive explaining a machine the user does not have.
        pill.root.set_visible(state.available);
        pill.set_checked(state.active);
        pill.set_subtitle(Some(if state.active { "On" } else { "Off" }));
    }

    /// Draw the power profiles.
    fn render_profiles(&self, state: &PowerProfilesState) {
        let (Some(pill), Some(section)) = (&self.power_mode, &self.power_mode_section) else {
            return;
        };

        pill.root.set_visible(state.available);
        if !state.available {
            section.collapse_now();
            pill.set_expanded(false);
            return;
        }

        // GNOME labels the collapsed pill with the *profile*, not with the
        // control: "Balanced" is the answer to the question the user is
        // asking, and "Power Mode" underneath says what kind of answer it is.
        if let Some(active) = &state.active {
            set_icon(&pill.icon, active.icon);
            pill.set_title(&active.label);
            pill.set_subtitle(Some("Power Mode"));
        } else {
            pill.set_title("Power Mode");
            pill.set_subtitle(None);
        }

        let ids: Vec<String> = state
            .profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect();
        if *self.built_profiles.borrow() != ids {
            self.rebuild_profiles(state);
            *self.built_profiles.borrow_mut() = ids;
        }
        self.mark_active(state);
    }

    /// Rebuild the radio rows: exactly the profiles the daemon reports.
    fn rebuild_profiles(&self, state: &PowerProfilesState) {
        while let Some(child) = self.power_mode_list.first_child() {
            self.power_mode_list.remove(&child);
        }
        let mut marks = Vec::new();

        for profile in &state.profiles {
            let row = Button::new();
            row.add_css_class(classes::QS_RADIO_ROW);

            let line = gtk4::Box::new(Orientation::Horizontal, 8);

            let icon = Image::from_icon_name(profile.icon);
            icon.add_css_class(classes::QS_ICON);
            line.append(&icon);

            let label = Label::new(Some(&profile.label));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            line.append(&label);

            let mark = Image::from_icon_name(icons::SELECTED);
            mark.add_css_class(classes::QS_RADIO_MARK);
            line.append(&mark);

            row.set_child(Some(&line));
            // The mark is remembered beside the identifier it belongs to, so
            // moving the checkmark is a lookup rather than a walk over the
            // widget tree guessing which child is which.
            marks.push((profile.id.clone(), mark));

            row.connect_clicked({
                let profiles = self.services.power_profiles.handle().clone();
                let id = profile.id.clone();
                move |_| {
                    let profiles = profiles.clone();
                    let id = id.clone();
                    attempt(
                        names::POWER_MODE,
                        async move { profiles.set_profile(id).await },
                    );
                }
            });
            self.power_mode_list.append(&row);
        }

        *self.profile_marks.borrow_mut() = marks;
    }

    /// Move the checkmark to the profile in force.
    ///
    /// The mark is always present and only its opacity moves, so the labels
    /// beside it do not shift sideways when the selection changes.
    fn mark_active(&self, state: &PowerProfilesState) {
        let active = state.active_id();
        for (id, mark) in self.profile_marks.borrow().iter() {
            mark.set_opacity(if Some(id.as_str()) == active {
                1.0
            } else {
                0.0
            });
        }
    }
}
