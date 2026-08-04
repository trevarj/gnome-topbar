//! Picking an icon for a notification.
//!
//! A sender may describe its icon five different ways, and most senders use
//! more than one at once. The order is the specification's, from most specific
//! to least: the pixels it attached, the image it pointed at, the icon name it
//! gave, the icon its desktop entry declares, and finally a generic glyph so
//! that a badly behaved sender still gets a row that looks like the others.
//!
//! [`choose`] is the decision and is pure; [`image`] carries it out, which is
//! the only part that needs an icon theme and a GDK texture.

use gtk4::prelude::*;
use gtk4::{Image, gdk, gio, glib};
use topbar_services::{IconSource, ImageData};

/// What a notification with nothing identifiable about it gets.
pub const FALLBACK: &str = "application-x-executable-symbolic";

/// Where the icon is actually coming from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Pixels the sender attached to the notification.
    Pixels,
    /// A file on disk.
    File(String),
    /// A name in the icon theme.
    Name(String),
    /// The application's desktop entry, which declares its own icon.
    Entry(String),
    /// Nothing identifiable: [`FALLBACK`].
    None,
}

/// Decide where a notification's icon comes from.
pub fn choose(source: &IconSource) -> Choice {
    if source.image_data.is_some() {
        return Choice::Pixels;
    }
    // `image-path` and `app_icon` are both allowed to be a path *or* a theme
    // name, and senders use both spellings freely.
    if let Some(choice) = source.image_path.as_deref().and_then(reference) {
        return choice;
    }
    if let Some(choice) = reference(&source.app_icon) {
        return choice;
    }
    if let Some(entry) = source
        .desktop_entry
        .as_deref()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        return Choice::Entry(entry.to_string());
    }
    Choice::None
}

/// Read one of the string icon fields, which may be a URI, a path, or a name.
fn reference(value: &str) -> Option<Choice> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(path) = value.strip_prefix("file://") {
        return Some(Choice::File(path.to_string()));
    }
    if value.starts_with('/') {
        return Some(Choice::File(value.to_string()));
    }
    Some(Choice::Name(value.to_string()))
}

/// Build the icon widget for `source` at `size` pixels.
pub fn image(source: &IconSource, size: i32) -> Image {
    let image = match choose(source) {
        Choice::Pixels => source
            .image_data
            .as_deref()
            .and_then(texture)
            .map(|texture| Image::from_paintable(Some(&texture)))
            .unwrap_or_else(|| Image::from_icon_name(FALLBACK)),
        Choice::File(path) => Image::from_file(path),
        Choice::Name(name) => Image::from_icon_name(&name),
        Choice::Entry(entry) => entry_icon(&entry)
            .map(|icon| Image::from_gicon(&icon))
            .unwrap_or_else(|| Image::from_icon_name(FALLBACK)),
        Choice::None => Image::from_icon_name(FALLBACK),
    };
    image.set_pixel_size(size);
    image
}

