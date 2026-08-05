//! The Bluetooth device list, and the pairing row that opens inside it.
//!
//! ```text
//! ┌───────────────────┬───────────────────┐
//! │ ᛒ WH-1000XM4  ⌄ │ …                 │   the pill in the grid
//! └───────────────────┴───────────────────┘
//!   🎧 WH-1000XM4        85%    [ ●──]     ← connected, with its battery
//!   🖱 MX Master                 [──○]
//!   ⌨  Magic Keyboard           [ ◜◝ ]     ← connecting
//!     Confirm this code matches on Pixel 8
//!     ┌────────┐
//!     │ 000042 │   Cancel   Confirm       ← a pairing the panel did not start
//!     └────────┘
//! ```
//!
//! Rows are **rebuilt only when the list changes shape**. A snapshot arrives
//! whenever a battery level moves, which for a connected headset is every
//! minute or so; tearing the rows down and building them again at that rate
//! would make a switch flip under the pointer.
//!
//! The switches are pessimistic. `Connect` on a device that is switched off
//! takes BlueZ the better part of ten seconds to give up on, so the row spins
//! for exactly that long and the switch does not move until BlueZ says the
//! device did. A switch that flipped and flipped back eight seconds later would
//! be a control that lied for eight seconds.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Image, Label, Orientation, Spinner, Switch};
use topbar_services::{BtDevice, BtState, Services};

use crate::style::{classes, icons};
use crate::surfaces::inline::names;
use crate::widgets::quick_settings::{attempt, set_text};

/// Gap between the parts of a row.
const GAP: i32 = 10;

/// What the list looks like right now, as one comparable string.
///
/// The battery percentage is deliberately **not** in it: it is drawn into a
/// label the render pass already updates, and putting it here would rebuild
/// every row each time a headset reported one point less.
fn signature(state: &BtState) -> String {
    let mut signature = format!("{}:{}:{}", state.available, state.powered, state.powering);
    for device in &state.devices {
        signature.push('\u{1}');
        signature.push_str(&format!(
            "{}\u{2}{}\u{2}{}\u{2}{}\u{2}{}",
            device.path,
            device.alias,
            icons::bluetooth_device(device.icon),
            device.connected,
            device.pending
        ));
    }
    // A prompt arriving or leaving moves the row it is attached to.
    match &state.prompt {
        Some(prompt) => {
            signature.push('\u{1}');
            signature.push_str(&prompt.path);
        }
        None => signature.push('\u{1}'),
    }
    signature
}

/// The parts of one row that change without the list changing shape.
struct Row {
    path: String,
    battery: Label,
    switch: Switch,
    spinner: Spinner,
    /// Raised while the render path is writing into the switch, so the
    /// `state-set` it causes is not mistaken for the user flipping it.
    updating: std::cell::Cell<bool>,
}

/// The list of paired devices.
pub struct BluetoothList {
    root: gtk4::Box,
    list: gtk4::Box,
    /// "Bluetooth is off", where the rows would be.
    off: Label,
    /// "No devices are paired", likewise.
    empty: Label,
    rows: RefCell<Vec<Rc<Row>>>,
    /// What the rows were last built from.
    built: RefCell<String>,
    /// The pairing box, re-parented under whichever row wants it.
    pairing: Rc<PairingBox>,
    services: Services,
}

impl BluetoothList {
    /// Build the list.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::QS_DEVICE_LIST);

        let list = gtk4::Box::new(Orientation::Vertical, 2);
        root.append(&list);

        // A radio that is off has no rows to show, and says why rather than
        // being a gap the user has to work out the meaning of.
        let off = Label::new(Some("Bluetooth is off"));
        off.add_css_class(classes::QS_HINT);
        off.set_xalign(0.0);
        off.set_visible(false);
        root.append(&off);

        let empty = Label::new(Some("No devices are paired"));
        empty.add_css_class(classes::QS_HINT);
        empty.set_xalign(0.0);
        empty.set_visible(false);
        root.append(&empty);

