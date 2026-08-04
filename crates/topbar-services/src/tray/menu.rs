//! The dbusmenu layout, parsed once at the edge.
//!
//! `com.canonical.dbusmenu` hands back a recursive `(ia{sv}av)` in which every
//! property is optional and every default matters. This module turns one of
//! those into a tree the widget can draw without ever seeing a
//! [`zvariant::Value`](zbus::zvariant::Value).

use std::collections::HashMap;

use zbus::zvariant::{Array, OwnedValue, Structure};

use super::proxy::RawNode;

/// One row of a menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuNode {
    /// The id `Event` and `AboutToShow` address this row by.
    pub id: i32,
    /// The label, with its mnemonic markers already taken out.
    pub label: String,
    /// Whether this is a row or a rule.
    pub kind: MenuKind,
    /// Whether activating it would do anything.
    pub enabled: bool,
    /// Whether it should be drawn at all.
    pub visible: bool,
    /// A Freedesktop icon name for the row.
    pub icon_name: Option<String>,
    /// PNG bytes for the row's icon, when the application sent one.
    pub icon_data: Option<Vec<u8>>,
    /// Whether the row carries a checkmark, a radio dot, or neither.
    pub toggle: ToggleKind,
    /// What that mark is set to.
    pub toggle_state: ToggleState,
    /// Whether the application says this row opens a submenu.
    pub has_submenu: bool,
    /// The submenu's rows, when they came down with the layout.
    pub children: Vec<MenuNode>,
}

impl Default for MenuNode {
    fn default() -> Self {
        Self {
            id: 0,
            label: String::new(),
            kind: MenuKind::Standard,
            // The specification's defaults, and the reason they are written
            // here rather than derived: a row with no `enabled` property is
            // enabled, and one with no `visible` property is visible.
            enabled: true,
            visible: true,
            icon_name: None,
            icon_data: None,
            toggle: ToggleKind::None,
            toggle_state: ToggleState::Off,
            has_submenu: false,
            children: Vec::new(),
        }
    }
}

impl MenuNode {
    /// Parse a layout node and everything under it.
    pub(super) fn parse(raw: &RawNode) -> Self {
        let properties = &raw.properties;

        let kind = match as_str(properties, "type").as_deref() {
            Some("separator") => MenuKind::Separator,
            _ => MenuKind::Standard,
        };

        let toggle = match as_str(properties, "toggle-type").as_deref() {
            Some("checkmark") => ToggleKind::Checkmark,
            Some("radio") => ToggleKind::Radio,
            _ => ToggleKind::None,
        };

        // Only meaningful beside a toggle type, and deliberately `Off` without
        // one: a row with no mark on it must never read as ticked.
        let toggle_state = match toggle {
            ToggleKind::None => ToggleState::Off,
            _ => match as_i32(properties, "toggle-state") {
                Some(1) => ToggleState::On,
                Some(0) => ToggleState::Off,
                Some(_) => ToggleState::Indeterminate,
                None => ToggleState::Off,
            },
        };

        let children: Vec<Self> = raw
            .children
            .iter()
            .filter_map(child_node)
            .map(|child| Self::parse(&child))
            .collect();

        Self {
            id: raw.id,
            label: strip_mnemonics(&as_str(properties, "label").unwrap_or_default()),
            kind,
            enabled: as_bool(properties, "enabled").unwrap_or(true),
            visible: as_bool(properties, "visible").unwrap_or(true),
            icon_name: as_str(properties, "icon-name").filter(|name| !name.is_empty()),
            icon_data: as_bytes(properties, "icon-data").filter(|data| !data.is_empty()),
            toggle,
            toggle_state,
            // Either the application says so, or it sent children anyway.
            has_submenu: as_str(properties, "children-display").as_deref() == Some("submenu")
                || !children.is_empty(),
            children,
        }
    }

    /// The rows worth drawing, in order.
    pub fn rows(&self) -> impl Iterator<Item = &Self> {
        self.children.iter().filter(|child| child.visible)
    }

    /// Whether there is nothing here to put on screen.
    ///
    /// A menu of nothing but hidden rows is an empty menu, and the panel would
    /// rather activate the item than open a blank surface over it.
    pub fn is_empty(&self) -> bool {
        self.rows().next().is_none()
    }

