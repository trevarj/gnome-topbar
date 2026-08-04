//! What the panel draws for one tray item, and how it is chosen.
//!
//! Everything here is pure. The bus side hands in the properties an
//! application published; what comes out is the one icon the widget should
//! draw and the handful of strings around it.

use std::sync::Arc;

/// The whole tray, as the widget sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayState {
    /// Every item worth drawing, in a stable order.
    ///
    /// Sorted by [`ItemView::id`], which is the item's bus name and object
    /// path: icons therefore keep their places for as long as the
    /// applications behind them stay on the bus.
    pub items: Vec<ItemView>,
}

impl TrayState {
    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item with this id, if it is still here.
    pub fn item(&self, id: &str) -> Option<&ItemView> {
        self.items.iter().find(|item| item.id == id)
    }
}

/// One tray icon.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemView {
    /// The item's bus name and object path, e.g. `:1.42/StatusNotifierItem`.
    ///
    /// Both halves are needed: an application may serve several items from one
    /// connection, and `Activate` has to reach the right one.
    pub id: String,
    /// What the application calls itself.
    pub title: String,
    /// Whether the item is idle, active, or shouting.
    pub status: Status,
    /// The tooltip, already composed from its title and its description.
    pub tooltip: Option<String>,
    /// The icon to draw.
    pub icon: IconView,
    /// Whether the item points at a dbusmenu object.
    pub has_menu: bool,
    /// Whether a left click should open the menu rather than activate.
    pub item_is_menu: bool,
}

impl ItemView {
    /// What the tooltip should say, falling back to the item's own name.
    pub fn tooltip_text(&self) -> &str {
        match self.tooltip.as_deref() {
            Some(tooltip) if !tooltip.is_empty() => tooltip,
            _ if !self.title.is_empty() => &self.title,
            _ => &self.id,
        }
    }
}

/// How much an item wants to be noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Idle. Deliberately never published: the specification says a
    /// visualization is likely to hide these, and a quiet panel is the point.
    Passive,
    /// The ordinary state, and what an item that says nothing is assumed to be.
    ///
    /// Defaulting to `Active` rather than `Passive` is deliberate: plenty of
    /// applications never set `Status` at all, and treating silence as "hide
    /// me" would make them disappear from the panel entirely.
    #[default]
    Active,
    /// Something needs the user. Drawn tinted, and pulsed once on arrival.
    NeedsAttention,
}

impl Status {
    /// Parse the `Status` property.
    pub fn parse(value: &str) -> Self {
        match value {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            // Includes "Active" and anything unrecognised: an item the panel
            // cannot classify is shown, not hidden.
            _ => Self::Active,
        }
    }

    /// Whether the panel draws an item in this state at all.
    pub fn is_visible(self) -> bool {
        !matches!(self, Self::Passive)
    }
}

/// The icon an item should be drawn with.
#[derive(Debug, Clone, PartialEq)]
pub enum IconView {
    /// A Freedesktop icon name, optionally with the application's own theme
    /// directory to look in first.
    Themed {
        /// The icon name.
        name: String,
        /// `IconThemePath`, searched before the system themes.
        theme_path: Option<String>,
    },
    /// Pixels the application sent, already the right way round.
    Pixels(Arc<Pixmap>),
    /// Nothing usable was published.
    Fallback,
}

/// The Freedesktop name drawn for an item that published nothing usable.
pub const FALLBACK_ICON: &str = "image-missing-symbolic";

/// An icon an application sent as pixels, converted once at the edge.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixmap {
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// `width * height * 4` bytes of straight-alpha RGBA.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pixmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba", &format_args!("{} bytes", self.rgba.len()))
            .finish()
    }
}

impl Pixmap {
    /// Convert one `(width, height, ARGB32)` triple from the bus.
    ///
    /// `None` for anything that cannot be a picture: a non-positive dimension,
    /// or a buffer too small for the size it claims. Both happen in the wild —
    /// an application that is shutting down publishes a truncated array — and
    /// neither is worth failing the whole item over.
    ///
    /// StatusNotifierItem pixmaps are ARGB32 in network byte order, so the
    /// bytes arrive A, R, G, B and leave R, G, B, A.
    pub fn from_argb(width: i32, height: i32, argb: &[u8]) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let wanted = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if argb.len() < wanted {
            return None;
        }