thread_local! {
    /// Desktop entries already looked up, including the ones that came to
    /// nothing.
    ///
    /// The lookup is a scan of every installed application, and a chat client
    /// sending fifty messages must not pay for fifty of them. The set of
    /// distinct senders on one desktop is small, so the cache is too.
    static ENTRY_ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<gio::Icon>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The icon an application's desktop entry declares.
fn entry_icon(entry: &str) -> Option<gio::Icon> {
    ENTRY_ICONS.with_borrow_mut(|cache| {
        cache
            .entry(entry.to_string())
            .or_insert_with(|| look_up_entry(entry))
            .clone()
    })
}

/// Find `entry` among the installed applications.
///
/// The hint is documented as the entry's name without the `.desktop` suffix
/// but plenty of senders include it, and a few get the case wrong, so the
/// comparison forgives both.
fn look_up_entry(entry: &str) -> Option<gio::Icon> {
    let wanted = entry.strip_suffix(".desktop").unwrap_or(entry);
    gio::AppInfo::all()
        .into_iter()
        .find(|info| {
            info.id().is_some_and(|id| {
                let id = id.as_str();
                id.strip_suffix(".desktop")
                    .unwrap_or(id)
                    .eq_ignore_ascii_case(wanted)
            })
        })
        .and_then(|info| info.icon())
}

/// Turn raw notification pixels into a texture.
///
/// The freedesktop format is RGB(A) with an explicit rowstride — not the
/// pre-multiplied ARGB the tray uses — so the bytes go through unchanged. The
/// daemon has already checked that the buffer covers the geometry, which is
/// what makes reading `rowstride * height` bytes here safe.
fn texture(pixels: &ImageData) -> Option<gdk::Texture> {
    let format = if pixels.has_alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let bytes = glib::Bytes::from(&pixels.data[..]);
    let texture = gdk::MemoryTexture::new(
        pixels.width,
        pixels.height,
        format,
        &bytes,
        pixels.rowstride as usize,
    );
    Some(texture.upcast())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn pixels() -> Arc<ImageData> {
        Arc::new(ImageData {
            width: 1,
            height: 1,
            rowstride: 4,
            has_alpha: true,
            bits_per_sample: 8,
            channels: 4,
            data: vec![1, 2, 3, 4],
        })
    }

    #[test]
    fn attached_pixels_beat_everything_else() {
        let source = IconSource {
            image_data: Some(pixels()),
            image_path: Some("/tmp/avatar.png".into()),
            app_icon: "org.gnome.Fractal".into(),
            desktop_entry: Some("org.gnome.Fractal".into()),
        };
        assert_eq!(choose(&source), Choice::Pixels);
    }

    #[test]
    fn the_image_path_beats_the_icon_name() {
        let source = IconSource {
            image_path: Some("/tmp/avatar.png".into()),
            app_icon: "org.gnome.Fractal".into(),
            ..IconSource::default()
        };
        assert_eq!(choose(&source), Choice::File("/tmp/avatar.png".into()));
    }

    #[test]
    fn a_file_uri_becomes_a_path() {
        let source = IconSource {
            image_path: Some("file:///home/ada/avatar.png".into()),
            ..IconSource::default()
        };
        assert_eq!(
            choose(&source),
            Choice::File("/home/ada/avatar.png".into()),
            "the URI scheme is stripped, not passed to the icon theme"
        );
    }

    #[test]
    fn an_image_path_may_also_be_a_theme_name() {
        let source = IconSource {
            image_path: Some("mail-unread-symbolic".into()),
            ..IconSource::default()
        };
        assert_eq!(choose(&source), Choice::Name("mail-unread-symbolic".into()));
    }

    #[test]
    fn the_icon_name_beats_the_desktop_entry() {
        let source = IconSource {
            app_icon: "mail-unread-symbolic".into(),
            desktop_entry: Some("org.gnome.Fractal".into()),
            ..IconSource::default()
        };
        assert_eq!(choose(&source), Choice::Name("mail-unread-symbolic".into()));
    }

    #[test]
    fn an_absolute_app_icon_is_read_as_a_path() {
        let source = IconSource {
            app_icon: "/usr/share/pixmaps/thing.png".into(),
            ..IconSource::default()
        };
        assert_eq!(
            choose(&source),
            Choice::File("/usr/share/pixmaps/thing.png".into())
        );
    }

    #[test]
    fn the_desktop_entry_is_the_last_thing_tried() {
        let source = IconSource {
            desktop_entry: Some("org.telegram.desktop".into()),
            ..IconSource::default()
        };
        assert_eq!(
            choose(&source),
            Choice::Entry("org.telegram.desktop".into())
        );
    }

    #[test]
    fn a_notification_describing_nothing_gets_the_generic_icon() {
        assert_eq!(choose(&IconSource::default()), Choice::None);

        let blank = IconSource {
            image_path: Some("  ".into()),
            app_icon: "".into(),
            desktop_entry: Some(String::new()),
            ..IconSource::default()
        };
        assert_eq!(choose(&blank), Choice::None, "blank fields are no fields");
    }
}
