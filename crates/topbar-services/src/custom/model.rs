//! What a script said, and what the bar should make of it.
//!
//! Pure, and the whole compatibility surface of the `custom-*` widgets: the
//! contract these functions implement is Waybar's, which is the one every
//! script anybody has written for a panel already speaks. v1 shipped it and
//! this is a port — the tests below are v1's, because they *are* the spec.

use serde::Deserialize;

/// The tint a script asked for.
///
/// Waybar's `class` key, reduced to the three states the panel has colours
/// for. Everything else is dropped rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomClass {
    /// Something finished, or is healthy.
    Success,
    /// Something wants looking at.
    Warning,
    /// Something is wrong.
    Urgent,
}

impl CustomClass {
    /// The class a Waybar-style name means, or `None` for one we do not tint.
    ///
    /// `error` is Waybar's word for what this panel calls urgent, and a script
    /// written for Waybar says `error`; mapping it is what makes those scripts
    /// work unchanged.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "success" | "good" | "ok" => Some(Self::Success),
            "warning" | "warn" => Some(Self::Warning),
            "urgent" | "error" | "critical" => Some(Self::Urgent),
            _ => None,
        }
    }
}

/// One reading, normalised out of whatever shape the script printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomOutput {
    /// What goes on the bar.
    pub text: String,
    /// What the tooltip says, when the script had an opinion.
    pub tooltip: Option<String>,
    /// The tint it asked for.
    pub class: Option<CustomClass>,
}

/// What the widget draws once the template and the fallback have been applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomDisplay {
    /// The label text.
    pub text: String,
    /// The tooltip, or `None` to fall back to the configured one.
    pub tooltip: Option<String>,
    /// The tint.
    pub class: Option<CustomClass>,
    /// Whether the widget is on the bar at all.
    pub visible: bool,
}

/// The JSON shape Waybar's custom modules print.
#[derive(Debug, Deserialize)]
struct Json {
    /// The primary key.
    #[serde(default)]
    text: String,
    /// The one simple scripts use instead.
    #[serde(default)]
    label: String,
    /// The tooltip, when there is one.
    #[serde(default)]
    tooltip: Option<String>,
    /// A number, used as the tooltip when nothing better was given.
    #[serde(default)]
    percentage: Option<Percentage>,
    /// The tint, as one name or as a list of them.
    #[serde(default)]
    class: Option<Classes>,
}

/// `percentage` is an integer in most scripts and a float in some.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Percentage {
    /// `"percentage": 72`
    Integer(i64),
    /// `"percentage": 72.4`
    Float(f64),
}

impl Percentage {
    /// The line it stands in for.
    fn to_tooltip(&self) -> String {
        match self {
            Self::Integer(value) => format!("{value}%"),
            Self::Float(value) => format!("{value:.0}%"),
        }
    }
}

/// `class` is one name in most scripts and a list in Waybar's own examples.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Classes {
    /// `"class": "warning"`
    One(String),
    /// `"class": ["warning", "custom-thing"]`
    Many(Vec<String>),
}

impl Classes {
    /// The first name that names a state the panel can paint.
    fn state(&self) -> Option<CustomClass> {
        match self {
            Self::One(name) => CustomClass::parse(name),
            Self::Many(names) => names.iter().find_map(|name| CustomClass::parse(name)),
        }
    }
}

impl Json {
    /// Fold the JSON down to one reading.
    fn into_output(self) -> CustomOutput {
        let text = if self.text.is_empty() {
            self.label
        } else {
            self.text
        };

        // An empty tooltip is not a tooltip: a script that always prints the
        // key and only sometimes fills it in must not suppress the percentage.
        let tooltip = self
            .tooltip
            .filter(|text| !text.is_empty())
            .or_else(|| self.percentage.map(|value| value.to_tooltip()));

        CustomOutput {
            text,
            tooltip,
            class: self.class.as_ref().and_then(Classes::state),
        }
    }
}

/// Read one run's standard output.
///
/// JSON when it looks like JSON, and the first line otherwise. v1 took the
/// first line and *then* looked for JSON, which meant a script printing
/// pretty-printed JSON — everything `jq` writes without `-c` — fell through to
/// the raw `{`. Parsing the whole output first fixes that without changing
/// what a one-line script gets.
pub fn parse(raw: &str) -> CustomOutput {
    let trimmed = raw.trim();
    if trimmed.starts_with('{')
        && let Ok(json) = serde_json::from_str::<Json>(trimmed)
    {
        return json.into_output();
    }

    CustomOutput {
        text: trimmed
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        ..CustomOutput::default()
    }
}