        let mut rgba = Vec::with_capacity(wanted);
        for pixel in argb[..wanted].chunks_exact(4) {
            rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
        }

        if looks_premultiplied(&rgba) {
            un_premultiply(&mut rgba);
        }

        Some(Self {
            width,
            height,
            rgba,
        })
    }

    /// The longer side, which is what "how big is this icon" means here.
    pub fn size(&self) -> i32 {
        self.width.max(self.height)
    }
}

/// Whether a buffer looks like it carries premultiplied alpha.
///
/// The specification says ARGB32, which is straight alpha, but toolkits that
/// keep their icons in a premultiplied surface sometimes publish them as they
/// lie. The two are told apart by the one thing premultiplication guarantees:
/// no channel may exceed its own alpha. A straight-alpha icon with antialiased
/// edges breaks that on its very first edge pixel, because an edge is full
/// colour at low alpha.
///
/// A buffer with no partly transparent pixel at all is identical either way,
/// so it is left alone.
fn looks_premultiplied(rgba: &[u8]) -> bool {
    let mut translucent = false;
    for pixel in rgba.chunks_exact(4) {
        let alpha = pixel[3];
        if alpha == 0 {
            // A fully transparent pixel is premultiplied to black; one that
            // kept its colour proves the buffer is straight.
            if pixel[0] | pixel[1] | pixel[2] != 0 {
                return false;
            }
            continue;
        }
        if alpha == u8::MAX {
            continue;
        }
        translucent = true;
        if pixel[0] > alpha || pixel[1] > alpha || pixel[2] > alpha {
            return false;
        }
    }
    translucent
}

/// Undo premultiplication in place.
fn un_premultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha == 0 || alpha == u8::MAX {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * 255 + u16::from(alpha) / 2) / u16::from(alpha))
                .min(255) as u8;
        }
    }
}

/// Pick the pixmap closest to `target` without going under it.
///
/// Scaling an icon up is what makes a tray look cheap, so the smallest pixmap
/// that is at least as big as the panel wants wins. When every pixmap is too
/// small the largest one is used, because there is nothing better.
pub fn nearest(pixmaps: &[Pixmap], target: i32) -> Option<&Pixmap> {
    pixmaps
        .iter()
        .filter(|pixmap| pixmap.size() >= target)
        .min_by_key(|pixmap| pixmap.size())
        .or_else(|| pixmaps.iter().max_by_key(|pixmap| pixmap.size()))
}

/// Which way a scroll went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// The usual one: a wheel.
    Vertical,
    /// Sideways, which a few applications map to something of their own.
    Horizontal,
}

impl ScrollAxis {
    /// The name the protocol uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }
}

/// Everything about an item's appearance, straight off the bus.
#[derive(Debug, Clone, Default)]
pub struct IconSource {
    /// `IconName`.
    pub icon_name: Option<String>,
    /// `IconPixmap`, already converted.
    pub icon_pixmap: Vec<Pixmap>,
    /// `AttentionIconName`.
    pub attention_icon_name: Option<String>,
    /// `AttentionIconPixmap`, already converted.
    pub attention_icon_pixmap: Vec<Pixmap>,
    /// `IconThemePath`.
    pub theme_path: Option<String>,
}

