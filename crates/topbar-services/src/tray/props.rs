//! The `org.kde.StatusNotifierItem` property dictionary, read once.
//!
//! One `GetAll` per item per burst rather than a `Get` per property: an
//! application that has just started is busy, and every round trip is one more
//! chance to be left waiting on it.

use std::collections::HashMap;

use tracing::warn;
use zbus::zvariant::{Array, OwnedValue, Structure};

use super::model::{IconSource, Pixmap, Status, tooltip};

/// The interface the properties belong to.
pub(super) const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";

/// Everything the panel reads off one item.
#[derive(Debug, Clone, Default)]
pub(super) struct ItemProps {
    /// `Title`, or the empty string.
    pub title: String,
    /// `Status`, defaulting to visible.
    pub status: Status,
    /// `ToolTip`, composed into one string.
    pub tooltip: Option<String>,
    /// Everything that decides which icon is drawn.
    pub icon: IconSource,
    /// `Menu`, when the item points at a dbusmenu object.
    pub menu_path: Option<String>,
    /// `ItemIsMenu`.
    pub item_is_menu: bool,
}

/// Read one item's properties.
///
/// Nothing here fails: an application that publishes a property of the wrong
/// type gets the default for it, because the alternative is an icon that never
/// appears because one field was malformed.
pub(super) fn parse(properties: &HashMap<String, OwnedValue>, id: &str) -> ItemProps {
    ItemProps {
        title: string(properties, "Title").unwrap_or_default(),
        status: string(properties, "Status")
            .map_or(Status::default(), |status| Status::parse(&status)),
        tooltip: properties.get("ToolTip").and_then(read_tooltip),
        icon: IconSource {
            icon_name: string(properties, "IconName"),
            icon_pixmap: pixmaps(properties, "IconPixmap", id),
            attention_icon_name: string(properties, "AttentionIconName"),
            attention_icon_pixmap: pixmaps(properties, "AttentionIconPixmap", id),
            theme_path: string(properties, "IconThemePath"),
        },
        // Published as an object path, but enough applications send a plain
        // string that refusing one would cost real menus. `/` is how an
        // application with no menu says so — an object path may not be empty,
        // so the root is the only "nothing" it can send.
        menu_path: object_path(properties, "Menu")
            .or_else(|| string(properties, "Menu"))
            .filter(|path| path != "/" && !path.is_empty()),
        item_is_menu: properties
            .get("ItemIsMenu")
            .and_then(|value| value.downcast_ref::<bool>().ok())
            .unwrap_or_default(),
    }
}

/// A string property.
fn string(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|value| value.downcast_ref::<&str>().ok())
        .map(ToString::to_string)
}

/// An object-path property.
fn object_path(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|value| value.downcast_ref::<zbus::zvariant::ObjectPath<'_>>().ok())
        .map(|path| path.as_str().to_string())
}

/// Compose the tooltip out of `(icon_name, icon_data, title, description)`.
fn read_tooltip(value: &OwnedValue) -> Option<String> {
    let Ok(structure) = value.downcast_ref::<Structure<'_>>() else {
        // A few applications publish a bare string here. It is a tooltip
        // either way, and refusing it would lose a real one.
        return value
            .downcast_ref::<&str>()
            .ok()
            .filter(|text| !text.trim().is_empty())
            .map(ToString::to_string);
    };
    let fields = structure.fields();
    let title = fields
        .get(2)
        .and_then(|field| field.downcast_ref::<&str>().ok())
        .unwrap_or_default();
    let description = fields
        .get(3)
        .and_then(|field| field.downcast_ref::<&str>().ok())
        .unwrap_or_default();
    tooltip(title, description)
}