    /// The submenu under `id`, wherever it is in the tree.
    pub fn find(&self, id: i32) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }
}

/// Whether a row is a row at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuKind {
    /// Something that can be clicked.
    #[default]
    Standard,
    /// A horizontal rule.
    Separator,
}

/// The kind of mark a row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleKind {
    /// No mark.
    #[default]
    None,
    /// An independent tick.
    Checkmark,
    /// One of a group, of which one is chosen.
    Radio,
}

/// What that mark is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleState {
    /// Not set.
    #[default]
    Off,
    /// Set.
    On,
    /// Neither — the application does not know.
    Indeterminate,
}

impl ToggleState {
    /// Whether the mark should be drawn filled.
    pub fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

/// What happened to a menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEvent {
    /// The user chose it.
    Clicked,
    /// The pointer went over it, which some applications use to build submenus.
    Hovered,
}

impl MenuEvent {
    /// The name the protocol uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clicked => "clicked",
            Self::Hovered => "hovered",
        }
    }
}

/// Take the mnemonic markers out of a label.
///
/// Two underscores are a literal underscore; a lone one marks the access key
/// and is not drawn. The panel has no menu-bar keyboard traversal, so the
/// marker is simply removed rather than turned into Pango underline markup.
fn strip_mnemonics(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '_' {
            out.push(character);
            continue;
        }
        if chars.peek() == Some(&'_') {
            chars.next();
            out.push('_');
        }
    }
    out
}

/// Unwrap one child of an `av` into the node it holds.
///
/// The recursion has to be walked by hand: D-Bus has no way to say a type
/// contains itself, so a layout's children arrive as bare variants and only
/// the outermost node is deserialized for us.
fn child_node(value: &OwnedValue) -> Option<RawNode> {
    let structure = value.downcast_ref::<&Structure<'_>>().ok()?;
    let fields = structure.fields();

    let id = fields.first()?.downcast_ref::<i32>().ok()?;
    let properties = fields
        .get(1)?
        .try_clone()
        .ok()
        .and_then(|dict| HashMap::<String, OwnedValue>::try_from(dict).ok())?;
    let children = fields
        .get(2)?
        .downcast_ref::<&Array<'_>>()
        .ok()?
        .iter()
        .filter_map(|child| OwnedValue::try_from(child.try_clone().ok()?).ok())
        .collect();

    Some(RawNode {
        id,
        properties,
        children,
    })
}

/// A string property, if it is there and really is a string.
fn as_str(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|value| value.downcast_ref::<&str>().ok())
        .map(ToString::to_string)
}

/// A boolean property.
fn as_bool(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    properties
        .get(key)
        .and_then(|value| value.downcast_ref::<bool>().ok())
}

/// An integer property.
fn as_i32(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    properties
        .get(key)
        .and_then(|value| value.downcast_ref::<i32>().ok())
}