/// Choose the icon to draw, in the order the specification recommends.
///
/// A name first — a themed icon follows the user's icon theme and their
/// scaling factor, and pixmaps do neither — then the pixels the application
/// sent, and only then the placeholder. An item shouting for attention offers
/// a second set of both, which is preferred while it is shouting.
pub fn resolve(source: &IconSource, status: Status, target: i32) -> IconView {
    let attention = status == Status::NeedsAttention;

    let names: [&Option<String>; 2] = if attention {
        [&source.attention_icon_name, &source.icon_name]
    } else {
        [&source.icon_name, &source.attention_icon_name]
    };
    for name in names {
        if let Some(name) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            return IconView::Themed {
                name: name.to_string(),
                theme_path: source
                    .theme_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(ToString::to_string),
            };
        }
    }

    let pixmaps: [&Vec<Pixmap>; 2] = if attention {
        [&source.attention_icon_pixmap, &source.icon_pixmap]
    } else {
        [&source.icon_pixmap, &source.attention_icon_pixmap]
    };
    for set in pixmaps {
        if let Some(pixmap) = nearest(set, target) {
            return IconView::Pixels(Arc::new(pixmap.clone()));
        }
    }

    IconView::Fallback
}

/// Split an item's identifier into the bus name and the object path.
///
/// An application may register itself by bus name alone, by object path alone
/// (in which case the sender is the bus name), or by both stuck together. All
/// three are in the wild, and all three end up here as one pair.
pub fn split_id(id: &str) -> Option<(String, String)> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    match id.split_once('/') {
        None => Some((id.to_string(), "/StatusNotifierItem".to_string())),
        Some((bus_name, path)) if !bus_name.is_empty() => {
            Some((bus_name.to_string(), format!("/{path}")))
        }
        Some(_) => None,
    }
}

/// Build the identifier an item is known by from its two halves.
pub fn make_id(bus_name: &str, path: &str) -> String {
    format!("{bus_name}{path}")
}

