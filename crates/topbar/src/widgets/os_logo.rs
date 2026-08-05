//! The distribution logo, at the far right of the bar.
//!
//! ```text
//!  ❄        NixOS 26.05 (Yarara)
//! ```
//!
//! **The one place the panel draws a glyph instead of an icon.** Everything
//! else in topbar is an Adwaita symbolic name, on purpose — but no icon theme
//! ships distribution logos as symbolics, and the alternative is either a
//! bundled bitmap per distribution or a generic computer icon that says nothing.
//! Nerd Fonts carry every one of them, the live configuration's font stack
//! already loads Symbols Nerd Font for exactly this, and a glyph inherits the
//! bar's colour and scales with its font the way an icon would.
//!
//! Three ways to decide what to draw, in order:
//!
//! 1. the glyph for this `ID`, or for whatever its `ID_LIKE` derives from —
//!    Linux Mint has no glyph of its own and is Debian underneath;
//! 2. the icon named by `LOGO=`, which some distributions do ship;
//! 3. a generic computer, which is honest about knowing nothing.
//!
//! Passive: no hover, no pointer, nothing to click unless the user configures
//! a command. It is a label, and it should read as one.

use std::path::{Path, PathBuf};

use gtk4::prelude::*;
use gtk4::{Image, Label, pango};
use topbar_core::config::OsLogoConfig;
use tracing::debug;

use crate::style::classes;
use crate::widgets::shell::WidgetShell;
use crate::widgets::{ellipsize, install_click_commands};

/// What a failed click command is reported against.
const WIDGET_NAME: &str = "os_logo";

/// Where the file lives on a normal machine.
const OS_RELEASE: &str = "etc/os-release";
/// Where it lives on a stateless one, which `/etc/os-release` symlinks to.
const OS_RELEASE_FALLBACK: &str = "usr/lib/os-release";

/// A whole os-release file to read instead of the machine's. Debug builds only.
const SMOKE_FILE: &str = "TOPBAR_SMOKE_OSRELEASE";
/// A directory to read it under, shared with the updates service.
const SMOKE_ROOT: &str = "TOPBAR_SMOKE_ROOT";

/// The icon drawn for a distribution the panel has never heard of.
const UNKNOWN_ICON: &str = "computer-symbolic";

/// The distribution logo widget.
pub struct OsLogoWidget {
    shell: WidgetShell,
    /// Held so GTK does not drop whichever of the two was built.
    _glyph: Option<Label>,
    /// The same, for the themed icon.
    _icon: Option<Image>,
}

impl OsLogoWidget {
    /// Build the widget from `[widgets.os_logo]`.
    pub fn new(settings: &OsLogoConfig) -> Self {
        let shell = WidgetShell::new(classes::OS_LOGO);
        let release = read_os_release().unwrap_or_default();

        let tooltip = settings
            .tooltip
            .clone()
            .unwrap_or_else(|| release.pretty_name.clone());
        shell.set_tooltip(&tooltip);

        let max_chars = settings.max_chars.map(|max| max as usize);
        let (glyph, icon) = match settings
            .label
            .clone()
            .map(Logo::Glyph)
            .unwrap_or_else(|| logo(&release))
        {
            Logo::Glyph(text) => {
                let label = Label::new(Some(&ellipsize(&text, max_chars)));
                label.add_css_class(classes::OS_LOGO_GLYPH);
                label.set_ellipsize(pango::EllipsizeMode::End);
                label.set_xalign(0.5);
                shell.content().append(&label);
                (Some(label), None)
            }
            Logo::Icon(name) => {
                let image = Image::from_icon_name(&name);
                image.add_css_class(classes::OS_LOGO_ICON);
                shell.content().append(&image);
                (None, Some(image))
            }
        };

        // Only interactive when the user asked for it: a logo that lights up
        // under the pointer promises something to click.
        if settings.on_click.is_some()
            || settings.on_click_right.is_some()
            || settings.on_click_middle.is_some()
        {
            shell.make_interactive();
        }
        install_left_click(shell.root(), settings.on_click.as_deref());
        install_click_commands(
            shell.root(),
            WIDGET_NAME,
            settings.on_click_right.as_deref(),
            settings.on_click_middle.as_deref(),
        );

        Self {
            shell,
            _glyph: glyph,
            _icon: icon,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// The three fields of `/etc/os-release` this widget reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsRelease {
    /// `ID`, the short name.
    pub id: String,
    /// `ID_LIKE`, the space-separated list of what it derives from.
    pub id_like: Vec<String>,
    /// `PRETTY_NAME`, or `NAME`, or the id.
    pub pretty_name: String,
    /// `LOGO`, an icon-theme name some distributions ship.
    pub logo: Option<String>,
}

impl Default for OsRelease {
    /// A machine with no os-release at all is simply Linux.
    fn default() -> Self {
        Self {
            id: "linux".to_string(),
            id_like: Vec::new(),
            pretty_name: "Linux".to_string(),
            logo: None,
        }
    }
}

/// What to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Logo {
    /// A Nerd Font glyph.
    Glyph(String),
    /// A name from the icon theme.
    Icon(String),
}

/// Decide what this machine's logo is.
fn logo(release: &OsRelease) -> Logo {
    if let Some(glyph) = glyph(&release.id) {
        return Logo::Glyph(glyph.to_string());
    }
    // A derivative with no glyph of its own: Linux Mint says `ID_LIKE=ubuntu
    // debian`, Manjaro says `arch`, Nobara says `fedora`. Most specific first,
    // which is the order the field is written in.
    for like in &release.id_like {
        if let Some(glyph) = glyph(like) {
            return Logo::Glyph(glyph.to_string());
        }
    }
    if let Some(name) = release.logo.clone().filter(|name| !name.is_empty()) {
        return Logo::Icon(name);
    }
    Logo::Icon(UNKNOWN_ICON.to_string())
}

/// The Nerd Font glyph for a distribution id.
///
/// Ported from v1 unchanged, codepoints and all. Everything else reaches one of
/// these through `ID_LIKE`, or falls through to `LOGO=`.
fn glyph(id: &str) -> Option<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        // nf-linux-guix
        "guix" => Some("\u{f325}"),
        // nf-linux-nixos
        "nixos" => Some("\u{f313}"),
        // nf-linux-archlinux
        "arch" | "archlinux" => Some("\u{f303}"),
        // nf-linux-debian
        "debian" => Some("\u{f306}"),
        // nf-linux-fedora
        "fedora" => Some("\u{f30a}"),
        // nf-linux-ubuntu
        "ubuntu" => Some("\u{f31b}"),
        // nf-dev-linux, the penguin
        "linux" => Some("\u{e712}"),
        _ => None,
    }
}

