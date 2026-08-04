//! The control panel's media card — GNOME's media section.
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │ ██████  Windowlicker                        │  art, 64px, radius 12
//! │ ██████  Aphex Twin                          │  title, artist (ellipsized)
//! │ ██████   ⏮   ⏸   ⏭                          │  transport
//! │ ───────●───────────────────────  1:12  3:41 │  seek, only when seekable
//! │ ( )( )                                      │  switcher, only with 2+
//! └─────────────────────────────────────────────┘
//! ```
//!
//! The whole card is hidden when no player is on the bus, which is the usual
//! state of a desktop: a card explaining that nothing is playing is a card
//! nobody asked for. Everything inside it hides on the same principle — the
//! seek bar only exists for something that can be seeked, the switcher only
//! when there is something to switch to.
//!
//! MPRIS does not signal the playback position, so the service polls it once a
//! second, and only while this card is on screen ([`Card::refresh`] switches
//! that on, [`Card::closed`] switches it off). Between polls the bar moves on
//! this card's own tick, extrapolated from the last sample — see
//! `PlayerView::position_at`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gtk4::prelude::*;
use gtk4::{
    Align, Button, EventControllerLegacy, Image, Label, Orientation, Scale, gdk, glib, pango,
};
use topbar_services::{ArtRef, MediaState, PlaybackStatus, PlayerView, Services};
use tracing::debug;

use crate::anim::{Animation, AnimationParams, Easing};
use crate::bridge::{self, ActionScope, BindingGuard};
use crate::style::classes;
use crate::widgets::app_icon;
use crate::widgets::rounded_picture::RoundedPicture;

/// Side of the album art, in pixels.
const ART_SIZE: i32 = 64;
/// Corner radius of the album art, in pixels.
const ART_RADIUS: f32 = 12.0;
/// How long the art takes to change.
const ART_FADE_MS: u64 = 150;
/// Size of a player's icon in the switcher.
const SWITCHER_ICON: i32 = 20;
/// How often the seek bar moves between position polls.
const TICK: std::time::Duration = std::time::Duration::from_millis(500);
/// Shown while a player has no cover.
const ART_PLACEHOLDER: &str = "audio-x-generic-symbolic";
/// Where this card's failures are reported.
const SCOPE: ActionScope = ActionScope::Toast { widget: "media" };

/// The media card.
pub struct Card {
    root: gtk4::Box,
    art: RoundedPicture,
    /// The art currently drawn, so the same cover is never decoded twice.
    art_key: Cell<Option<u64>>,
    /// Bumped by every art load, so a slow decode cannot overwrite a newer one.
    art_generation: Rc<Cell<u64>>,
    art_fade: Animation,
    title: Label,
    artist: Label,
    previous: Button,
    play_pause: Button,
    play_icon: Image,
    next: Button,
    seek_row: gtk4::Box,
    seek: Scale,
    elapsed: Label,
    duration: Label,
    switcher: gtk4::Box,
    /// The players the switcher is drawn for, so it is only rebuilt when the
    /// set of players actually changes.
    switcher_players: RefCell<Vec<String>>,
    /// The active player, for the tick between polls.
    active: RefCell<Option<PlayerView>>,
    /// Set while the user is dragging the seek bar: the service's own position
    /// must not fight the thumb under their finger.
    dragging: Rc<Cell<bool>>,
    /// Where the drag has got to, committed on release.
    pending_seek: Rc<Cell<Option<i64>>>,
    /// The tick that moves the seek bar; only runs while the panel is open.
    ticker: RefCell<Option<glib::SourceId>>,
    services: Services,
    binding: RefCell<Option<BindingGuard>>,
}

impl Card {
    /// Build the card and subscribe it to the media service.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 10);
        root.add_css_class(classes::CARD);
        root.add_css_class(classes::MEDIA_CARD);
        // Nothing is playing until the service says otherwise, and a card with
        // nothing in it must never take up a row of the column.
        root.set_visible(false);