/// Read one `a(iiay)` pixmap array.
///
/// Entries that cannot be a picture are dropped with a warning rather than
/// taking the rest of the set down with them: applications publish truncated
/// arrays while they are shutting down, and one bad size in a set of four is
/// no reason to fall back to a placeholder.
fn pixmaps(properties: &HashMap<String, OwnedValue>, key: &str, id: &str) -> Vec<Pixmap> {
    let Some(array) = properties
        .get(key)
        .and_then(|value| value.downcast_ref::<Array<'_>>().ok())
    else {
        return Vec::new();
    };

    let mut pixmaps = Vec::new();
    for entry in array.iter() {
        let Ok(structure) = entry.downcast_ref::<Structure<'_>>() else {
            continue;
        };
        let fields = structure.fields();
        let width = fields
            .first()
            .and_then(|field| field.downcast_ref::<i32>().ok())
            .unwrap_or_default();
        // The second field, and it has to be said out loud: the crate this
        // module replaced read the height out of the first one, so every
        // non-square icon arrived square and every buffer was the wrong size.
        let height = fields
            .get(1)
            .and_then(|field| field.downcast_ref::<i32>().ok())
            .unwrap_or_default();
        let Some(bytes) = fields
            .get(2)
            .and_then(|field| field.downcast_ref::<Array<'_>>().ok())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|byte| byte.downcast_ref::<u8>().ok())
                    .collect::<Vec<u8>>()
            })
        else {
            continue;
        };

        match Pixmap::from_argb(width, height, &bytes) {
            Some(pixmap) => pixmaps.push(pixmap),
            None => warn!(
                "{id}: skipping a malformed {key} entry: {width}x{height} in {} bytes",
                bytes.len()
            ),
        }
    }
    pixmaps
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{ObjectPath, Value};

    fn dict(entries: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        entries
            .into_iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    OwnedValue::try_from(value).expect("a fixture value can be owned"),
                )
            })
            .collect()
    }

    /// One `(width, height, ARGB bytes)` pixmap, as an application sends it.
    fn pixmap_entry(width: i32, height: i32, bytes: usize) -> Value<'static> {
        Value::from(zbus::zvariant::Structure::from((
            width,
            height,
            vec![0xffu8; bytes],
        )))
    }

    #[test]
    fn a_full_item_reads_every_field() {
        let properties = dict(vec![
            ("Title", "Syncthing".into()),
            ("Status", "NeedsAttention".into()),
            ("IconName", "syncthing".into()),
            ("IconThemePath", "/opt/syncthing/icons".into()),
            ("ItemIsMenu", true.into()),
            (
                "Menu",
                Value::ObjectPath(ObjectPath::try_from("/MenuBar").expect("a path")),
            ),
            (
                "ToolTip",
                Value::from(zbus::zvariant::Structure::from((
                    String::new(),
                    Vec::<(i32, i32, Vec<u8>)>::new(),
                    "Syncthing".to_string(),
                    "Up to date".to_string(),
                ))),
            ),
        ]);

        let parsed = parse(&properties, ":1.2/StatusNotifierItem");
        assert_eq!(parsed.title, "Syncthing");
        assert_eq!(parsed.status, Status::NeedsAttention);
        assert_eq!(parsed.tooltip.as_deref(), Some("Syncthing\nUp to date"));
        assert_eq!(parsed.icon.icon_name.as_deref(), Some("syncthing"));
        assert_eq!(
            parsed.icon.theme_path.as_deref(),
            Some("/opt/syncthing/icons")
        );
        assert_eq!(parsed.menu_path.as_deref(), Some("/MenuBar"));
        assert!(parsed.item_is_menu);
    }

    #[test]
    fn an_item_that_publishes_nothing_still_parses() {
        let parsed = parse(&HashMap::new(), ":1.2/StatusNotifierItem");
        assert_eq!(parsed.title, "");
        assert_eq!(
            parsed.status,
            Status::Active,
            "silence must not hide an item"
        );
        assert_eq!(parsed.tooltip, None);
        assert_eq!(parsed.menu_path, None);
        assert!(!parsed.item_is_menu);
        assert!(parsed.icon.icon_pixmap.is_empty());
    }

    #[test]
    fn non_square_pixmaps_keep_their_shape_and_bad_ones_are_dropped() {
        let properties = dict(vec![(
            "IconPixmap",
            Value::from(vec![
                pixmap_entry(4, 2, 4 * 2 * 4),
                // Truncated: claims 16x16 and carries 64 bytes.
                pixmap_entry(16, 16, 64),
                pixmap_entry(24, 24, 24 * 24 * 4),
            ]),
        )]);

        let parsed = parse(&properties, ":1.2/StatusNotifierItem");
        let sizes: Vec<(i32, i32)> = parsed
            .icon
            .icon_pixmap
            .iter()
            .map(|pixmap| (pixmap.width, pixmap.height))
            .collect();
        assert_eq!(
            sizes,
            vec![(4, 2), (24, 24)],
            "the malformed entry is skipped, the rest survive"
        );
    }

    #[test]
    fn a_tooltip_sent_as_a_bare_string_is_still_a_tooltip() {
        let properties = dict(vec![("ToolTip", "Just words".into())]);
        assert_eq!(
            parse(&properties, ":1.2/x").tooltip.as_deref(),
            Some("Just words")
        );
    }

    #[test]
    fn a_menu_sent_as_a_string_is_still_a_menu() {
        let properties = dict(vec![("Menu", "/MenuBar".into())]);
        assert_eq!(
            parse(&properties, ":1.2/x").menu_path.as_deref(),
            Some("/MenuBar")
        );
    }

    #[test]
    fn the_root_path_means_the_item_has_no_menu() {
        // An object path may not be empty, so `/` is the only way an
        // application with no menu can say so — and plenty of them do.
        let properties = dict(vec![(
            "Menu",
            Value::ObjectPath(ObjectPath::try_from("/").expect("the root path")),
        )]);
        assert_eq!(parse(&properties, ":1.2/x").menu_path, None);
    }

    #[test]
    fn a_property_of_the_wrong_type_falls_back_rather_than_failing() {
        let properties = dict(vec![
            ("Title", 7i32.into()),
            ("Status", true.into()),
            ("ItemIsMenu", "yes".into()),
            ("IconPixmap", "not an array".into()),
        ]);
        let parsed = parse(&properties, ":1.2/x");
        assert_eq!(parsed.title, "");
        assert_eq!(parsed.status, Status::Active);
        assert!(!parsed.item_is_menu);
        assert!(parsed.icon.icon_pixmap.is_empty());
    }
}
