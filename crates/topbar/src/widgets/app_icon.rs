//! Finding an application's icon from its desktop entry.
//!
//! Two things in the panel identify an application by its desktop entry and
//! want its icon: a notification's sender and a media player. The lookup is a
//! scan of every installed application, so it is done once per entry and
//! remembered — including the entries that came to nothing, which are the ones
//! a badly behaved sender repeats.

use gtk4::gio;
use gtk4::prelude::*;

thread_local! {
    /// Desktop entries already looked up.
    ///
    /// The set of distinct applications on one desktop is small, so the cache
    /// is too, and it is never invalidated: installing an application while
    /// the panel is running is rare enough to be worth a restart.
    static ENTRY_ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<gio::Icon>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The icon an application's desktop entry declares.
pub fn lookup(entry: &str) -> Option<gio::Icon> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    ENTRY_ICONS.with_borrow_mut(|cache| {
        cache
            .entry(entry.to_string())
            .or_insert_with(|| scan(entry))
            .clone()
    })
}

/// Find `entry` among the installed applications.
///
/// The hint is documented as the entry's name without the `.desktop` suffix
/// but plenty of senders include it, and a few get the case wrong, so the
/// comparison forgives both.
fn scan(entry: &str) -> Option<gio::Icon> {
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