        // --- art -----------------------------------------------------------
        let art = RoundedPicture::new(ART_SIZE, ART_RADIUS);
        art.add_css_class(classes::MEDIA_ART);

        // The placeholder is the *measured* child, so the slot is exactly one
        // square whether or not there is a cover; the icon and the art are
        // overlays on top of it, which is what keeps a card with no art the
        // same shape as one with art. Nothing here expands: an expanding child
        // would push the square out to the width of the whole card.
        let placeholder = gtk4::Box::new(Orientation::Vertical, 0);
        placeholder.add_css_class(classes::MEDIA_ART_PLACEHOLDER);
        placeholder.set_size_request(ART_SIZE, ART_SIZE);

        let placeholder_icon = Image::from_icon_name(ART_PLACEHOLDER);
        placeholder_icon.set_halign(Align::Center);
        placeholder_icon.set_valign(Align::Center);

        let art_slot = gtk4::Overlay::new();
        art_slot.set_child(Some(&placeholder));
        art_slot.add_overlay(&placeholder_icon);
        art_slot.add_overlay(&art);
        art_slot.set_halign(Align::Start);
        art_slot.set_valign(Align::Start);

        // --- text and transport --------------------------------------------
        let title = Label::new(None);
        title.add_css_class(classes::MEDIA_TITLE);
        title.set_xalign(0.0);
        title.set_single_line_mode(true);
        title.set_ellipsize(pango::EllipsizeMode::End);

        let artist = Label::new(None);
        artist.add_css_class(classes::MEDIA_ARTIST);
        artist.set_xalign(0.0);
        artist.set_single_line_mode(true);
        artist.set_ellipsize(pango::EllipsizeMode::End);

        let (previous, _) = control_button("media-skip-backward-symbolic", "Previous");
        let (play_pause, play_icon) = control_button("media-playback-start-symbolic", "Play");
        play_pause.add_css_class(classes::MEDIA_CONTROL_PRIMARY);
        let (next, _) = control_button("media-skip-forward-symbolic", "Next");

        let controls = gtk4::Box::new(Orientation::Horizontal, 4);
        controls.add_css_class(classes::MEDIA_CONTROLS);
        controls.set_halign(Align::Start);
        controls.append(&previous);
        controls.append(&play_pause);
        controls.append(&next);

        let text = gtk4::Box::new(Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(Align::Center);
        text.append(&title);
        text.append(&artist);
        text.append(&controls);

        let top = gtk4::Box::new(Orientation::Horizontal, 12);
        top.append(&art_slot);
        top.append(&text);

        // --- seek ------------------------------------------------------------
        let seek = Scale::with_range(Orientation::Horizontal, 0.0, 1.0, 1_000_000.0);
        seek.add_css_class(classes::MEDIA_SEEK);
        seek.set_draw_value(false);
        seek.set_hexpand(true);

        let elapsed = time_label();
        elapsed.set_halign(Align::Start);
        let duration = time_label();
        duration.set_halign(Align::End);

        let times = gtk4::Box::new(Orientation::Horizontal, 0);
        times.add_css_class(classes::MEDIA_TIME);
        elapsed.set_hexpand(true);
        times.append(&elapsed);
        times.append(&duration);

        let seek_row = gtk4::Box::new(Orientation::Vertical, 0);
        seek_row.append(&seek);
        seek_row.append(&times);
        seek_row.set_visible(false);

        // --- switcher --------------------------------------------------------
        let switcher = gtk4::Box::new(Orientation::Horizontal, 6);
        switcher.add_css_class(classes::MEDIA_SWITCHER);
        switcher.set_visible(false);

        root.append(&top);
        root.append(&seek_row);
        root.append(&switcher);

        let card = Rc::new(Self {
            art_fade: Animation::new(&art),
            root,
            art,
            art_key: Cell::new(None),
            art_generation: Rc::new(Cell::new(0)),
            title,
            artist,
            previous,
            play_pause,
            play_icon,
            next,
            seek_row,
            seek,
            elapsed,
            duration,
            switcher,
            switcher_players: RefCell::new(Vec::new()),
            active: RefCell::new(None),
            dragging: Rc::new(Cell::new(false)),
            pending_seek: Rc::new(Cell::new(None)),
            ticker: RefCell::new(None),
            services: services.clone(),
            binding: RefCell::new(None),
        });

        card.connect_transport();
        card.connect_seek();

        let binding = bridge::bind_state(&card.root, services.media.state(), {
            let card = Rc::downgrade(&card);
            move |_, state| {
                if let Some(card) = card.upgrade() {
                    card.render(state);
                }
            }
        });
        *card.binding.borrow_mut() = Some(binding);

        card
    }

