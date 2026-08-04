//! What the panel knows about power profiles, and how a profile is presented.
//!
//! The daemon speaks in identifiers — `power-saver`, `balanced`,
//! `performance` — and the panel has to show a name and an icon for each. The
//! mapping is a pure function so the toggle's label can be checked without a
//! bus, and so an identifier nobody has seen before still renders as something
//! rather than as nothing.

/// One profile, as a widget needs it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProfileView {
    /// The daemon's own identifier, which is what `set_profile` takes.
    pub id: String,
    /// What to show the user.
    pub label: String,
    /// The Adwaita symbolic icon for it.
    pub icon: &'static str,
}

impl ProfileView {
    /// Describe the profile the daemon calls `id`.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label(id),
            icon: icon(id),
        }
    }
}

/// The icon for a profile identifier.
///
/// The three the daemon actually reports have icons of their own; anything
/// else — a vendor profile from a future release — borrows the balanced one,
/// which is the honest choice for "a profile, and we do not know which".
pub fn icon(id: &str) -> &'static str {
    match id {
        "power-saver" => "power-profile-power-saver-symbolic",
        "performance" => "power-profile-performance-symbolic",
        _ => "power-profile-balanced-symbolic",
    }
}

/// The display name for a profile identifier.
///
/// Unknown identifiers are title-cased with hyphens turned into spaces, which
/// turns `ultra-performance` into `Ultra Performance` rather than into a
/// dash-ridden slug in the middle of a menu.
pub fn label(id: &str) -> String {
    match id {
        "power-saver" => "Power Saver".to_string(),
        "balanced" => "Balanced".to_string(),
        "performance" => "Performance".to_string(),
        other => title_case(other),
    }
}

/// `ultra-performance` → `Ultra Performance`.
fn title_case(id: &str) -> String {
    if id.is_empty() {
        return "Unknown".to_string();
    }
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Everything the panel knows about power profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PowerProfilesState {
    /// Whether a power-profiles daemon is on the bus at all.
    ///
    /// False hides the Quick Settings toggle outright: a machine with no
    /// daemon has no profiles, and a disabled control explaining that would be
    /// a row of dead space on every desktop that never had one.
    pub available: bool,
    /// The profile in force, when the daemon has said.
    pub active: Option<ProfileView>,
    /// Exactly the profiles the daemon reports, in the order it reports them.
    pub profiles: Vec<ProfileView>,
}

impl PowerProfilesState {
    /// The identifier of the active profile, if there is one.
    pub fn active_id(&self) -> Option<&str> {
        self.active.as_ref().map(|active| active.id.as_str())
    }

    /// Whether `id` is one of the profiles the daemon offers.
    pub fn offers(&self, id: &str) -> bool {
        self.profiles.iter().any(|profile| profile.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_known_profiles_map_to_their_own_icons() {
        assert_eq!(icon("power-saver"), "power-profile-power-saver-symbolic");
        assert_eq!(icon("balanced"), "power-profile-balanced-symbolic");
        assert_eq!(icon("performance"), "power-profile-performance-symbolic");
    }

    #[test]
    fn the_three_known_profiles_are_named_as_gnome_names_them() {
        assert_eq!(label("power-saver"), "Power Saver");
        assert_eq!(label("balanced"), "Balanced");
        assert_eq!(label("performance"), "Performance");
    }

    #[test]
    fn an_unknown_profile_still_renders_as_something() {
        let view = ProfileView::new("ultra-performance");
        assert_eq!(view.label, "Ultra Performance");
        assert_eq!(view.icon, "power-profile-balanced-symbolic");
        assert_eq!(view.id, "ultra-performance");
    }

    #[test]
    fn an_empty_identifier_does_not_produce_an_empty_row() {
        assert_eq!(label(""), "Unknown");
    }

    #[test]
    fn underscores_are_treated_like_hyphens() {
        assert_eq!(label("power_saver_plus"), "Power Saver Plus");
    }

    #[test]
    fn nothing_is_available_before_the_daemon_answers() {
        let state = PowerProfilesState::default();
        assert!(!state.available);
        assert_eq!(state.active_id(), None);
        assert!(!state.offers("balanced"));
    }

    #[test]
    fn a_state_knows_which_profiles_it_may_be_set_to() {
        let state = PowerProfilesState {
            available: true,
            active: Some(ProfileView::new("balanced")),
            profiles: vec![
                ProfileView::new("balanced"),
                ProfileView::new("performance"),
            ],
        };
        assert_eq!(state.active_id(), Some("balanced"));
        assert!(state.offers("performance"));
        assert!(!state.offers("power-saver"));
    }
}