/// A byte-array property.
fn as_bytes(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<Vec<u8>> {
    let array = properties
        .get(key)
        .and_then(|value| value.downcast_ref::<Array>().ok())?;
    array
        .iter()
        .map(|byte| byte.downcast_ref::<u8>().ok())
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use zbus::zvariant::{Structure, Value};

    /// Build a properties dictionary from `(key, value)` pairs.
    pub(crate) fn props(entries: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
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

    /// Build one layout node.
    pub(crate) fn node(
        id: i32,
        entries: Vec<(&str, Value<'static>)>,
        children: Vec<RawNode>,
    ) -> RawNode {
        RawNode {
            id,
            properties: props(entries),
            children: children.into_iter().map(wrap).collect(),
        }
    }

    /// Put a node back into the variant an `av` carries it in.
    fn wrap(node: RawNode) -> OwnedValue {
        let structure = Structure::from((
            node.id,
            node.properties
                .into_iter()
                .map(|(key, value)| (key, Value::from(value)))
                .collect::<std::collections::HashMap<_, _>>(),
            node.children
                .into_iter()
                .map(Value::from)
                .collect::<Vec<_>>(),
        ));
        OwnedValue::try_from(Value::from(structure)).expect("a fixture node can be owned")
    }

    /// The menu the fake application serves, and the one the tests assert on.
    pub(crate) fn sample_menu() -> RawNode {
        node(
            0,
            vec![("children-display", "submenu".into())],
            vec![
                node(1, vec![("label", "Op__en _File".into())], vec![]),
                node(
                    2,
                    vec![
                        ("label", "Notifications".into()),
                        ("toggle-type", "checkmark".into()),
                        ("toggle-state", 1i32.into()),
                    ],
                    vec![],
                ),
                node(
                    3,
                    vec![
                        ("label", "Online".into()),
                        ("toggle-type", "radio".into()),
                        ("toggle-state", 1i32.into()),
                    ],
                    vec![],
                ),
                node(
                    4,
                    vec![
                        ("label", "Away".into()),
                        ("toggle-type", "radio".into()),
                        ("toggle-state", 0i32.into()),
                    ],
                    vec![],
                ),
                node(5, vec![("type", "separator".into())], vec![]),
                node(
                    6,
                    vec![("label", "Not Available".into()), ("enabled", false.into())],
                    vec![],
                ),
                node(
                    7,
                    vec![("label", "Secret".into()), ("visible", false.into())],
                    vec![],
                ),
                node(
                    8,
                    vec![
                        ("label", "Preferences".into()),
                        ("icon-name", "preferences-system-symbolic".into()),
                    ],
                    vec![],
                ),
                node(
                    9,
                    vec![
                        ("label", "More".into()),
                        ("children-display", "submenu".into()),
                    ],
                    vec![
                        node(10, vec![("label", "About".into())], vec![]),
                        node(
                            11,
                            vec![
                                ("label", "Deeper".into()),
                                ("children-display", "submenu".into()),
                            ],
                            vec![node(12, vec![("label", "Bottom".into())], vec![])],
                        ),
                    ],
                ),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{node, sample_menu};
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn a_layout_becomes_a_tree_of_rows() {
        let menu = MenuNode::parse(&sample_menu());
        assert_eq!(menu.id, 0);
        assert_eq!(menu.children.len(), 9);
        assert_eq!(
            menu.rows().count(),
            8,
            "the hidden row is not one of the rows"
        );
    }

    #[test]
    fn mnemonic_markers_are_taken_out_and_doubled_ones_are_kept() {
        assert_eq!(strip_mnemonics("Op__en _File"), "Op_en File");
        assert_eq!(strip_mnemonics("_Quit"), "Quit");
        assert_eq!(strip_mnemonics("__"), "_");
        assert_eq!(strip_mnemonics("no markers"), "no markers");
        assert_eq!(strip_mnemonics(""), "");
        assert_eq!(strip_mnemonics("trailing_"), "trailing");

        let menu = MenuNode::parse(&sample_menu());
        assert_eq!(menu.children[0].label, "Op_en File");
    }

    #[test]
    fn a_checkmark_and_a_radio_group_keep_their_marks() {
        let menu = MenuNode::parse(&sample_menu());

        let check = &menu.children[1];
        assert_eq!(check.toggle, ToggleKind::Checkmark);
        assert_eq!(check.toggle_state, ToggleState::On);
        assert!(check.toggle_state.is_on());

        let chosen = &menu.children[2];
        assert_eq!(chosen.toggle, ToggleKind::Radio);
        assert!(chosen.toggle_state.is_on());

        let other = &menu.children[3];
        assert_eq!(other.toggle, ToggleKind::Radio);
        assert!(!other.toggle_state.is_on());
    }

    #[test]
    fn a_row_with_no_toggle_type_never_reads_as_ticked() {
        // The trap the `system-tray` crate falls into: its `ToggleState`
        // derives `Default` on the `On` variant, so an ordinary row comes back
        // toggled on.
        let plain = MenuNode::parse(&node(1, vec![("label", "Quit".into())], vec![]));
        assert_eq!(plain.toggle, ToggleKind::None);
        assert!(!plain.toggle_state.is_on());

        // Even one that carries a state without a type.
        let confused = MenuNode::parse(&node(
            2,
            vec![("label", "Odd".into()), ("toggle-state", 1i32.into())],
            vec![],
        ));
        assert!(!confused.toggle_state.is_on());
    }

    #[test]
    fn an_unknown_toggle_state_is_indeterminate() {
        let node = MenuNode::parse(&node(
            1,
            vec![
                ("toggle-type", "checkmark".into()),
                ("toggle-state", (-1i32).into()),
            ],
            vec![],
        ));
        assert_eq!(node.toggle_state, ToggleState::Indeterminate);
        assert!(!node.toggle_state.is_on());
    }

    #[test]
    fn separators_disabled_rows_and_hidden_rows_survive_the_parse() {
        let menu = MenuNode::parse(&sample_menu());

        assert_eq!(menu.children[4].kind, MenuKind::Separator);
        assert!(menu.children[4].label.is_empty());

        assert!(!menu.children[5].enabled);
        assert!(menu.children[5].visible);

        assert!(!menu.children[6].visible);
        assert!(
            !menu.rows().any(|row| row.label == "Secret"),
            "a hidden row is never offered for drawing"
        );
    }

    #[test]
    fn defaults_follow_the_specification_rather_than_rust() {
        let bare = MenuNode::parse(&node(7, vec![], vec![]));
        assert!(bare.enabled, "a row with no `enabled` property is enabled");
        assert!(bare.visible, "a row with no `visible` property is visible");
        assert_eq!(bare.kind, MenuKind::Standard);
        assert!(!bare.has_submenu);
    }

    #[test]
    fn nested_submenus_come_down_whole() {
        let menu = MenuNode::parse(&sample_menu());
        let more = &menu.children[8];
        assert_eq!(more.label, "More");
        assert!(more.has_submenu);
        assert_eq!(more.children.len(), 2);

        let deeper = &more.children[1];
        assert!(deeper.has_submenu);
        assert_eq!(deeper.children[0].label, "Bottom");

        let found = menu.find(12).expect("the deepest row is reachable");
        assert_eq!(found.label, "Bottom");
        assert!(menu.find(999).is_none());
    }

    #[test]
    fn a_row_with_children_is_a_submenu_even_without_the_property() {
        let implied = MenuNode::parse(&node(
            1,
            vec![("label", "Recent".into())],
            vec![node(2, vec![("label", "file.txt".into())], vec![])],
        ));
        assert!(
            implied.has_submenu,
            "an application that sent children meant them to be reachable"
        );
    }

    #[test]
    fn a_row_may_carry_an_icon_by_name_or_by_bytes() {
        let menu = MenuNode::parse(&sample_menu());
        assert_eq!(
            menu.children[7].icon_name.as_deref(),
            Some("preferences-system-symbolic")
        );

        let png = vec![0x89u8, 0x50, 0x4e, 0x47];
        let with_data = MenuNode::parse(&node(
            1,
            vec![("icon-data", Value::from(png.clone()))],
            vec![],
        ));
        assert_eq!(with_data.icon_data, Some(png));

        let empty = MenuNode::parse(&node(
            2,
            vec![
                ("icon-name", "".into()),
                ("icon-data", Value::from(Vec::<u8>::new())),
            ],
            vec![],
        ));
        assert_eq!(empty.icon_name, None);
        assert_eq!(empty.icon_data, None, "an empty icon is no icon");
    }

    #[test]
    fn a_property_of_the_wrong_type_is_ignored_rather_than_fatal() {
        let wrong = MenuNode::parse(&node(
            1,
            vec![
                ("label", 42i32.into()),
                ("enabled", "yes".into()),
                ("toggle-state", "on".into()),
                ("toggle-type", "checkmark".into()),
            ],
            vec![],
        ));
        assert_eq!(wrong.label, "");
        assert!(wrong.enabled, "the default stands in for nonsense");
        assert!(!wrong.toggle_state.is_on());
    }

    #[test]
    fn a_menu_of_nothing_but_hidden_rows_is_an_empty_menu() {
        let hidden = MenuNode::parse(&node(
            0,
            vec![],
            vec![node(1, vec![("visible", false.into())], vec![])],
        ));
        assert!(hidden.is_empty());
        assert!(MenuNode::parse(&node(0, vec![], vec![])).is_empty());
        assert!(!MenuNode::parse(&sample_menu()).is_empty());
    }

    #[test]
    fn menu_events_use_the_names_the_protocol_does() {
        assert_eq!(MenuEvent::Clicked.as_str(), "clicked");
        assert_eq!(MenuEvent::Hovered.as_str(), "hovered");
    }
}
