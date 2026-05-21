//! Built-in operating-system logo widget.
//!
//! Reads os-release metadata and displays the current distribution's logo in
//! the top bar. Known distros use Nerd Font glyphs because many icon themes do
//! not ship distro logos as symbolic GTK icons.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::Label;

use crate::services::icons::IconHandle;
use crate::styles::widget as wgt;
use crate::widgets::base::BaseWidget;
use crate::widgets::{WidgetConfig, warn_unknown_options};

const OS_RELEASE_PATH: &str = "/etc/os-release";
const OS_RELEASE_FALLBACK_PATH: &str = "/usr/lib/os-release";
const KNOWN_OPTIONS: &[&str] = &["tooltip", "label", "max_chars"];

#[derive(Debug, Clone)]
pub struct OsLogoConfig {
    tooltip: Option<String>,
    label: Option<String>,
    max_chars: Option<i32>,
}

impl Default for OsLogoConfig {
    fn default() -> Self {
        Self {
            tooltip: None,
            label: None,
            max_chars: Some(4),
        }
    }
}

impl WidgetConfig for OsLogoConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("os_logo", entry, KNOWN_OPTIONS);
        let default = Self::default();
        let tooltip = entry
            .options
            .get("tooltip")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let label = entry
            .options
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let max_chars = entry
            .options
            .get("max_chars")
            .and_then(toml::Value::as_integer)
            .map(|v| i32::try_from(v.max(1)).unwrap_or(i32::MAX))
            .or(default.max_chars);

        Self {
            tooltip,
            label,
            max_chars,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OsReleaseInfo {
    id: String,
    pretty_name: String,
    logo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OsLogoDisplay {
    Label(String),
    Icon(String),
}

pub struct OsLogoWidget {
    base: BaseWidget,
    #[allow(dead_code)]
    label: Option<Label>,
    #[allow(dead_code)]
    icon_handle: Option<IconHandle>,
}

impl OsLogoWidget {
    pub fn new(config: OsLogoConfig) -> Self {
        let base = BaseWidget::new(&[wgt::OS_LOGO, "os-logo"]);
        let info = read_os_release().unwrap_or_else(default_os_release_info);
        let tooltip = config
            .tooltip
            .clone()
            .unwrap_or_else(|| info.pretty_name.clone());

        if !tooltip.is_empty() {
            base.set_tooltip(&tooltip);
        }

        let display = config
            .label
            .clone()
            .map(OsLogoDisplay::Label)
            .unwrap_or_else(|| os_logo_display(&info));

        let (label, icon_handle) = match display {
            OsLogoDisplay::Label(text) => {
                let label = base.add_label(Some(&text), &[wgt::OS_LOGO_LABEL]);
                label.set_xalign(0.5);
                if let Some(max_chars) = config.max_chars {
                    label.set_max_width_chars(max_chars);
                    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                }
                (Some(label), None)
            }
            OsLogoDisplay::Icon(icon_name) => {
                let icon = base.add_icon(&icon_name, &[wgt::OS_LOGO_ICON]);
                (None, Some(icon))
            }
        };

        Self {
            base,
            label,
            icon_handle,
        }
    }

    pub fn widget(&self) -> &gtk4::Box {
        self.base.widget()
    }
}

fn read_os_release() -> Option<OsReleaseInfo> {
    read_os_release_path(Path::new(OS_RELEASE_PATH))
        .or_else(|| read_os_release_path(Path::new(OS_RELEASE_FALLBACK_PATH)))
}

fn read_os_release_path(path: &Path) -> Option<OsReleaseInfo> {
    let content = fs::read_to_string(path).ok()?;
    parse_os_release(&content)
}

fn parse_os_release(content: &str) -> Option<OsReleaseInfo> {
    let values = content
        .lines()
        .filter_map(parse_os_release_line)
        .collect::<HashMap<_, _>>();
    let id = values.get("ID")?.to_string();
    let pretty_name = values
        .get("PRETTY_NAME")
        .or_else(|| values.get("NAME"))
        .cloned()
        .unwrap_or_else(|| id.clone());
    let logo = values
        .get("LOGO")
        .filter(|value| !value.is_empty())
        .cloned();

    Some(OsReleaseInfo {
        id,
        pretty_name,
        logo,
    })
}

fn parse_os_release_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.to_string(), unquote_os_release_value(value)))
}

fn unquote_os_release_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        value.to_string()
    }
}

fn default_os_release_info() -> OsReleaseInfo {
    OsReleaseInfo {
        id: "linux".to_string(),
        pretty_name: "Linux".to_string(),
        logo: None,
    }
}

fn os_logo_display(info: &OsReleaseInfo) -> OsLogoDisplay {
    if let Some(symbol) = os_logo_symbol(&info.id) {
        return OsLogoDisplay::Label(symbol.to_string());
    }
    if let Some(logo) = info.logo.as_deref().filter(|value| !value.is_empty()) {
        return OsLogoDisplay::Icon(logo.to_string());
    }
    OsLogoDisplay::Icon("computer-symbolic".to_string())
}

fn os_logo_symbol(id: &str) -> Option<&'static str> {
    match id.to_ascii_lowercase().as_str() {
        "guix" => Some(""),
        "nixos" => Some(""),
        "arch" | "archlinux" => Some(""),
        "debian" => Some(""),
        "fedora" => Some(""),
        "ubuntu" => Some(""),
        "linux" => Some(""),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    fn make_entry(options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: "os_logo".to_string(),
            options,
        }
    }

    #[test]
    fn os_logo_config_defaults_to_auto_label() {
        let config = OsLogoConfig::from_entry(&make_entry(HashMap::new()));
        assert!(config.tooltip.is_none());
        assert!(config.label.is_none());
        assert_eq!(config.max_chars, Some(4));
    }

    #[test]
    fn parses_quoted_os_release_values() {
        let info = parse_os_release(
            r#"
NAME="Guix System"
ID=guix
PRETTY_NAME="Guix System"
LOGO=guix-icon
"#,
        )
        .expect("os-release parses");

        assert_eq!(info.id, "guix");
        assert_eq!(info.pretty_name, "Guix System");
        assert_eq!(info.logo.as_deref(), Some("guix-icon"));
    }

    #[test]
    fn guix_uses_distro_symbol_before_logo_icon() {
        let info = OsReleaseInfo {
            id: "guix".to_string(),
            pretty_name: "Guix System".to_string(),
            logo: Some("guix-icon".to_string()),
        };

        assert_eq!(
            os_logo_display(&info),
            OsLogoDisplay::Label("".to_string())
        );
    }

    #[test]
    fn unknown_os_uses_logo_icon_then_computer_fallback() {
        let with_logo = OsReleaseInfo {
            id: "example".to_string(),
            pretty_name: "ExampleOS".to_string(),
            logo: Some("example-logo".to_string()),
        };
        let without_logo = OsReleaseInfo {
            logo: None,
            ..with_logo.clone()
        };

        assert_eq!(
            os_logo_display(&with_logo),
            OsLogoDisplay::Icon("example-logo".to_string())
        );
        assert_eq!(
            os_logo_display(&without_logo),
            OsLogoDisplay::Icon("computer-symbolic".to_string())
        );
    }
}
