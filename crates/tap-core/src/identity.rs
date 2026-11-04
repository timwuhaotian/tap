//! Every identifier that names Tap, in one place (docs/PLAN.md §6). Search scattered
//! `com.officecommun.search` and "Search" through its files; nothing outside this module
//! spells Tap's.

/// The bundle identifier, as in `packaging/Info.plist`.
pub const BUNDLE_ID: &str = "com.timwuhaotian.tap";
/// The app's name, and its folder under Application Support.
pub const NAME: &str = "Tap";
/// The internal pages' scheme: `tap://settings` and the rest (Search's `search://`).
pub const SCHEME: &str = "tap";
/// The log subsystem (Search's `com.officecommun.search`).
pub const LOG_SUBSYSTEM: &str = "com.timwuhaotian.tap";
/// The environment variable that makes a run a test run in a world of its own
/// (Search's `SEARCH_PROBE`).
pub const PROBE_ENV: &str = "TAP_PROBE";

/// The folder name under `~/Library/Application Support` for a world: `Tap` for the
/// real one, `Tap (<world>)` for a test run's (Search's `Store.swift:126`).
#[must_use]
pub fn folder_name(world: Option<&str>) -> String {
    match world {
        None => NAME.to_owned(),
        Some(world) => format!("{NAME} ({world})"),
    }
}

/// A test run's world, named the way Search's `Store.world` names it: lower case,
/// letters, digits and hyphens; "test" when the variable is set but says nothing usable.
#[must_use]
pub fn world(probe: Option<&str>) -> Option<String> {
    let probe = probe?;
    let name: String = probe
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    Some(if name.is_empty() || name == "1" {
        "test".to_owned()
    } else {
        name
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worlds_are_named_as_search_names_them() {
        assert_eq!(world(None), None);
        assert_eq!(world(Some("1")).as_deref(), Some("test"));
        assert_eq!(world(Some("")).as_deref(), Some("test"));
        assert_eq!(
            world(Some("CI Download-7")).as_deref(),
            Some("cidownload-7")
        );
        assert_eq!(folder_name(None), "Tap");
        assert_eq!(folder_name(Some("test")), "Tap (test)");
    }
}