    /// The widget to put in the panel's right column.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render, and start following the playback position.
    ///
    /// Called on every open. Switching tracking on also asks the player where
    /// it is straight away, so the bar is right on the frame it appears.
    pub fn refresh(self: &Rc<Self>) {
        let receiver = self.services.media.state();
        let state = receiver.borrow().clone();
        self.render(&state);
        self.set_tracking(true);
        self.start_ticking();
    }

    /// Stop following the position: the panel is closed and nobody is looking.
    pub fn closed(&self) {
        self.stop_ticking();
        self.set_tracking(false);
    }

    /// Tell the service whether the seek bar is on screen.
    fn set_tracking(&self, tracking: bool) {
        let handle = self.services.media.handle().clone();
        bridge::act(SCOPE, async move {
            handle.set_position_tracking(tracking).await
        });
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Draw `state`.
    fn render(self: &Rc<Self>, state: &MediaState) {
        let Some(view) = state.active() else {
            self.root.set_visible(false);
            self.active.replace(None);
            self.stop_ticking();
            return;
        };
        self.root.set_visible(true);

        set_text(
            &self.title,
            view.title.as_deref().unwrap_or("Unknown title"),
        );
        set_text(
            &self.artist,
            view.artist.as_deref().unwrap_or("Unknown artist"),
        );

        let playing = view.status == PlaybackStatus::Playing;
        let icon = if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        if self.play_icon.icon_name().as_deref() != Some(icon) {
            self.play_icon.set_icon_name(Some(icon));
        }
        self.play_pause
            .set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));

        // A control that would do nothing is dimmed rather than hidden: the
        // row must not change shape as tracks come and go.
        self.previous.set_sensitive(view.can_go_previous);
        self.next.set_sensitive(view.can_go_next);
        self.play_pause.set_sensitive(if playing {
            view.can_pause
        } else {
            view.can_play
        });

        self.render_seek(view);
        self.render_switcher(state);
        self.render_art(view.art.as_ref());