        let pairing = PairingBox::new(services);

        Rc::new(Self {
            root,
            list,
            off,
            empty,
            rows: RefCell::new(Vec::new()),
            built: RefCell::new(String::new()),
            pairing,
            services: services.clone(),
        })
    }

    /// The widget to put in the section.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Draw the list from `state`.
    pub fn render(self: &Rc<Self>, state: &BtState) {
        let signature = signature(state);
        if *self.built.borrow() != signature {
            self.rebuild(state);
            *self.built.borrow_mut() = signature;
        }

        for (row, device) in self.rows.borrow().iter().zip(&state.devices) {
            debug_assert_eq!(row.path, device.path);
            render_battery(&row.battery, device);
            row.spinner.set_visible(device.pending);
            if device.pending {
                row.spinner.start();
            } else {
                row.spinner.stop();
            }
            // The switch is only touched when it disagrees, and the guard stops
            // the write being read back as the user flipping it.
            row.switch.set_visible(!device.pending);
            if row.switch.is_active() != device.connected {
                row.updating.set(true);
                row.switch.set_active(device.connected);
                row.updating.set(false);
            }
            // `state` is the *confirmed* half of a GtkSwitch, and the handler
            // below deliberately refuses to move it. This is the other end of
            // that contract: BlueZ said what the device is doing, so the switch
            // is told. Without it the two halves disagree for ever and the
            // switch draws in its in-between look.
            row.switch.set_state(device.connected);
            row.switch.set_sensitive(state.powered);
        }

        self.list.set_visible(state.powered);
        self.off.set_visible(!state.powered);
        self.empty
            .set_visible(state.powered && state.devices.is_empty());
        self.pairing.render(state, self);
    }

    /// Rebuild every row.
    fn rebuild(self: &Rc<Self>, state: &BtState) {
        // The pairing box is re-parented into the list, so it has to come out
        // before the rows around it are dropped — GTK asserts otherwise.
        self.pairing.detach();

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut rows = Vec::new();
        for device in &state.devices {
            let (widget, row) = self.row(device);
            self.list.append(&widget);
            rows.push(row);
        }
        *self.rows.borrow_mut() = rows;
    }

    /// One device's row.
    fn row(self: &Rc<Self>, device: &BtDevice) -> (gtk4::Box, Rc<Row>) {
        // A box rather than a button: the switch is the control, and a row that
        // was *also* clickable would give one line two different meanings.
        let line = gtk4::Box::new(Orientation::Horizontal, GAP);
        line.add_css_class(classes::QS_DEVICE_ROW);
        line.set_valign(Align::Center);

        let icon = Image::from_icon_name(icons::bluetooth_device(device.icon));
        icon.add_css_class(classes::QS_ICON);
        line.append(&icon);

        let name = Label::new(Some(&device.alias));
        name.add_css_class(classes::QS_DEVICE_NAME);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        line.append(&name);

        let battery = Label::new(None);
        battery.add_css_class(classes::QS_DEVICE_BATTERY);
        battery.set_visible(false);
        line.append(&battery);

        // The spinner and the switch share a slot: exactly one is visible, so
        // a row that starts connecting does not change width.
        let spinner = Spinner::new();
        spinner.set_visible(false);
        spinner.set_valign(Align::Center);
        line.append(&spinner);

        let switch = Switch::new();
        switch.add_css_class(classes::QS_DEVICE_SWITCH);
        switch.set_valign(Align::Center);
        line.append(&switch);

        let row = Rc::new(Row {
            path: device.path.clone(),
            battery,
            switch,
            spinner,
            updating: std::cell::Cell::new(false),
        });
        render_battery(&row.battery, device);

        row.switch.connect_state_set({
            let list = Rc::downgrade(self);
            let row = Rc::downgrade(&row);
            move |_, wanted| {
                let (Some(list), Some(row)) = (list.upgrade(), row.upgrade()) else {
                    return gtk4::glib::Propagation::Proceed;
                };
                if row.updating.get() {
                    return gtk4::glib::Propagation::Proceed;
                }
                let bluetooth = list.services.bluetooth.handle().clone();
                let path = row.path.clone();
                attempt(names::BLUETOOTH, async move {
                    if wanted {
                        bluetooth.connect(path).await
                    } else {
                        bluetooth.disconnect(path).await
                    }
                });
                // Refusing the default handler is what makes this switch
                // *asynchronous*: GTK leaves `state` where it was, and the
                // render pass sets it once BlueZ has said what the device is
                // actually doing. That is what makes a failed connect revert
                // rather than lie for the ten seconds BlueZ takes to give up.
                gtk4::glib::Propagation::Stop
            }
        });

        (line, row)
    }

    /// Put the pairing box under the row for `path`, or at the end.
    fn place_pairing(&self, path: Option<&str>) {
        let Some(path) = path else {
            self.pairing.detach();
            return;
        };
        let position = self
            .rows
            .borrow()
            .iter()
            .position(|row| row.path == path)
            .map(|index| index + 1);
        self.pairing.attach(&self.list, position);
    }
}