/// Compose the tooltip from the `(icon, pixmaps, title, description)` struct.
pub fn tooltip(title: &str, description: &str) -> Option<String> {
    let title = title.trim();
    let description = description.trim();
    match (title.is_empty(), description.is_empty()) {
        (true, true) => None,
        (true, false) => Some(description.to_string()),
        (false, true) => Some(title.to_string()),
        (false, false) => Some(format!("{title}\n{description}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One opaque pixel, as it arrives: alpha, red, green, blue.
    fn argb(pixels: &[[u8; 4]]) -> Vec<u8> {
        pixels.iter().flatten().copied().collect()
    }

    #[test]
    fn argb_becomes_rgba_in_the_order_gtk_wants() {
        let data = argb(&[[0xff, 0x11, 0x22, 0x33], [0x80, 0xff, 0xff, 0xff]]);
        let pixmap = Pixmap::from_argb(2, 1, &data).expect("a two-pixel picture");
        assert_eq!(pixmap.width, 2);
        assert_eq!(pixmap.height, 1);
        assert_eq!(
            pixmap.rgba,
            vec![0x11, 0x22, 0x33, 0xff, 0xff, 0xff, 0xff, 0x80]
        );
    }

    #[test]
    fn a_non_square_pixmap_keeps_both_of_its_dimensions() {
        // The bug that sent the `system-tray` crate back: it read the height
        // out of the width field, so this arrived as 4x4.
        let data = vec![0u8; 4 * 2 * 4];
        let pixmap = Pixmap::from_argb(4, 2, &data).expect("a 4x2 picture");
        assert_eq!((pixmap.width, pixmap.height), (4, 2));
        assert_eq!(pixmap.rgba.len(), 4 * 2 * 4);
    }

    #[test]
    fn a_malformed_pixmap_is_skipped_rather_than_guessed_at() {
        assert!(Pixmap::from_argb(0, 16, &[0; 64]).is_none(), "no width");
        assert!(Pixmap::from_argb(16, -1, &[0; 64]).is_none(), "no height");
        assert!(
            Pixmap::from_argb(16, 16, &[0; 64]).is_none(),
            "16x16 needs 1024 bytes, not 64"
        );
    }

    #[test]
    fn a_pixmap_longer_than_it_claims_is_read_to_its_stated_size() {
        // Some applications pad the array. The extra is ignored, not refused.
        let data = vec![0xffu8; 4 + 17];
        let pixmap = Pixmap::from_argb(1, 1, &data).expect("a one-pixel picture");
        assert_eq!(pixmap.rgba.len(), 4);
    }

    #[test]
    fn straight_alpha_is_left_exactly_as_it_arrived() {
        // A hard edge: full colour at half alpha, which premultiplied data
        // cannot contain.
        let data = argb(&[[0x80, 0xff, 0x00, 0x00], [0xff, 0x00, 0x00, 0xff]]);
        let pixmap = Pixmap::from_argb(2, 1, &data).expect("two pixels");
        assert_eq!(pixmap.rgba[..4], [0xff, 0x00, 0x00, 0x80]);
    }

    #[test]
    fn premultiplied_alpha_is_undone_so_edges_are_not_drawn_twice() {
        // Half-alpha red, premultiplied: 0x80 rather than 0xff in the red.
        let data = argb(&[[0x80, 0x80, 0x00, 0x00], [0xff, 0x00, 0x00, 0xff]]);
        let pixmap = Pixmap::from_argb(2, 1, &data).expect("two pixels");
        assert_eq!(
            pixmap.rgba[..4],
            [0xff, 0x00, 0x00, 0x80],
            "the red is restored to full strength"
        );
        assert_eq!(
            pixmap.rgba[4..],
            [0x00, 0x00, 0xff, 0xff],
            "an opaque pixel is untouched either way"
        );
    }

    #[test]
    fn a_fully_opaque_icon_is_never_treated_as_premultiplied() {
        let data = argb(&[[0xff, 0x10, 0x20, 0x30], [0xff, 0x40, 0x50, 0x60]]);
        let pixmap = Pixmap::from_argb(2, 1, &data).expect("two pixels");
        assert_eq!(pixmap.rgba[..4], [0x10, 0x20, 0x30, 0xff]);
    }

    fn pixmap(size: i32) -> Pixmap {
        Pixmap {
            width: size,
            height: size,
            rgba: vec![0; (size * size * 4) as usize],
        }
    }

    #[test]
    fn the_smallest_pixmap_that_is_big_enough_wins() {
        let set = [pixmap(16), pixmap(22), pixmap(48)];
        assert_eq!(nearest(&set, 18).expect("a pixmap").size(), 22);
        assert_eq!(nearest(&set, 16).expect("a pixmap").size(), 16);
        assert_eq!(nearest(&set, 22).expect("a pixmap").size(), 22);
    }

    #[test]
    fn an_icon_that_is_too_small_everywhere_uses_its_biggest() {
        let set = [pixmap(8), pixmap(16)];
        assert_eq!(nearest(&set, 32).expect("a pixmap").size(), 16);
    }

    #[test]
    fn a_non_square_pixmap_is_measured_by_its_longer_side() {
        let wide = Pixmap {
            width: 24,
            height: 12,
            rgba: vec![0; 24 * 12 * 4],
        };
        assert_eq!(wide.size(), 24);
        assert!(nearest(&[wide], 18).is_some());
    }

    #[test]
    fn nothing_to_choose_from_chooses_nothing() {
        assert!(nearest(&[], 18).is_none());
    }

    #[test]
    fn a_name_is_preferred_to_pixels() {
        let source = IconSource {
            icon_name: Some("firefox".into()),
            icon_pixmap: vec![pixmap(22)],
            theme_path: Some("/opt/app/icons".into()),
            ..IconSource::default()
        };
        assert_eq!(
            resolve(&source, Status::Active, 18),
            IconView::Themed {
                name: "firefox".into(),
                theme_path: Some("/opt/app/icons".into()),
            }
        );
    }

    #[test]
    fn pixels_are_used_when_there_is_no_name() {
        let source = IconSource {
            icon_pixmap: vec![pixmap(16), pixmap(24)],
            ..IconSource::default()
        };
        let IconView::Pixels(chosen) = resolve(&source, Status::Active, 18) else {
            panic!("pixels should have been chosen");
        };
        assert_eq!(chosen.size(), 24);
    }

    #[test]
    fn an_item_that_published_nothing_gets_the_placeholder() {
        let source = IconSource {
            icon_name: Some("   ".into()),
            ..IconSource::default()
        };
        assert_eq!(resolve(&source, Status::Active, 18), IconView::Fallback);
    }

    #[test]
    fn an_empty_theme_path_is_not_a_theme_path() {
        let source = IconSource {
            icon_name: Some("firefox".into()),
            theme_path: Some("  ".into()),
            ..IconSource::default()
        };
        assert_eq!(
            resolve(&source, Status::Active, 18),
            IconView::Themed {
                name: "firefox".into(),
                theme_path: None
            }
        );
    }

    #[test]
    fn an_item_wanting_attention_shows_its_attention_icon() {
        let source = IconSource {
            icon_name: Some("quiet".into()),
            attention_icon_name: Some("loud".into()),
            ..IconSource::default()
        };
        assert_eq!(
            resolve(&source, Status::NeedsAttention, 18),
            IconView::Themed {
                name: "loud".into(),
                theme_path: None
            }
        );
        assert_eq!(
            resolve(&source, Status::Active, 18),
            IconView::Themed {
                name: "quiet".into(),
                theme_path: None
            }
        );
    }

    #[test]
    fn attention_falls_back_to_the_ordinary_icon() {
        let source = IconSource {
            icon_name: Some("quiet".into()),
            ..IconSource::default()
        };
        assert_eq!(
            resolve(&source, Status::NeedsAttention, 18),
            IconView::Themed {
                name: "quiet".into(),
                theme_path: None
            },
            "an item with no attention icon keeps the one it has"
        );
    }

    #[test]
    fn status_defaults_to_shown_rather_than_hidden() {
        assert_eq!(Status::parse("Active"), Status::Active);
        assert_eq!(Status::parse("Passive"), Status::Passive);
        assert_eq!(Status::parse("NeedsAttention"), Status::NeedsAttention);
        assert_eq!(
            Status::parse(""),
            Status::Active,
            "an item that says nothing is still an item"
        );
        assert_eq!(Status::parse("nonsense"), Status::Active);
        assert!(!Status::Passive.is_visible());
        assert!(Status::Active.is_visible() && Status::NeedsAttention.is_visible());
    }

    #[test]
    fn an_identifier_splits_into_a_name_and_a_path() {
        assert_eq!(
            split_id(":1.58/StatusNotifierItem"),
            Some((":1.58".into(), "/StatusNotifierItem".into()))
        );
        assert_eq!(
            split_id("org.example.App"),
            Some(("org.example.App".into(), "/StatusNotifierItem".into())),
            "a bare bus name gets the default path"
        );
        assert_eq!(
            split_id(":1.72/org/ayatana/NotificationItem/dropbox_client_1"),
            Some((
                ":1.72".into(),
                "/org/ayatana/NotificationItem/dropbox_client_1".into()
            )),
            "an application may live anywhere it likes"
        );
        assert_eq!(split_id(""), None);
        assert_eq!(split_id("/StatusNotifierItem"), None, "no bus name");
    }

    #[test]
    fn scroll_axes_use_the_names_the_protocol_does() {
        assert_eq!(ScrollAxis::Vertical.as_str(), "vertical");
        assert_eq!(ScrollAxis::Horizontal.as_str(), "horizontal");
    }

    #[test]
    fn a_tooltip_is_its_title_and_its_description() {
        assert_eq!(
            tooltip("Syncthing", "Up to date"),
            Some("Syncthing\nUp to date".into())
        );
        assert_eq!(tooltip("Syncthing", "  "), Some("Syncthing".into()));
        assert_eq!(tooltip("", "Up to date"), Some("Up to date".into()));
        assert_eq!(tooltip("", ""), None);
    }

    #[test]
    fn an_item_with_no_tooltip_falls_back_to_its_name() {
        let mut item = ItemView {
            id: ":1.4/StatusNotifierItem".into(),
            title: "Syncthing".into(),
            status: Status::Active,
            tooltip: None,
            icon: IconView::Fallback,
            has_menu: false,
            item_is_menu: false,
        };
        assert_eq!(item.tooltip_text(), "Syncthing");
        item.title = String::new();
        assert_eq!(item.tooltip_text(), ":1.4/StatusNotifierItem");
        item.tooltip = Some("Up to date".into());
        assert_eq!(item.tooltip_text(), "Up to date");
    }
}