        self.active.replace(Some(view.clone()));
        if self.ticker.borrow().is_some() {
            self.start_ticking();
        }
    }

    /// Draw the seek bar, unless this player has nothing to seek.
    fn render_seek(&self, view: &PlayerView) {
        let seekable = view.can_seek && view.length_us > 0;
        self.seek_row.set_visible(seekable);
        if !seekable {
            return;
        }

        let length = view.length_us as f64;
        if (self.seek.adjustment().upper() - length).abs() > f64::EPSILON {
            self.seek.set_range(0.0, length);
        }
        set_text(&self.duration, &format_duration(view.length_us));
        if !self.dragging.get() {
            self.show_position(view.position_at(Instant::now()));
        }
    }

    /// Move the thumb and the elapsed label to `position`.
    fn show_position(&self, position: i64) {
        let value = position as f64;
        if (self.seek.value() - value).abs() > 1.0 {
            self.seek.set_value(value);
        }
        set_text(&self.elapsed, &format_duration(position));
    }

    /// Draw one button per player, unless there is only one player.
    fn render_switcher(self: &Rc<Self>, state: &MediaState) {
        let players: Vec<String> = state.players.iter().map(switcher_key).collect();
        self.switcher.set_visible(players.len() > 1);
        if players.len() < 2 {
            self.switcher_players.replace(players);
            clear(&self.switcher);
            return;
        }

        // Rebuilt only when the players themselves change: a track change must
        // not throw away the buttons the user is about to click.
        if *self.switcher_players.borrow() != players {
            clear(&self.switcher);
            for view in &state.players {
                self.switcher.append(&self.switcher_button(view));
            }
            self.switcher_players.replace(players);
        }

        let active = state.active().map(|view| view.bus_name.as_str());
        let mut child = self.switcher.first_child();
        for view in &state.players {
            let Some(button) = child else {
                break;
            };
            let chosen = Some(view.bus_name.as_str()) == active;
            if button.has_css_class(classes::MEDIA_SWITCHER_ACTIVE) != chosen {
                if chosen {
                    button.add_css_class(classes::MEDIA_SWITCHER_ACTIVE);
                } else {
                    button.remove_css_class(classes::MEDIA_SWITCHER_ACTIVE);
                }
            }
            child = button.next_sibling();
        }
    }

    /// One player's button: its icon, or the initial of its name.
    fn switcher_button(self: &Rc<Self>, view: &PlayerView) -> Button {
        let button = Button::new();
        button.add_css_class(classes::MEDIA_SWITCHER_BUTTON);
        button.set_focus_on_click(false);
        button.set_tooltip_text(Some(&view.identity));

        let icon = view
            .desktop_entry
            .as_deref()
            .and_then(app_icon::lookup)
            .map(|icon| {
                let image = Image::from_gicon(&icon);
                image.set_pixel_size(SWITCHER_ICON);
                image.upcast::<gtk4::Widget>()
            })
            .unwrap_or_else(|| {
                // No desktop entry: a player is still recognisable by the
                // first letter of the name it gave itself.
                let initial = view
                    .identity
                    .chars()
                    .next()
                    .map(|first| first.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let label = Label::new(Some(&initial));
                label.add_css_class(classes::MEDIA_SWITCHER_INITIAL);
                label.upcast()
            });
        button.set_child(Some(&icon));

        button.connect_clicked({
            let handle = self.services.media.handle().clone();
            let bus_name = view.bus_name.clone();
            move |_| {
                let handle = handle.clone();
                let bus_name = bus_name.clone();
                bridge::act(SCOPE, async move { handle.select_player(bus_name).await });
            }
        });
        button
    }

    // -----------------------------------------------------------------------
    // Album art
    // -----------------------------------------------------------------------

    /// Show `art`, crossfading if it is not what is already there.
    ///
    /// The decode happens on a worker thread — a cover is a JPEG the size of a
    /// window, and the panel's main thread has a frame to draw.
    fn render_art(self: &Rc<Self>, art: Option<&ArtRef>) {
        let key = art.map(|art| art.key);
        if self.art_key.get() == key && (key.is_none() || !self.art.is_empty()) {
            return;
        }
        self.art_key.set(key);

        let generation = self.art_generation.get().wrapping_add(1);
        self.art_generation.set(generation);

        let Some(art) = art.cloned() else {
            self.fade_to(None);
            return;
        };

        if let Some(texture) = cached_texture(art.key) {
            self.fade_to(Some(&texture));
            return;
        }

        let size = ART_SIZE * self.art.scale_factor().max(1);
        let card = Rc::downgrade(self);
        let generations = Rc::clone(&self.art_generation);
        glib::spawn_future_local(async move {
            let path = art.path.clone();
            let decoded = gtk4::gio::spawn_blocking(move || decode(&path, size))
                .await
                .ok()
                .flatten();

            // A newer cover was asked for while this one was being decoded.
            if generations.get() != generation {
                return;
            }
            let Some(card) = card.upgrade() else {
                return;
            };
            match decoded {
                Some(texture) => {
                    remember_texture(art.key, &texture);
                    card.fade_to(Some(&texture));
                }
                None => {
                    debug!("could not read album art from {}", art.path.display());
                    card.fade_to(None);
                }
            }
        });
    }

    /// Crossfade the art to `texture`, or to the placeholder for `None`.
    fn fade_to(&self, texture: Option<&gdk::Texture>) {
        self.art.crossfade_to(texture);
        let art = self.art.clone();
        self.art_fade.start(
            AnimationParams::new(ART_FADE_MS).with_easing(Easing::Linear),
            Box::new(move |progress| art.set_fade(progress)),
            None,
        );
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    /// Wire the three transport buttons to the service.
    fn connect_transport(self: &Rc<Self>) {
        let handle = self.services.media.handle().clone();
        self.previous.connect_clicked({
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                bridge::act(SCOPE, async move { handle.previous().await });
            }
        });
        self.play_pause.connect_clicked({
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                bridge::act(SCOPE, async move { handle.play_pause().await });
            }
        });
        self.next.connect_clicked({
            let handle = handle.clone();
            move |_| {
                let handle = handle.clone();
                bridge::act(SCOPE, async move { handle.next().await });
            }
        });
    }

    /// Wire the seek bar. A drag seeks once, on release.
    ///
    /// Seeking per frame would send a D-Bus call every few milliseconds and
    /// make the player stutter through the whole drag; GNOME's own slider does
    /// the same thing.
    fn connect_seek(self: &Rc<Self>) {
        let buttons = EventControllerLegacy::new();
        buttons.connect_event({
            let dragging = Rc::clone(&self.dragging);
            let pending = Rc::clone(&self.pending_seek);
            let handle = self.services.media.handle().clone();
            move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => dragging.set(true),
                    gdk::EventType::ButtonRelease => {
                        dragging.set(false);
                        if let Some(position) = pending.take() {
                            let handle = handle.clone();
                            bridge::act(SCOPE, async move { handle.seek_to(position).await });
                        }
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        });
        self.seek.add_controller(buttons);

        self.seek.connect_change_value({
            let card = Rc::downgrade(self);
            let dragging = Rc::clone(&self.dragging);
            let pending = Rc::clone(&self.pending_seek);
            let handle = self.services.media.handle().clone();
            move |_, _, value| {
                let position = value as i64;
                if let Some(card) = card.upgrade() {
                    set_text(&card.elapsed, &format_duration(position));
                }
                if dragging.get() {
                    // Banked until the button comes up.
                    pending.set(Some(position));
                } else {
                    // A keyboard or scroll-wheel seek has no release to wait
                    // for, so it goes out at once.
                    let handle = handle.clone();
                    bridge::act(SCOPE, async move { handle.seek_to(position).await });
                }
                glib::Propagation::Proceed
            }
        });
    }

    // -----------------------------------------------------------------------
    // The tick between polls
    // -----------------------------------------------------------------------

    /// Move the seek bar while the panel is open.
    ///
    /// The service polls the player once a second; this fills in the gap from
    /// the last sample so the bar moves rather than steps. It runs only while
    /// the popover is open and only while something is playing, which is the
    /// one place in the panel a repeating timer is allowed.
    fn start_ticking(self: &Rc<Self>) {
        self.stop_ticking();
        let playing = self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|view| view.status == PlaybackStatus::Playing && view.can_seek);
        if !playing {
            return;
        }

        let card = Rc::downgrade(self);
        let source = glib::timeout_add_local(TICK, move || {
            let Some(card) = card.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if card.dragging.get() {
                return glib::ControlFlow::Continue;
            }
            let position = card
                .active
                .borrow()
                .as_ref()
                .filter(|view| view.status == PlaybackStatus::Playing)
                .map(|view| view.position_at(Instant::now()));
            match position {
                Some(position) => {
                    card.show_position(position);
                    glib::ControlFlow::Continue
                }
                None => {
                    card.ticker.replace(None);
                    glib::ControlFlow::Break
                }
            }
        });
        self.ticker.replace(Some(source));
    }

    /// Stop the tick. Called when the panel closes, and when nothing is
    /// playing: an animation nobody can see is an animation that should stop.
    fn stop_ticking(&self) {
        if let Some(source) = self.ticker.take() {
            source.remove();
        }
    }
}