/// Write a device's battery level, or take the label away.
fn render_battery(label: &Label, device: &BtDevice) {
    match device.battery_pct {
        // Only while connected: a headset in a case reports whatever it last
        // said, and a stale number beside an idle row reads as a live one.
        Some(percent) if device.connected => {
            set_text(label, &format!("{percent}%"));
            label.set_visible(true);
        }
        _ => label.set_visible(false),
    }
}

/// The inline pairing box.
///
/// One widget, moved about, rather than one per row — the same arrangement the
/// Wi-Fi password box uses, and for a related reason: there is at most one
/// pairing in flight, and a box per device would be a dozen widgets waiting for
/// something that happens twice a year.
struct PairingBox {
    root: gtk4::Box,
    question: Label,
    code: Label,
    buttons: gtk4::Box,
    /// The device the box is currently open for.
    path: RefCell<Option<String>>,
}

impl PairingBox {
    /// Build it, closed.
    fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 6);
        root.add_css_class(classes::QS_PAIRING_ROW);
        root.set_visible(false);

        let question = Label::new(None);
        question.add_css_class(classes::QS_HINT);
        question.set_xalign(0.0);
        question.set_wrap(true);
        root.append(&question);

        // The code is the whole point of the row: it has to be readable across
        // a desk at the same time as the phone showing it.
        let code = Label::new(None);
        code.add_css_class(classes::QS_PAIRING_CODE);
        code.set_halign(Align::Center);
        code.set_visible(false);
        root.append(&code);

        let buttons = gtk4::Box::new(Orientation::Horizontal, 6);
        buttons.set_halign(Align::End);
        let cancel = Button::with_label("Cancel");
        cancel.add_css_class(classes::QS_PASSWORD_BUTTON);
        buttons.append(&cancel);
        let confirm = Button::with_label("Confirm");
        confirm.add_css_class(classes::QS_PASSWORD_BUTTON);
        confirm.add_css_class(classes::CHECKED);
        buttons.append(&confirm);
        root.append(&buttons);

        for (button, yes) in [(&cancel, false), (&confirm, true)] {
            button.connect_clicked({
                let bluetooth = services.bluetooth.handle().clone();
                move |_| {
                    let bluetooth = bluetooth.clone();
                    attempt(names::BLUETOOTH, async move {
                        if yes {
                            bluetooth.confirm_pairing().await
                        } else {
                            bluetooth.cancel_pairing().await
                        }
                    });
                }
            });
        }

        Rc::new(Self {
            root,
            question,
            code,
            buttons,
            path: RefCell::new(None),
        })
    }

    /// Show or hide the box, and say what it is asking.
    fn render(self: &Rc<Self>, state: &BtState, list: &Rc<BluetoothList>) {
        let Some(prompt) = &state.prompt else {
            if self.path.borrow().is_some() {
                *self.path.borrow_mut() = None;
                self.root.set_visible(false);
                list.place_pairing(None);
            }
            return;
        };

        *self.path.borrow_mut() = Some(prompt.path.clone());
        set_text(&self.question, &prompt.question());
        match &prompt.code {
            Some(code) => {
                set_text(&self.code, code);
                self.code.set_visible(true);
            }
            None => self.code.set_visible(false),
        }
        // A code the user is told to type on the other device has nothing to
        // answer, so it gets no buttons rather than two that do nothing.
        self.buttons.set_visible(prompt.answerable());
        self.root.set_visible(true);
        list.place_pairing(Some(&prompt.path));
    }

    /// Put it into `list` at `position`, or at the end.
    fn attach(&self, list: &gtk4::Box, position: Option<usize>) {
        if let Some(parent) = self.root.parent()
            && let Some(parent) = parent.downcast_ref::<gtk4::Box>()
        {
            if parent == list {
                reorder(list, &self.root, position);
                return;
            }
            parent.remove(&self.root);
        }
        list.append(&self.root);
        reorder(list, &self.root, position);
    }

    /// Take it out of whatever it is in.
    fn detach(&self) {
        if let Some(parent) = self.root.parent()
            && let Some(parent) = parent.downcast_ref::<gtk4::Box>()
        {
            parent.remove(&self.root);
        }
    }
}

