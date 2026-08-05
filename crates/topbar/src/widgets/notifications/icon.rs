//! Picking an icon for a notification.
//!
//! A sender may describe its icon five different ways, and most senders use
//! more than one at once. The order is the specification's, from most specific
//! to least: the pixels it attached, the image it pointed at, the icon name it
//! gave, the icon its desktop entry declares, and finally a generic glyph so
//! that a badly behaved sender still gets a row that looks like the others.
//!
//! [`candidates`] is the decision and is pure; [`image`] carries it out, and is
//! the only part that needs an icon theme and a GDK texture. It is also the
//! only part that can tell whether a name resolves to anything, which is why it
//! is given the whole list rather than one answer.

use gtk4::prelude::*;
use gtk4::{Image, gdk, glib};
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
}

/// Everywhere the icon could come from, most specific first.
///
/// A list rather than one answer, because "most specific" and "actually
/// resolvable" are different questions and only the second one can be asked of
/// a widget. Telegram sends `app_icon = "telegram"`; Adwaita 50 dropped the
/// whole family of legacy application names, so that is a name nothing on the
/// machine has — and a `GtkImage` given a name the theme does not know draws
/// GTK's broken-image glyph and keeps it. Every notification from every
/// application whose icon is not installed looked like a failed download.
pub fn candidates(source: &IconSource) -> Vec<Choice> {
    let mut found = Vec::new();
    if source.image_data.is_some() {
        found.push(Choice::Pixels);
    }
    // `image-path` and `app_icon` are both allowed to be a path *or* a theme
    // name, and senders use both spellings freely.
    found.extend(source.image_path.as_deref().and_then(reference));
    found.extend(reference(&source.app_icon));
    found.extend(
        source
            .desktop_entry
            .as_deref()
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| Choice::Entry(entry.to_string())),
    );
    found
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
///
/// The first candidate that actually resolves wins. A sender naming five
/// things, four of which are not installed, still gets an icon that means
/// something rather than the first one that happened to be listed.
pub fn image(source: &IconSource, size: i32) -> Image {
    let image = candidates(source)
        .iter()
        .find_map(|choice| resolve(choice, source))
        .unwrap_or_else(|| Image::from_icon_name(FALLBACK));
    image.set_pixel_size(size);
    image
}

/// Build the widget for one candidate, or `None` if it is not there.
fn resolve(choice: &Choice, source: &IconSource) -> Option<Image> {
    match choice {
        Choice::Pixels => source
            .image_data
            .as_deref()
            .and_then(texture)
            .map(|texture| Image::from_paintable(Some(&texture))),
        // A path is checked rather than trusted: `image-path` routinely points
        // at an avatar in a cache the sending application has since cleared.
        Choice::File(path) => std::path::Path::new(path)
            .is_file()
            .then(|| Image::from_file(path)),
        Choice::Name(name) => has_icon(name).then(|| Image::from_icon_name(name)),
        Choice::Entry(entry) => {
            crate::widgets::app_icon::lookup(entry).map(|icon| Image::from_gicon(&icon))
        }
    }
}

/// Whether the icon theme in use has an icon by that name.
pub fn has_icon(name: &str) -> bool {
    gdk::Display::default()
        .map(|display| gtk4::IconTheme::for_display(&display))
        .is_some_and(|theme| theme.has_icon(name))
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

    /// What the sender most wants used, which is where resolution starts.
    fn first(source: &IconSource) -> Option<Choice> {
        candidates(source).into_iter().next()
    }

    #[test]
    fn attached_pixels_beat_everything_else() {
        let source = IconSource {
            image_data: Some(pixels()),
            image_path: Some("/tmp/avatar.png".into()),
            app_icon: "org.gnome.Fractal".into(),
            desktop_entry: Some("org.gnome.Fractal".into()),
        };
        assert_eq!(first(&source), Some(Choice::Pixels));
    }

    #[test]
    fn the_image_path_beats_the_icon_name() {
        let source = IconSource {
            image_path: Some("/tmp/avatar.png".into()),
            app_icon: "org.gnome.Fractal".into(),
            ..IconSource::default()
        };
        assert_eq!(first(&source), Some(Choice::File("/tmp/avatar.png".into())));
    }

    #[test]
    fn a_file_uri_becomes_a_path() {
        let source = IconSource {
            image_path: Some("file:///home/ada/avatar.png".into()),
            ..IconSource::default()
        };
        assert_eq!(
            first(&source),
            Some(Choice::File("/home/ada/avatar.png".into())),
            "the URI scheme is stripped, not passed to the icon theme"
        );
    }

    #[test]
    fn an_image_path_may_also_be_a_theme_name() {
        let source = IconSource {
            image_path: Some("mail-unread-symbolic".into()),
            ..IconSource::default()
        };
        assert_eq!(
            first(&source),
            Some(Choice::Name("mail-unread-symbolic".into()))
        );
    }

    #[test]
    fn the_icon_name_beats_the_desktop_entry() {
        let source = IconSource {
            app_icon: "mail-unread-symbolic".into(),
            desktop_entry: Some("org.gnome.Fractal".into()),
            ..IconSource::default()
        };
        assert_eq!(
            first(&source),
            Some(Choice::Name("mail-unread-symbolic".into()))
        );
    }

    #[test]
    fn an_absolute_app_icon_is_read_as_a_path() {
        let source = IconSource {
            app_icon: "/usr/share/pixmaps/thing.png".into(),
            ..IconSource::default()
        };
        assert_eq!(
            first(&source),
            Some(Choice::File("/usr/share/pixmaps/thing.png".into()))
        );
    }

    #[test]
    fn the_desktop_entry_is_the_last_thing_tried() {
        let source = IconSource {
            desktop_entry: Some("org.telegram.desktop".into()),
            ..IconSource::default()
        };
        assert_eq!(
            first(&source),
            Some(Choice::Entry("org.telegram.desktop".into()))
        );
    }

    #[test]
    fn every_source_the_sender_named_is_kept_as_a_candidate() {
        // The order is the specification's, and all of it survives: a name the
        // icon theme has never heard of has to be able to fall through to the
        // desktop entry behind it rather than ending the search.
        let source = IconSource {
            image_data: Some(pixels()),
            image_path: Some("/tmp/avatar.png".into()),
            app_icon: "telegram".into(),
            desktop_entry: Some("org.telegram.desktop".into()),
        };
        assert_eq!(
            candidates(&source),
            vec![
                Choice::Pixels,
                Choice::File("/tmp/avatar.png".into()),
                Choice::Name("telegram".into()),
                Choice::Entry("org.telegram.desktop".into()),
            ]
        );
    }

    #[test]
    fn a_notification_describing_nothing_names_no_candidate_at_all() {
        // Which is what puts [`FALLBACK`] on screen: `image` runs out of
        // candidates rather than being handed a special one.
        assert!(candidates(&IconSource::default()).is_empty());

        let blank = IconSource {
            image_path: Some("  ".into()),
            app_icon: String::new(),
            desktop_entry: Some(String::new()),
            ..IconSource::default()
        };
        assert!(candidates(&blank).is_empty(), "blank fields are no fields");
    }
}