impl Drop for Card {
    fn drop(&mut self) {
        self.stop_ticking();
    }
}

thread_local! {
    /// Decoded covers, keyed the way the service keys them.
    ///
    /// Small on purpose: it exists so that flipping between two players, or a
    /// track repeating, costs no decode — not to hold every cover of the day.
    static TEXTURES: RefCell<Vec<(u64, gdk::Texture)>> = const { RefCell::new(Vec::new()) };
}

/// How many decoded covers are kept in memory.
const TEXTURE_CACHE: usize = 8;

/// A cover that has already been decoded.
fn cached_texture(key: u64) -> Option<gdk::Texture> {
    TEXTURES.with_borrow(|cache| {
        cache
            .iter()
            .find(|(cached, _)| *cached == key)
            .map(|(_, texture)| texture.clone())
    })
}

/// Keep a decoded cover, dropping the oldest once the cache is full.
fn remember_texture(key: u64, texture: &gdk::Texture) {
    TEXTURES.with_borrow_mut(|cache| {
        cache.retain(|(cached, _)| *cached != key);
        cache.insert(0, (key, texture.clone()));
        cache.truncate(TEXTURE_CACHE);
    });
}

/// Read and downscale a cover. Runs off the main thread.
fn decode(path: &std::path::Path, size: i32) -> Option<gdk::Texture> {
    let pixbuf = gtk4::gdk_pixbuf::Pixbuf::from_file_at_scale(path, size, size, true).ok()?;
    Some(gdk::Texture::for_pixbuf(&pixbuf))
}

