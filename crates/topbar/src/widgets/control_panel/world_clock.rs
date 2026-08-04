//! The secondary time zones listed under the local time.
//!
//! A row is `New York  · Tue, Aug 4        16:05`: the configured label, the
//! zone's own date dimmed beside it, and its time on the right. The date is
//! there because the whole point of a world clock is that the other place may
//! not be on the same day as you.
//!
//! Zones are resolved once, when the panel is built. A time zone chrono-tz
//! does not know is dropped with a warning rather than failing the panel:
//! losing one row is better than losing the calendar too.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use gtk4::prelude::*;
use gtk4::{Align, Label, Orientation};
use topbar_core::config::WorldClock;
use tracing::warn;

use crate::style::classes;

/// Format of the dimmed date beside a world clock's label.
const DATE_FORMAT: &str = "%a, %b %-d";
/// Format of the time itself.
const TIME_FORMAT: &str = "%H:%M";

/// A configured zone that resolved to a real time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    /// The label from the configuration.
    pub label: String,
    /// The resolved IANA zone.
    pub timezone: Tz,
}

/// Resolve `[widgets.clock].world_clocks`, dropping zones that do not exist.
///
/// The warning names the offending value, because "New Yrok" is a typo the
/// user can only fix if they are told which entry was skipped.
pub fn resolve(configured: &[WorldClock]) -> Vec<Zone> {
    configured
        .iter()
        .filter_map(|clock| match clock.timezone.parse::<Tz>() {
            Ok(timezone) => Some(Zone {
                label: clock.label.clone(),
                timezone,
            }),
            Err(_) => {
                warn!(
                    "widgets.clock.world_clocks: `{}` is not a known time zone; skipping `{}`",
                    clock.timezone, clock.label
                );
                None
            }
        })
        .collect()
}

/// The dimmed date and the time a row shows at `now`.
pub fn format(zone: &Zone, now: DateTime<Utc>) -> (String, String) {
    let local = now.with_timezone(&zone.timezone);
    (
        format!("· {}", local.format(DATE_FORMAT)),
        local.format(TIME_FORMAT).to_string(),
    )
}

/// One world clock's row of labels.
pub struct Row {
    zone: Zone,
    date: Label,
    time: Label,
    /// The row itself, so the caller can append it.
    root: gtk4::Box,
}

impl Row {
    /// Build the row for `zone`.
    pub fn new(zone: Zone) -> Self {
        let root = gtk4::Box::new(Orientation::Horizontal, 6);
        root.add_css_class(classes::WORLD_CLOCK_ROW);

        let name = Label::new(Some(&zone.label));
        name.add_css_class(classes::WORLD_CLOCK_NAME);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let date = Label::new(None);
        date.add_css_class(classes::WORLD_CLOCK_ZONE);
        date.set_xalign(0.0);
        date.set_hexpand(true);
        date.set_halign(Align::Start);

        let time = Label::new(None);
        time.add_css_class(classes::WORLD_CLOCK_TIME);
        time.set_xalign(1.0);

        root.append(&name);
        root.append(&date);
        root.append(&time);

        Self {
            zone,
            date,
            time,
            root,
        }
    }

    /// The widget to append to the time card.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render for `now`.
    pub fn render(&self, now: DateTime<Utc>) {
        let (date, time) = format(&self.zone, now);
        if self.date.text() != date {
            self.date.set_text(&date);
        }
        if self.time.text() != time {
            self.time.set_text(&time);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn clock(label: &str, timezone: &str) -> WorldClock {
        WorldClock {
            label: label.to_string(),
            timezone: timezone.to_string(),
        }
    }

    /// 2026-08-04 13:05 UTC — a summer instant, so New York is on EDT.
    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 4, 13, 5, 0)
            .single()
            .expect("a real UTC instant")
    }

    #[test]
    fn resolves_the_live_configs_zones() {
        let zones = resolve(&[
            clock("New York", "America/New_York"),
            clock("UTC", "Etc/UTC"),
        ]);
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].timezone, Tz::America__New_York);
        assert_eq!(zones[1].timezone, Tz::Etc__UTC);
    }

    #[test]
    fn an_unknown_zone_is_skipped_not_fatal() {
        let zones = resolve(&[
            clock("Nowhere", "Mars/Olympus_Mons"),
            clock("UTC", "Etc/UTC"),
        ]);
        assert_eq!(zones.len(), 1, "the good zone survives the bad one");
        assert_eq!(zones[0].label, "UTC");
    }

    #[test]
    fn formats_the_zones_own_day_and_time() {
        let [new_york, utc] = [
            Zone {
                label: "New York".to_string(),
                timezone: Tz::America__New_York,
            },
            Zone {
                label: "UTC".to_string(),
                timezone: Tz::Etc__UTC,
            },
        ];

        assert_eq!(
            format(&new_york, instant()),
            ("· Tue, Aug 4".to_string(), "09:05".to_string()),
            "13:05 UTC is 09:05 on EDT"
        );
        assert_eq!(
            format(&utc, instant()),
            ("· Tue, Aug 4".to_string(), "13:05".to_string())
        );
    }

    #[test]
    fn a_zone_across_the_date_line_shows_its_own_day() {
        // 23:30 UTC is already tomorrow in Tokyo, which is the whole reason
        // the date is on the row at all.
        let tokyo = Zone {
            label: "Tokyo".to_string(),
            timezone: Tz::Asia__Tokyo,
        };
        let late = Utc
            .with_ymd_and_hms(2026, 8, 4, 23, 30, 0)
            .single()
            .expect("a real UTC instant");
        assert_eq!(
            format(&tokyo, late),
            ("· Wed, Aug 5".to_string(), "08:30".to_string())
        );
    }
}