/// Apply the template and the static fallback to a reading.
///
/// The rule that surprises people is the empty one, and it is deliberate: a
/// script that prints nothing is saying "there is nothing to report", and a
/// widget with nothing to report takes itself off the bar — unless a static
/// `label` was configured, which is the user saying what to show instead.
pub fn display(raw: &str, fallback: &str, template: Option<&str>) -> CustomDisplay {
    let output = parse(raw);
    if output.text.is_empty() {
        return CustomDisplay {
            text: fallback.to_string(),
            tooltip: None,
            class: None,
            visible: !fallback.is_empty(),
        };
    }

    let text = match template {
        Some(template) => template.replace("{output}", &output.text),
        None => output.text,
    };

    CustomDisplay {
        text,
        tooltip: output.tooltip,
        class: output.class,
        visible: true,
    }
}

/// Whether the placeholder is shown while a run is out.
///
/// v1's truth table, ported unchanged. The placeholder means "there is nothing
/// here yet", so it appears whenever there is in fact nothing on screen — a
/// hidden widget, or a visible one with an empty label — and never in front of
/// a value somebody can already read.
pub fn shows_loading(visible: bool, text: &str) -> bool {
    !visible || text.is_empty()
}

/// The line a failed run adds to the tooltip.
///
/// The value stays on the bar, because half-hour-old prices with their failure
/// admitted to beat a widget that vanishes every time a script has a bad
/// minute. The tooltip is where the admission goes.
pub fn failure_note(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("Last update failed (exit {code})"),
        None => "Last update failed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_the_text() {
        assert_eq!(
            parse("hello"),
            CustomOutput {
                text: "hello".to_string(),
                ..CustomOutput::default()
            }
        );
    }

    #[test]
    fn only_the_first_line_of_plain_output_reaches_the_bar() {
        // v1's rule: a script that prints a value and then some diagnostics
        // puts the value on the bar and nothing else.
        assert_eq!(parse("103412\nand some noise").text, "103412");
    }

    #[test]
    fn a_waybar_percentage_becomes_the_tooltip() {
        // A Nerd Font glyph, written the way JSON escapes one — which is what a
        // script piping through `jq` actually emits.
        assert_eq!(
            parse(r#"{"text":"","percentage":72}"#),
            CustomOutput {
                text: "\u{f04e}".to_string(),
                tooltip: Some("72%".to_string()),
                class: None,
            }
        );
    }

    #[test]
    fn a_fractional_percentage_is_rounded_for_the_tooltip() {
        assert_eq!(
            parse(r#"{"text":"x","percentage":72.4}"#)
                .tooltip
                .as_deref(),
            Some("72%")
        );
    }

    #[test]
    fn an_explicit_tooltip_wins_over_the_percentage() {
        assert_eq!(
            parse(r#"{"text":"x","tooltip":"Headset 72%","percentage":9}"#),
            CustomOutput {
                text: "x".to_string(),
                tooltip: Some("Headset 72%".to_string()),
                class: None,
            }
        );
    }

    #[test]
    fn an_empty_tooltip_falls_through_to_the_percentage() {
        assert_eq!(
            parse(r#"{"text":"x","tooltip":"","percentage":9}"#)
                .tooltip
                .as_deref(),
            Some("9%")
        );
    }

    #[test]
    fn the_label_key_stands_in_for_text() {
        assert_eq!(
            parse(r#"{"label":"VPN"}"#),
            CustomOutput {
                text: "VPN".to_string(),
                ..CustomOutput::default()
            }
        );
    }

    #[test]
    fn text_wins_when_a_script_prints_both_keys() {
        assert_eq!(parse(r#"{"text":"A","label":"B"}"#).text, "A");
    }

    #[test]
    fn pretty_printed_json_parses_where_v1_gave_up() {
        let output = parse("{\n  \"text\": \"BTC 103412\",\n  \"class\": \"warning\"\n}\n");
        assert_eq!(output.text, "BTC 103412");
        assert_eq!(output.class, Some(CustomClass::Warning));
    }

    #[test]
    fn something_that_starts_like_json_but_is_not_stays_text() {
        assert_eq!(parse("{not json at all").text, "{not json at all");
    }

    #[test]
    fn the_class_table_covers_waybars_names() {
        assert_eq!(CustomClass::parse("success"), Some(CustomClass::Success));
        assert_eq!(CustomClass::parse("warning"), Some(CustomClass::Warning));
        assert_eq!(CustomClass::parse("urgent"), Some(CustomClass::Urgent));
        // Waybar's own word for the state this panel calls urgent.
        assert_eq!(CustomClass::parse("error"), Some(CustomClass::Urgent));
        assert_eq!(CustomClass::parse("critical"), Some(CustomClass::Urgent));
        assert_eq!(CustomClass::parse("WARNING"), Some(CustomClass::Warning));
        assert_eq!(CustomClass::parse("anything-else"), None);
    }

    #[test]
    fn a_list_of_classes_is_read_for_the_first_one_that_means_something() {
        assert_eq!(
            parse(r#"{"text":"x","class":["custom-thing","warning"]}"#).class,
            Some(CustomClass::Warning)
        );
        assert_eq!(parse(r#"{"text":"x","class":["nothing"]}"#).class, None);
    }

    #[test]
    fn empty_output_with_no_fallback_takes_the_widget_off_the_bar() {
        assert_eq!(
            display("", "", None),
            CustomDisplay {
                text: String::new(),
                tooltip: None,
                class: None,
                visible: false,
            }
        );
    }

    #[test]
    fn empty_output_falls_back_to_the_static_label() {
        assert_eq!(
            display(r#"{"text":""}"#, "Weather", None),
            CustomDisplay {
                text: "Weather".to_string(),
                tooltip: None,
                class: None,
                visible: true,
            }
        );
    }

    #[test]
    fn a_zero_is_a_value_rather_than_an_absence() {
        assert_eq!(
            display("0", "", Some("count={output}")),
            CustomDisplay {
                text: "count=0".to_string(),
                tooltip: None,
                class: None,
                visible: true,
            }
        );
    }

    #[test]
    fn a_zero_percentage_still_makes_a_tooltip() {
        assert_eq!(
            display(r#"{"text":"Headset","percentage":0}"#, "", None),
            CustomDisplay {
                text: "Headset".to_string(),
                tooltip: Some("0%".to_string()),
                class: None,
                visible: true,
            }
        );
    }

    #[test]
    fn the_template_wraps_the_output_rather_than_replacing_it() {
        assert_eq!(
            display("21", "", Some("\u{f2c9} {output}")).text,
            "\u{f2c9} 21"
        );
        // Twice, because a template may want the value on both sides.
        assert_eq!(display("7", "", Some("{output}/{output}")).text, "7/7");
    }

    #[test]
    fn the_template_is_not_applied_to_the_static_fallback() {
        // The fallback is what the user wrote; wrapping it would put the
        // template's decoration on a label that never came from a script.
        assert_eq!(display("", "Weather", Some("[{output}]")).text, "Weather");
    }

    #[test]
    fn the_scripts_people_actually_migrated_stay_on_the_bar() {
        let cases = [
            ("BTC 103421 ETH 3850", "BTC 103421 ETH 3850"),
            ("\u{2600} 72\u{b0}F", "\u{2600} 72\u{b0}F"),
            (
                // JSON's own escape, not Rust's: this string is what the script
                // prints, byte for byte.
                r#"{"text":"","tooltip":"Headset: 72%"}"#,
                "\u{f025}",
            ),
            (r#"{"label":"VPN"}"#, "VPN"),
        ];
        for (raw, expected) in cases {
            let display = display(raw, "", None);
            assert_eq!(display.text, expected);
            assert!(display.visible, "{raw} should stay on the bar");
        }
    }

    #[test]
    fn a_vpn_script_with_nothing_connected_hides_its_widget() {
        assert!(!display("", "", None).visible);
    }

    #[test]
    fn the_placeholder_covers_an_empty_bar_and_nothing_else() {
        assert!(shows_loading(false, ""), "hidden and blank");
        assert!(shows_loading(true, ""), "visible but blank");
        assert!(
            shows_loading(false, "BTC 103421"),
            "hidden with a stale value"
        );
        assert!(
            !shows_loading(true, "BTC 103421"),
            "a value somebody can read must never be replaced by an ellipsis"
        );
    }

    #[test]
    fn a_failure_says_what_the_exit_status_was() {
        assert_eq!(failure_note(Some(1)), "Last update failed (exit 1)");
        assert_eq!(failure_note(Some(127)), "Last update failed (exit 127)");
        // Killed by a signal, or never started at all.
        assert_eq!(failure_note(None), "Last update failed");
    }
}