/// What a player's switcher button is drawn from.
///
/// The name is in it as well as the bus name, because a player is added to the
/// row the moment its name appears on the bus and only says what it is called
/// a moment later — a button rebuilt on bus name alone would keep the
/// stand-in initial for as long as the player ran.
fn switcher_key(view: &PlayerView) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        view.bus_name,
        view.identity,
        view.desktop_entry.as_deref().unwrap_or_default()
    )
}

/// One transport button and the icon inside it.
fn control_button(icon_name: &str, tooltip: &str) -> (Button, Image) {
    let icon = Image::from_icon_name(icon_name);
    let button = Button::new();
    button.set_child(Some(&icon));
    button.add_css_class(classes::MEDIA_CONTROL);
    button.set_focus_on_click(false);
    button.set_tooltip_text(Some(tooltip));
    (button, icon)
}

/// A label for a timestamp: dimmed, and with figures that do not jump.
fn time_label() -> Label {
    let label = Label::new(Some("0:00"));
    label.add_css_class(classes::MEDIA_TIME_LABEL);
    label
}

/// Empty a container.
fn clear(container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Set a label only when the text actually changed: a needless `set_text`
/// costs a relayout of the whole panel.
fn set_text(label: &Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}

/// Microseconds as `m:ss`, or `h:mm:ss` for anything over an hour.
pub fn format_duration(microseconds: i64) -> String {
    let total = (microseconds.max(0) / 1_000_000) as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_the_way_a_player_writes_them() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(1_000_000), "0:01");
        assert_eq!(format_duration(72_000_000), "1:12");
        assert_eq!(format_duration(221_000_000), "3:41");
        assert_eq!(format_duration(3_600_000_000), "1:00:00");
        assert_eq!(format_duration(3_661_000_000), "1:01:01");
    }

    #[test]
    fn a_negative_position_reads_as_the_beginning() {
        // Extrapolation clamps at zero, but a player may report anything.
        assert_eq!(format_duration(-5_000_000), "0:00");
    }

    #[test]
    fn a_part_second_never_rounds_up_past_the_track() {
        // 3:41 of a 3:41 track must not read 3:42 half a second early.
        assert_eq!(format_duration(221_999_999), "3:41");
    }
}
