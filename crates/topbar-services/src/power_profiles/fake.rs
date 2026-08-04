//! A power-profiles daemon that exists to be talked to.
//!
//! Test support only: behind `cfg(test)` for the bus tests and behind the
//! `fake-power` feature for `topbar-fake-power`, the sidecar the nested-niri
//! smoke run puts on its private bus. The packaged panel contains none of it.
//!
//! The real daemon answers to two bus names, so this one can serve either or
//! both — which is how "the legacy name still works" is a test rather than a
//! hope. Every `ActiveProfile` write is recorded, and setting the property
//! from outside (`gdbus … Properties.Set`) is what stands in for a click in
//! the smoke run, where there is no pointer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::zvariant::OwnedValue;

use super::{ENDPOINTS, Endpoint};

/// The state both interfaces share, so either name sees the same machine.
#[derive(Debug)]
pub struct Shared {
    active: Mutex<String>,
    profiles: Vec<String>,
    /// Every profile the daemon has been asked for, in order.
    writes: Mutex<Vec<String>>,
    /// Whether writes are refused, for the revert path.
    refuse: bool,
}

impl Shared {
    /// A daemon offering `profiles`, currently running `active`.
    pub fn new(active: &str, profiles: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(active.to_string()),
            profiles: profiles
                .iter()
                .map(|profile| (*profile).to_string())
                .collect(),
            writes: Mutex::new(Vec::new()),
            refuse: false,
        })
    }

    /// The same, but refusing every write — for the optimistic-revert test.
    pub fn refusing(active: &str, profiles: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(active.to_string()),
            profiles: profiles
                .iter()
                .map(|profile| (*profile).to_string())
                .collect(),
            writes: Mutex::new(Vec::new()),
            refuse: true,
        })
    }

    /// The profile in force.
    pub fn active(&self) -> String {
        lock(&self.active).clone()
    }

    /// Every profile the daemon has been asked for.
    pub fn writes(&self) -> Vec<String> {
        lock(&self.writes).clone()
    }

    /// The `Profiles` property, as the daemon publishes it.
    fn published(&self) -> Vec<HashMap<String, OwnedValue>> {
        self.profiles
            .iter()
            .map(|profile| {
                let mut entry = HashMap::new();
                if let Ok(value) =
                    OwnedValue::try_from(zbus::zvariant::Value::from(profile.as_str()))
                {
                    entry.insert("Profile".to_string(), value);
                }
                if let Ok(driver) = OwnedValue::try_from(zbus::zvariant::Value::from("topbar-fake"))
                {
                    entry.insert("Driver".to_string(), driver);
                }
                entry
            })
            .collect()
    }

    /// Take a write, or refuse it.
    fn write(&self, profile: String) -> zbus::fdo::Result<()> {
        if self.refuse {
            return Err(zbus::fdo::Error::AccessDenied(
                "this fake refuses every profile".into(),
            ));
        }
        if !self.profiles.contains(&profile) {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "no such profile: {profile}"
            )));
        }
        lock(&self.writes).push(profile.clone());
        *lock(&self.active) = profile;
        Ok(())
    }
}

/// Lock through poisoning: the state is plain data.
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The interface under the current bus name.
struct Modern(Arc<Shared>);

#[zbus::interface(name = "org.freedesktop.UPower.PowerProfiles")]
impl Modern {
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.0.active()
    }

    #[zbus(property)]
    fn set_active_profile(&mut self, profile: String) -> zbus::fdo::Result<()> {
        self.0.write(profile)
    }

    #[zbus(property)]
    fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
        self.0.published()
    }

    #[zbus(property)]
    fn performance_degraded(&self) -> String {
        String::new()
    }
}

/// The same interface under the name the daemon used before 0.20.
struct Legacy(Arc<Shared>);

#[zbus::interface(name = "net.hadess.PowerProfiles")]
impl Legacy {
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.0.active()
    }

    #[zbus(property)]
    fn set_active_profile(&mut self, profile: String) -> zbus::fdo::Result<()> {
        self.0.write(profile)
    }

    #[zbus(property)]
    fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
        self.0.published()
    }

    #[zbus(property)]
    fn performance_degraded(&self) -> String {
        String::new()
    }
}

/// Which names a fake should answer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Names {
    /// `org.freedesktop.UPower.PowerProfiles` only, as 0.20 and later.
    Modern,
    /// `net.hadess.PowerProfiles` only, as before it.
    Legacy,
    /// Both, as a real daemon does today.
    Both,
}

impl Names {
    /// The endpoints this choice covers.
    fn endpoints(self) -> Vec<Endpoint> {
        match self {
            Self::Modern => vec![ENDPOINTS[0]],
            Self::Legacy => vec![ENDPOINTS[1]],
            Self::Both => ENDPOINTS.to_vec(),
        }
    }

    /// Parse a `--name` argument.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "modern" | "upower" => Some(Self::Modern),
            "legacy" | "hadess" => Some(Self::Legacy),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

/// Serve a power-profiles daemon on `address`, under `names`.
///
/// The returned connections hold the bus names; dropping them is how a test
/// makes the daemon go away.
pub async fn serve(
    address: &str,
    names: Names,
    state: &Arc<Shared>,
) -> zbus::Result<Vec<zbus::Connection>> {
    let mut connections = Vec::new();
    for endpoint in names.endpoints() {
        let builder = zbus::connection::Builder::address(address)?.name(endpoint.name)?;
        let connection = if endpoint.name == ENDPOINTS[0].name {
            builder
                .serve_at(endpoint.path, Modern(Arc::clone(state)))?
                .build()
                .await?
        } else {
            builder
                .serve_at(endpoint.path, Legacy(Arc::clone(state)))?
                .build()
                .await?
        };
        connections.push(connection);
    }
    Ok(connections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_choice_covers_the_endpoints_it_says_it_does() {
        assert_eq!(Names::Modern.endpoints(), vec![ENDPOINTS[0]]);
        assert_eq!(Names::Legacy.endpoints(), vec![ENDPOINTS[1]]);
        assert_eq!(Names::Both.endpoints().len(), 2);
    }

    #[test]
    fn the_name_argument_takes_both_spellings() {
        assert_eq!(Names::parse("upower"), Some(Names::Modern));
        assert_eq!(Names::parse("hadess"), Some(Names::Legacy));
        assert_eq!(Names::parse("both"), Some(Names::Both));
        assert_eq!(Names::parse("nonsense"), None);
    }

    #[test]
    fn a_write_of_an_unknown_profile_is_refused() {
        let shared = Shared::new("balanced", &["balanced", "performance"]);
        assert!(shared.write("performance".into()).is_ok());
        assert_eq!(shared.active(), "performance");
        assert!(shared.write("turbo".into()).is_err());
        assert_eq!(shared.writes(), vec!["performance".to_string()]);
    }

    #[test]
    fn a_refusing_daemon_keeps_the_profile_it_had() {
        let shared = Shared::refusing("balanced", &["balanced", "performance"]);
        assert!(shared.write("performance".into()).is_err());
        assert_eq!(shared.active(), "balanced");
        assert!(shared.writes().is_empty());
    }
}