/// Read this machine's os-release, wherever it is.
fn read_os_release() -> Option<OsRelease> {
    if let Some(path) = smoke(SMOKE_FILE) {
        debug!("reading os-release from {}", path.display());
        return std::fs::read_to_string(path).ok().map(|text| parse(&text));
    }
    let root = smoke(SMOKE_ROOT).unwrap_or_else(|| PathBuf::from("/"));
    read_under(&root)
}

/// The same, under an explicit root.
fn read_under(root: &Path) -> Option<OsRelease> {
    for relative in [OS_RELEASE, OS_RELEASE_FALLBACK] {
        if let Ok(text) = std::fs::read_to_string(root.join(relative)) {
            return Some(parse(&text));
        }
    }
    None
}

/// A smoke override, in debug builds only.
fn smoke(variable: &str) -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read the four fields out of an os-release file.
///
/// The values are shell-quoted, and both quoted and bare forms occur in the
/// wild: `ID=nixos` on this machine, `ID="fedora"` on that one.
fn parse(text: &str) -> OsRelease {
    let field = |key: &str| {
        text.lines()
            .map(str::trim)
            .filter_map(|line| line.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| unquote(value).to_string())
            .filter(|value| !value.is_empty())
    };

    let id = field("ID").unwrap_or_else(|| "linux".to_string());
    OsRelease {
        id_like: field("ID_LIKE")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        pretty_name: field("PRETTY_NAME")
            .or_else(|| field("NAME"))
            .unwrap_or_else(|| id.clone()),
        logo: field("LOGO"),
        id,
    }
}