/// Move `child` to `position` inside `list`.
fn reorder(list: &gtk4::Box, child: &gtk4::Box, position: Option<usize>) {
    let Some(position) = position else { return };
    let mut sibling = list.first_child();
    let mut index = 0;
    while let Some(current) = sibling {
        if index + 1 == position {
            list.reorder_child_after(child, Some(&current));
            return;
        }
        sibling = current.next_sibling();
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use topbar_services::{IconKind, PairingPrompt, PromptKind};

    fn device(alias: &str, connected: bool) -> BtDevice {
        BtDevice {
            path: format!("/org/bluez/hci0/dev_{alias}"),
            alias: alias.to_string(),
            icon: IconKind::Headset,
            connected,
            paired: true,
            battery_pct: None,
            pending: false,
        }
    }

    fn powered(devices: Vec<BtDevice>) -> BtState {
        BtState {
            available: true,
            powered: true,
            devices,
            ..BtState::default()
        }
    }

    #[test]
    fn the_list_is_rebuilt_when_its_shape_changes_and_not_when_a_battery_moves() {
        let mut state = powered(vec![device("Buds", true), device("Mouse", false)]);
        state.devices[0].battery_pct = Some(85);
        let before = signature(&state);

        // A headset losing a percent changes a label, not a row.
        state.devices[0].battery_pct = Some(84);
        assert_eq!(signature(&state), before, "no rebuild for a battery tick");

        // A row starting to spin does change the shape.
        let mut spinning = state.clone();
        spinning.devices[1].pending = true;
        assert_ne!(signature(&spinning), before);

        // And so does one connecting.
        let mut connected = state.clone();
        connected.devices[1].connected = true;
        assert_ne!(signature(&connected), before);
    }

    #[test]
    fn switching_the_radio_off_changes_the_list_the_user_is_shown() {
        let on = powered(vec![device("Buds", true)]);
        let mut off = on.clone();
        off.powered = false;
        assert_ne!(signature(&on), signature(&off));
    }

    #[test]
    fn a_pairing_question_arriving_moves_the_row_it_belongs_to() {
        let state = powered(vec![device("Pixel", false)]);
        let mut asking = state.clone();
        asking.prompt = Some(PairingPrompt {
            path: state.devices[0].path.clone(),
            alias: "Pixel".into(),
            code: Some("000042".into()),
            kind: PromptKind::Confirm,
        });
        assert_ne!(signature(&state), signature(&asking));
    }
}
