//! Tap's own pages (Search's `InternalPages.swift`): native views drawn where a page would
//! be, under `tap://` addresses that never reach WebKit (docs/PLAN.md §3.1). There is no
//! scheme handler, so nothing a page runs can reach them.
//!
//! Settings is the one there is so far.

use crate::identity;

/// One of Tap's own pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Settings,
}

impl Page {
    /// The address it stands under in the field and the session.
    #[must_use]
    pub fn address(self) -> String {
        match self {
            Page::Settings => format!("{}://settings", identity::SCHEME),
        }
    }

    /// What its tab says.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Page::Settings => "Settings",
        }
    }
}

/// The page an address names, if it is one of Tap's own: `tap://settings`, with or
/// without a section after it.
#[must_use]
pub fn page(url: &str) -> Option<Page> {
    let rest = url.strip_prefix(identity::SCHEME)?.strip_prefix("://")?;
    let name = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    (name == "settings").then_some(Page::Settings)
}

/// Whether an address is under Tap's own scheme at all, a page it knows or not: such an
/// address is never handed to WebKit.
#[must_use]
pub fn is_internal(url: &str) -> bool {
    url.get(..identity::SCHEME.len() + 3)
        .is_some_and(|head| head.eq_ignore_ascii_case(&format!("{}://", identity::SCHEME)))
}

/// An address from a Search session, as Tap keeps it: Search's own Settings becomes
/// Tap's; Search's other pages of its own (`search://history` and the rest) are `None`,
/// since Tap has no such page to show yet.
#[must_use]
pub fn from_search(url: &str) -> Option<String> {
    match url.strip_prefix("search://") {
        Some(rest) if rest.to_lowercase().starts_with("settings") => Some(Page::Settings.address()),
        Some(_) => None,
        None => Some(url.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_addresses_name_tap_pages() {
        assert_eq!(page("tap://settings"), Some(Page::Settings));
        assert_eq!(page("tap://Settings/general"), Some(Page::Settings));
        assert_eq!(page("tap://history"), None);
        assert_eq!(page("https://settings"), None);
        assert!(is_internal("TAP://history") && !is_internal("https://tap.dev"));
        assert_eq!(Page::Settings.address(), "tap://settings");
    }

    #[test]
    fn search_pages_come_over_as_tap_pages_or_not_at_all() {
        assert_eq!(
            from_search("search://settings?page=tabs").as_deref(),
            Some("tap://settings")
        );
        assert_eq!(from_search("search://history"), None);
        assert_eq!(
            from_search("https://example.com").as_deref(),
            Some("https://example.com")
        );
    }
}