/// Strip one layer of shell quoting.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|value| value.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// A left click runs the configured command and nothing else.
fn install_left_click(anchor: &gtk4::Box, command: Option<&str>) {
    let Some(command) = command.map(str::to_string) else {
        return;
    };
    let click = gtk4::GestureClick::new();
    click.set_button(gtk4::gdk::BUTTON_PRIMARY);
    click.connect_released(move |_, _, _, _| {
        let command = command.clone();
        crate::bridge::act(
            crate::bridge::ActionScope::Toast {
                widget: WIDGET_NAME,
            },
            async move { topbar_services::proc::run(&command).await },
        );
    });
    anchor.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This machine's own file, byte for byte.
    const NIXOS: &str = r#"ANSI_COLOR="0;38;2;126;186;228"
BUG_REPORT_URL="https://github.com/NixOS/nixpkgs/issues"
ID=nixos
ID_LIKE=""
LOGO="nix-snowflake"
NAME=NixOS
PRETTY_NAME="NixOS 26.05 (Yarara)"
VERSION_ID="26.05"
"#;

    #[test]
    fn this_machines_own_file_parses_to_the_snowflake() {
        let release = parse(NIXOS);
        assert_eq!(release.id, "nixos");
        assert_eq!(release.pretty_name, "NixOS 26.05 (Yarara)");
        assert_eq!(release.logo.as_deref(), Some("nix-snowflake"));
        // An empty ID_LIKE is a field, not a derivative.
        assert!(release.id_like.is_empty());
        assert_eq!(logo(&release), Logo::Glyph("\u{f313}".to_string()));
    }

    #[test]
    fn a_glyph_beats_the_logo_key() {
        // NixOS ships `LOGO=nix-snowflake`, and Adwaita does not have it: the
        // glyph is the one that will actually draw something.
        let release = parse(NIXOS);
        assert!(release.logo.is_some());
        assert!(matches!(logo(&release), Logo::Glyph(_)));
    }

    #[test]
    fn a_derivative_borrows_the_glyph_of_what_it_derives_from() {
        let mint =
            parse("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nPRETTY_NAME=\"Linux Mint 22\"\n");
        assert_eq!(logo(&mint), Logo::Glyph("\u{f31b}".to_string()), "Ubuntu's");

        let manjaro = parse("ID=manjaro\nID_LIKE=arch\n");
        assert_eq!(logo(&manjaro), Logo::Glyph("\u{f303}".to_string()));

        let nobara = parse("ID=nobara\nID_LIKE=fedora\n");
        assert_eq!(logo(&nobara), Logo::Glyph("\u{f30a}".to_string()));
    }

    #[test]
    fn the_id_like_list_is_read_most_specific_first() {
        // Both are known; the one written first wins, which is the field's own
        // documented order.
        let release = parse("ID=example\nID_LIKE=\"debian arch\"\n");
        assert_eq!(logo(&release), Logo::Glyph("\u{f306}".to_string()));
    }

    #[test]
    fn an_unknown_distribution_falls_back_to_its_logo_key() {
        let release = parse("ID=example\nPRETTY_NAME=\"ExampleOS\"\nLOGO=example-logo\n");
        assert_eq!(logo(&release), Logo::Icon("example-logo".to_string()));
    }

    #[test]
    fn an_unknown_distribution_with_nothing_at_all_gets_a_computer() {
        let release = parse("ID=example\nPRETTY_NAME=\"ExampleOS\"\n");
        assert_eq!(logo(&release), Logo::Icon(UNKNOWN_ICON.to_string()));
    }

    #[test]
    fn a_machine_with_no_os_release_is_simply_linux() {
        let none = OsRelease::default();
        assert_eq!(none.pretty_name, "Linux");
        assert_eq!(logo(&none), Logo::Glyph("\u{e712}".to_string()));
    }

    #[test]
    fn quoted_and_bare_values_are_the_same_value() {
        assert_eq!(unquote(r#""Guix System""#), "Guix System");
        assert_eq!(unquote("'Guix System'"), "Guix System");
        assert_eq!(unquote("guix"), "guix");
        assert_eq!(unquote("  guix  "), "guix");
    }

    #[test]
    fn comments_and_blank_lines_are_not_fields() {
        let release = parse("# a comment\n\nID=guix\nPRETTY_NAME=\"Guix System\"\n");
        assert_eq!(release.id, "guix");
        assert_eq!(release.pretty_name, "Guix System");
    }

    #[test]
    fn a_file_with_no_pretty_name_falls_back_to_name_then_to_the_id() {
        assert_eq!(
            parse("ID=guix\nNAME=\"Guix System\"\n").pretty_name,
            "Guix System"
        );
        assert_eq!(parse("ID=guix\n").pretty_name, "guix");
    }

    #[test]
    fn every_distribution_v1_drew_still_draws() {
        for id in [
            "guix",
            "nixos",
            "arch",
            "archlinux",
            "debian",
            "fedora",
            "ubuntu",
            "linux",
        ] {
            assert!(glyph(id).is_some(), "`{id}` lost its glyph");
        }
        assert!(
            glyph("NixOS").is_some(),
            "the id is matched case-insensitively"
        );
        assert!(glyph("plan9").is_none());
    }

    #[test]
    fn the_fallback_path_is_read_when_etc_has_nothing() {
        let root = std::env::temp_dir().join(format!("topbar-osrel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("usr/lib")).expect("the fixture root");
        std::fs::write(root.join("usr/lib/os-release"), NIXOS).expect("the fixture file");

        let release = read_under(&root).expect("a stateless machine still has one");
        assert_eq!(release.id, "nixos");

        assert!(read_under(&root.join("nowhere")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_glyph_is_cut_to_the_configured_width() {
        // `max_chars` defaults to four, which every glyph and most short
        // labels fit inside; a long custom label is what it exists for.
        assert_eq!(ellipsize("\u{f313}", Some(4)), "\u{f313}");
        assert_eq!(ellipsize("NixOS Unstable", Some(4)), "Nix…");
    }
}
